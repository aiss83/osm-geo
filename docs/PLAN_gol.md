# План интеграции формата GOL (GeoDesk)

## Цель

Добавить в `osm-geo` поддержку формата **GOL** (Geo-Object Library) от
[GeoDesk](https://geodesk.com):

1. Конвертация `osm.pbf` → `.gol`.
2. Выполнение задачи геокодирования (извлечение адресов и POI) не только
   напрямую из PBF, но и через GOL.
3. Единый программный интерфейс, позволяющий работать с обоими форматами
   без изменения остального пайплайна (нормализация, привязка городов/стран,
   дедупликация, индексация).

## Что такое GOL (резюме исследования)

GOL — компактная, read-оптимизированная одноплатформенная пространственная БД
для фич OpenStreetMap.

- Создаётся из `.osm.pbf`: `gol build <out.gol> <in.osm.pbf>` (вход — только PBF).
- Размер на 20–50% больше исходного PBF; запросы «в ~50 раз быстрее SQL»,
  конвертация «в ~20 раз быстрее импорта в SQL».
- Внутри: тайл-дерево (zoom-уровни) + R-tree на тайл, глобальная таблица строк
  (частые ключи/значения тегов), опционально ID нод геометрии (`--waynode-ids`).
- Координаты — 32-битные целые в сферической проекции Меркатора («imp»,
  разрешение ~1 см).
- Язык запросов GOQL: `na[amenity=restaurant][name]`, `w[highway][!oneway]`,
  regex `~`, числовые сравнения `>=`.
- `gol query` умеет выгружать в `brief`, `count`, `csv`, `geojson`/`geojsonl`,
  `list`, `pbf`, `wkt`, `xml`; фильтр `-b bbox` / `-a area`; выбор колонок `--keys`.
- Бинарь `gol` — self-contained (v2.3.2, Linux/macOS/Windows). Формат файла
  изменился в 2.0 и несовместим с 1.x.

**Важно для Rust:** официального Rust SDK нет (crates.io: 0 результатов). SDK есть
только для Java (`clarisma/geodesk`), C++ (`clarisma/libgeodesk`) и Python
(`clarisma/geodesk-py`).

## Способы интеграции с Rust

1. **CLI-подпроцесс** (`gol build` / `gol query`) — дёшево, без FFI.
   Минус: внешняя зависимость на бинарь `gol`.
2. **FFI к `libgeodesk` (C++)** — нужен C++20, CMake и C-шов поверх C++ API
   (шаблоны/итераторы). Средне-высокая трудоёмкость.
3. **Нативный Rust-читатель GOL** — реализация бинарного формата своими силами.
   Максимальная автономность, но формат сложный и публично не оформлен как
   открытая спека — высокий риск.

**Решение:** начинаем с варианта 1 (CLI), пряча его за трейтом источника, чтобы
позже безболезненно перейти на FFI/нативный читатель.

## Унифицированный интерфейс

Сейчас `cmd_build` жёстко завязан на `PbfParser`:

```
resolve_input → PbfParser.parse_file() → Vec<GeoObject>
  → infer_missing_cities → merge_typo_cities → infer_missing_countries
  → normalize → dedup → Indexer / CompactWriter
```

Вводим абстракцию источника:

```rust
pub trait FeatureSource {
    fn set_corrector(&mut self, corrector: Option<Corrector>);
    fn parse(&mut self, path: &Path) -> Result<Vec<GeoObject>>;
}
```

- `PbfParser` реализует `FeatureSource` (существующая логика osmpbf).
- `GolSource` реализует `FeatureSource` (чтение GOL).
- Всё, что ниже по пайплайну, не меняется.

Выбор источника в `cmd_build`: по расширению (`.gol` → GolSource, иначе PbfSource)
+ явный флаг `--source auto|pbf|gol`.

Для чтения GOL два внутренних пути (оба за трейтом):

- **A. `gol query … -f pbf | osmpbf`** — максимальное переиспользование
  существующего парсера. Требует сборки GOL с `--waynode-ids` (`-w`), иначе
  теряются ноды геометрии way/relation.
- **B. `gol query … -f geojsonl`** — парсим JSON-lines и прогоняем те же
  `extract_address` / `extract_named_object`. Надёжнее по геометрии, но
  дублирует часть extraction-логики.

На этапе 3 начинаем с пути A.

## Этапы

### Этап 0 — фиксация решений
CLI-интеграция; путь чтения A (`-f pbf`); сборка GOL с `-w`.

### Этап 1 — конвертация PBF → GOL
- `src/gol.rs`: `GolTool` — поиск бинаря `gol` (PATH / кэш / скачивание
  платформенной сборки), запуск `gol build -w <out.gol> <in.pbf>`.
- CLI: подкоманда `osm-geo convert --input region.osm.pbf --output region.gol`.

### Этап 2 — унификация источника
- `src/source.rs`: `trait FeatureSource`, `SourceKind`, `detect_source`.
- Рефакторинг `PbfParser` в реализацию `FeatureSource` (без изменения поведения).
- Автоопределение формата по расширению + флаг `--source`.

### Этап 3 — чтение GOL (GolSource)
- Подпроцесс `gol query region.gol '<GOQL>' -f pbf` → `ElementReader::new(stdout)`.
- GOQL под наши критерии извлечения (адреса `addr:*`, POI `historic`/`tourism`,
  `associatedStreet`, admin-boundary); на первой итерации допустим `*`.

### Этап 4 — сквозная проверка
- Один регион (например, `data/russia-kaliningrad-latest.osm.pbf`) через PBF и GOL.
- Сравнить `object_count`, `addr_count`, `named_count`, выборку координат/имён.
- Тест «GOL vs PBF» на маленьком экстракте.

### Этап 5 — опционально (будущее)
FFI-бэкенд или нативный читатель за тем же трейтом; оценка лицензии/редистрибуции
бинаря `gol`.

## Риски и открытые вопросы

- Внешний бинарь `gol`: лицензия и правила перераспределения при бандлинге.
- `--waynode-ids` (`-w`) обязателен для корректной геометрии way/relation в режиме
  `-f pbf`; это +~20% к размеру GOL.
- GOQL не поддерживает префикс по ключу тега (`addr:*`): фильтрацию придётся
  перечислять ключами либо запрашивать всё и резать существующей логикой.
- Формат GOL сменился в 2.0 и не обратно совместим — фиксируем версию `gol` 2.x.
- Двойное преобразование (`gol → pbf → osmpbf`) добавляет накладные расходы на
  сериализацию — цена за максимальное переиспользование кода.
