# Формат базы данных osm-geo

> **Целевая аудитория:** разработчик мобильной библиотеки-потребителя (Android/iOS), которая открывает эту базу и выполняет поиск.

## 1. Обзор

osm-geo поддерживает два формата выходных данных:

**SQLite** (`.db`) — для отладки и случаев, когда нужны FTS5/R*Tree на устройстве.

**Compact binary** (`.bin`) — для production: в 4-6× компактнее, без зависимостей от SQLite/FTS5.

Оба формата сжимаются Zstandard (`.zst`) для транспортировки.

### SQLite

База данных osm-geo в формате SQLite — это **один файл** (`.db`), содержащий:

- адреса (страна → город → улица → дом) с координатами,
- именованные объекты (кафе, памятники, музеи, административные единицы),
- полнотекстовый индекс (FTS5) для поиска по тексту с учётом русской морфологии,
- пространственный индекс (B-tree) для поиска по географическим координатам,
- таблицу метаданных.

База **НЕ содержит** встроенного поискового движка — все запросы выполняются через стандартное SQLite API средствами мобильной библиотеки-потребителя.

Файл базы может быть сжат алгоритмом Zstandard (`.db.zst`). Перед использованием на устройстве файл необходимо разжать.

---

## 2. SQLite-схема

База создаётся со следующими прагмами:

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA mmap_size = 268435456;  -- 256 МБ
PRAGMA cache_size = -65536;    -- 64 МБ страничного кэша
PRAGMA page_size = 4096;
```

### 2.1. Таблица `objects` — основные данные

Все поля хранятся в плоских колонках (BLOB не используется).

```sql
CREATE TABLE objects (
    id          INTEGER PRIMARY KEY,
    type        INTEGER NOT NULL,   -- 0 = Address, 1 = NamedObject
    lat         REAL    NOT NULL,   -- широта в градусах (WGS 84)
    lon         REAL    NOT NULL,   -- долгота в градусах (WGS 84)
    -- Address fields (type = 0)
    country      TEXT,
    city         TEXT,
    street       TEXT,
    housenumber  TEXT,
    postcode     TEXT,
    -- NamedObject fields (type = 1)
    name         TEXT,
    translit     TEXT,
    category     TEXT,
);
```

| Колонка | Тип | Описание |
|---------|-----|----------|
| `id` | INTEGER | Автоинкрементный первичный ключ, используется как `rowid` в FTS |
| `type` | INTEGER | `0` — адрес, `1` — POI |
| `lat`, `lon` | REAL | Координаты WGS 84 |
| `country`…`postcode` | TEXT | Адресные поля (NULL для type=1) |
| `name` | TEXT | Русское название POI (name:ru или name) |
| `translit` | TEXT | Транслитерация названия латиницей |
| `category` | TEXT | Категория: amenity, tourism, shop, … |

### 2.2. FTS5: `fts_address` — полнотекстовый индекс адресов

```sql
CREATE VIRTUAL TABLE fts_address USING fts5(
    country, city, street, housenumber, postcode,
    tokenize = 'unicode61',
    content  = '',
    prefix   = '2 3 4'
);
```

**Что индексируется:** для каждого адреса поля `country`, `city`, `street` проходят через **русский snowball-стеммер** и записываются в нижнем регистре. Поля `housenumber` и `postcode` записываются как есть (стемминг не применяется).

**Как искать:**

```sql
-- Префиксный поиск (стемминг на стороне клиента обязателен!)
SELECT rowid, rank
FROM fts_address
WHERE fts_address MATCH 'москв* тверск*'
ORDER BY rank
LIMIT 20;
```

**Важно:** токенизатор — `unicode61` (НЕ `trigram`). Стемминг выполняется на этапе сборки и **не выполняется** в SQLite при поиске. Мобильная библиотека **обязана** применять тот же snowball-стеммер (`Algorithm::Russian`) к поисковому запросу перед отправкой в FTS5 и добавлять `*` к каждому токену для префиксного поиска.

**Пример стемминга запроса (псевдокод):**

```
"Москва Тверская улица 15"
  → стемминг каждого слова
  → "москв тверск улиц 15"
  → добавляем * к токенам > 2 символов
  → "москв* тверск* улиц* 15"
  → отправляем в MATCH
```

Параметр `prefix = '2 3 4'` означает, что FTS5 хранит префиксы длиной 2, 3 и 4 символа для каждого токена. Это позволяет искать по первым буквам: `"тв*"` найдёт «Тверская».

### 2.3. FTS5: `fts_named` — полнотекстовый индекс именованных объектов

```sql
CREATE VIRTUAL TABLE fts_named USING fts5(
    name, translit, category,
    tokenize = 'unicode61',
    content  = '',
    prefix   = '2 3 4'
);
```

| Поле | Содержимое |
|------|------------|
| `name` | Основное русское название: `name:ru` или `name` (стемминг) |
| `translit` | Транслитерация названия латиницей (стемминг). Позволяет искать латиницей: `"moskv*"` → «Москва» |
| `category` | Категория объекта: `amenity`, `tourism`, `shop`, `highway`, `boundary`, … (без стемминга) |

Правила поиска и стемминга — те же, что для `fts_address`.

### 2.4. B-tree: `idx_objects_lat_lon` — пространственный индекс

Для точечных объектов используется **составной B-tree индекс** вместо R\*Tree.
Это экономит ~21% размера базы при той же производительности bounding-box запросов.

```sql
CREATE INDEX idx_objects_lat_lon ON objects(lat, lon);
```

**Поиск по bounding box:**

```sql
-- Все объекты в прямоугольнике: широта [55.7, 55.8], долгота [37.5, 37.7]
SELECT id, lat, lon FROM objects
WHERE lat BETWEEN 55.7 AND 55.8
  AND lon BETWEEN 37.5 AND 37.7;
```

**Сортировка по расстоянию до фокусной точки:**

```sql
-- Объекты в радиусе ~1 км от точки (55.7558, 37.6173), упорядоченные по расстоянию
SELECT id, lat, lon
FROM objects
WHERE lat BETWEEN 55.7468 AND 55.7648
  AND lon BETWEEN 37.6023 AND 37.6323
ORDER BY (lat - 55.7558)*(lat - 55.7558) + (lon - 37.6173)*(lon - 37.6173)
LIMIT 20;
```

Приближённая формула: 1° широты ≈ 111.32 км, 1° долготы ≈ 111.32 × cos(lat) км.

### 2.5. Таблица `meta` — метаданные

```sql
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

Типовые ключи:

| Ключ | Пример значения | Описание |
|------|----------------|----------|
| `version` | `0.1.0` | Версия osm-geo, собравшего базу |
| `region` | `RU-CFD` | Код региона |
| `build_date` | `2026-07-10` | Дата сборки (ISO 8601) |
| `source` | `/path/to/file.osm.pbf` | Путь к исходному PBF-файлу |
| `object_count` | `2260484` | Общее количество объектов |
| `addr_count` | `1377584` | Количество адресов |
| `named_count` | `882900` | Количество POI |

---

## 3. Категории NamedObject

Категории извлекаются из следующих OSM-тегов (в порядке приоритета):

| Тег OSM | Примеры значений |
|---------|-----------------|
| `amenity` | cafe, restaurant, school, hospital, bank, pharmacy, parking |
| `tourism` | hotel, museum, attraction, viewpoint |
| `shop` | supermarket, bakery, clothes, mall |
| `historic` | monument, memorial, castle, ruins |
| `leisure` | park, playground, sports_centre |
| `office` | government, company, ngo |
| `boundary` | administrative |
| `highway` | bus_stop, crossing, traffic_signals |
| `railway` | station, halt, tram_stop |
| `aeroway` | aerodrome, terminal |
| `waterway` | dock |
| `natural` | peak, spring, beach |

Административные единицы (`boundary=administrative`) получают категорию вида `admin_level:N`, где N — уровень админки OSM (2 = страна, 4 = регион, 6 = район, 8 = город, …).

---

## 4. Сжатие и транспортировка

### 4.1. Сжатый файл

База сжимается алгоритмом Zstandard (уровень 3) в файл с расширением `.db.zst`:

```
cfd.db        (593 MB)  — исходный SQLite-файл
cfd.db.zst    (209 MB)  — сжатый Zstd, для передачи на устройство
```

Мобильное приложение должно:
1. Скопировать `.db.zst` на устройство.
2. Разжать в `.db` (Zstd-декомпрессия).
3. Открыть `.db` через SQLite API.

### 4.2. Файл метаданных

Рядом со сжатой базой создаётся JSON-файл метаданных:

```json
{
  "version": "0.1.0",
  "region": "RU-CFD",
  "build_date": "2026-07-10",
  "source_pbf": "central-fed-district-latest.osm.pbf",
  "object_count": 2260484,
  "address_count": 1377584,
  "named_count": 882900,
  "db_size_bytes": 621547520,
  "compressed_size_bytes": null,
  "sha256": "8d88be222acd2791d190f1519a117f9f88499526eaca4202d509bc5f8256ab63"
}
```

Поле `sha256` содержит хеш **несжатого** `.db` файла. Мобильное приложение может проверить целостность базы после разжатия.

---

## 5. Рекомендуемые паттерны запросов

### 5.1. Полнотекстовый поиск адреса

```
Пользовательский ввод: "Москва Тверская 15"

Шаги мобильной библиотеки:
1. Стемминг каждого слова запроса (snowball, русский):
   "Москва" → "москв"
   "Тверская" → "тверск"
   "15" → "15"

2. Формирование FTS5-запроса:
   "москв* тверск* 15"

3. Выполнение:
   SELECT objects.id, objects.lat, objects.lon,
          objects.country, objects.city, objects.street, objects.housenumber
   FROM fts_address
   JOIN objects ON objects.id = fts_address.rowid
   WHERE fts_address MATCH 'москв* тверск* 15'
   ORDER BY rank
   LIMIT 20;

4. Чтение полей адреса из колонок objects (country, city, street, housenumber)
5. Отображение результатов пользователю
```

### 5.2. Полнотекстовый поиск POI

```
Пользовательский ввод: "красная площадь"

1. Стемминг: "красн* площад*"
2. FTS5: fts_named MATCH 'красн* площад*'
3. JOIN objects → десериализация NamedObject
```

### 5.3. Поиск POI с фильтром по категории

```sql
-- Все кафе, содержащие "шоколад" в названии
SELECT o.id, o.lat, o.lon, o.name, o.category
FROM fts_named
JOIN objects o ON o.id = fts_named.rowid
WHERE fts_named MATCH 'шоколад*'
  AND fts_named.category = 'amenity'
LIMIT 20;
```

### 5.4. Пространственный поиск: объекты рядом

```sql
-- POI в радиусе ~500м от пользователя (55.7558, 37.6173)
SELECT o.id, o.lat, o.lon, o.name, o.category,
       ((o.lat - 55.7558)*(o.lat - 55.7558) + (o.lon - 37.6173)*(o.lon - 37.6173)) AS dist2
FROM objects o
WHERE o.lat BETWEEN 55.7513 AND 55.7603
  AND o.lon BETWEEN 37.6110 AND 37.6236
  AND o.type = 1             -- только POI
ORDER BY dist2
LIMIT 20;
```

### 5.5. Комбинированный поиск: текст + координаты

```sql
-- Кафе "шоколад" рядом с пользователем
SELECT o.*
FROM fts_named
JOIN objects o ON o.id = fts_named.rowid
WHERE fts_named MATCH 'шоколад*'
  AND o.lat BETWEEN :min_lat AND :max_lat
  AND o.lon BETWEEN :min_lon AND :max_lon
ORDER BY rank
LIMIT 20;
```

---

## 6. Версионирование и совместимость

| Параметр | Значение |
|----------|----------|
| SQLite-совместимость | ≥ 3.20 (FTS5) |
| Стеммер | Snowball (UTF-8 Russian) |
| Транслитерация | Внутренняя схема osm-geo (Yandex/Google-совместимая) |

При смене версии osm-geo поле `meta.version` обновляется. Базы, собранные более новыми версиями, могут содержать дополнительные колонки — библиотека-потребитель должна выбирать только известные ей поля (SELECT col1, col2, …).

---

## 7. Зависимости для мобильной библиотеки

Минимальный набор для чтения базы:

| Платформа | Компонент |
|-----------|-----------|
| **Android** | `android.database.sqlite.SQLiteDatabase` (встроен) |
| **iOS** | `libsqlite3.tbd` (встроен) |
| **Стеммер (рус.)** | Snowball stemmer: `org.tartarus.snowball.ext.RussianStemmer` (lucene-analyzers-common) / `Snowball` Swift package |
| **Zstd** | zstd-jni (Android), ZstdKit (iOS) |

Все компоненты доступны под MIT/Apache-2.0 лицензиями.

---

## 8. Пример: полный цикл на Android (Kotlin)

```kotlin
// 1. Разжать базу
val dbFile = File(cacheDir, "cfd.db")
ZstdInputStream(assets.open("cfd.db.zst")).use { input ->
    dbFile.outputStream().use { output -> input.copyTo(output) }
}

// 2. Проверить целостность
val sha256 = MessageDigest.getInstance("SHA-256")
    .digest(dbFile.readBytes())
    .joinToString("") { "%02x".format(it) }
// Сравнить с sha256 из metadata.json

// 3. Открыть SQLite
val db = SQLiteDatabase.openDatabase(
    dbFile.absolutePath, null, SQLiteDatabase.OPEN_READONLY
)

// 4. Стеммер
val stemmer = RussianStemmer()

// 5. Поиск
fun search(query: String): List<GeoObject> {
    val stemmed = query.split(" ")
        .map { stemmer.stem(it.lowercase()) + "*" }
        .joinToString(" ")

    val cursor = db.rawQuery("""
        SELECT o.id, o.lat, o.lon, o.name, o.category
        FROM fts_named
        JOIN objects o ON o.id = fts_named.rowid
        WHERE fts_named MATCH ?
        ORDER BY rank
        LIMIT 20
    """, arrayOf(stemmed))

    return cursor.use { cur ->
        generateSequence { if (cur.moveToNext()) cur else null }
            .map { cur ->
                GeoObject(
                    id = cur.getLong(0),
                    lat = cur.getDouble(1),
                    lon = cur.getDouble(2),
                    name = cur.getString(3) ?: "",
                    category = cur.getString(4) ?: ""
                )
            }
            .toList()
    }
}

---

## 9. Компактный бинарный формат (`.bin`)

Альтернативный формат без SQLite. Предназначен для production-использования на мобильных устройствах.

### 9.1. Сборка

```bash
osm-geo build --input russia/central-fed-district --format compact --output cfd.bin
```

### 9.2. Структура файла

| Секция | Размер | Описание |
|--------|--------|----------|
| Header | 24 B | magic ("OSMG"), version, offsets |
| String pool | ~10 MB | Все уникальные строки (города, улицы, названия). Индекс 0 — пустая строка. |
| Named Index | ~8 MB | Сортирован по name → бинарный поиск для префиксного поиска |
| Address Index | ~14 MB | Сортирован по (city, street, housenumber) |
| Record Block | ~30 MB | Сортирован по (lat, lon) → бинарный поиск для пространственных запросов |

### 9.3. Записи

**Record Block** (сортирован по lat, lon):
```
type:     u8      — 0=Address, 1=Named
lat:      f32
lon:      f32
Address:  city_idx:u16, street_idx:u16, housenumber_idx:u16   (9 байт)
Named:    name_idx:u16, translit_idx:u16, category:u8         (6 байт)
```

**Named Index** (сортирован по строке имени):
```
name_idx:      u16
translit_idx:  u16
category:      u8    — тег категории (1=amenity, 2=tourism, …)
record_idx:    u32   — индекс в Record Block
```

**Address Index** (сортирован по city, street, housenumber):
```
city_idx:         u16
street_idx:       u16
housenumber_idx:  u16
record_idx:       u32
```

### 9.4. Поиск

Потребитель выполняет бинарный поиск по сортированным массивам:

- **По имени**: бинарный поиск в Named Index → получение record_idx → чтение координат из Record Block
- **По адресу**: бинарный поиск в Address Index → record_idx → координаты
- **По координатам**: бинарный поиск в Record Block по lat

### 9.5. Размеры (ЦФО, 2.26M объектов)

| Формат | Без сжатия | Zstd |
|--------|-----------|------|
| SQLite | 434 MB | 160 MB |
| Compact | 71 MB | 35 MB |

---

## 10. CLI-справочник

### Сборка базы

```bash
# SQLite (по умолчанию)
osm-geo build --input russia/central-fed-district --region RU-CFD

# Компактный бинарный формат
osm-geo build --input russia/central-fed-district --format compact

# Из сжатого PBF (авто-распаковка .gz / .zst)
osm-geo build --input data/region.osm.pbf.gz
osm-geo build --input data/region.osm.pbf.zst

# Из локального файла
osm-geo build --input /path/to/file.osm.pbf --output my.db

# Из URL
osm-geo build --input https://example.com/region.osm.pbf
```

### Список регионов

```bash
# Континенты / страны
osm-geo list

# Подрегионы России
osm-geo list russia
```

### Просмотр базы

```bash
# Метаданные
osm-geo info --db data/cfd.db

# Тестовый поиск
osm-geo query --db data/cfd.db "Москва Тверская"
```
```
