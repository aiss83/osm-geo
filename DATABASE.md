# Формат базы данных osm-geo

> **Целевая аудитория:** разработчик мобильной библиотеки-потребителя (Android/iOS), которая открывает эту базу и выполняет поиск.

## 1. Обзор

osm-geo поддерживает два формата выходных данных:

**SQLite** (`.db`) — для отладки и случаев, когда нужны FTS5/R*Tree на устройстве.

**Compact binary** (`.bin`) — для production: в 4-6× компактнее, без зависимостей от SQLite/FTS5.

Оба формата сжимаются Zstandard (`.zst`) для транспортировки.

### Обработка текста

На этапе сборки все названия улиц и городов проходят через несколько стадий очистки:

1. **Нормализация названий** (всегда активна) — раскрытие сокращений и нормализация падежей:
   - Сокращения: `ул` → `улица`, `пр-т` → `проспект`, `пл` → `площадь` (20+ типов)
   - Падежи: `улицы` → `улица`, `проспекту` → `проспект` (30+ форм)
   - Комбинации: `пр-ту Мира` → `проспект Мира`
   
   Точность rule-based нормализатора: **90.6%**.

   Опционально — нейросетевой нормализатор (mt5-small, ONNX), повышающий точность до **98.1%**:
   - Исправляет согласование прилагательных: `Калининградской улица` → `Калининградская улица`
   - Требует `libonnxruntime` и ONNX-модели в `models/` (см. [README](README.md#сборка-с-нейросетевым-нормализатором-onnx))

2. **Коррекция опечаток** (SymSpell) — по частотному словарю русского языка (`ru_full.txt`, ~1.4M слов, 28 МБ). Словарь включён в репозиторий; при его отсутствии скачивается автоматически. Исправляет типичные опечатки: «Масква» → «Москва».
    
    Из коррекции исключены (защищены от ложной перекоррекции):
    - Типы улиц (улица, проспект, переулок, бульвар, ...)
    - Названия городов с характерными окончаниями (-ск, -цк, -град, -бург, -поль)
    - Топонимы на -ово/-ево/-ино (Исаково, Бородино, ...)

### Постобработка адресов

После парсинга выполняются два дополнительных шага для повышения качества адресных данных:

5. **Привязка к городам** — адреса без поля `addr:city` автоматически привязываются к ближайшему городу по координатам (Haversine distance). Строится карта центроидов городов по адресам с известным городом, затем каждый бесхозный адрес получает ближайший город.

6. **Склеивание опечаток в городах** — названия городов, отличающиеся от более частотных на 1–2 символа (Левенштейн), объединяются: «Калининнград» → «Калининград». Порог: опечаточный город должен иметь минимум вдвое меньше адресов, чем канонический.

### Фильтрация POI

В базу попадают именованные объекты трёх категорий: `historic` (памятники, мемориалы), `shop` (магазины, ТЦ), `tourism` (зоопарки, музеи, отели). Остановки транспорта, дороги, офисы, административные границы и природные объекты — не включаются.

Коррекция уменьшает задвоение записей из-за опечаток, разного регистра и падежных несогласованностей в исходных данных OSM.

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
    category     TEXT
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
| Header | 88 B | magic, version, счётчики, timestamp, регион, offsets (см. 9.3) |
| String pool | ~10 MB | Все уникальные строки (города, улицы, названия). Индекс 0 — пустая строка. |
| Named Index | ~8 MB | Сортирован по name → бинарный поиск для префиксного поиска |
| Address Index | ~14 MB | Сортирован по (city, street, housenumber) |
| Record Block | ~30 MB | Сортирован по (lat, lon) → бинарный поиск для пространственных запросов |

### 9.3. Заголовок (Header, 88 байт, little-endian)

| Смещение | Размер | Поле | Описание |
|----------|--------|------|----------|
| 0 | 4 B | `magic` | `OSMG` (0x4F 0x53 0x4D 0x47) |
| 4 | 2 B | `version` | Версия формата (1) |
| 6 | 4 B | `record_count` | Общее количество объектов |
| 10 | 4 B | `addr_count` | Количество адресов |
| 14 | 4 B | `named_count` | Количество POI |
| 18 | 8 B | `build_timestamp` | Unix timestamp времени сборки |
| 26 | 46 B | `region` | Код региона, UTF-8, zero-padded (напр. `RU-CFD`) |
| 72 | 4 B | `string_pool_offset` | Смещение до String Pool |
| 76 | 4 B | `named_index_offset` | Смещение до Named Index |
| 80 | 4 B | `addr_index_offset` | Смещение до Address Index |
| 84 | 4 B | `records_offset` | Смещение до Record Block |

### 9.4. Записи

**Record Block** (сортирован по lat, lon):
```
type:     u8      — 0=Address, 1=Named
lat:      f32
lon:      f32
Address:  city_idx:u16, street_idx:u16, housenumber_idx:u16   (6 байт, 2+2+2)
Named:    name_idx:u16, translit_idx:u16, category:u8         (5 байт, 2+2+1)
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

### 9.5. Поиск

Потребитель выполняет бинарный поиск по сортированным массивам:

- **По имени**: бинарный поиск в Named Index → получение record_idx → чтение координат из Record Block
- **По адресу**: бинарный поиск в Address Index → record_idx → координаты
- **По координатам**: бинарный поиск в Record Block по lat

### 9.6. Размеры (ЦФО, 2.26M объектов)

| Формат | Без сжатия | Zstd |
|--------|-----------|------|
| SQLite | 434 MB | 160 MB |
| Compact | 71 MB | 35 MB |

### 9.7. Пример C-структур для чтения

Ниже — компактное определение всех структур бинарного формата на C.
Все поля little-endian, выравнивание отключено (`#pragma pack`), чтобы
побайтово совпадать с тем, что записывает `compact.rs`.

```c
#pragma pack(push, 1)

/* ── Заголовок файла (88 байт) ─────────────────────────────────────── */
typedef struct {
    uint8_t  magic[4];           /* 0x4F 0x53 0x4D 0x47 = "OSMG"       */
    uint16_t version;            /* версия формата (1)                   */
    uint32_t record_count;       /* общее количество объектов            */
    uint32_t addr_count;         /* количество адресов                   */
    uint32_t named_count;        /* количество POI                      */
    uint64_t build_timestamp;    /* Unix timestamp секунд сборки         */
    uint8_t  region[46];         /* код региона UTF-8, zero-padded       */
    uint32_t string_pool_offset;
    uint32_t named_index_offset;
    uint32_t addr_index_offset;
    uint32_t records_offset;
} Header;

/* ── Запись в Named Index (9 байт) ─────────────────────────────────── */
typedef struct {
    uint16_t name_idx;           /* индекс строки имени в String Pool     */
    uint16_t translit_idx;       /* индекс строки транслитерации          */
    uint8_t  category;           /* тег категории (1=Amenity, …)          */
    uint32_t record_idx;         /* индекс в Record Block                 */
} NamedIndexEntry;

/* ── Запись в Address Index (10 байт) ──────────────────────────────── */
typedef struct {
    uint16_t city_idx;           /* индекс строки города                  */
    uint16_t street_idx;         /* индекс строки улицы                   */
    uint16_t housenumber_idx;    /* индекс строки номера дома             */
    uint32_t record_idx;         /* индекс в Record Block                 */
} AddrIndexEntry;

/* ── Запись в Record Block (9 или 8 байт после type+lat+lon) ──────── */
typedef struct {
    uint16_t city_idx;
    uint16_t street_idx;
    uint16_t housenumber_idx;
} RecordAddr;                    /* поля Address-записи (6 байт)          */

typedef struct {
    uint16_t name_idx;
    uint16_t translit_idx;
    uint8_t  category;
} RecordNamed;                   /* поля Named-записи (5 байт)            */

#pragma pack(pop)
```

**Чтение Record Block.** Каждая запись начинается одинаково — `type`,
`lat`, `lon` — а дальше, в зависимости от `type`, читается либо
`RecordAddr`, либо `RecordNamed`:

```c
/* Прочитать все записи из уже загруженного в память Record Block */
void read_records(const uint8_t *data, const Header *hdr) {
    const uint8_t *ptr = data + hdr->records_offset;
    uint32_t count;
    memcpy(&count, ptr, 4);  ptr += 4;   /* первое u32 — количество записей */

    for (uint32_t i = 0; i < count; i++) {
        uint8_t   type = *ptr;        ptr += 1;
        float     lat, lon;
        memcpy(&lat, ptr, 4);  ptr += 4;
        memcpy(&lon, ptr, 4);  ptr += 4;

        if (type == 0) {
            /* Address */
            RecordAddr addr;
            memcpy(&addr, ptr, sizeof(RecordAddr));
            ptr += sizeof(RecordAddr);
            /* addr.city_idx, addr.street_idx, addr.housenumber_idx → String Pool */
        } else if (type == 1) {
            /* NamedObject */
            RecordNamed named;
            memcpy(&named, ptr, sizeof(RecordNamed));
            ptr += sizeof(RecordNamed);
            /* named.name_idx, named.translit_idx, named.category → String Pool */
        }
    }
}
```

**Чтение String Pool.** Пул строк начинается сразу после заголовка.
Формат: `u32 count`, затем для каждой строки `u16 len` + `len` байт UTF-8.

```c
/* Прочитать все строки из String Pool; вернуть массив указателей и их количество */
const char **read_string_pool(const uint8_t *data, const Header *hdr,
                              uint32_t *out_count) {
    const uint8_t *ptr = data + hdr->string_pool_offset;
    uint32_t count;
    memcpy(&count, ptr, 4);  ptr += 4;

    const char **pool = malloc(count * sizeof(const char *));
    for (uint32_t i = 0; i < count; i++) {
        uint16_t len;
        memcpy(&len, ptr, 2);  ptr += 2;
        pool[i] = (const char *)ptr;
        ptr += len;
    }
    *out_count = count;
    return pool;
}
```

**Открытие файла целиком.** Рекомендуемый способ — mmap:

```c
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#include <stdint.h>
#include <string.h>

int main(int argc, char *argv[]) {
    int fd = open(argv[1], O_RDONLY);
    struct stat st;
    fstat(fd, &st);
    const uint8_t *data = mmap(NULL, st.st_size, PROT_READ,
                                MAP_PRIVATE, fd, 0);

    const Header *hdr = (const Header *)data;

    /* проверка magic */
    if (memcmp(hdr->magic, "OSMG", 4) != 0) return 1;

    uint32_t      sp_count;
    const char  **pool = read_string_pool(data, hdr, &sp_count);

    /* разрешить city_idx → название */
    const char *city_name = pool[3];  /* например */

    read_records(data, hdr);

    munmap((void *)data, st.st_size);
    close(fd);
    return 0;
}
```

**Бинарный поиск по Named Index.** Named Index отсортирован лексикографически
по строкам имени **без учёта регистра** (case-insensitive, Unicode lowercasing).
Текст берётся из String Pool по `name_idx`.

Потребитель выполняет `bsearch` или собственный бинарный поиск,
**обязательно приводя к нижнему регистру и запрос, и строку из пула**:

```c
/* Case-insensitive сравнение: и запрос, и строка пула — в нижнем регистре.
   Реализация для UTF-8 должна корректно обрабатывать кириллицу. */
static int named_cmp(const void *key, const void *entry) {
    const char          *query = (const char *)key;
    const NamedIndexEntry *e   = (const NamedIndexEntry *)entry;
    const char *name = string_pool[e->name_idx];  /* pool — глобальный */
    return utf8_casefold_cmp(query, name);  /* case-insensitive strcmp для UTF-8 */
}

/* Бинарный поиск в Named Index. query должен быть уже в нижнем регистре. */
NamedIndexEntry *find_named(const char *query, NamedIndexEntry *named_base,
                            uint32_t named_count) {
    return bsearch(query, named_base, named_count,
                   sizeof(NamedIndexEntry), named_cmp);
}
```

**Важно:** Named Index сортирован по **полному** имени (case-insensitive).
Для префиксного поиска (например, «красн» → «Красная площадь») потребитель
должен выполнить бинарный поиск до первого вхождения, а затем линейно
сканировать вперёд до изменения префикса. И запрос, и строки пула
сравниваются в нижнем регистре.

### 9.8. Типовые ошибки при реализации ридера

#### 9.8.1. Выравнивание структур (struct padding)

**Самая частая ошибка.** Без `#pragma pack` компилятор C вставляет
padding-байты между полями разного размера. Например, `NamedIndexEntry`
без упаковки компилируется в 12 байт вместо 9 — три лишних байта между
`category` (u8) и `record_idx` (u32). В результате `record_idx` читает
мусор из следующей записи.

**Проверка:** распечатайте `sizeof(NamedIndexEntry)`. Должно быть **9**,
не 12. Для `AddrIndexEntry` — **10**, не 12.

```c
/* Правильно */
#pragma pack(push, 1)
typedef struct { uint16_t name_idx; uint16_t translit_idx;
                 uint8_t category; uint32_t record_idx; } NamedIndexEntry;
#pragma pack(pop)
/* sizeof(NamedIndexEntry) == 9 */

/* Неправильно — без pack, sizeof == 12 */
typedef struct { uint16_t name_idx; uint16_t translit_idx;
                 uint8_t category; uint32_t record_idx; } NamedIndexEntry;
```

#### 9.8.2. Record Block — записи переменной длины

Record Block содержит записи **разного размера**:

| Тип | Байт |
|-----|------|
| Address (type=0) | 1 (type) + 4 (lat) + 4 (lon) + 2+2+2 (city,street,hn) = **15 байт** |
| Named (type=1)   | 1 (type) + 4 (lat) + 4 (lon) + 2+2+1 (name,translit,cat) = **14 байт** |

**`record_idx` в Named Index и Address Index — это логический номер
записи (0-based), а НЕ байтовое смещение!** Нельзя вычислить позицию
как `base + idx * sizeof(RecordEntry)` — записи Address и Named имеют
разную длину.

Для произвольного доступа по `record_idx` нужно либо:
- Пройти Record Block **последовательно** от начала до нужного номера,
- **Построить lookup-таблицу** байтовых смещений при загрузке файла.

```c
/* Построение таблицы смещений Record Block за один проход */
uint32_t *build_record_offsets(const uint8_t *data, const Header *hdr,
                               uint32_t *out_count) {
    uint32_t count;
    memcpy(&count, data + hdr->records_offset, 4);
    uint32_t *offsets = malloc(count * sizeof(uint32_t));
    const uint8_t *ptr = data + hdr->records_offset + 4;
    for (uint32_t i = 0; i < count; i++) {
        offsets[i] = (uint32_t)(ptr - data);  /* байтовое смещение */
        uint8_t type = *ptr;
        ptr += 1 + 8;  /* type + lat + lon */
        ptr += (type == 0) ? 6 : 5;  /* поля адреса или POI */
    }
    *out_count = count;
    return offsets;
}

/* Произвольный доступ: чтение записи по record_idx */
void read_record_by_idx(const uint8_t *data, uint32_t *offsets, uint32_t idx) {
    const uint8_t *ptr = data + offsets[idx];
    uint8_t  type = *ptr;
    float    lat, lon;
    memcpy(&lat, ptr + 1, 4);
    memcpy(&lon, ptr + 5, 4);
    /* ... чтение полей в зависимости от type ... */
}
```

#### 9.8.3. Нулевые координаты у первых записей

Record Block отсортирован по `(lat, lon)`. Первыми идут объекты с
координатами `(0.0, 0.0)` — это OSM-отношения (маршруты дорог,
административные границы), у которых нет точечных координат. Их
немного (десятки-сотни на регион). Нулевые координаты — не ошибка
формата, а особенность данных.

#### 9.8.4. Диагностика: проверка файла скриптом

Быстрая проверка целостности `.bin`-файла Python-скриптом:

```python
import struct, sys
with open(sys.argv[1], 'rb') as f:
    data = f.read()
magic, ver, total = struct.unpack_from('<4sHI', data, 0)[:3]
sp_off, ni_off, ai_off, rec_off = struct.unpack_from('<IIII', data, 72)
print(f'Magic={magic} ver={ver} total={total}')
print(f'Sizes: SP={ni_off-sp_off}, NI={ai_off-ni_off}, AI={rec_off-ai_off}')

# Проверка Named Index (9 байт на запись, БЕЗ padding)
ni_count = struct.unpack_from('<I', data, ni_off)[0]
pos = ni_off + 4
for i in range(5):
    name_idx, _, _, rec_idx = struct.unpack_from('<HHBI', data, pos)
    print(f'  NI[{i}]: name_idx={name_idx} rec_idx={rec_idx}')
    pos += 9
assert ni_count * 9 + 4 == ai_off - ni_off, 'Named Index size mismatch!'
print('Named Index: OK')
```

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
