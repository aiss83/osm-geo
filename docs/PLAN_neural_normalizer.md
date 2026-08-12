# План: нейросетевая нормализация названий

> Статус: **реализовано** (v0.2.0)
> Дата плана: 2026-08-10 | Дата реализации: 2026-08-11

## Цель

Заменить эвристическую нормализацию названий в `corrector.rs` компактной нейросетью (ONNX), которая:

1. ~~Переводит имена (`name`) из osm.pbf на русский язык~~ (отложено — `name:ru` уже покрывает большинство случаев)
2. Приводит названия к именительному падежу и единому склонению
3. Раскрывает титульные сокращения (`ул` → `улица`, `пр` → `проспект`, ...)
4. Исправляет согласование прилагательных с существительными

---

## 1. Что изменилось в пайплайне

### Текущий поток (реализован)

```
PBF → parser (извлекает name + alt_names)
     → normalizer (rule-based ВСЕГДА: сокращения, падежи)
     → normalizer (ONNX опционально: согласование прилагательных через mt5-small)
     → corrector (SymSpell: опечатки, регистр, защита от перекоррекции)
     → dedup → indexer/compact
```

### Реализация

Модуль `src/normalizer.rs` (653 строки):
- **Rule-based уровень** — всегда активен, раскрывает 20+ сокращений и 30+ падежных форм. Точность 90.6%.
- **ONNX уровень** — опциональный, за feature-флагом `neural-normalizer`. Использует `tract-onnx` (чистый Rust, без системных библиотек) и `sentencepiece-rs` (чистый Rust) для токенизации.

---

## 2. Модуль: `src/normalizer.rs`

```rust
//! Нейросетевая нормализация названий — замена эвристик corrector.rs.
//!
//! Два уровня нормализации:
//! 1. Rule-based (всегда активен) — раскрытие сокращений и нормализация падежей.
//! 2. ONNX (за feature-флагом neural-normalizer) — согласование прилагательных.

pub struct Normalizer {
    cache: HashMap<String, String>,
    // ONNX (опционально):
    encoder_path: Option<PathBuf>,
    decoder_path: Option<PathBuf>,
    tokenizer: Option<SentencePieceProcessor>,
    // ...
}

impl Normalizer {
    pub fn new() -> Self { /* rule-based only */ }
    pub fn with_onnx(encoder: PathBuf, decoder: PathBuf, spiece: PathBuf) -> Result<Self> { /* ... */ }
    pub fn normalize(&mut self, name: &str) -> String { /* rule-based + опционально ONNX */ }
    pub fn normalize_objects(&mut self, objects: &mut [GeoObject]) { /* ... */ }
}
```

---

## 3. Выбор модели и runtime (реализованный)

| Компонент | Выбор | Причина |
|---|---|---|
| Runtime | **tract-onnx** (0.23) | Чистый Rust, без C++ зависимостей, кроссплатформенный |
| Токенизатор | **sentencepiece-rs** (0.2) | Чистый Rust, совместим с SentencePiece моделями |
| Базовая модель | **google/mt5-small** (300M) | Мультиязычный, хорошая русификация |
| Квантизация | INT8 | ~400 MB FP32 → ~141+269 MB encoder/decoder |
| Формат модели | Два файла: `encoder.onnx` + `decoder.onnx` | Авторегрессивная генерация с KV-кэшем |

---

## 4. Сокращения и падежи (rule-based, всегда активен)

| Сокращение | Полная форма | Сокращение | Полная форма |
|---|---|---|---|
| ул | улица | ул. | улица |
| пр | проспект | пр. | проспект |
| пр-т | проспект | пр-д | проезд |
| пер | переулок | пер. | переулок |
| бул | бульвар | бульв | бульвар |
| пл | площадь | пл. | площадь |
| наб | набережная | наб. | набережная |
| ш | шоссе | ш. | шоссе |
| туп | тупик | ал | аллея |
| лин | линия | сп | спуск |
| п | переулок | п. | переулок |

Падежные формы → именительный:

| Косвенный падеж | Именительный |
|---|---|
| улицы, улице, улицу, улицей | улица |
| проспекта, проспекту, проспектом | проспект |
| переулка, переулку, переулком | переулок |
| площади, площадью | площадь |
| набережной | набережная |

---

## 5. Интеграция в `main.rs` (реализовано)

Точка вставки — в `cmd_build`, после парсинга и перед корректором:

```rust
// В cmd_build:
let mut normalizer = normalizer::Normalizer::new();

// Опционально загружаем ONNX модель
#[cfg(feature = "neural-normalizer")]
if use_neural {
    normalizer = normalizer.with_onnx(encoder_path, decoder_path, spiece_path)?;
}

normalizer.normalize_objects(&mut objects);
```

---

## 6. Что осталось в corrector.rs

| Функция | Статус | Причина |
|---|---|---|
| SymSpell коррекция опечаток | **Оставлена** | Орфографические ошибки нейросеть не всегда исправит |
| `fix_adjective_agreement()` | `#[deprecated]` | Заменена на `normalizer::normalize_rule_based` |
| `normalize_case()` | **Оставлена** | Регистр — простая постобработка |
| `is_protected_word()` | **Оставлена** | Защита от SymSpell-перекоррекции |

---

## 7. Этапы реализации (все выполнены)

- [x] Выбрать базовую модель (mt5-small)
- [x] Конвертировать в ONNX, квантизировать до INT8
- [x] Создать синтетический датасет (~71K пар) — `models/generate_dataset.py`
- [x] Файнтюнинг на GPU — `models/train.py`
- [x] Оценить качество: 98.1% Exact Match — `models/evaluate.py`
- [x] Добавить зависимости `tract-onnx` и `sentencepiece-rs` в Cargo.toml
- [x] Реализовать `src/normalizer.rs` с rule-based + ONNX инференсом
- [x] Интегрировать в `cmd_build` (main.rs)
- [x] Feature-флаг `neural-normalizer` + `neural-tokenizer`
- [x] Кэширование результатов нормализации
- [x] Написать тесты для rule-based нормализатора
- [x] Обновить документацию (`DATABASE.md`, `README.md`)

---

## 8. Производительность

| Метрика | Значение |
|---|---|
| Размер модели (INT8) | encoder ~141 MB + decoder ~269 MB = ~410 MB |
| Время загрузки модели | 1–3 секунды |
| Инференс на одно название (rule-based) | < 0.1 мс |
| Инференс на одно название (ONNX) | 10–50 мс |
| Кэш нормализации | HashMap в памяти |

---

## 9. Риски и mitigation

| Риск | Mitigation |
|---|---|
| ONNX модель не загружена | Fallback на rule-based (90.6%) |
| Модель портит редкие названия | Кэш + ручная проверка на тестовом наборе |
| Зависимости конфликтуют | Feature-gate: `neural-normalizer` + `neural-tokenizer` |

---

## 10. Зависимости (Cargo.toml — фактические)

```toml
[features]
default = []
neural-normalizer = ["tract-onnx", "ndarray"]
neural-tokenizer = ["sentencepiece-rs"]

[dependencies]
tract-onnx = { version = "0.23", optional = true }
sentencepiece-rs = { version = "0.2", optional = true }
ndarray = { version = "0.15", optional = true }
```

---

## 11. Связанные файлы

| Файл | Статус |
|---|---|
| `src/normalizer.rs` | Реализован |
| `src/corrector.rs` | Частично упрощён (`fix_adjective_agreement` deprecated) |
| `src/parser.rs` | Без изменений (alt_names пока не игнорируются) |
| `src/main.rs` | Вызов normalizer в `cmd_build` |
| `Cargo.toml` | Feature-флаги `neural-normalizer`, `neural-tokenizer` |
| `DATABASE.md` | Документация обновлена |
| `README.md` | Обновлён |
| `models/` | ML-пайплайн: генерация, обучение, экспорт, оценка |
