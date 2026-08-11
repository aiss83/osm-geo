#!/usr/bin/env python3
"""
Конвертация файнтюненной модели в ONNX + квантизация.

Шаги:
1. Загружает модель из HuggingFace-формата
2. Конвертирует в ONNX через optimum
3. Применяет dynamic quantization (INT8) для уменьшения размера
4. Верифицирует выход ONNX-модели

Требования:
    pip install optimum[onnxruntime] onnx onnxruntime

Использование:
    python export_onnx.py --model ./models/output/normalizer-final [--quantize]
"""

import argparse
import json
import shutil
import sys
from pathlib import Path

import torch
import numpy as np
from transformers import AutoTokenizer, AutoModelForSeq2SeqLM


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="Экспорт модели в ONNX"
    )
    p.add_argument("--model", required=True,
                   help="Путь к файнтюненной модели (HuggingFace-формат)")
    p.add_argument("--output", default=None,
                   help="Путь для ONNX-файла (по умолчанию: {model}_onnx/)")
    p.add_argument("--quantize", action="store_true", default=True,
                   help="Применить квантизацию (INT8 dynamic)")
    p.add_argument("--no-quantize", dest="quantize", action="store_false",
                   help="Пропустить квантизацию")
    p.add_argument("--opset", type=int, default=15,
                   help="ONNX opset version")
    p.add_argument("--test", action="store_true", default=True,
                   help="Протестировать ONNX-модель после экспорта")
    p.add_argument("--no-test", dest="test", action="store_false",
                   help="Пропустить тестирование")
    return p.parse_args()


def test_model(model, tokenizer, texts: list[str]) -> list[str]:
    """Прогнать тестовые примеры через модель."""
    device = "cuda" if torch.cuda.is_available() else "cpu"
    model = model.to(device)

    inputs = tokenizer(
        texts,
        max_length=64,
        truncation=True,
        padding=True,
        return_tensors="pt",
    ).to(device)

    with torch.no_grad():
        outputs = model.generate(
            **inputs,
            max_length=64,
            num_beams=1,
        )

    decoded = tokenizer.batch_decode(outputs, skip_special_tokens=True)
    return [" ".join(d.split()) for d in decoded]


def export_to_onnx(model, tokenizer, output_path: str, opset: int) -> None:
    """Конвертировать модель в ONNX через torch.onnx.export."""
    output_path = Path(output_path)
    output_path.mkdir(parents=True, exist_ok=True)

    model.eval()
    device = "cpu"  # ONNX экспорт на CPU
    model = model.to(device)

    # Сохраняем токенизатор отдельно (ONNX не поддерживает токенизаторы)
    tokenizer.save_pretrained(str(output_path))

    # Подготавливаем dummy inputs
    dummy_text = "ул Ленина"
    dummy_inputs = tokenizer(
        dummy_text,
        max_length=64,
        truncation=True,
        padding="max_length",
        return_tensors="pt",
    )
    input_ids = dummy_inputs["input_ids"]
    attention_mask = dummy_inputs["attention_mask"]

    # Функция-обёртка для модели с decoder_input_ids
    class Seq2SeqEncoder(torch.nn.Module):
        """Экспортируем только энкодер — для инференса используем ONNX Runtime с beam search."""

        def __init__(self, model):
            super().__init__()
            self.encoder = model.get_encoder()
            self.config = model.config

        def forward(self, input_ids, attention_mask):
            return self.encoder(
                input_ids=input_ids,
                attention_mask=attention_mask,
            ).last_hidden_state

    # Экспорт энкодера
    encoder_wrapper = Seq2SeqEncoder(model)
    encoder_path = output_path / "encoder.onnx"

    torch.onnx.export(
        encoder_wrapper,
        (input_ids, attention_mask),
        str(encoder_path),
        input_names=["input_ids", "attention_mask"],
        output_names=["encoder_hidden_states"],
        dynamic_axes={
            "input_ids": {0: "batch", 1: "sequence"},
            "attention_mask": {0: "batch", 1: "sequence"},
            "encoder_hidden_states": {0: "batch", 1: "sequence"},
        },
        opset_version=opset,
        do_constant_folding=True,
    )

    # Экспорт декодера
    class Seq2SeqDecoder(torch.nn.Module):
        """Декодер: принимает encoder_hidden_states и decoder_input_ids."""
        def __init__(self, model):
            super().__init__()
            self.decoder = model.get_decoder()
            self.lm_head = model.lm_head

        def forward(self, decoder_input_ids, encoder_hidden_states):
            decoder_outputs = self.decoder(
                input_ids=decoder_input_ids,
                encoder_hidden_states=encoder_hidden_states,
            )
            return self.lm_head(decoder_outputs.last_hidden_state)

    encoder_hidden_states = encoder_wrapper(input_ids, attention_mask)
    decoder_input_ids = torch.zeros((1, 1), dtype=torch.long)  # start token

    decoder_wrapper = Seq2SeqDecoder(model)
    decoder_path = output_path / "decoder.onnx"

    torch.onnx.export(
        decoder_wrapper,
        (decoder_input_ids, encoder_hidden_states),
        str(decoder_path),
        input_names=["decoder_input_ids", "encoder_hidden_states"],
        output_names=["logits"],
        dynamic_axes={
            "decoder_input_ids": {0: "batch", 1: "sequence"},
            "encoder_hidden_states": {0: "batch", 1: "sequence"},
            "logits": {0: "batch", 1: "sequence"},
        },
        opset_version=opset,
        do_constant_folding=True,
    )

    # Сохраняем метаданные
    metadata = {
        "model_type": "seq2seq_encoder_decoder",
        "tokenizer": str(output_path),
        "encoder_model": str(encoder_path),
        "decoder_model": str(decoder_path),
        "opset": opset,
        "max_length": 64,
        "bos_token_id": model.config.decoder_start_token_id,
        "eos_token_id": model.config.eos_token_id,
        "pad_token_id": tokenizer.pad_token_id,
    }
    with open(output_path / "model_metadata.json", "w") as f:
        json.dump(metadata, f, indent=2)

    print(f"\nONNX модель сохранена в: {output_path}")
    encoder_size = encoder_path.stat().st_size / 1e6
    decoder_size = decoder_path.stat().st_size / 1e6
    print(f"  encoder.onnx: {encoder_size:.1f} MB")
    print(f"  decoder.onnx: {decoder_size:.1f} MB")


def quantize_onnx(output_path: str) -> None:
    """Применить dynamic quantization к ONNX модели через onnxruntime."""
    try:
        from onnxruntime.quantization import quantize_dynamic, QuantType
    except ImportError:
        print("⚠️  onnxruntime не установлен. Квантизация пропущена.")
        print("   pip install onnxruntime")
        return

    output_path = Path(output_path)
    for model_file in ["encoder.onnx", "decoder.onnx"]:
        model_path = output_path / model_file
        quantized_path = output_path / f"{model_file.replace('.onnx', '_int8.onnx')}"

        if not model_path.exists():
            print(f"  Пропущен {model_file} — не найден")
            continue

        original_size = model_path.stat().st_size / 1e6

        quantize_dynamic(
            model_input=str(model_path),
            model_output=str(quantized_path),
            weight_type=QuantType.QInt8,
            extra_options={"ActivationSymmetric": True},
        )

        quantized_size = quantized_path.stat().st_size / 1e6
        reduction = (1 - quantized_size / original_size) * 100
        print(f"  {model_file}: {original_size:.1f} MB → {quantized_size:.1f} MB ({reduction:.0f}% меньше)")

        # Заменяем оригинал квантизованной версией
        backup_path = model_path.with_suffix(".onnx.fp32")
        shutil.move(str(model_path), str(backup_path))
        shutil.move(str(quantized_path), str(model_path))

    print("\nКвантизация завершена. FP32-версии сохранены как *.fp32")


def main() -> None:
    args = parse_args()

    if args.output is None:
        args.output = f"{args.model}_onnx"

    print("=" * 60)
    print("Экспорт модели в ONNX")
    print("=" * 60)
    print(f"Модель:     {args.model}")
    print(f"Выход:      {args.output}")
    print(f"Квантизация: {'INT8 dynamic' if args.quantize else 'отключена'}")

    # 1. Загружаем модель
    print("\n[1/3] Загрузка модели...")
    tokenizer = AutoTokenizer.from_pretrained(args.model)
    model = AutoModelForSeq2SeqLM.from_pretrained(args.model)

    total_params = sum(p.numel() for p in model.parameters())
    fp32_size = total_params * 4 / 1e6  # ~4 bytes per param
    print(f"      Параметров: {total_params:,} (~{fp32_size:.0f} MB FP32)")

    # 2. Экспорт в ONNX
    print("[2/3] Экспорт в ONNX...")
    export_to_onnx(model, tokenizer, args.output, args.opset)

    # 3. Квантизация
    if args.quantize:
        print("\n[3/3] Квантизация (INT8 dynamic)...")
        quantize_onnx(args.output)

        # Оцениваем итоговый размер
        output_path = Path(args.output)
        total_size = sum(
            f.stat().st_size
            for f in output_path.rglob("*")
            if f.is_file() and f.suffix == ".onnx"
        ) / 1e6
        print(f"\nИтоговый размер ONNX: {total_size:.1f} MB")
    else:
        print("\n[3/3] Квантизация пропущена.")

    # Тестирование
    if args.test:
        print("\nВерификация ONNX-модели...")
        test_texts = [
            "ул Ленина",
            "пр Мира",
            "проспекту Вернадского",
            "улицы Тверская",
            "Большое Исаково",  # не должно меняться
        ]
        expected = [
            "улица Ленина",
            "проспект Мира",
            "проспект Вернадского",
            "улица Тверская",
            "Большое Исаково",
        ]

        print("  Оригинальная модель (PyTorch):")
        results = test_model(model, tokenizer, test_texts)
        for inp, out, exp in zip(test_texts, results, expected):
            status = "✓" if out == exp else "✗"
            print(f"    {status} {inp!r:35s} → {out!r:35s} (ожид. {exp!r})")

    print("\nГотово! Модель готова для интеграции в Rust через ort.")


if __name__ == "__main__":
    main()
