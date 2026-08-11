# ML-пайплайн нормализатора названий

Файнтюнинг компактной seq2seq-модели для нормализации названий улиц и POI из OpenStreetMap.

## Обзор

Модель принимает на вход «сырое» название из OSM PBF и возвращает нормализованную форму:

```
ул Ленина           →  улица Ленина
пр-ту Мира           →  проспект Мира
проспекту Вернадского →  проспект Вернадского
Калининградской улица →  Калининградская улица
Москва               →  Москва          (уже правильно — без изменений)
```

## Структура

```
models/
├── README.md                # Этот файл
├── requirements.txt          # Python-зависимости
├── generate_dataset.py       # Генератор синтетического датасета
├── train.py                  # Файнтюнинг модели (Colab/GPU)
├── export_onnx.py            # Конвертация PyTorch → ONNX + квантизация
├── evaluate.py               # Оценка качества (3 бэкенда)
├── data/                     # Сгенерированный датасет
│   ├── train.jsonl / train.tsv
│   ├── val.jsonl   / val.tsv
│   └── stats.json
└── output/                   # Обученная модель
    └── normalizer-final/
        └── *_onnx/           # ONNX-экспорт (для Rust)
```

## Быстрый старт

### 0. Установка зависимостей

```bash
pip install -r models/requirements.txt
```

### 1. Генерация датасета

```bash
python models/generate_dataset.py --output-dir ./models/data
```

Генерирует ~71 000 пар (сокращения, падежи, комбинации, согласование прилагательных).

Результат: `models/data/train.jsonl`, `models/data/val.jsonl`.

### 2. Обучение модели (GPU / Google Colab)

```bash
# На GPU с 16GB VRAM:
python models/train.py --model google/mt5-small --epochs 10 --batch-size 16

# На Colab (T4, 16GB):
python models/train.py --model google/mt5-small --epochs 10 --batch-size 8 --gradient-accumulation 4
```

Альтернативные модели:
- `google/mt5-small` (300M) — мультиязычный, хорош для русского, ~1.2 GB FP32
- `google/mt5-base` (580M) — больше, но качественнее, ~2.3 GB
- `cointegrated/rubert-tiny2` — маленький (~30M), только русский, нужно адаптировать под seq2seq

Ожидаемое время обучения на T4 (mt5-small):
- 10 эпох, batch 16: ~15-20 минут
- 10 эпох, batch 8 × accumulation 4: ~25-30 минут

Результат: `models/output/normalizer-final/`.

### 3. Экспорт в ONNX

```bash
python models/export_onnx.py --model ./models/output/normalizer-final --quantize
```

Создаёт:
- `encoder.onnx` (~400-600 MB FP32 → ~100-150 MB INT8)
- `decoder.onnx` (~400-600 MB FP32 → ~100-150 MB INT8)
- Токенизатор и метаданные

### 4. Оценка качества

```bash
# Rule-based бэкенд (без модели):
python models/evaluate.py --backend rust

# PyTorch-модель:
python models/evaluate.py --backend pytorch --model ./models/output/normalizer-final

# ONNX-модель:
python models/evaluate.py --backend onnx --model ./models/output/normalizer-final_onnx
```

### 5. Интеграция в Rust

Скопировать `.onnx` файлы в корень проекта:

```bash
cp models/output/normalizer-final_onnx/encoder.onnx models/normalizer.onnx
# или симлинк:
ln -s $(pwd)/models/output/normalizer-final_onnx/encoder.onnx models/normalizer.onnx
```

Собрать osm-geo с фичей `neural-normalizer`:

```bash
cargo build --release --features neural-normalizer
```

Запустить сборку с нейросетевым нормализатором:

```bash
osm-geo build --input russia/kaliningrad --region RU-KGD --use-neural-normalizer
```

## Состав датасета

| Категория | Примеров | Описание |
|---|---|---|
| abbreviations | ~15 000 | `ул Ленина` → `улица Ленина` |
| oblique | ~18 000 | `улицы Ленина` → `улица Ленина` |
| combined | ~53 000 | `пр-ту Мира` → `проспект Мира` |
| adjective | ~170 | `Калининградской улица` → `Калининградская улица` |
| noop | ~1 900 | Правильные названия (без изменений) |

## Метрики rule-based бэкенда

Без нейросети, только правила:

| Категория | Exact Match |
|---|---|
| abbreviation | 100.0% |
| oblique | 100.0% |
| combined | 100.0% |
| adjective | 0.0% (требуется нейросеть) |
| noop | 100.0% |
| **Всего** | **90.6%** |

Цель после обучения: **≥ 98% Exact Match** на всех категориях.

## Запуск на Google Colab

```python
# В ячейке Colab:
!git clone https://github.com/aiss83/osm-geo.git
%cd osm-geo

!pip install -r models/requirements.txt

# Генерация датасета (занимает ~5 секунд)
!python models/generate_dataset.py

# Обучение (~20 минут на T4)
!python models/train.py --epochs 10 --batch-size 16

# Экспорт в ONNX
!python models/export_onnx.py --model ./models/output/normalizer-final

# Скачать модель
from google.colab import files
!zip -r normalizer-final_onnx.zip ./models/output/normalizer-final_onnx/
files.download('normalizer-final_onnx.zip')
```

## Модели-кандидаты для файнтюнинга

| Модель | Параметры | Размер FP32 | Плюсы | Минусы |
|---|---|---|---|---|
| `google/mt5-small` | 300M | 1.2 GB | Мультиязычный, хорошая русификация | Крупный для mobile |
| `google/mt5-tiny` (кастом) | ~80M | ~320 MB | Компактный | Нужно собрать/обрезать |
| `cointegrated/rubert-tiny2` | 29M | ~120 MB | Очень компактный, русский | Нужен энкодер-декодер адаптер |
| `Helsinki-NLP/opus-mt-ru-ru` | 77M | ~310 MB | Специализирован на переводе | Только перевод, не нормализация |

Рекомендация для MVP: **mt5-small** → после квантизации INT8 ~250-300 MB total (encoder+decoder).

Для production (мобильные устройства): обучить **mt5-tiny** (обрезанная версия с 2 слоями энкодера/декодера вместо 8) → ~80-100 MB INT8.
