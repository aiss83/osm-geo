//! Парсер PBF-файлов OpenStreetMap.
//!
//! Извлекает адресную информацию и POI из OSM-элементов
//! в соответствии с моделью данных (раздел 2.2.2 ТЗ).
//!
//! Поддерживает:
//! - Nodes и DenseNodes (с координатами)
//! - Ways (с вычислением центроида из нод)
//! - Relations (admin_boundary, associatedStreet — базовая поддержка)

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use log::info;
use osmpbf::{Element, ElementReader};
use std::collections::HashMap;
use std::path::Path;

use crate::model::{Address, GeoObject, NamedObject};
use crate::corrector::Corrector;

/// Парсер PBF-файлов OSM.
pub struct PbfParser {
    nodes: u64,
    ways: u64,
    relations: u64,
    addresses: u64,
    named_objects: u64,
    /// Кэш координат нод (node_id → (lat, lon)).
    node_coords: HashMap<i64, (f64, f64)>,
    /// Включить разрешение координат way (может потреблять много памяти).
    resolve_way_coords: bool,
    /// Статистика: сколько way получили реальные координаты.
    ways_resolved: u64,
    ways_unresolved: u64,
    /// Опциональный корректор опечаток (SymSpell).
    corrector: Option<Corrector>,
}

impl PbfParser {
    pub fn new() -> Self {
        Self {
            nodes: 0,
            ways: 0,
            relations: 0,
            addresses: 0,
            named_objects: 0,
            node_coords: HashMap::new(),
            resolve_way_coords: true,
            ways_resolved: 0,
            ways_unresolved: 0,
            corrector: None,
        }
    }

    /// Включить коррекцию опечаток (SymSpell).
    /// Словарь загружается из `data/ru_full.txt` (скачивается при необходимости).
    pub fn with_corrector(mut self, corrector: Corrector) -> Self {
        self.corrector = Some(corrector);
        self
    }

    #[allow(dead_code)]
    pub fn new_no_way_coords() -> Self {
        Self {
            resolve_way_coords: false,
            ..Self::new()
        }
    }

    /// Разобрать PBF-файл и вернуть вектор GeoObject.
    pub fn parse_file(&mut self, path: &Path) -> Result<Vec<GeoObject>> {
        info!("Парсинг PBF-файла: {:?}", path);

        let file_size = std::fs::metadata(path)
            .map(|m| m.len())
            .unwrap_or(0);
        let pb = ProgressBar::new(file_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] Парсинг PBF: {bytes}/{total_bytes} ({bytes_per_sec}) — объектов: {msg}")
                .unwrap()
                .progress_chars("##-"),
        );
        pb.set_message("0".to_string());

        let reader = ElementReader::from_path(path)?;
        let mut objects = Vec::new();

        reader.for_each(|element| {
            match element {
                Element::Node(node) => {
                    self.nodes += 1;
                    pb.set_position(pb.position() + 128);
                    let id = node.id();

                    if self.resolve_way_coords {
                        self.node_coords.insert(id, (node.lat(), node.lon()));
                    }

                    let tags: HashMap<String, String> = node
                        .tags()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    self.process_element(&tags, node.lat(), node.lon(), &mut objects);
                }
                Element::DenseNode(node) => {
                    self.nodes += 1;
                    pb.set_position(pb.position() + 64);
                    let id = node.id();

                    if self.resolve_way_coords {
                        self.node_coords.insert(id, (node.lat(), node.lon()));
                    }

                    let tags: HashMap<String, String> = node
                        .tags()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    self.process_element(&tags, node.lat(), node.lon(), &mut objects);
                }
                Element::Way(way) => {
                    self.ways += 1;
                    pb.set_position(pb.position() + 256);
                    let tags: HashMap<String, String> = way
                        .tags()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();

                    let (lat, lon) = if self.resolve_way_coords {
                        self.compute_way_centroid(&way)
                    } else {
                        (0.0, 0.0)
                    };

                    self.process_element(&tags, lat, lon, &mut objects);
                }
                Element::Relation(rel) => {
                    self.relations += 1;
                    pb.set_position(pb.position() + 512);
                    let tags: HashMap<String, String> = rel
                        .tags()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    self.process_relation(&tags, &mut objects);
                }
            }
        })?;

        pb.set_message(format!("{}", objects.len()));
        pb.finish_with_message(format!("Извлечено {} объектов", objects.len()));

        if self.resolve_way_coords {
            info!(
                "Кэш нод: {} записей, ways с координатами: {}, без: {}",
                self.node_coords.len(),
                self.ways_resolved,
                self.ways_unresolved
            );
            self.node_coords.clear();
            self.node_coords.shrink_to_fit();
        }

        info!(
            "Парсинг завершён: nodes={}, ways={}, relations={}, addresses={}, named_objects={}",
            self.nodes, self.ways, self.relations, self.addresses, self.named_objects
        );

        Ok(objects)
    }

    fn compute_way_centroid(&mut self, way: &osmpbf::Way) -> (f64, f64) {
        let mut sum_lat = 0.0f64;
        let mut sum_lon = 0.0f64;
        let mut count = 0u64;

        for node_id in way.refs() {
            if let Some(&(lat, lon)) = self.node_coords.get(&node_id) {
                sum_lat += lat;
                sum_lon += lon;
                count += 1;
            }
        }

        if count > 0 {
            self.ways_resolved += 1;
            (sum_lat / count as f64, sum_lon / count as f64)
        } else {
            self.ways_unresolved += 1;
            (0.0, 0.0)
        }
    }

    fn process_relation(
        &mut self,
        tags: &HashMap<String, String>,
        objects: &mut Vec<GeoObject>,
    ) {
        let rel_type = tags.get("type").map(|s| s.as_str());

        match rel_type {
            Some("boundary") if tags.get("boundary") == Some(&"administrative".to_string()) => {
                if let Some(obj) = extract_named_object(tags, 0.0, 0.0, self.corrector.as_ref()) {
                    let admin_level = tags.get("admin_level").cloned();
                    let mut obj = obj;
                    if admin_level.is_some() && obj.category.is_none() {
                        obj.category = Some(format!(
                            "admin_level:{}",
                            admin_level.as_deref().unwrap_or("?")
                        ));
                    }
                    objects.push(GeoObject::Named(obj));
                    self.named_objects += 1;
                }
            }
            Some("associatedStreet") => {
                let street_name = tags.get("name").or_else(|| tags.get("name:ru"));
                if let Some(name) = street_name {
                    let mut addr_tags = HashMap::new();
                    addr_tags.insert("addr:street".to_string(), name.clone());
                    if let Some(city) = tags.get("addr:city") {
                        addr_tags.insert("addr:city".to_string(), city.clone());
                    }
                    if let Some(addr) = extract_address(&addr_tags, 0.0, 0.0, self.corrector.as_ref()) {
                        objects.push(GeoObject::Address(addr));
                        self.addresses += 1;
                    }
                }
            }
            _ => {
                if let Some(obj) = extract_named_object(tags, 0.0, 0.0, self.corrector.as_ref()) {
                    objects.push(GeoObject::Named(obj));
                    self.named_objects += 1;
                }
            }
        }
    }

    fn process_element(
        &mut self,
        tags: &HashMap<String, String>,
        lat: f64,
        lon: f64,
        objects: &mut Vec<GeoObject>,
    ) {
        let has_address = tags.contains_key("addr:housenumber")
            || tags.contains_key("addr:street")
            || tags.contains_key("addr:city");

        let has_name = tags.contains_key("name") || tags.contains_key("name:ru");

        if has_address
            && let Some(addr) = extract_address(tags, lat, lon, self.corrector.as_ref())
        {
            objects.push(GeoObject::Address(addr));
            self.addresses += 1;
        }

        if has_name
            && let Some(obj) = extract_named_object(tags, lat, lon, self.corrector.as_ref())
        {
            objects.push(GeoObject::Named(obj));
            self.named_objects += 1;
        }
    }

    #[allow(dead_code)]
    pub fn stats(&self) -> (u64, u64, u64, u64, u64) {
        (
            self.nodes,
            self.ways,
            self.relations,
            self.addresses,
            self.named_objects,
        )
    }
}

/// Извлечь Address из тегов.
fn extract_address(tags: &HashMap<String, String>, lat: f64, lon: f64, corrector: Option<&Corrector>) -> Option<Address> {
    let country = tags.get("addr:country").cloned();
    let city = correct_field(
        tags.get("addr:city")
            .or_else(|| tags.get("addr:town"))
            .or_else(|| tags.get("addr:place"))
            .cloned(),
        corrector,
    );
    let street = correct_field(tags.get("addr:street").cloned(), corrector);
    let housenumber = tags.get("addr:housenumber").cloned();
    let postcode = tags.get("addr:postcode").cloned();

    if city.is_none() && (street.is_none() || housenumber.is_none()) {
        return None;
    }

    Some(Address {
        country,
        city,
        street,
        housenumber,
        postcode,
        lat,
        lon,
    })
}

/// Применить корректор к полю, если оно задано и корректор доступен.
/// Сначала исправляет опечатки, затем нормализует регистр.
fn correct_field(value: Option<String>, corrector: Option<&Corrector>) -> Option<String> {
    match (value, corrector) {
        (Some(v), Some(c)) => {
            let corrected = c.correct(&v);
            let agreed = Corrector::fix_adjective_agreement(&corrected);
            let normalized = Corrector::normalize_case(&agreed);
            Some(normalized)
        }
        (v, _) => v,
    }
}

/// Извлечь NamedObject из тегов. Только русское имя + транслитерация.
fn extract_named_object(tags: &HashMap<String, String>, lat: f64, lon: f64, corrector: Option<&Corrector>) -> Option<NamedObject> {
    let name = correct_field(
        tags.get("name:ru")
            .or_else(|| tags.get("name"))
            .cloned(),
        corrector,
    )?;

    let category = tags
        .get("amenity")
        .or_else(|| tags.get("tourism"))
        .or_else(|| tags.get("shop"))
        .or_else(|| tags.get("historic"))
        .or_else(|| tags.get("leisure"))
        .or_else(|| tags.get("office"))
        .or_else(|| tags.get("boundary"))
        .or_else(|| tags.get("highway"))
        .or_else(|| tags.get("railway"))
        .or_else(|| tags.get("aeroway"))
        .or_else(|| tags.get("waterway"))
        .or_else(|| tags.get("natural"))
        .cloned();

    let translit = crate::translit::transliterate(&name);

    Some(NamedObject {
        name,
        translit,
        category,
        lat,
        lon,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_address_minimal() {
        let mut tags = HashMap::new();
        tags.insert("addr:city".to_string(), "Москва".to_string());
        tags.insert("addr:street".to_string(), "Тверская".to_string());
        tags.insert("addr:housenumber".to_string(), "1".to_string());

        let addr = extract_address(&tags, 55.7558, 37.6173, None).unwrap();
        assert_eq!(addr.city.unwrap(), "Москва");
        assert_eq!(addr.street.unwrap(), "Тверская");
    }

    #[test]
    fn test_extract_address_no_city_no_street() {
        let mut tags = HashMap::new();
        tags.insert("addr:housenumber".to_string(), "1".to_string());
        assert!(extract_address(&tags, 0.0, 0.0, None).is_none());
    }

    #[test]
    fn test_extract_named_object_no_name() {
        let tags = HashMap::new();
        assert!(extract_named_object(&tags, 0.0, 0.0, None).is_none());
    }

    #[test]
    fn test_parser_no_way_coords() {
        let parser = PbfParser::new_no_way_coords();
        assert!(!parser.resolve_way_coords);
    }

    #[test]
    fn test_extract_named_object_full() {
        let mut tags = HashMap::new();
        tags.insert("name:ru".to_string(), "Красная площадь".to_string());
        tags.insert("amenity".to_string(), "tourism".to_string());
        let obj = extract_named_object(&tags, 55.75, 37.62, None).unwrap();
        assert_eq!(obj.name, "Красная площадь");
        assert_eq!(obj.category.unwrap(), "tourism");
        // Кириллическое имя → транслитерация должна быть
        assert!(obj.translit.is_some());
    }

    #[test]
    fn test_extract_named_object_fallback_to_name() {
        let mut tags = HashMap::new();
        tags.insert("name".to_string(), "Red Square".to_string());
        let obj = extract_named_object(&tags, 0.0, 0.0, None).unwrap();
        assert_eq!(obj.name, "Red Square");
        // Латиница → транслитерации нет
        assert!(obj.translit.is_none());
    }

    #[test]
    fn test_extract_named_object_category_priority() {
        let mut tags = HashMap::new();
        tags.insert("name".to_string(), "Test".to_string());
        tags.insert("amenity".to_string(), "cafe".to_string());
        let obj = extract_named_object(&tags, 0.0, 0.0, None).unwrap();
        assert_eq!(obj.category.unwrap(), "cafe");
    }
}
