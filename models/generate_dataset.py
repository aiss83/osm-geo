#!/usr/bin/env python3
"""
Генератор синтетического датасета для обучения нормализатора названий.

Генерирует три категории пар (input → output):
1. abbreviations — раскрытие сокращений (ул → улица, пр → проспект)
2. oblique      — нормализация падежей (улицы → улица, проспекту → проспект)
3. combined     — комбинированные случаи (пр-ту → проспект)

Выходные форматы:
- JSONL (для seq2seq моделей): {"input": "...", "output": "..."}
- TSV (для T5/mT5/NLLB): input\toutput

Использование:
    python generate_dataset.py [--output-dir ./data] [--size 50000]
"""

import argparse
import json
import os
import random
import sys
from pathlib import Path

# ─── Конфигурация ────────────────────────────────────────────────────────────

# Полные формы типов улиц (именительный падеж, ед. число)
STREET_TYPES: list[str] = [
    "улица", "проспект", "переулок", "бульвар", "проезд",
    "площадь", "набережная", "шоссе", "аллея", "тупик",
    "линия", "вал", "спуск", "дорога", "тракт",
    "мост", "канал", "съезд", "микрорайон",
]

# Сокращения → полная форма
ABBREVIATIONS: dict[str, str] = {
    "ул": "улица", "ул.": "улица",
    "пр": "проспект", "пр.": "проспект", "пр-т": "проспект",
    "пер": "переулок", "пер.": "переулок", "п": "переулок", "п.": "переулок",
    "бул": "бульвар", "бульв": "бульвар",
    "пл": "площадь", "пл.": "площадь",
    "наб": "набережная", "наб.": "набережная",
    "ш": "шоссе", "ш.": "шоссе",
    "пр-д": "проезд",
    "туп": "тупик",
    "ал": "аллея",
    "лин": "линия",
    "сп": "спуск",
    "мкр": "микрорайон", "мкр.": "микрорайон",
}

# Падежные формы типов улиц → именительный
OBLIQUE_CASES: dict[str, str] = {
    # улица
    "улицы": "улица", "улице": "улица", "улицу": "улица", "улицей": "улица",
    # проспект
    "проспекта": "проспект", "проспекту": "проспект", "проспектом": "проспект",
    # переулок
    "переулка": "переулок", "переулку": "переулок", "переулком": "переулок",
    # бульвар
    "бульвара": "бульвар", "бульвару": "бульвар", "бульваром": "бульвар",
    # площадь
    "площади": "площадь", "площадью": "площадь",
    # набережная
    "набережной": "набережная",
    # проезд
    "проезда": "проезд", "проезду": "проезд", "проездом": "проезд",
    # тупик
    "тупика": "тупик", "тупику": "тупик", "тупиком": "тупик",
    # аллея
    "аллеи": "аллея", "аллее": "аллея", "аллею": "аллея", "аллеей": "аллея",
    # линия
    "линии": "линия", "линией": "линия",
    # спуск
    "спуска": "спуск", "спуску": "спуск", "спуском": "спуск",
    # дорога
    "дороги": "дорога", "дороге": "дорога", "дорогу": "дорога", "дорогой": "дорога",
    # тракт
    "тракта": "тракт", "тракту": "тракт", "трактом": "тракт",
    # мост
    "моста": "мост", "мосту": "мост", "мостом": "мост",
    # микрорайон
    "микрорайона": "микрорайон", "микрорайону": "микрорайон", "микрорайоном": "микрорайон",
}

# Окончания для генерации падежных форм сокращений
OBLIQUE_SUFFIXES: list[str] = ["а", "у", "ом", "е", "ой", "ей", "ы", "и"]


def _reverse_abbr_map() -> dict[str, list[str]]:
    """Полная форма → список сокращений."""
    result: dict[str, list[str]] = {}
    for abbr, full in ABBREVIATIONS.items():
        result.setdefault(full, []).append(abbr)
    for full in result:
        result[full] = list(dict.fromkeys(result[full]))  # dedup
    return result


ABBR_TO_FULL = ABBREVIATIONS
FULL_TO_ABBRS = _reverse_abbr_map()


# ─── Реалистичные названия улиц (≈1000+) ─────────────────────────────────────

REAL_STREET_NAMES: list[str] = [
    # Улицы Москвы
    "Тверская", "Арбат", "Ленина", "Пушкина", "Гагарина", "Мира",
    "Садовая", "Лесная", "Полевая", "Нагорная", "Центральная",
    "Советская", "Молодёжная", "Школьная", "Октябрьская", "Кирова",
    "Первомайская", "Комсомольская", "Пионерская", "Строителей",
    "Московская", "Красная", "Зелёная", "Берёзовая", "Сосновая",
    "Парковая", "Речная", "Озёрная", "Солнечная", "Цветочная",
    "Вишнёвая", "Яблоневая", "Сиреневая", "Кленовая", "Дубовая",
    "Победы", "Свободы", "Космонавтов", "Энтузиастов",
    "Некрасова", "Чехова", "Толстого", "Достоевского", "Лермонтова",
    "Маяковского", "Есенина", "Горького", "Тургенева", "Булгакова",
    "Чайковского", "Репина", "Сурикова", "Айвазовского", "Васнецова",
    "Менделеева", "Ломоносова", "Циолковского", "Курчатова", "Королёва",
    "Жукова", "Рокоссовского", "Кутузова", "Суворова", "Нахимова",
    "Багратиона", "Ушакова", "Адмирала", "Генерала", "Маршала",
    "Большая", "Малая", "Средняя", "Верхняя", "Нижняя",
    "Новая", "Старая", "Дальняя", "Ближняя", "Крайняя",
    "Восточная", "Западная", "Северная", "Южная",
    "Весенняя", "Осенняя", "Зимняя", "Летняя",
    "Рабочая", "Трудовая", "Фабричная", "Заводская", "Индустриальная",
    "Вокзальная", "Станционная", "Деповская", "Локомотивная",
    "Спортивная", "Олимпийская", "Стадионная",
    "Театральная", "Музейная", "Библиотечная",
    "Больничная", "Аптечная", "Поликлиническая",
    # Проспекты
    "Ленинский", "Кутузовский", "Ломоносовский", "Вернадского",
    "Андропова", "Сахарова", "Мира", "Победы", "Славы",
    "Московский", "Невский", "Лиговский", "Вознесенский",
    "Рязанский", "Волгоградский", "Севастопольский", "Балаклавский",
    "Свободный", "Коммунистический", "Социалистический",
    # Переулки
    "Сивцев Вражек", "Камергерский", "Столешников", "Газетный",
    "Малый", "Большой", "Средний", "Тихий", "Кривой",
    "Садовый", "Цветочный", "Тенистый", "Солнечный",
    # Бульвары
    "Тверской", "Страстной", "Петровский", "Рождественский",
    "Сретенский", "Чистопрудный", "Покровский", "Яузский",
    # Площади
    "Красная", "Манежная", "Театральная", "Пушкинская",
    "Триумфальная", "Смоленская", "Таганская", "Комсомольская",
    # Набережные
    "Кремлёвская", "Софийская", "Берсеневская", "Пречистенская",
    "Фрунзенская", "Лужнецкая", "Новодевичья", "Андреевская",
    # Шоссе
    "Варшавское", "Каширское", "Дмитровское", "Ленинградское",
    "Можайское", "Рублёвское", "Новорижское", "Ярославское",
    "Волоколамское", "Щёлковское", "Алтуфьевское",
    "Энтузиастов", "Коровинское", "Дмитровское",
    # Микрорайоны
    "Северный", "Южный", "Центральный", "Солнечный",
    "Зелёный", "Прибрежный", "Лесной", "Радужный",
    # Города (для проверки, что не портим)
    "Москва", "Санкт-Петербург", "Новосибирск", "Екатеринбург",
    "Казань", "Нижний Новгород", "Челябинск", "Самара",
    "Омск", "Ростов-на-Дону", "Уфа", "Красноярск",
    "Воронеж", "Пермь", "Волгоград", "Краснодар",
    "Калининград", "Саратов", "Тюмень", "Тольятти",
    "Барнаул", "Ижевск", "Хабаровск", "Владивосток",
]

# ─── Генераторы ──────────────────────────────────────────────────────────────

def generate_abbreviation_pairs() -> list[tuple[str, str]]:
    """Пары: «сокращение + название» → «полная форма + название»."""
    pairs: list[tuple[str, str]] = []

    for name in REAL_STREET_NAMES:
        for abbr, full in ABBREVIATIONS.items():
            input_variants = [
                f"{abbr} {name}",
                f"{abbr}. {name}",
                f"{abbr}.{name}",
            ]
            output = f"{full} {name}"
            for inp in input_variants:
                pairs.append((inp, output))

    return pairs


def generate_oblique_pairs() -> list[tuple[str, str]]:
    """Пары: «название + падежная форма типа» → «название + именительный тип»."""
    pairs: list[tuple[str, str]] = []

    for name in REAL_STREET_NAMES:
        for oblique, canonical in OBLIQUE_CASES.items():
            # Тип улицы в начале: «улицы Ленина»
            input_prefix = f"{oblique} {name}"
            output_prefix = f"{canonical} {name}"
            pairs.append((input_prefix, output_prefix))

            # Тип улицы в конце: «Ленинский проспекта»
            input_suffix = f"{name} {oblique}"
            output_suffix = f"{name} {canonical}"
            pairs.append((input_suffix, output_suffix))

    return pairs


def generate_abbreviated_oblique_pairs() -> list[tuple[str, str]]:
    """Пары: «сокращение + падежное окончание» → «полная форма».

    Пример: «пр-ту Мира» → «проспект Мира»
    """
    pairs: list[tuple[str, str]] = []

    for name in REAL_STREET_NAMES:
        for abbr, full in ABBREVIATIONS.items():
            for suffix in OBLIQUE_SUFFIXES:
                # Пропускаем чистые точки и дефисы в аббревиатуре
                abbr_stem = abbr.rstrip(".")
                input_text = f"{abbr_stem}{suffix} {name}"
                output_text = f"{full} {name}"
                pairs.append((input_text, output_text))

                # С точкой тоже
                if "." in abbr:
                    input_text_dot = f"{abbr_stem}{suffix}. {name}"
                    pairs.append((input_text_dot, output_text))

    return pairs


def generate_adjective_agreement_pairs() -> list[tuple[str, str]]:
    """Пары: «Прилагательное в род. падеже + Существительное» → именительный."""
    pairs: list[tuple[str, str]] = []

    # Женский род: -ой→-ая, -ей→-яя
    feminine_pairs = [
        ("Московской", "Московская"),
        ("Тверской", "Тверская"),
        ("Ленинградской", "Ленинградская"),
        ("Калининградской", "Калининградская"),
        ("Казанской", "Казанская"),
        ("Сибирской", "Сибирская"),
        ("Российской", "Российская"),
        ("Большой", "Большая"),
        ("Красной", "Красная"),
        ("Весенней", "Весенняя"),
        ("Зимней", "Зимняя"),
        ("Летней", "Летняя"),
        ("Осенней", "Осенняя"),
        ("Соседней", "Соседняя"),
        ("Дальней", "Дальняя"),
    ]
    feminine_nouns = ["улица", "площадь", "набережная", "аллея", "дорога", "линия"]

    for adj_oblique, adj_nominative in feminine_pairs:
        for noun in feminine_nouns:
            input_text = f"{adj_oblique} {noun}"
            output_text = f"{adj_nominative} {noun}"
            pairs.append((input_text, output_text))

    # Мужской род: -ой→-ый/-ий, -ей→-ий
    masculine_pairs = [
        ("Ленинской", "Ленинский"),   # мягкая основа (к/г/х)
        ("Московской", "Московский"),
        ("Кутузовской", "Кутузовский"),
        ("Советской", "Советский"),
        ("Большой", "Большой"),       # твёрдая основа → -ой остаётся
        ("Красной", "Красный"),
        ("Малой", "Малый"),
        ("Синей", "Синий"),
        ("Последней", "Последний"),
    ]
    masculine_nouns = ["проспект", "переулок", "бульвар", "проезд", "тупик", "мост"]

    for adj_oblique, adj_nominative in masculine_pairs:
        for noun in masculine_nouns:
            input_text = f"{adj_oblique} {noun}"
            output_text = f"{adj_nominative} {noun}"
            pairs.append((input_text, output_text))

    # Средний род: -ой→-ое, -ей→-ее
    neuter_pairs = [
        ("Большой", "Большое"),
        ("Красной", "Красное"),
        ("Синей", "Синее"),
    ]
    neuter_nouns = ["шоссе", "озеро", "поле"]

    for adj_oblique, adj_nominative in neuter_pairs:
        for noun in neuter_nouns:
            input_text = f"{adj_oblique} {noun}"
            output_text = f"{adj_nominative} {noun}"
            pairs.append((input_text, output_text))

    # С реальными названиями
    real_examples = [
        ("Калининградской улица", "Калининградская улица"),
        ("Калининградской зоопарк", "Калининградский зоопарк"),
        ("Калининградской шоссе", "Калининградское шоссе"),
        ("Московской проспект", "Московский проспект"),
        ("Московской площадь", "Московская площадь"),
        ("Тверской улица", "Тверская улица"),
        ("Тверской бульвар", "Тверской бульвар"),
        ("Ленинградской набережная", "Ленинградская набережная"),
        ("Невской проспект", "Невский проспект"),
        ("Арбатской переулок", "Арбатский переулок"),
        ("Садовой кольцо", "Садовое кольцо"),
        ("Красной ворота", "Красные ворота"),
        ("Петровской парк", "Петровский парк"),
        ("Смоленской метро", "Смоленская метро"),
    ]
    pairs.extend(real_examples)

    return pairs


def generate_noop_pairs() -> list[tuple[str, str]]:
    """Пары, где вход уже корректен (чтобы модель не портила хорошее).

    Включает названия без типов улиц (чтобы модель пропускала их без изменений).
    """
    pairs: list[tuple[str, str]] = []

    # Правильные названия: тип улицы уже в именительном, без сокращений
    for stype in STREET_TYPES:
        for name in REAL_STREET_NAMES[:100]:
            text = f"{stype} {name}"
            pairs.append((text, text))

    # Города (не должны меняться)
    cities = [
        "Москва", "Санкт-Петербург", "Новосибирск", "Екатеринбург",
        "Казань", "Калининград", "Владивосток", "Сочи",
        "Нижний Новгород", "Ростов-на-Дону", "Красноярск", "Иркутск",
        "Большое Исаково", "Малое Исаково", "Старое Крюково",
        "Красная Поляна", "Зелёный Бор", "Солнечный Город",
    ]
    for city in cities:
        pairs.append((city, city))

    return pairs


# ─── Выход ───────────────────────────────────────────────────────────────────

def deduplicate(pairs: list[tuple[str, str]]) -> list[tuple[str, str]]:
    """Удалить дубликаты, сохранив порядок."""
    seen: set[tuple[str, str]] = set()
    result: list[tuple[str, str]] = []
    for p in pairs:
        if p not in seen:
            seen.add(p)
            result.append(p)
    return result


def write_jsonl(pairs: list[tuple[str, str]], path: str) -> None:
    """Записать пары в JSONL (подходит для HuggingFace datasets)."""
    with open(path, "w", encoding="utf-8") as f:
        for inp, out in pairs:
            record = {"input": inp, "output": out}
            f.write(json.dumps(record, ensure_ascii=False) + "\n")


def write_tsv(pairs: list[tuple[str, str]], path: str) -> None:
    """Записать пары в TSV (для прямого использования в Seq2Seq)."""
    with open(path, "w", encoding="utf-8") as f:
        f.write("input\toutput\n")
        for inp, out in pairs:
            f.write(f"{inp}\t{out}\n")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Генератор датасета для нормализатора названий"
    )
    parser.add_argument(
        "--output-dir",
        default="./models/data",
        help="Директория для выходных файлов",
    )
    parser.add_argument(
        "--split",
        type=float,
        default=0.9,
        help="Доля тренировочных данных (остальное — валидация)",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=42,
        help="Random seed",
    )
    args = parser.parse_args()

    random.seed(args.seed)

    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    print("=" * 60)
    print("Генерация синтетического датасета")
    print("=" * 60)

    # 1. Abbreviations
    print("\n[1/5] Генерация пар «сокращение → полная форма»...")
    abbr_pairs = generate_abbreviation_pairs()
    print(f"      Сгенерировано: {len(abbr_pairs)} пар")

    # 2. Oblique cases
    print("[2/5] Генерация пар «падеж → именительный»...")
    oblique_pairs = generate_oblique_pairs()
    print(f"      Сгенерировано: {len(oblique_pairs)} пар")

    # 3. Abbreviated + oblique
    print("[3/5] Генерация пар «сокращение+падеж → полная форма»...")
    abbr_oblique_pairs = generate_abbreviated_oblique_pairs()
    print(f"      Сгенерировано: {len(abbr_oblique_pairs)} пар")

    # 4. Adjective agreement
    print("[4/5] Генерация пар «согласование прилагательных»...")
    adj_pairs = generate_adjective_agreement_pairs()
    print(f"      Сгенерировано: {len(adj_pairs)} пар")

    # 5. No-op (уже правильные названия)
    print("[5/5] Генерация no-op пар (правильные названия)...")
    noop_pairs = generate_noop_pairs()
    print(f"      Сгенерировано: {len(noop_pairs)} пар")

    # Объединяем и дедуплицируем
    all_pairs = abbr_pairs + oblique_pairs + abbr_oblique_pairs + adj_pairs + noop_pairs
    all_pairs = deduplicate(all_pairs)
    print(f"\nВсего уникальных пар: {len(all_pairs)}")

    # Перемешиваем
    random.shuffle(all_pairs)

    # Разбиваем на train/val
    split_idx = int(len(all_pairs) * args.split)
    train_pairs = all_pairs[:split_idx]
    val_pairs = all_pairs[split_idx:]

    print(f"Train: {len(train_pairs)} пар")
    print(f"Val:   {len(val_pairs)} пар")

    # Записываем
    print(f"\nЗапись в {output_dir.resolve()}...")

    # JSONL (для HuggingFace datasets)
    write_jsonl(train_pairs, str(output_dir / "train.jsonl"))
    write_jsonl(val_pairs, str(output_dir / "val.jsonl"))

    # TSV (для прямого использования)
    write_tsv(train_pairs, str(output_dir / "train.tsv"))
    write_tsv(val_pairs, str(output_dir / "val.tsv"))

    # Статистика
    stats = {
        "total_pairs": len(all_pairs),
        "train_pairs": len(train_pairs),
        "val_pairs": len(val_pairs),
        "abbreviation_pairs": len(abbr_pairs),
        "oblique_pairs": len(oblique_pairs),
        "abbreviated_oblique_pairs": len(abbr_oblique_pairs),
        "adjective_agreement_pairs": len(adj_pairs),
        "noop_pairs": len(noop_pairs),
    }
    stats_path = output_dir / "stats.json"
    with open(stats_path, "w", encoding="utf-8") as f:
        json.dump(stats, f, ensure_ascii=False, indent=2)
    print(f"Статистика: {stats_path}")

    # Примеры
    print(f"\nПримеры тренировочных пар:")
    for inp, out in random.sample(train_pairs, min(10, len(train_pairs))):
        print(f"  {inp!r:40s} → {out!r}")

    print("\nГотово!")


if __name__ == "__main__":
    main()
