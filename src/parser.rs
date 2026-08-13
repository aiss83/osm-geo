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
use crate::utils::{levenshtein_distance, haversine_approx};

/// Парсер PBF-файлов OSM.
pub struct PbfParser {
    nodes: u64,
    ways: u64,
    relations: u64,
    addresses: u64,
    named_objects: u64,
    /// Кэш координат нод (node_id → (lat, lon)).
    node_coords: HashMap<i64, (f64, f64)>,
    /// Кэш центроидов way'ев (way_id → (lat, lon)) — для вычисления центроида relation.
    way_coords: HashMap<i64, (f64, f64)>,
    /// Опциональный корректор опечаток (SymSpell).
    corrector: Option<Corrector>,
    /// Типы населённых пунктов: название → описание (СНТ, посёлок, ...)
    place_types: HashMap<String, String>,
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
            way_coords: HashMap::new(),
            corrector: None,
            place_types: HashMap::new(),
        }
    }

    /// Быстрый проход по PBF: собрать place/landuse-типы для всех именованных мест.
    /// Возвращает общее число элементов в файле для точного прогресса основного прохода.
    fn collect_place_types(&mut self, path: &Path) -> Result<u64> {
        let reader = ElementReader::from_path(path)?;

        // Точный прогресс здесь невозможен: общее число элементов мы узнаём
        // только в конце этого прохода. Показываем счётчик + спиннер.
        let pb = ProgressBar::new_spinner();
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        pb.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{elapsed_precise}] Сбор типов мест: {pos} элементов",
            )
            .unwrap(),
        );

        let mut total: u64 = 0;
        reader.for_each(|element| {
            total += 1;
            let tags: HashMap<String, String> = match &element {
                Element::Node(n) => n.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
                Element::DenseNode(n) => n.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
                Element::Way(w) => w.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
                Element::Relation(r) => r.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            };
            if let Some(place_name) = tags.get("name").or_else(|| tags.get("name:ru")) {
                if let Some(place_type) = place_type_label(&tags) {
                    self.place_types
                        .entry(place_name.to_string())
                        .or_insert(place_type);
                }
            }
            pb.inc(1);
        })?;

        pb.finish_with_message(format!("Собрано типов мест: {}", self.place_types.len()));
        Ok(total)
    }

    /// Разобрать PBF-файл и вернуть вектор GeoObject.
    pub fn parse_file(&mut self, path: &Path) -> Result<Vec<GeoObject>> {
        info!("Парсинг PBF-файла: {:?}", path);

        // Предварительный проход: собираем типы населённых пунктов
        // и общее число элементов для точного прогресса.
        let total_elements = self.collect_place_types(path)?;

        let pb = ProgressBar::new(total_elements);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] Парсинг PBF: {percent:>3}% [{bar:40}] {pos}/{len} элементов, ETA {eta}")
                .unwrap()
                .progress_chars("##-"),
        );

        let reader = ElementReader::from_path(path)?;
        let mut objects = Vec::new();

        reader.for_each(|element| {
            match element {
                Element::Node(node) => {
                    self.nodes += 1;
                    let id = node.id();

                    self.node_coords.insert(id, (node.lat(), node.lon()));

                    let tags: HashMap<String, String> = node
                        .tags()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    self.process_element(&tags, node.lat(), node.lon(), &mut objects);
                }
                Element::DenseNode(node) => {
                    self.nodes += 1;
                    let id = node.id();

                    self.node_coords.insert(id, (node.lat(), node.lon()));

                    let tags: HashMap<String, String> = node
                        .tags()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    self.process_element(&tags, node.lat(), node.lon(), &mut objects);
                }
                Element::Way(way) => {
                    self.ways += 1;
                    let way_id = way.id();
                    let tags: HashMap<String, String> = way
                        .tags()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();

                    let (lat, lon) = self.compute_way_centroid(&way);
                    if lat != 0.0 || lon != 0.0 {
                        self.way_coords.insert(way_id, (lat, lon));
                    }

                    self.process_element(&tags, lat, lon, &mut objects);
                }
                Element::Relation(rel) => {
                    self.relations += 1;
                    let tags: HashMap<String, String> = rel
                        .tags()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    let (lat, lon) = self.compute_relation_centroid(&rel);
                    self.process_relation(&tags, lat, lon, &mut objects);
                }
            }

            pb.inc(1);
        })?;

        pb.finish_with_message(format!("Извлечено {} объектов", objects.len()));

        info!("Кэш нод: {} записей, way'ев: {}", self.node_coords.len(), self.way_coords.len());
        self.node_coords.clear();
        self.node_coords.shrink_to_fit();
        self.way_coords.clear();
        self.way_coords.shrink_to_fit();

        info!(
            "Парсинг завершён: nodes={}, ways={}, relations={}, addresses={}, named_objects={}",
            self.nodes, self.ways, self.relations, self.addresses, self.named_objects
        );

        Ok(objects)
    }

    /// Вычислить центроид relation по координатам node-членов.
    /// Возвращает (0, 0) если ни одной ноды не найдено в кэше.
    fn compute_relation_centroid(&self, rel: &osmpbf::Relation) -> (f64, f64) {
        let mut sum_lat = 0.0f64;
        let mut sum_lon = 0.0f64;
        let mut count = 0u64;

        for member in rel.members() {
            match member.member_type {
                osmpbf::RelMemberType::Node => {
                    if let Some(&(lat, lon)) = self.node_coords.get(&member.member_id) {
                        sum_lat += lat;
                        sum_lon += lon;
                        count += 1;
                    }
                }
                osmpbf::RelMemberType::Way => {
                    if let Some(&(lat, lon)) = self.way_coords.get(&member.member_id) {
                        sum_lat += lat;
                        sum_lon += lon;
                        count += 1;
                    }
                }
                _ => {}
            }
        }

        if count > 0 {
            (sum_lat / count as f64, sum_lon / count as f64)
        } else {
            (0.0, 0.0)
        }
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
            (sum_lat / count as f64, sum_lon / count as f64)
        } else {
            (0.0, 0.0)
        }
    }

    fn process_relation(
        &mut self,
        tags: &HashMap<String, String>,
        lat: f64,
        lon: f64,
        objects: &mut Vec<GeoObject>,
    ) {
        let rel_type = tags.get("type").map(|s| s.as_str());

        match rel_type {
            Some("boundary") if tags.get("boundary") == Some(&"administrative".to_string()) => {
                if let Some(obj) = extract_named_object(tags, lat, lon, self.corrector.as_ref()) {
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
                    if let Some(addr) = extract_address(&addr_tags, lat, lon, self.corrector.as_ref(), &self.place_types) {
                        objects.push(GeoObject::Address(addr));
                        self.addresses += 1;
                    }
                }
            }
            _ => {
                if let Some(obj) = extract_named_object(tags, lat, lon, self.corrector.as_ref()) {
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
            && let Some(addr) = extract_address(tags, lat, lon, self.corrector.as_ref(), &self.place_types)
        {
            objects.push(GeoObject::Address(addr));
            self.addresses += 1;
        }

        else if has_name
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

impl crate::source::FeatureSource for PbfParser {
    fn set_corrector(&mut self, corrector: Option<Corrector>) {
        self.corrector = corrector;
    }

    fn parse(&mut self, path: &Path) -> Result<Vec<GeoObject>> {
        self.parse_file(path)
    }
}

/// Привязать адреса без города к ближайшему известному городу по координатам.
///
/// Строит карту «город → центроид» по адресам, у которых город уже указан,
/// затем для каждого адреса без города находит ближайший центроид и назначает его.
pub fn infer_missing_cities(objects: &mut [GeoObject]) {
    use std::collections::HashMap;

    // 1. Собираем центроиды городов (усреднённые координаты адресов с известным городом)
    let mut city_coords: HashMap<String, (f64, f64, u64)> = HashMap::new();
    let pb = crate::utils::progress_bar(objects.len() as u64, "Сбор центроидов городов");
    for obj in objects.iter() {
        let (lat, lon, city_opt) = match obj {
            GeoObject::Address(addr) => (addr.lat, addr.lon, addr.city.as_ref()),
            GeoObject::Named(named) => (named.lat, named.lon, named.city.as_ref()),
        };
        if let Some(city) = city_opt {
            if !city.is_empty() {
                let entry = city_coords.entry(city.clone()).or_insert((0.0, 0.0, 0));
                entry.0 += lat;
                entry.1 += lon;
                entry.2 += 1;
            }
        }
        pb.inc(1);
    }
    pb.finish_and_clear();

    if city_coords.is_empty() {
        return;
    }

    // Вычисляем центроиды
    let centroids: Vec<(&str, f64, f64)> = city_coords
        .iter()
        .map(|(name, (sum_lat, sum_lon, count))| {
            (name.as_str(), sum_lat / *count as f64, sum_lon / *count as f64)
        })
        .collect();

    // 2. Для адресов без города находим ближайший
    let mut assigned = 0u64;
    let pb = crate::utils::progress_bar(objects.len() as u64, "Привязка к ближайшему городу");
    for obj in objects.iter_mut() {
        let (lat, lon, city_ref) = match obj {
            GeoObject::Address(addr) => (addr.lat, addr.lon, &mut addr.city),
            GeoObject::Named(named) => (named.lat, named.lon, &mut named.city),
        };
        if city_ref.as_ref().map_or(true, |c| c.is_empty()) {
            let mut best_city: Option<&str> = None;
            let mut best_dist = f64::MAX;

            for &(city_name, city_lat, city_lon) in &centroids {
                let dist = haversine_approx(lat, lon, city_lat, city_lon);
                if dist < best_dist {
                    best_dist = dist;
                    best_city = Some(city_name);
                }
            }

            if let Some(city) = best_city {
                *city_ref = Some(city.to_string());
                assigned += 1;
            }
        }
        pb.inc(1);
    }
    pb.finish_and_clear();

    if assigned > 0 {
        log::info!(
            "Привязка городов: {} объектам без города назначен ближайший город (из {} известных)",
            assigned, centroids.len()
        );
    }
}

/// Проставить страну для всех объектов, у которых она не задана.
///
/// Источники (в порядке приоритета):
/// 1. Если страна уже задана в объекте — не трогаем.
/// 2. Если известен город — ищем страну по другим объектам в том же городе.
/// 3. Извлекаем код страны из кода региона (напр. "RU-CFD" → "RU").
pub fn infer_missing_countries(objects: &mut [GeoObject], region_code: Option<&str>) {
    use std::collections::HashMap;

    // 1. Строим карту «город → страна» по объектам, где известны оба
    let mut city_country: HashMap<String, String> = HashMap::new();
    let pb = crate::utils::progress_bar(objects.len() as u64, "Сбор карты город → страна");
    for obj in objects.iter() {
        let (city_opt, country_opt) = match obj {
            GeoObject::Address(addr) => (addr.city.as_ref(), addr.country.as_ref()),
            GeoObject::Named(named) => (named.city.as_ref(), named.country.as_ref()),
        };
        if let (Some(city), Some(country)) = (city_opt, country_opt) {
            if !city.is_empty() && !country.is_empty() {
                city_country.entry(city.clone()).or_insert_with(|| country.clone());
            }
        }
        pb.inc(1);
    }
    pb.finish_and_clear();

    // 2. Извлекаем код страны из региона (первые 2 символа до дефиса или всё)
    let region_country = region_code
        .and_then(|r| r.split(['-', '_']).next())
        .filter(|c| c.len() == 2 && c.chars().all(|ch| ch.is_ascii_uppercase()));

    // 3. Проставляем страну
    let mut assigned = 0u64;
    let pb = crate::utils::progress_bar(objects.len() as u64, "Привязка страны");
    for obj in objects.iter_mut() {
        pb.inc(1);
        // Извлекаем город ДО мутабельного заимствования страны
        let city_opt: Option<String> = match obj {
            &mut GeoObject::Address(ref addr) => addr.city.clone(),
            &mut GeoObject::Named(ref named) => named.city.clone(),
        };

        let country_ref: &mut Option<String> = match obj {
            GeoObject::Address(addr) => &mut addr.country,
            GeoObject::Named(named) => &mut named.country,
        };

        // Уже задана — пропускаем
        if country_ref.as_ref().map_or(false, |c| !c.is_empty()) {
            continue;
        }

        // Пробуем найти страну по городу
        if let Some(ref city) = city_opt {
            if let Some(country) = city_country.get(city.as_str()) {
                *country_ref = Some(country.clone());
                assigned += 1;
                continue;
            }
        }

        // Fallback: из кода региона
        if let Some(rc) = region_country {
            *country_ref = Some(rc.to_string());
            assigned += 1;
        }
    }
    pb.finish_and_clear();

    if assigned > 0 {
        log::info!(
            "Привязка стран: {} объектам проставлена страна (городская карта: {} городов, регион: {:?})",
            assigned, city_country.len(), region_country
        );
    }
}

/// Склеить города-опечатки с каноническими названиями.
///
/// Если название города отличается от более частотного на 1 символ
/// (вставка, удаление или замена), все адреса опечаточного города
/// переназначаются на канонический.
///
/// Пример: «Калининнград» (2001 адрес) → «Калининград» (17280 адресов).
pub fn merge_typo_cities(objects: &mut [GeoObject]) {
    use std::collections::HashMap;

    // 1. Считаем частоты городов
    let mut city_counts: HashMap<String, u64> = HashMap::new();
    let pb = crate::utils::progress_bar(objects.len() as u64, "Подсчёт частот городов");
    for obj in objects.iter() {
        let city_opt = match obj {
            GeoObject::Address(addr) => addr.city.as_ref(),
            GeoObject::Named(named) => named.city.as_ref(),
        };
        if let Some(city) = city_opt {
            if !city.is_empty() {
                *city_counts.entry(city.clone()).or_default() += 1;
            }
        }
        pb.inc(1);
    }
    pb.finish_and_clear();

    // 2. Сортируем по убыванию частоты: частые — канонические кандидаты
    let mut sorted: Vec<(String, u64)> = city_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    // 3. Для каждого города ищем каноническую замену среди более частых
    let mut replacements: HashMap<String, String> = HashMap::new();
    let pb = crate::utils::progress_bar(sorted.len() as u64, "Поиск опечаток городов");
    for (i, (city, count)) in sorted.iter().enumerate() {
        // Ищем среди более частых городов ближайший по Левенштейну
        for (canonical, canon_count) in sorted[..i].iter() {
            // Пропускаем, если разница в частоте неочевидна (< 2×)
            if *count * 2 >= *canon_count {
                continue;
            }
            // Редакционное расстояние 1 или 2 — опечатка/дублирование символа
            let dist = levenshtein_distance(city, canonical);
            if dist == 1 || (dist == 2 && city.len() >= 6) {
                replacements.insert(city.clone(), canonical.clone());
                break;
            }
        }
        pb.inc(1);
    }
    pb.finish_and_clear();

    if replacements.is_empty() {
        return;
    }

    // 4. Применяем замены
    let mut merged = 0u64;
    let pb = crate::utils::progress_bar(objects.len() as u64, "Применение замен городов");
    for obj in objects.iter_mut() {
        let city_ref: &mut Option<String> = match obj {
            GeoObject::Address(addr) => &mut addr.city,
            GeoObject::Named(named) => &mut named.city,
        };
        if let Some(city) = city_ref.as_deref() {
            if let Some(fixed) = replacements.get(city) {
                *city_ref = Some(fixed.clone());
                merged += 1;
            }
        }
        pb.inc(1);
    }
    pb.finish_and_clear();

    log::info!(
        "Склеивание опечаток: {} городов исправлено, {} объектов переназначено",
        replacements.len(), merged
    );
}

/// Извлечь Address из тегов.
fn extract_address(tags: &HashMap<String, String>, lat: f64, lon: f64, corrector: Option<&Corrector>, place_types: &HashMap<String, String>) -> Option<Address> {
    let country = tags.get("addr:country").cloned();
    let city = correct_field(
        tags.get("addr:city")
            .or_else(|| tags.get("addr:town"))
            .or_else(|| tags.get("addr:place"))
            .cloned(),
        corrector,
    );
    let mut street = correct_field(tags.get("addr:street").cloned(), corrector);
    let housenumber = tags.get("addr:housenumber").cloned();
    let postcode = tags.get("addr:postcode").cloned();

    // Если улицы нет, пробуем подставить тип населённого пункта
    if street.is_none() {
        if let Some(ref city_name) = city {
            if let Some(place_desc) = place_types.get(city_name) {
                street = Some(format!("{} {}", place_desc, city_name));
            }
        }
    }

    // Требуем улицу и номер дома; адреса без улицы не включаем
    if street.is_none() || housenumber.is_none() {
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

/// Определить русское описание типа населённого пункта по OSM-тегам.
fn place_type_label(tags: &HashMap<String, String>) -> Option<String> {
    if let Some(place) = tags.get("place") {
        match place.as_str() {
            "allotments" => return Some("СНТ".into()),
            "hamlet" => return Some("посёлок".into()),
            "village" => return Some("деревня".into()),
            "isolated_dwelling" => return Some("хутор".into()),
            "locality" => return Some("урочище".into()),
            "suburb" => return Some("микрорайон".into()),
            "neighbourhood" => return Some("квартал".into()),
            "town" => return Some("город".into()),
            "city" => return Some("город".into()),
            "borough" => return Some("район".into()),
            _ => {}
        }
    }
    if tags.get("landuse").map(|s| s.as_str()) == Some("allotments") {
        return Some("СНТ".into());
    }
    None
}

/// Применить корректор к полю, если оно задано и корректор доступен.
/// Порядок важен: сначала SymSpell (он пропускает не-кириллицу вроде Ƒ),
/// затем normalize_chars исправляет битые OSM-символы.
fn correct_field(value: Option<String>, corrector: Option<&Corrector>) -> Option<String> {
    // 1. SymSpell-коррекция опечаток + нормализация падежа + согласование + регистр
    let value = match (&value, corrector) {
        (Some(v), Some(c)) => {
            let corrected = c.correct(v);
            let case_fixed = Corrector::normalize_street_types_case(&corrected);
            let agreed = Corrector::fix_adjective_agreement(&case_fixed);
            let normalized = Corrector::normalize_case(&agreed);
            Some(normalized)
        }
        _ => value,
    };
    // 2. Нормализация битых символов (Ƒ→Д/Т/К)
    value.map(|v| Corrector::normalize_chars(&v))
}

/// Извлечь NamedObject из тегов. Только русское имя + транслитерация.
///
/// В базу попадают две категории POI:
/// - **historic** — объекты культурного наследия, памятники, мемориалы
/// - **tourism** — туристические объекты (зоопарки, музеи, отели, ...)
///
/// Всё остальное (магазины, транспорт, дороги, досуг, офисы, природные объекты и т.д.)
/// в Named-индекс не включается.
fn extract_named_object(tags: &HashMap<String, String>, lat: f64, lon: f64, corrector: Option<&Corrector>) -> Option<NamedObject> {
    // Требуем ровно одну из допустимых категорий — иначе это не целевой POI.
    let (category_key, _category_value) = if let Some(v) = tags.get("historic") {
        ("historic", v)
    } else if let Some(v) = tags.get("tourism") {
        ("tourism", v)
    } else {
        return None;
    };

    let name = correct_field(
        tags.get("name:ru")
            .or_else(|| tags.get("name"))
            .cloned(),
        corrector,
    )?;

    let country = tags.get("addr:country").cloned();
    let city = tags.get("addr:city")
        .or_else(|| tags.get("addr:town"))
        .or_else(|| tags.get("addr:place"))
        .cloned();

    Some(NamedObject {
        name,
        category: Some(category_key.to_string()),
        country,
        city,
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

        let addr = extract_address(&tags, 55.7558, 37.6173, None, &HashMap::new()).unwrap();
        assert_eq!(addr.city.unwrap(), "Москва");
        assert_eq!(addr.street.unwrap(), "Тверская");
    }

    #[test]
    fn test_extract_address_no_city_no_street() {
        let mut tags = HashMap::new();
        tags.insert("addr:housenumber".to_string(), "1".to_string());
        assert!(extract_address(&tags, 0.0, 0.0, None, &HashMap::new()).is_none());
    }

    #[test]
    fn test_extract_named_object_no_name() {
        let tags = HashMap::new();
        assert!(extract_named_object(&tags, 0.0, 0.0, None).is_none());
    }

    #[test]
    fn test_extract_named_object_historic() {
        let mut tags = HashMap::new();
        tags.insert("name:ru".to_string(), "Красная площадь".to_string());
        tags.insert("historic".to_string(), "monument".to_string());
        let obj = extract_named_object(&tags, 55.75, 37.62, None).unwrap();
        assert_eq!(obj.name, "Красная площадь");
        assert_eq!(obj.category.as_deref().unwrap(), "historic");
    }

    #[test]
    fn test_extract_named_object_excluded_shop() {
        // shop исключён из whitelist — должен вернуть None
        let mut tags = HashMap::new();
        tags.insert("name".to_string(), "Пятёрочка".to_string());
        tags.insert("shop".to_string(), "supermarket".to_string());
        assert!(extract_named_object(&tags, 0.0, 0.0, None).is_none());
    }

    #[test]
    fn test_extract_named_object_excluded_amenity() {
        // amenity не входит в whitelist — должен вернуть None
        let mut tags = HashMap::new();
        tags.insert("name".to_string(), "Test".to_string());
        tags.insert("amenity".to_string(), "cafe".to_string());
        assert!(extract_named_object(&tags, 0.0, 0.0, None).is_none());
    }

    #[test]
    fn test_extract_named_object_excluded_no_category() {
        // Без historic/shop — даже с именем не попадает в Named
        let mut tags = HashMap::new();
        tags.insert("name".to_string(), "Red Square".to_string());
        assert!(extract_named_object(&tags, 0.0, 0.0, None).is_none());
    }

    #[test]
    fn test_extract_named_object_excluded_highway() {
        // Дороги исключены
        let mut tags = HashMap::new();
        tags.insert("name".to_string(), "Московский проспект".to_string());
        tags.insert("highway".to_string(), "primary".to_string());
        assert!(extract_named_object(&tags, 0.0, 0.0, None).is_none());
    }
}
