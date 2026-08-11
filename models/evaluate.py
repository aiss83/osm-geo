#!/usr/bin/env python3
"""
Оценка качества нормализатора на тестовом наборе.

Поддерживает три бэкенда:
- pytorch — оригинальная HuggingFace модель
- onnx    — ONNX-модель (через onnxruntime)
- rust    — сравнение с Rust rule-based нормализатором (через subprocess)

Метрики:
- Exact Match (полное совпадение строки)
- Character Accuracy (посимвольная точность)
- Per-category breakdown (abbreviations, oblique, combined, noop)

Использование:
    python evaluate.py --backend pytorch --model ./models/output/normalizer-final
    python evaluate.py --backend rust --bin ../target/release/osm-geo
"""

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

# ─── Тестовый набор ──────────────────────────────────────────────────────────

# Ручной тестовый набор — ключевые примеры, которые модель ДОЛЖНА обрабатывать
TEST_CASES: list[dict] = [
    # === Abbreviations ===
    {"input": "ул Ленина",           "output": "улица Ленина",           "category": "abbreviation"},
    {"input": "ул. Ленина",          "output": "улица Ленина",           "category": "abbreviation"},
    {"input": "пр Мира",             "output": "проспект Мира",          "category": "abbreviation"},
    {"input": "пр. Мира",            "output": "проспект Мира",          "category": "abbreviation"},
    {"input": "пр-т Мира",           "output": "проспект Мира",          "category": "abbreviation"},
    {"input": "пер Садовый",         "output": "переулок Садовый",      "category": "abbreviation"},
    {"input": "пер. Садовый",        "output": "переулок Садовый",      "category": "abbreviation"},
    {"input": "бул Победы",          "output": "бульвар Победы",         "category": "abbreviation"},
    {"input": "бульв Победы",        "output": "бульвар Победы",         "category": "abbreviation"},
    {"input": "пл Ленина",           "output": "площадь Ленина",         "category": "abbreviation"},
    {"input": "пл. Ленина",          "output": "площадь Ленина",         "category": "abbreviation"},
    {"input": "наб Обводного",       "output": "набережная Обводного",   "category": "abbreviation"},
    {"input": "наб. Обводного",      "output": "набережная Обводного",   "category": "abbreviation"},
    {"input": "ш Энтузиастов",       "output": "шоссе Энтузиастов",     "category": "abbreviation"},
    {"input": "ш. Энтузиастов",      "output": "шоссе Энтузиастов",     "category": "abbreviation"},
    {"input": "пр-д Строителей",     "output": "проезд Строителей",     "category": "abbreviation"},
    {"input": "туп Строителей",      "output": "тупик Строителей",      "category": "abbreviation"},
    {"input": "ал Парковая",         "output": "аллея Парковая",        "category": "abbreviation"},
    {"input": "лин 1-я",             "output": "линия 1-я",             "category": "abbreviation"},
    {"input": "сп Крутой",           "output": "спуск Крутой",          "category": "abbreviation"},
    {"input": "мкр Северный",        "output": "микрорайон Северный",   "category": "abbreviation"},
    {"input": "мкр. Северный",       "output": "микрорайон Северный",   "category": "abbreviation"},
    {"input": "п Строителей",        "output": "переулок Строителей",   "category": "abbreviation"},
    {"input": "п. Строителей",       "output": "переулок Строителей",   "category": "abbreviation"},

    # === Oblique cases ===
    {"input": "улицы Ленина",        "output": "улица Ленина",           "category": "oblique"},
    {"input": "улице Ленина",        "output": "улица Ленина",           "category": "oblique"},
    {"input": "улицу Ленина",        "output": "улица Ленина",           "category": "oblique"},
    {"input": "проспекту Мира",      "output": "проспект Мира",          "category": "oblique"},
    {"input": "проспекта Мира",      "output": "проспект Мира",          "category": "oblique"},
    {"input": "проспектом Мира",     "output": "проспект Мира",          "category": "oblique"},
    {"input": "переулка Садового",   "output": "переулок Садового",      "category": "oblique"},
    {"input": "площади Ленина",      "output": "площадь Ленина",         "category": "oblique"},
    {"input": "набережной Обводного","output": "набережная Обводного",   "category": "oblique"},
    {"input": "бульвара Победы",     "output": "бульвар Победы",         "category": "oblique"},
    # Тип улицы в конце
    {"input": "Ленинский проспекта",  "output": "Ленинский проспект",    "category": "oblique"},
    {"input": "Тверская улицы",      "output": "Тверская улица",         "category": "oblique"},

    # === Combined (abbreviation + oblique) ===
    {"input": "пр-ту Мира",          "output": "проспект Мира",          "category": "combined"},
    {"input": "пр-та Мира",          "output": "проспект Мира",          "category": "combined"},

    # === Adjective agreement ===
    {"input": "Калининградской улица", "output": "Калининградская улица", "category": "adjective"},
    {"input": "Калининградской зоопарк", "output": "Калининградский зоопарк", "category": "adjective"},
    {"input": "Московской проспект",  "output": "Московский проспект",   "category": "adjective"},
    {"input": "Тверской улица",      "output": "Тверская улица",         "category": "adjective"},
    {"input": "Ленинградской набережная", "output": "Ленинградская набережная", "category": "adjective"},

    # === No-op (уже правильные) ===
    {"input": "улица Ленина",        "output": "улица Ленина",           "category": "noop"},
    {"input": "проспект Мира",       "output": "проспект Мира",          "category": "noop"},
    {"input": "бульвар Победы",      "output": "бульвар Победы",         "category": "noop"},
    {"input": "площадь Ленина",      "output": "площадь Ленина",         "category": "noop"},
    {"input": "набережная Обводного","output": "набережная Обводного",   "category": "noop"},
    {"input": "шоссе Энтузиастов",   "output": "шоссе Энтузиастов",     "category": "noop"},
    {"input": "Москва",              "output": "Москва",                 "category": "noop"},
    {"input": "Калининград",         "output": "Калининград",            "category": "noop"},
    {"input": "Большое Исаково",     "output": "Большое Исаково",       "category": "noop"},
    {"input": "Красная площадь",     "output": "Красная площадь",       "category": "noop"},
]


def evaluate_pytorch(model_path: str) -> dict:
    """Оценка через PyTorch/HuggingFace модель."""
    import torch
    from transformers import AutoTokenizer, AutoModelForSeq2SeqLM

    print(f"Загрузка модели: {model_path}")
    tokenizer = AutoTokenizer.from_pretrained(model_path)
    model = AutoModelForSeq2SeqLM.from_pretrained(model_path)

    device = "cuda" if torch.cuda.is_available() else "cpu"
    model = model.to(device)
    model.eval()

    results = []
    start = time.time()

    for tc in TEST_CASES:
        inputs = tokenizer(
            tc["input"],
            max_length=64,
            truncation=True,
            return_tensors="pt",
        ).to(device)

        with torch.no_grad():
            outputs = model.generate(
                **inputs,
                max_length=64,
                num_beams=1,
            )

        predicted = tokenizer.decode(outputs[0], skip_special_tokens=True)
        predicted = " ".join(predicted.split())

        results.append({
            **tc,
            "predicted": predicted,
            "match": predicted == tc["output"],
        })

    elapsed = time.time() - start
    return {
        "results": results,
        "elapsed": elapsed,
        "backend": "pytorch",
    }


def evaluate_onnx(model_path: str) -> dict:
    """Оценка через ONNX Runtime."""
    import numpy as np
    import onnxruntime as ort

    print(f"Загрузка ONNX модели: {model_path}")
    encoder_path = f"{model_path}/encoder.onnx"
    decoder_path = f"{model_path}/decoder.onnx"

    enc_session = ort.InferenceSession(encoder_path)
    dec_session = ort.InferenceSession(decoder_path)

    # Загружаем токенизатор
    from transformers import AutoTokenizer
    tokenizer = AutoTokenizer.from_pretrained(model_path)

    results = []
    start = time.time()

    for tc in TEST_CASES:
        inputs = tokenizer(
            tc["input"],
            max_length=64,
            truncation=True,
            padding="max_length",
            return_tensors="np",
        )

        # Encoder
        enc_out = enc_session.run(
            None,
            {
                "input_ids": inputs["input_ids"],
                "attention_mask": inputs["attention_mask"],
            },
        )
        encoder_hidden = enc_out[0]

        # Decoder (авторегрессивно)
        decoder_input = np.array([[tokenizer.pad_token_id or 0]], dtype=np.int64)
        bos_token_id = tokenizer.bos_token_id or 0
        eos_token_id = tokenizer.eos_token_id or 1

        if bos_token_id > 0:
            decoder_input = np.array([[bos_token_id]], dtype=np.int64)

        generated_ids = []
        for _ in range(64):
            dec_out = dec_session.run(
                None,
                {
                    "decoder_input_ids": decoder_input,
                    "encoder_hidden_states": encoder_hidden,
                },
            )
            logits = dec_out[0]
            next_token = int(np.argmax(logits[0, -1, :]))
            generated_ids.append(next_token)
            if next_token == eos_token_id:
                break
            decoder_input = np.array([generated_ids], dtype=np.int64)

        predicted = tokenizer.decode(generated_ids, skip_special_tokens=True)
        predicted = " ".join(predicted.split())

        results.append({
            **tc,
            "predicted": predicted,
            "match": predicted == tc["output"],
        })

    elapsed = time.time() - start
    return {
        "results": results,
        "elapsed": elapsed,
        "backend": "onnx",
    }


def evaluate_rust(bin_path: str) -> dict:
    """Оценка через Rust rule-based нормализатор (subprocess).

    Использует скрипт-обёртку для вызова Rust-функций.
    """
    # Пока возвращаем заглушку — реальный вызов через FFI или отдельный бинарник
    print("⚠️  Rust-бэкенд: используем rule-based нормализатор напрямую из Python")

    # Импортируем rule-based логику (дублирует src/normalizer.rs)
    from generate_dataset import ABBREVIATIONS, OBLIQUE_CASES, OBLIQUE_SUFFIXES

    def expand_abbreviations(text: str) -> str:
        trimmed = text.strip()
        if not trimmed:
            return text
        space = trimmed.find(" ")
        first = trimmed[:space] if space > 0 else trimmed
        rest = trimmed[space:] if space > 0 else ""
        key = first.lower().rstrip(".")

        for abbr, full in ABBREVIATIONS.items():
            if key == abbr:
                return f"{full}{rest}"
            # Сокращение + падежное окончание
            if key.startswith(abbr):
                suffix = key[len(abbr):]
                if suffix in OBLIQUE_SUFFIXES:
                    return f"{full}{rest}"

        return text

    def normalize_oblique(text: str) -> str:
        words = text.split()
        result = []
        for w in words:
            result.append(OBLIQUE_CASES.get(w.lower(), w))
        return " ".join(result)

    def normalize(text: str) -> str:
        return normalize_oblique(expand_abbreviations(text))

    results = []
    start = time.time()

    for tc in TEST_CASES:
        predicted = normalize(tc["input"])
        results.append({
            **tc,
            "predicted": predicted,
            "match": predicted == tc["output"],
        })

    elapsed = time.time() - start
    return {
        "results": results,
        "elapsed": elapsed,
        "backend": "rust-rule-based",
    }


def print_report(eval_result: dict) -> None:
    """Вывести отчёт об оценке."""
    results = eval_result["results"]
    elapsed = eval_result["elapsed"]
    backend = eval_result["backend"]

    total = len(results)
    matches = sum(1 for r in results if r["match"])

    print("\n" + "=" * 70)
    print(f"  Результаты оценки ({backend})")
    print("=" * 70)
    print(f"  Всего примеров:  {total}")
    print(f"  Exact Match:     {matches}/{total} ({matches/total:.1%})")
    print(f"  Время:           {elapsed:.3f}s ({elapsed/total*1000:.1f}ms/пример)")
    print()

    # По категориям
    categories = {}
    for r in results:
        cat = r["category"]
        if cat not in categories:
            categories[cat] = {"total": 0, "match": 0}
        categories[cat]["total"] += 1
        if r["match"]:
            categories[cat]["match"] += 1

    print("  По категориям:")
    print(f"  {'Категория':<25s} {'Точно':>8s} {'Всего':>8s} {'%':>8s}")
    print(f"  {'-'*25} {'-'*8} {'-'*8} {'-'*8}")
    for cat, stats in categories.items():
        pct = stats["match"] / stats["total"] if stats["total"] > 0 else 0
        print(f"  {cat:<25s} {stats['match']:>8d} {stats['total']:>8d} {pct:>7.1%}")

    # Ошибки
    errors = [r for r in results if not r["match"]]
    if errors:
        print(f"\n  Ошибки ({len(errors)}):")
        for r in errors:
            print(f"    ✗ {r['input']!r:35s} → {r['predicted']!r:35s} (ожид. {r['output']!r})")
    else:
        print(f"\n  ✓ Все примеры обработаны корректно!")


def main() -> None:
    parser = argparse.ArgumentParser(description="Оценка качества нормализатора")
    parser.add_argument("--backend", choices=["pytorch", "onnx", "rust"],
                        default="rust", help="Бэкенд для инференса")
    parser.add_argument("--model", default="./models/output/normalizer-final",
                        help="Путь к модели (для pytorch/onnx)")
    parser.add_argument("--bin", default="./target/release/osm-geo",
                        help="Путь к Rust-бинарнику (для rust)")
    parser.add_argument("--output", default=None,
                        help="Сохранить результаты в JSON")
    args = parser.parse_args()

    if args.backend == "pytorch":
        result = evaluate_pytorch(args.model)
    elif args.backend == "onnx":
        result = evaluate_onnx(args.model)
    elif args.backend == "rust":
        result = evaluate_rust(args.bin)
    else:
        sys.exit(f"Неизвестный бэкенд: {args.backend}")

    print_report(result)

    if args.output:
        with open(args.output, "w") as f:
            json.dump(result, f, indent=2, ensure_ascii=False, default=str)
        print(f"\nРезультаты сохранены: {args.output}")


if __name__ == "__main__":
    main()
