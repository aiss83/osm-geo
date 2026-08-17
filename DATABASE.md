# Формат базы данных osm-geo

> **Целевая аудитория:** разработчик мобильной библиотеки-потребителя (Android/iOS), которая открывает эту базу и выполняет поиск.

## 1. Обзор

`osm-geo` формирует **один** выходной формат — компактный бинарный файл
**`.bin`** (версия формата `2`).

Файл `.bin` самодостаточен и не требует SQLite/FTS5/R*Tree на устройстве:

- содержит все данные (адреса и именованные объекты) и **готовые индексы**;
- для поиска достаточно `mmap`-ить файл и выполнять бинарный/инвертированный поиск по сортированным массивам;
- полнотекстовый поиск по улице/имени выполняется по встроенному инвертированному индексу (стемминг выполнен на этапе сборки).

Для транспортировки файл сжимается Zstandard: `.bin.zst`.

### Обработка текста

На этапе сборки названия улиц и городов проходят очистку:

1. **Нормализация названий** (всегда активна):
   - раскрытие сокращений: `ул` → `улица`, `пр-т` → `проспект`, `пл` → `площадь`;
   - нормализация падежей: `улицы` → `улица`, `проспекту` → `проспект`;
   - комбинации: `пр-ту Мира` → `проспект Мира`.

   Точность rule-based нормализатора — **90.6%**. Опционально подключается
   нейросетевой нормализатор (mt5-small, ONNX), повышающий точность до **98.1%**.

2. **Коррекция опечаток** (SymSpell) по частотному словарю `ru_full.txt`.
   Из коррекции исключены типы улиц, названия городов и топонимы на
   `-ово/-ево/-ино`, чтобы не допустить ложной перекоррекции.

### Постобработка адресов и POI

- **Привязка к городам** — адреса и POI без `addr:city` привязываются к ближайшему населённому пункту по координатам (Haversine).
- **Склеивание опечаток в городах** — «Калининнград» → «Калининград» по редакционному расстоянию.
- **Привязка к стране** — для объектов без страны код определяется по городу или из кода региона (`-r RU-CFD` → `RU`).

### Фильтрация POI

В базу попадают именованные объекты двух категорий: `historic` и `tourism`.
Остановки транспорта, дороги, офисы, административные границы и природные
объекты не включаются.

### Модель данных

- **Address** — иерархический адрес: `country`, `city`, `street`,
  `housenumber`, `postcode` + координаты `(lat, lon)`.
- **NamedObject** — именованный объект: `name`, `category`,
  `country`, `city` + координаты `(lat, lon)`.

## 2. Структура файла `.bin`

| Секция | Описание |
|--------|----------|
| Header | 88 B — magic, version, счётчики, timestamp, регион, offsets |
| Section Directory | таблица секций `(id, offset, length)` |
| String Pool | все уникальные строки, индекс 0 — пустая строка |
| Named Index | сортирован по имени, для префиксного поиска |
| Address Index | сортирован по `(city, street, housenumber)` |
| Record Block | сортирован по `(lat, lon)`, для пространственного поиска |
| FTS Address tokens/postings | инвертированный индекс адресов |
| FTS Named tokens/postings | инвертированный индекс POI |

Все многобайтовые поля — little-endian.

Графически файл выглядит так (сверху вниз):

```text
Смещение 0
┌───────────────────────────────┐
│ Header (88 B)                 │
├───────────────────────────────┤ 88
│ Section Directory (84 B)      │
├───────────────────────────────┤ string_pool_offset
│ String Pool                   │
├───────────────────────────────┤ named_index_offset
│ Named Index                   │
├───────────────────────────────┤ addr_index_offset
│ Address Index                 │
├───────────────────────────────┤ records_offset
│ Record Block                  │
├───────────────────────────────┤ id 5 (offset/length в Section Directory)
│ FTS Address token dictionary  │
├───────────────────────────────┤ id 6
│ FTS Address postings          │
├───────────────────────────────┤ id 7
│ FTS Named token dictionary    │
├───────────────────────────────┤ id 8
│ FTS Named postings            │
└───────────────────────────────┘
```

Смещения первых четырёх секций продублированы в заголовке; FTS-секции
находятся только по Section Directory (id 5–8).

### 2.1. Заголовок (Header, 88 байт)

| Смещение | Размер | Поле | Описание |
|----------|--------|------|----------|
| 0 | 4 B | `magic` | `OSMG` (0x4F 0x53 0x4D 0x47) |
| 4 | 2 B | `version` | `2` |
| 6 | 4 B | `record_count` | общее количество объектов |
| 10 | 4 B | `addr_count` | количество адресов |
| 14 | 4 B | `named_count` | количество POI |
| 18 | 8 B | `build_timestamp` | Unix timestamp сборки |
| 26 | 46 B | `region` | код региона UTF-8, zero-padded |
| 72 | 4 B | `string_pool_offset` | смещение String Pool |
| 76 | 4 B | `named_index_offset` | смещение Named Index |
| 80 | 4 B | `addr_index_offset` | смещение Address Index |
| 84 | 4 B | `records_offset` | смещение Record Block |

### 2.2. Section Directory

Начинается сразу после заголовка (смещение `88`):

```
count:                 u32
entries[count]:        id:u16, offset:u32, length:u32   (10 байт)
```

Идентификаторы секций:

| id | Секция |
|----|--------|
| 1 | String Pool |
| 2 | Named Index |
| 3 | Address Index |
| 4 | Record Block |
| 5 | FTS Address token dictionary |
| 6 | FTS Address postings |
| 7 | FTS Named token dictionary |
| 8 | FTS Named postings |

### 2.3. String Pool

```
count:             u32
for each string:
  len:             u32
  bytes:           UTF-8, len байт
```

Индекс `0` всегда пустая строка. Все остальные индексы (`city_idx`,
`street_idx`, `name_idx`, …) — это **u32-индексы** в этот пул.

### 2.4. Named Index

Сортирован по тексту имени (сравнение в нижнем регистре).

```
count:                 u32
entries[count]:        name_idx:u32, category:u8, record_idx:u32   (9 байт)
```

`category` — байтовый тег категории:

| Значение | Категория |
|----------|-----------|
| 0 | None |
| 1 | amenity |
| 2 | tourism |
| 3 | shop |
| 4 | historic |
| 5 | leisure |
| 6 | office |
| 7 | boundary (в т.ч. `admin_level:*`) |
| 8 | highway |
| 9 | railway |
| 10 | aeroway |
| 11 | waterway |
| 12 | natural |

В текущей сборке фактически встречаются только `2` (tourism) и `4` (historic).

### 2.5. Address Index

Сортирован по `(city, street, housenumber)` (сравнение в нижнем регистре).

```
count:                 u32
entries[count]:        city_idx:u32, street_idx:u32,
                       housenumber_idx:u32, record_idx:u32   (16 байт)
```

### 2.6. Record Block

Сортирован по `(lat, lon)`.

```
count:                 u32
records[count]:
  type:                u8    — 0=Address, 1=Named
  lat:                 f32
  lon:                 f32
  Address:             city_idx:u32, street_idx:u32, housenumber_idx:u32   (12 байт)
  Named:               country_idx:u32, city_idx:u32, name_idx:u32, category:u8  (13 байт)
```

`record_idx` в Named/Address Index — это **логический номер записи в Record
Block (0-based)**, а не байтовое смещение. Записи имеют переменную длину, поэтому
для произвольного доступа нужно пройти Record Block последовательно или построить
таблицу смещений при загрузке.

## 3. Полнотекстовый индекс (FTS)

В `.bin` встроен инвертированный индекс для поиска по улице/имени без построения
индекса на устройстве.

- **FTS Address** — токены из полей адреса: `city` и `street` (стемминг),
  `housenumber` (без стемминга, нижний регистр).
- **FTS Named** — токены из полей POI: `name` и `city` (стемминг).

### 3.1. Словарь токенов

```
count:                 u32
entries[count]:
  token_len:           u16
  token:               UTF-8, token_len байт
  postings_offset:     u32   — смещение в секции postings
  postings_count:      u32   — число record_idx
```

Словарь отсортирован по `token`. Токены — слова в нижнем регистре после
русского snowball-стемминга; для `housenumber` — только нижний регистр без
стемминга.

### 3.2. Постинг-листы

```
total_count:           u32
postings:              varint-поток (unsigned LEB128)
```

Для каждого токена список `record_idx` отсортирован по возрастанию и закодирован
дельтами: `delta_i = record_idx_i - record_idx_{i-1}`.

### 3.3. Алгоритм поиска

1. Нормализовать запрос так же, как при сборке (нижний регистр → стемминг;
   для номера дома — только нижний регистр).
2. Для каждого токена найти диапазон в словаре бинарным поиском по префиксу.
3. Собрать и пересечь постинг-листы (AND), начиная с самой короткой.
4. `record_idx` → запись в Record Block → координаты и поля.

### 3.4. Стеммер (Snowball Russian)

Для полноценного FTS клиент должен применять **тот же стеммер**, что и сборщик,
иначе запрос не совпадёт со стеммированными токенами в `.bin`.

Используемый алгоритм — стандартный **Snowball-стеммер русского языка**
(алгоритм `russian` из проекта Snowball, snowballstem.org).

Готовые реализации по платформам:

- **Rust (как в сборщике):** crate `rust-stemmers`, `Stemmer::create(Algorithm::Russian)`.
- **Android (Kotlin/Java):** `org.tartarus.snowball.ext.RussianStemmer`
  (из `lucene-analyzers-common` или пакета Snowball).
- **iOS (Swift):** пакет `Snowball` (Swift) или `libstemmer`.
- **C/C++:** `libstemmer` — `sb_stemmer_new("russian", "UTF_8")`.

Правила токенизации (их нужно повторить точно):

1. разбить текст по пробельным символам;
2. у каждого слова срезать обрамляющие не-буквенно-цифровые символы
   (кавычки, скобки, запятые); внутренние дефис/слэш сохраняются
   (`1-й`, `12/1`);
3. привести к нижнему регистру;
4. для текстовых полей (`city`, `street`, `name`) применить Snowball Russian
   к каждому слову; для `housenumber`/`postcode`/`category` — не стеммировать.

Примеры:

| Поле | Исходное | Токены |
|------|----------|--------|
| street | `Тверская улица` | `тверск`, `улиц` |
| city | `Москва` | `москв` |
| name | `"Красная площадь"` | `красн`, `площад` |
| housenumber | `12/1` | `12/1` |

## 4. Сжатие и метаданные

Сборка создаёт:

- `{stem}.bin` — несжатая база;
- `{stem}.bin.zst` — сжатая Zstandard (уровень 3) версия для передачи;
- `{stem}.metadata.json` — метаданные.

### 4.1. Файл метаданных

```json
{
  "version": "0.2.0",
  "region": "RU-CFD",
  "build_date": "2026-08-14",
  "source_pbf": "data/central-fed-district-latest.osm.pbf",
  "object_count": 1923920,
  "address_count": 1897875,
  "named_count": 26045,
  "db_size_bytes": 84873581,
  "compressed_size_bytes": 35610000,
  "sha256": "..."
}
```

`sha256` — хеш **несжатого** `.bin` файла.

### 4.2. Размеры (ЦФО)

Замер на Центральном федеральном округе (`central-fed-district`, PBF 828 МБ,
после дедупликации 1 923 920 объектов):

| Формат | Размер |
|--------|--------|
| `.bin` | 80,94 МБ |
| `.bin.zst` | 33,96 МБ |

Время сборки (release, прямой PBF): **145 с**.

## 5. Пример C-структур для чтения

Все поля little-endian, выравнивание отключено (`#pragma pack`).

```c
#pragma pack(push, 1)

typedef struct {
    uint8_t  magic[4];           /* 0x4F 0x53 0x4D 0x47 = "OSMG"       */
    uint16_t version;            /* 2                                    */
    uint32_t record_count;
    uint32_t addr_count;
    uint32_t named_count;
    uint64_t build_timestamp;
    uint8_t  region[46];
    uint32_t string_pool_offset;
    uint32_t named_index_offset;
    uint32_t addr_index_offset;
    uint32_t records_offset;
} Header;

typedef struct {
    uint16_t id;
    uint32_t offset;
    uint32_t length;
} SectionEntry;

typedef struct {
    uint32_t name_idx;
    uint8_t  category;
    uint32_t record_idx;
} NamedIndexEntry;              /* 9 байт */

typedef struct {
    uint32_t city_idx;
    uint32_t street_idx;
    uint32_t housenumber_idx;
    uint32_t record_idx;
} AddrIndexEntry;               /* 16 байт */

typedef struct {
    uint32_t city_idx;
    uint32_t street_idx;
    uint32_t housenumber_idx;
} RecordAddr;                   /* 12 байт после type+lat+lon */

typedef struct {
    uint32_t country_idx;
    uint32_t city_idx;
    uint32_t name_idx;
    uint8_t  category;
} RecordNamed;                  /* 13 байт после type+lat+lon */

typedef struct {
    uint16_t token_len;
    uint8_t  token[];           /* token_len байт UTF-8                */
    uint32_t postings_offset;
    uint32_t postings_count;
} FtsDictEntry;

#pragma pack(pop)
```

### Чтение String Pool

```c
const char **read_string_pool(const uint8_t *data, const Header *hdr,
                              uint32_t *out_count) {
    const uint8_t *ptr = data + hdr->string_pool_offset;
    uint32_t count;
    memcpy(&count, ptr, 4);  ptr += 4;

    const char **pool = malloc(count * sizeof(const char *));
    for (uint32_t i = 0; i < count; i++) {
        uint32_t len;
        memcpy(&len, ptr, 4);  ptr += 4;
        pool[i] = (const char *)ptr;
        ptr += len;
    }
    *out_count = count;
    return pool;
}
```

### Построение таблицы смещений Record Block

```c
uint32_t *build_record_offsets(const uint8_t *data, const Header *hdr,
                               uint32_t *out_count) {
    uint32_t count;
    memcpy(&count, data + hdr->records_offset, 4);
    uint32_t *offsets = malloc(count * sizeof(uint32_t));
    const uint8_t *ptr = data + hdr->records_offset + 4;

    for (uint32_t i = 0; i < count; i++) {
        offsets[i] = (uint32_t)(ptr - data);
        uint8_t type = *ptr;
        ptr += 1 + 4 + 4;              /* type + lat + lon */
        ptr += (type == 0) ? 12 : 13;  /* поля адреса или POI */
    }
    *out_count = count;
    return offsets;
}
```

### Диагностика

```python
import struct, sys
with open(sys.argv[1], 'rb') as f:
    data = f.read()
magic, ver, total = struct.unpack_from('<4sHI', data, 0)[:3]
sp_off, ni_off, ai_off, rec_off = struct.unpack_from('<IIII', data, 72)
print(f'Magic={magic} ver={ver} total={total}')
print(f'Sizes: SP={ni_off-sp_off}, NI={ai_off-ni_off}, AI={rec_off-ai_off}')

ni_count = struct.unpack_from('<I', data, ni_off)[0]
pos = ni_off + 4
for i in range(min(ni_count, 5)):
    name_idx, cat, rec_idx = struct.unpack_from('<IBI', data, pos)
    print(f'  NI[{i}]: name_idx={name_idx} cat={cat} rec_idx={rec_idx}')
    pos += 9
assert ni_count * 9 + 4 == ai_off - ni_off, 'Named Index size mismatch!'
print('Named Index: OK')
```

## 6. CLI-справочник

### Сборка базы

```bash
# Из локального PBF-файла
osm-geo build --input data/region.osm.pbf --region RU-CFD

# Из региона Geofabrik (авто-загрузка)
osm-geo build --input russia/central-fed-district --region RU-CFD

# Из сжатого PBF (авто-распаковка .gz / .zst)
osm-geo build --input data/region.osm.pbf.zst

# Из URL
osm-geo build --input https://example.com/region.osm.pbf

# Источник данных: auto (по расширению), pbf или gol
osm-geo build --input data/region.gol --source gol
```

### Конвертация PBF → GOL

```bash
osm-geo convert --input data/region.osm.pbf --output data/region.gol
```

### Список регионов Geofabrik

```bash
osm-geo list
osm-geo list russia
```
