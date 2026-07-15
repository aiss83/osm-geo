//! Компактный бинарный формат для мобильных устройств.
//!
//! Замена SQLite: в 4-6× компактнее за счёт:
//! - Словарного кодирования строк (String pool)
//! - Сортированных массивов для бинарного поиска
//! - Отсутствия накладных расходов SQLite
//!
//! Формат файла:
//!   [Header: 88B] — magic, version, counts, timestamp, region, offsets
//!   [String Pool]
//!   [Named Index]    — сортирован по name, для префиксного поиска
//!   [Address Index]  — сортирован по (city, street, housenumber)
//!   [Record Block]   — сортирован по (lat, lon), для пространственного поиска

use anyhow::Result;
use log::info;
use std::collections::HashMap;
use std::io::Write;

use crate::model::GeoObject;

/// Заголовок файла (88 байт, little-endian).
#[repr(C, packed)]
struct Header {
    magic: [u8; 4],             // "OSMG"
    version: u16,               // 1
    record_count: u32,
    addr_count: u32,
    named_count: u32,
    build_timestamp: u64,       // Unix timestamp (секунды)
    region: [u8; 46],           // UTF-8, zero-padded
    string_pool_offset: u32,
    named_index_offset: u32,
    addr_index_offset: u32,
    records_offset: u32,
}

/// Тег категории (1 байт).
#[repr(u8)]
enum CategoryTag {
    None = 0,
    Amenity = 1,
    Tourism = 2,
    Shop = 3,
    Historic = 4,
    Leisure = 5,
    Office = 6,
    Boundary = 7,
    Highway = 8,
    Railway = 9,
    Aeroway = 10,
    Waterway = 11,
    Natural = 12,
}

fn category_to_tag(cat: Option<&str>) -> u8 {
    match cat {
        Some("amenity") => CategoryTag::Amenity as u8,
        Some("tourism") => CategoryTag::Tourism as u8,
        Some("shop") => CategoryTag::Shop as u8,
        Some("historic") => CategoryTag::Historic as u8,
        Some("leisure") => CategoryTag::Leisure as u8,
        Some("office") => CategoryTag::Office as u8,
        Some("boundary") => CategoryTag::Boundary as u8,
        Some("highway") => CategoryTag::Highway as u8,
        Some("railway") => CategoryTag::Railway as u8,
        Some("aeroway") => CategoryTag::Aeroway as u8,
        Some("waterway") => CategoryTag::Waterway as u8,
        Some("natural") => CategoryTag::Natural as u8,
        Some(s) if s.starts_with("admin_level:") => CategoryTag::Boundary as u8,
        _ => CategoryTag::None as u8,
    }
}

/// Словарь строк: собирает уникальные строки, назначает u16-индексы.
/// Индекс 0 зарезервирован для пустой строки.
pub struct StringPool {
    strings: Vec<String>,
    map: HashMap<String, u16>,
}

impl StringPool {
    pub fn new() -> Self {
        let mut pool = Self {
            strings: vec![String::new()],
            map: HashMap::new(),
        };
        pool.map.insert(String::new(), 0);
        pool
    }

    /// Добавить строку, вернуть индекс. Возвращает 0 для None/пустой.
    pub fn intern(&mut self, s: Option<&str>) -> u16 {
        let s = s.unwrap_or("");
        if s.is_empty() {
            return 0;
        }
        if let Some(&idx) = self.map.get(s) {
            return idx;
        }
        let idx = self.strings.len() as u16;
        self.strings.push(s.to_string());
        self.map.insert(s.to_string(), idx);
        idx
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// Сериализовать пул строк в буфер.
    pub fn write_to(&self, w: &mut impl Write) -> Result<()> {
        let count = self.strings.len() as u32;
        w.write_all(&count.to_le_bytes())?;
        for s in &self.strings {
            let len = s.len() as u16;
            w.write_all(&len.to_le_bytes())?;
            w.write_all(s.as_bytes())?;
        }
        Ok(())
    }

    /// Размер сериализованного пула в байтах.
    pub fn serialized_size(&self) -> usize {
        4 + self.strings.iter().map(|s| 2 + s.len()).sum::<usize>()
    }
}

/// Запись в Record Block.
#[derive(Clone)]
struct RecordEntry {
    obj_type: u8,   // 0 = Address, 1 = Named
    lat: f32,
    lon: f32,
    /// Для Address: (city, street, housenumber)
    addr_indices: Option<(u16, u16, u16)>,
    /// Для Named: (name, translit, category)
    named_data: Option<(u16, u16, u8)>,
}

/// Запись в Named Index (сортирована по имени).
struct NamedIndexEntry {
    name_idx: u16,
    translit_idx: u16,
    category: u8,
    record_idx: u32,
}

/// Запись в Address Index (сортирована по city+street+housenumber).
struct AddrIndexEntry {
    city_idx: u16,
    street_idx: u16,
    housenumber_idx: u16,
    record_idx: u32,
}

/// Построитель компактного бинарного файла.
pub struct CompactWriter {
    pool: StringPool,
    records: Vec<RecordEntry>,
    named_index: Vec<NamedIndexEntry>,
    addr_index: Vec<AddrIndexEntry>,
}

impl CompactWriter {
    pub fn new() -> Self {
        Self {
            pool: StringPool::new(),
            records: Vec::new(),
            named_index: Vec::new(),
            addr_index: Vec::new(),
        }
    }

    /// Добавить объекты, отсортировать, построить индексы, записать файл.
    pub fn build(
        &mut self,
        objects: &[GeoObject],
        output_path: &std::path::Path,
        region: &str,
        timestamp: u64,
    ) -> Result<()> {
        info!("Построение компактного бинарного формата...");

        // 1. Первый проход: собираем все строки и строим записи
        let mut temp_records: Vec<(f64, f64, RecordEntry)> = Vec::with_capacity(objects.len());

        for (_i, obj) in objects.iter().enumerate() {
            let (lat, lon) = obj.lat_lon();
            let record = match obj {
                GeoObject::Address(addr) => {
                    let city_idx = self.pool.intern(addr.city.as_deref());
                    let street_idx = self.pool.intern(addr.street.as_deref());
                    let hn_idx = self.pool.intern(addr.housenumber.as_deref());
                    // Интернируем страну и индекс — не храним, но они нужны для полноты пула
                    self.pool.intern(addr.country.as_deref());
                    self.pool.intern(addr.postcode.as_deref());

                    RecordEntry {
                        obj_type: 0,
                        lat: lat as f32,
                        lon: lon as f32,
                        addr_indices: Some((city_idx, street_idx, hn_idx)),
                        named_data: None,
                    }
                }
                GeoObject::Named(obj) => {
                    let name_idx = self.pool.intern(Some(&obj.name));
                    let translit_idx = self.pool.intern(obj.translit.as_deref());
                    let category = category_to_tag(obj.category.as_deref());

                    RecordEntry {
                        obj_type: 1,
                        lat: lat as f32,
                        lon: lon as f32,
                        addr_indices: None,
                        named_data: Some((name_idx, translit_idx, category)),
                    }
                }
            };

            temp_records.push((lat, lon, record));
        }

        info!("Строк в пуле: {}", self.pool.len());

        // 2. Сортируем записи по (lat, lon) для Record Block
        temp_records.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap()
                .then_with(|| a.1.partial_cmp(&b.1).unwrap())
        });

        self.records = temp_records.iter().map(|(_, _, r)| r.clone()).collect();

        // 3. Строим Named Index — сортирован по строке имени
        for (record_idx, rec) in self.records.iter().enumerate() {
            if let Some((name_idx, translit_idx, category)) = rec.named_data {
                self.named_index.push(NamedIndexEntry {
                    name_idx,
                    translit_idx,
                    category,
                    record_idx: record_idx as u32,
                });
            }
        }

        // Сортируем Named Index по тексту имени (из пула), case-insensitive
        // для поддержки регистронезависимого бинарного поиска.
        // Потребитель обязан lowercasing-ить запрос и строку пула при сравнении.
        let pool = &self.pool;
        // sort_by_cached_key: вычисляет lowercase один раз на элемент, а не O(n log n) раз
        self.named_index.sort_by_cached_key(|e| {
            pool.strings[e.name_idx as usize].to_lowercase()
        });

        info!("Named Index: {} записей", self.named_index.len());

        // 4. Строим Address Index — сортирован по (city, street, housenumber)
        for (record_idx, rec) in self.records.iter().enumerate() {
            if let Some((city_idx, street_idx, hn_idx)) = rec.addr_indices {
                self.addr_index.push(AddrIndexEntry {
                    city_idx,
                    street_idx,
                    housenumber_idx: hn_idx,
                    record_idx: record_idx as u32,
                });
            }
        }

        // Сортируем Address Index по (city, street, housenumber), case-insensitive
        // для поддержки регистронезависимого бинарного поиска.
        // Потребитель обязан lowercasing-ить запрос и строки пула при сравнении.
        // sort_by_cached_key: вычисляет lowercase-ключи один раз на элемент
        self.addr_index.sort_by_cached_key(|e| {
            (
                pool.strings[e.city_idx as usize].to_lowercase(),
                pool.strings[e.street_idx as usize].to_lowercase(),
                pool.strings[e.housenumber_idx as usize].to_lowercase(),
            )
        });

        info!("Address Index: {} записей", self.addr_index.len());

        // 5. Вычисляем offsets и пишем файл
        let string_pool_offset = std::mem::size_of::<Header>() as u32;
        let named_index_offset = string_pool_offset + self.pool.serialized_size() as u32;
        let addr_index_offset =
            named_index_offset + (4 + self.named_index.len() * 9) as u32; // count + entries
        let records_offset =
            addr_index_offset + (4 + self.addr_index.len() * 10) as u32;

        let addr_count = self
            .records
            .iter()
            .filter(|r| r.obj_type == 0)
            .count() as u32;
        let named_count = self.records.len() as u32 - addr_count;

        let mut region_bytes = [0u8; 46];
        let region_utf8 = region.as_bytes();
        let copy_len = region_utf8.len().min(45); // last byte stays 0
        region_bytes[..copy_len].copy_from_slice(&region_utf8[..copy_len]);

        let header = Header {
            magic: *b"OSMG",
            version: 1,
            record_count: self.records.len() as u32,
            addr_count,
            named_count,
            build_timestamp: timestamp,
            region: region_bytes,
            string_pool_offset,
            named_index_offset,
            addr_index_offset,
            records_offset,
        };

        let mut file = std::fs::File::create(output_path)?;

        // Пишем header
        let header_bytes = unsafe {
            std::slice::from_raw_parts(
                &header as *const Header as *const u8,
                std::mem::size_of::<Header>(),
            )
        };
        file.write_all(header_bytes)?;

        // Пишем string pool
        self.pool.write_to(&mut file)?;

        // Пишем Named Index
        let count = self.named_index.len() as u32;
        file.write_all(&count.to_le_bytes())?;
        for entry in &self.named_index {
            file.write_all(&entry.name_idx.to_le_bytes())?;
            file.write_all(&entry.translit_idx.to_le_bytes())?;
            file.write_all(&[entry.category])?;
            file.write_all(&entry.record_idx.to_le_bytes())?;
        }

        // Пишем Address Index
        let count = self.addr_index.len() as u32;
        file.write_all(&count.to_le_bytes())?;
        for entry in &self.addr_index {
            file.write_all(&entry.city_idx.to_le_bytes())?;
            file.write_all(&entry.street_idx.to_le_bytes())?;
            file.write_all(&entry.housenumber_idx.to_le_bytes())?;
            file.write_all(&entry.record_idx.to_le_bytes())?;
        }

        // Пишем Record Block
        let count = self.records.len() as u32;
        file.write_all(&count.to_le_bytes())?;
        for rec in &self.records {
            file.write_all(&[rec.obj_type])?;
            file.write_all(&rec.lat.to_le_bytes())?;
            file.write_all(&rec.lon.to_le_bytes())?;
            match rec.obj_type {
                0 => {
                    let (city, street, hn) = rec.addr_indices.unwrap();
                    file.write_all(&city.to_le_bytes())?;
                    file.write_all(&street.to_le_bytes())?;
                    file.write_all(&hn.to_le_bytes())?;
                }
                1 => {
                    let (name, translit, category) = rec.named_data.unwrap();
                    file.write_all(&name.to_le_bytes())?;
                    file.write_all(&translit.to_le_bytes())?;
                    file.write_all(&[category])?;
                }
                _ => unreachable!(),
            }
        }

        let size = file.metadata()?.len();
        info!(
            "Компактный файл записан: {:.2} МБ",
            size as f64 / (1024.0 * 1024.0)
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Address, NamedObject};

    #[test]
    fn test_string_pool() {
        let mut pool = StringPool::new();
        assert_eq!(pool.intern(None), 0);
        assert_eq!(pool.intern(Some("")), 0);
        let idx1 = pool.intern(Some("Москва"));
        let idx2 = pool.intern(Some("Москва"));
        assert_eq!(idx1, idx2);
        assert!(idx1 > 0);
        assert_eq!(pool.len(), 2); // "" + "Москва"
    }

    #[test]
    fn test_category_to_tag() {
        assert_eq!(category_to_tag(Some("amenity")), 1);
        assert_eq!(category_to_tag(Some("tourism")), 2);
        assert_eq!(category_to_tag(Some("admin_level:4")), 7);
        assert_eq!(category_to_tag(None), 0);
        assert_eq!(category_to_tag(Some("unknown")), 0);
    }

    #[test]
    fn test_compact_build() {
        let objects = vec![
            GeoObject::Named(NamedObject {
                name: "Красная площадь".into(),
                translit: Some("Krasnaya ploshchad".into()),
                category: Some("tourism".into()),
                lat: 55.7539,
                lon: 37.6208,
            }),
            GeoObject::Address(Address {
                country: Some("Россия".into()),
                city: Some("Москва".into()),
                street: Some("Тверская".into()),
                housenumber: Some("1".into()),
                postcode: None,
                lat: 55.7558,
                lon: 37.6173,
            }),
        ];

        let path = std::path::PathBuf::from("/tmp/test_compact.bin");
        let mut writer = CompactWriter::new();
        writer.build(&objects, &path, "RU-TEST", 0).unwrap();

        assert!(path.exists());
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(size > 24); // минимум header
        assert!(size < 500); // должно быть очень маленьким

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_compact_named_index_sorted() {
        // Объекты в обратном порядке — индекс должен отсортироваться по имени
        let objects = vec![
            GeoObject::Named(NamedObject {
                name: "В".into(),
                translit: None,
                category: None,
                lat: 55.0,
                lon: 37.0,
            }),
            GeoObject::Named(NamedObject {
                name: "Б".into(),
                translit: None,
                category: None,
                lat: 56.0,
                lon: 38.0,
            }),
            GeoObject::Named(NamedObject {
                name: "А".into(),
                translit: None,
                category: None,
                lat: 57.0,
                lon: 39.0,
            }),
        ];

        let path = std::path::PathBuf::from("/tmp/test_compact_sort.bin");
        let mut writer = CompactWriter::new();
        writer.build(&objects, &path, "RU-TEST", 0).unwrap();

        // Проверяем, что Named Index отсортирован: А < Б < В
        assert!(writer.named_index.len() == 3);
        let names: Vec<&str> = writer
            .named_index
            .iter()
            .map(|e| writer.pool.strings[e.name_idx as usize].as_str())
            .collect();
        assert_eq!(names, vec!["А", "Б", "В"]);

        // Record Block должен быть по lat: 55, 56, 57
        assert!(writer.records[0].lat == 55.0);
        assert!(writer.records[1].lat == 56.0);
        assert!(writer.records[2].lat == 57.0);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_compact_roundtrip_problematic_address() {
        // Проверка, что проблемные строки не искажаются в компактном формате.
        // Адрес как есть (уже после коррекции) должен сохраниться без изменений.
        let objects = vec![GeoObject::Address(Address {
            country: None,
            city: Some("Большое Исаково".into()),
            street: Some("Московский проспект съезд 1".into()),
            housenumber: Some("1".into()),
            postcode: None,
            lat: 54.7,
            lon: 20.5,
        })];

        let path = std::path::PathBuf::from("/tmp/test_compact_roundtrip.bin");
        let mut writer = CompactWriter::new();
        writer.build(&objects, &path, "RU-TEST", 0).unwrap();

        // Читаем файл обратно и проверяем строки в пуле
        let data = std::fs::read(&path).unwrap();

        // Пропускаем header (88 байт)
        let sp_off = 88;
        let sp_count = u32::from_le_bytes(data[sp_off..sp_off+4].try_into().unwrap()) as usize;

        let mut pool: Vec<String> = Vec::new();
        let mut pos = sp_off + 4;
        for _ in 0..sp_count {
            let slen = u16::from_le_bytes(data[pos..pos+2].try_into().unwrap()) as usize;
            pos += 2;
            let s = String::from_utf8(data[pos..pos+slen].to_vec()).unwrap();
            pool.push(s);
            pos += slen;
        }

        // Ищем строки в пуле
        eprintln!("String pool: {pool:?}");
        assert!(
            pool.contains(&"Большое Исаково".to_string()),
            "Пул не содержит «Большое Исаково»: {pool:?}"
        );
        assert!(
            pool.contains(&"Московский проспект съезд 1".to_string()),
            "Пул не содержит «Московский проспект съезд 1»: {pool:?}"
        );
        assert!(
            !pool.iter().any(|s| s.contains("Исакова")),
            "Пул содержит искажённое «Исакова»: {pool:?}"
        );
        assert!(
            !pool.iter().any(|s| s.contains("проспекту")),
            "Пул содержит искажённое «проспекту»: {pool:?}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_compact_addr_index_sorted() {
        let objects = vec![
            GeoObject::Address(Address {
                country: None,
                city: Some("Москва".into()),
                street: Some("Я".into()),
                housenumber: Some("10".into()),
                postcode: None,
                lat: 55.0,
                lon: 37.0,
            }),
            GeoObject::Address(Address {
                country: None,
                city: Some("Москва".into()),
                street: Some("А".into()),
                housenumber: Some("1".into()),
                postcode: None,
                lat: 56.0,
                lon: 38.0,
            }),
        ];

        let path = std::path::PathBuf::from("/tmp/test_compact_addr.bin");
        let mut writer = CompactWriter::new();
        writer.build(&objects, &path, "RU-TEST", 0).unwrap();

        assert!(writer.addr_index.len() == 2);

        // Адресный индекс сортирован по (city, street, housenumber)
        let first = &writer.addr_index[0];
        let first_street = &writer.pool.strings[first.street_idx as usize];
        assert_eq!(first_street, "А");

        let second = &writer.addr_index[1];
        let second_street = &writer.pool.strings[second.street_idx as usize];
        assert_eq!(second_street, "Я");

        let _ = std::fs::remove_file(&path);
    }
}
