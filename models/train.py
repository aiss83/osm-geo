#!/usr/bin/env python3
"""
Файнтюнинг seq2seq-модели для нормализации названий улиц.

На вход: сокращённое/падежное название («ул Ленина», «проспекту Мира»)
На выход: нормализованное название («улица Ленина», «проспект Мира»)

Совместимость: Google Colab (T4 GPU, бесплатный тир).

Использование:
    python train.py --model google/mt5-small --epochs 5 --batch-size 16

Или на Colab:
    !python train.py --epochs 10 --batch-size 8 --gradient-accumulation 4
"""

import argparse
import json
import math
import os
import sys
from pathlib import Path

import torch
from datasets import Dataset, DatasetDict
from transformers import (
    AutoTokenizer,
    AutoModelForSeq2SeqLM,
    Seq2SeqTrainingArguments,
    Seq2SeqTrainer,
    DataCollatorForSeq2Seq,
    EarlyStoppingCallback,
)
import evaluate

# ─── Аргументы ────────────────────────────────────────────────────────────────

def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="Файнтюнинг нормализатора названий"
    )
    p.add_argument("--model", default="google/mt5-small",
                   help="Базовая модель (HuggingFace model id)")
    p.add_argument("--data-dir", default="./models/data",
                   help="Директория с train.jsonl и val.jsonl")
    p.add_argument("--train-file", default="train.jsonl",
                   help="Имя тренировочного файла (в data-dir)")
    p.add_argument("--val-file", default="val.jsonl",
                   help="Имя валидационного файла (в data-dir)")
    p.add_argument("--output-dir", default="./models/output",
                   help="Директория для чекпоинтов")
    p.add_argument("--final-model", default="./models/output/normalizer-final",
                   help="Путь для финальной модели")
    p.add_argument("--epochs", type=int, default=10,
                   help="Количество эпох")
    p.add_argument("--batch-size", type=int, default=16,
                   help="Размер батча")
    p.add_argument("--gradient-accumulation", type=int, default=1,
                   help="Шагов градиентной аккумуляции")
    p.add_argument("--lr", type=float, default=1e-4,
                   help="Learning rate")
    p.add_argument("--warmup-steps", type=int, default=500,
                   help="Шагов прогрева")
    p.add_argument("--max-input-length", type=int, default=64,
                   help="Максимальная длина входа в токенах")
    p.add_argument("--max-target-length", type=int, default=64,
                   help="Максимальная длина выхода в токенах")
    p.add_argument("--fp16", action="store_true", default=True,
                   help="Использовать mixed precision (только CUDA)")
    p.add_argument("--no-fp16", dest="fp16", action="store_false",
                   help="Отключить mixed precision")
    p.add_argument("--save-steps", type=int, default=500,
                   help="Сохранять чекпоинт каждые N шагов")
    p.add_argument("--eval-steps", type=int, default=500,
                   help="Оценивать каждые N шагов")
    p.add_argument("--logging-steps", type=int, default=100,
                   help="Логировать каждые N шагов")
    p.add_argument("--early-stopping-patience", type=int, default=3,
                   help="Терпение для early stopping (0 = отключено)")
    return p.parse_args()


# ─── Данные ───────────────────────────────────────────────────────────────────

def load_dataset(data_dir: str, train_file: str, val_file: str) -> DatasetDict:
    """Загрузить train/val из JSONL."""
    data_dir = Path(data_dir)
    train_path = data_dir / train_file
    val_path = data_dir / val_file

    if not train_path.exists():
        sys.exit(f"Ошибка: {train_path} не найден. Запустите generate_dataset.py сначала.")

    dataset = DatasetDict({
        "train": Dataset.from_json(str(train_path)),
        "validation": Dataset.from_json(str(val_path)),
    })
    return dataset


def preprocess(examples: dict, tokenizer, max_input: int, max_target: int) -> dict:
    """Токенизация для Seq2Seq."""
    inputs = examples["input"]
    targets = examples["output"]

    model_inputs = tokenizer(
        inputs,
        max_length=max_input,
        truncation=True,
        padding=False,
    )

    labels = tokenizer(
        targets,
        max_length=max_target,
        truncation=True,
        padding=False,
    )

    model_inputs["labels"] = labels["input_ids"]
    return model_inputs


# ─── Метрики ──────────────────────────────────────────────────────────────────

def compute_metrics(eval_pred, tokenizer):
    """Вычислить BLEU и Exact Match."""
    predictions, labels = eval_pred

    # Декодируем predictions
    if isinstance(predictions, tuple):
        predictions = predictions[0]  # для Seq2Seq

    decoded_preds = tokenizer.batch_decode(predictions, skip_special_tokens=True)
    decoded_labels = tokenizer.batch_decode(labels, skip_special_tokens=True)

    # Убираем лишние пробелы
    decoded_preds = [" ".join(p.split()) for p in decoded_preds]
    decoded_labels = [" ".join(l.split()) for l in decoded_labels]

    # Exact Match
    exact_matches = sum(
        1 for p, l in zip(decoded_preds, decoded_labels) if p == l
    )
    exact_match = exact_matches / len(decoded_preds) if decoded_preds else 0.0

    # Character-level accuracy (для коротких строк BLEU не очень показателен)
    char_correct = 0
    char_total = 0
    for p, l in zip(decoded_preds, decoded_labels):
        min_len = min(len(p), len(l))
        char_correct += sum(1 for a, b in zip(p, l) if a == b)
        char_total += max(len(p), len(l))
    char_acc = char_correct / char_total if char_total > 0 else 0.0

    return {
        "exact_match": exact_match,
        "char_accuracy": char_acc,
    }


# ─── Main ─────────────────────────────────────────────────────────────────────

def main() -> None:
    args = parse_args()

    print("=" * 60)
    print("Файнтюнинг нормализатора названий")
    print("=" * 60)
    print(f"Модель:  {args.model}")
    print(f"Данные:  {args.data_dir}")
    print(f"Выход:   {args.output_dir}")
    print(f"Эпохи:   {args.epochs}")
    print(f"Батч:    {args.batch_size} × {args.gradient_accumulation}")
    print(f"LR:      {args.lr}")

    # Устройство
    if torch.cuda.is_available():
        device = "cuda"
    elif torch.backends.mps.is_available():
        device = "mps"
    else:
        device = "cpu"
    print(f"Device:  {device}")
    if device == "cpu":
        print("⚠️  CPU-обучение будет очень медленным. Рекомендуется GPU.")
        args.fp16 = False
    if device == "mps":
        print("⚠️  MPS (Apple GPU) — fp16 отключён (не поддерживается MPS).")
        args.fp16 = False

    # Загружаем токенизатор и модель
    print("\n[1/5] Загрузка модели и токенизатора...")
    tokenizer = AutoTokenizer.from_pretrained(args.model)

    # Для некоторых моделей pad_token не задан
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token

    model = AutoModelForSeq2SeqLM.from_pretrained(args.model)
    model.to(device)

    total_params = sum(p.numel() for p in model.parameters())
    trainable_params = sum(p.numel() for p in model.parameters() if p.requires_grad)
    print(f"      Параметров: {total_params:,} (обучаемых: {trainable_params:,})")

    # Загружаем датасет
    print("[2/5] Загрузка датасета...")
    dataset = load_dataset(args.data_dir, args.train_file, args.val_file)
    print(f"      Train: {len(dataset['train'])} примеров")
    print(f"      Val:   {len(dataset['validation'])} примеров")

    # Токенизируем
    print("[3/5] Токенизация...")
    tokenized = dataset.map(
        lambda x: preprocess(x, tokenizer, args.max_input_length, args.max_target_length),
        batched=True,
        remove_columns=dataset["train"].column_names,
        desc="Tokenizing",
    )

    # Коллатор
    data_collator = DataCollatorForSeq2Seq(
        tokenizer,
        model=model,
        padding="longest",
    )

    # Аргументы тренировки
    training_args = Seq2SeqTrainingArguments(
        output_dir=args.output_dir,
        num_train_epochs=args.epochs,
        per_device_train_batch_size=args.batch_size,
        per_device_eval_batch_size=args.batch_size,
        gradient_accumulation_steps=args.gradient_accumulation,
        learning_rate=args.lr,
        warmup_steps=args.warmup_steps,
        weight_decay=0.01,
        logging_dir=f"{args.output_dir}/logs",
        logging_steps=args.logging_steps,
        eval_strategy="steps",
        eval_steps=args.eval_steps,
        save_strategy="steps",
        save_steps=args.save_steps,
        save_total_limit=3,
        load_best_model_at_end=True,
        metric_for_best_model="eval_loss",
        greater_is_better=False,
        fp16=args.fp16 and device == "cuda",
        predict_with_generate=False,  # отключено из-за бага SentencePiece OverflowError
        report_to="none",        # отключаем wandb
        dataloader_num_workers=2,
        ddp_find_unused_parameters=False,
        push_to_hub=False,
    )

    # Early stopping (опционально)
    callbacks = []
    if args.early_stopping_patience > 0:
        callbacks.append(
            EarlyStoppingCallback(
                early_stopping_patience=args.early_stopping_patience
            )
        )

    # Тренер
    print("[4/5] Запуск обучения...")
    trainer = Seq2SeqTrainer(
        model=model,
        args=training_args,
        train_dataset=tokenized["train"],
        eval_dataset=tokenized["validation"],
        processing_class=tokenizer,
        data_collator=data_collator,
        # compute_metrics отключён: SentencePiece OverflowError при batch_decode
        # Качество оценивается отдельно через models/evaluate.py
        callbacks=callbacks,
    )

    # Обучаем
    train_result = trainer.train()
    print(f"\nОбучение завершено!")
    print(f"  Train loss: {train_result.training_loss:.4f}")
    print(f"  Эпох:       {train_result.metrics.get('epoch', 0):.1f}")

    # Финальная оценка
    print("[5/5] Финальная оценка...")
    eval_results = trainer.evaluate()
    print(f"  Eval loss:      {eval_results.get('eval_loss', 0):.4f}")
    print(f"  Exact Match:    {eval_results.get('eval_exact_match', 0):.2%}")
    print(f"  Char Accuracy:  {eval_results.get('eval_char_accuracy', 0):.2%}")

    # Сохраняем финальную модель
    final_path = Path(args.final_model)
    final_path.mkdir(parents=True, exist_ok=True)
    trainer.save_model(str(final_path))
    tokenizer.save_pretrained(str(final_path))

    # Сохраняем метаданные обучения
    metadata = {
        "base_model": args.model,
        "train_examples": len(dataset["train"]),
        "val_examples": len(dataset["validation"]),
        "epochs": args.epochs,
        "batch_size": args.batch_size,
        "learning_rate": args.lr,
        "exact_match": eval_results.get("eval_exact_match", 0),
        "char_accuracy": eval_results.get("eval_char_accuracy", 0),
        "eval_loss": eval_results.get("eval_loss", 0),
    }
    with open(final_path / "training_metadata.json", "w") as f:
        json.dump(metadata, f, indent=2, ensure_ascii=False)

    print(f"\nМодель сохранена: {final_path}")
    print(f"Размер:           {sum(f.stat().st_size for f in final_path.rglob('*') if f.is_file()) / 1e9:.2f} GB")
    print("\nГотово! Теперь запустите export_onnx.py для конвертации в ONNX.")


if __name__ == "__main__":
    main()
