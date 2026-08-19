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
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::model::{Address, Country, CountryBoundary, GeoObject, NamedObject};
use crate::corrector::Corrector;
use crate::utils::{levenshtein_distance, haversine_approx};

/// Пространственная сетка городов: клетка → список (название, lat, lon).
type CityGrid<'a> = HashMap<(i64, i64), Vec<(&'a str, f64, f64)>>;

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
    /// Страна, определённая по `admin_level=2` relation (если найдена).
    admin_country: Option<Country>,
    /// Итоговая страна (по `addr:country` большинству или admin_country).
    detected_country: Option<Country>,
    /// Счётчик значений `addr:country` для fallback-определения страны.
    addr_country_counts: HashMap<String, u64>,
    /// Граница страны (MultiPolygon), извлечённая из `admin_level=2` relation.
    boundary: Option<CountryBoundary>,
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
            admin_country: None,
            detected_country: None,
            addr_country_counts: HashMap::new(),
            boundary: None,
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
            if let Some(place_name) = tags.get("name").or_else(|| tags.get("name:ru"))
                && let Some(place_type) = place_type_label(&tags) {
                    self.place_types
                        .entry(place_name.to_string())
                        .or_insert(place_type);
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

        // Страна: сначала большинство `addr:country`, затем admin_level=2 ISO.
        let majority = {
            let mut best: Option<(u64, String)> = None;
            for (val, count) in &self.addr_country_counts {
                let code = val.trim().to_ascii_uppercase();
                if code.len() == 2 && code.chars().all(|c| c.is_ascii_uppercase())
                    && best.as_ref().map_or(true, |(bc, _)| *count > *bc) {
                        best = Some((*count, code));
                    }
            }
            best.map(|(_, code)| code)
        };
        self.detected_country = majority
            .and_then(|code| crate::country::from_code(&code))
            .or_else(|| self.admin_country.take());

        self.extract_boundary(path)?;

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

    /// Извлечь границу страны (`admin_level=2`) из PBF отдельными проходами.
    fn extract_boundary(&mut self, path: &Path) -> Result<()> {
        use osmpbf::RelMemberType;

        // Проход 1: найти relation страны и собрать member-way по ролям.
        let mut outer_ids: Vec<i64> = Vec::new();
        let mut inner_ids: Vec<i64> = Vec::new();
        let mut found = false;
        {
            let reader = ElementReader::from_path(path)?;
            reader.for_each(|element| {
                if found {
                    return;
                }
                if let Element::Relation(rel) = element {
                    let tags: HashMap<String, String> = rel
                        .tags()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    if tags.get("boundary").map(|s| s.as_str()) == Some("administrative")
                        && tags.get("admin_level").map(|s| s.as_str()) == Some("2")
                        && let Some(c) = crate::country::from_tags(&tags)
                        && self
                            .detected_country
                            .as_ref()
                            .map_or(true, |dc| dc.code.is_empty() || dc.code == c.code) {
                            for member in rel.members() {
                                if member.member_type == RelMemberType::Way {
                                    if member.role().unwrap_or("") == "inner" {
                                        inner_ids.push(member.member_id);
                                    } else {
                                        outer_ids.push(member.member_id);
                                    }
                                }
                            }
                            found = true;
                        }
                }
            })?;
        }

        if outer_ids.is_empty() {
            return Ok(());
        }

        // Проход 2: собрать node refs для нужных way'ев.
        let way_ids: HashSet<i64> = outer_ids.iter().chain(inner_ids.iter()).cloned().collect();
        let mut way_refs: HashMap<i64, Vec<i64>> = HashMap::new();
        {
            let reader = ElementReader::from_path(path)?;
            reader.for_each(|element| {
                if let Element::Way(way) = element
                    && way_ids.contains(&way.id()) {
                        way_refs.insert(way.id(), way.refs().collect());
                    }
            })?;
        }

        // Проход 3: собрать координаты нужных нод.
        let node_ids: HashSet<i64> = way_refs.values().flatten().cloned().collect();
        let mut node_coords: HashMap<i64, (f64, f64)> = HashMap::new();
        {
            let reader = ElementReader::from_path(path)?;
            reader.for_each(|element| {
                match &element {
                    Element::Node(n) if node_ids.contains(&n.id()) => {
                        node_coords.insert(n.id(), (n.lat(), n.lon()));
                    }
                    Element::DenseNode(n) if node_ids.contains(&n.id()) => {
                        node_coords.insert(n.id(), (n.lat(), n.lon()));
                    }
                    _ => {}
                }
            })?;
        }

        let to_polyline = |ids: &[i64]| -> Option<Vec<(f32, f32)>> {
            let mut pts = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(&(lat, lon)) = node_coords.get(id) {
                    pts.push((lat as f32, lon as f32));
                }
            }
            (pts.len() >= 2).then_some(pts)
        };

        let outer_ways: Vec<Vec<(f32, f32)>> = way_refs
            .iter()
            .filter(|(id, _)| outer_ids.contains(id))
            .filter_map(|(_, refs)| to_polyline(refs))
            .collect();
        let inner_ways: Vec<Vec<(f32, f32)>> = way_refs
            .iter()
            .filter(|(id, _)| inner_ids.contains(id))
            .filter_map(|(_, refs)| to_polyline(refs))
            .collect();

        self.boundary = crate::boundary::build_boundary(outer_ways, inner_ways);
        if self.boundary.is_some() {
            log::info!("Граница страны извлечена (полигонов: {})", self.boundary.as_ref().unwrap().polygons.len());
        }
        Ok(())
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
                // Определяем страну по границе страны (admin_level=2).
                if tags.get("admin_level").map(|s| s.as_str()) == Some("2")
                    && let Some(c) = crate::country::from_tags(tags) {
                        self.admin_country = Some(c);
                    }

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
        // Считаем addr:country для fallback-определения страны.
        if let Some(c) = tags.get("addr:country")
            && !c.is_empty() {
                *self.addr_country_counts.entry(c.clone()).or_default() += 1;
            }

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

    fn country(&self) -> Option<Country> {
        self.detected_country.clone()
    }

    fn boundary(&self) -> Option<CountryBoundary> {
        self.boundary.clone()
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
        if let Some(city) = city_opt
            && !city.is_empty() {
                let entry = city_coords.entry(city.clone()).or_insert((0.0, 0.0, 0));
                entry.0 += lat;
                entry.1 += lon;
                entry.2 += 1;
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

    // 2. Строим равномерную пространственную сетку для быстрого поиска.
    //    Поиск остаётся точным: клетки расширяются кольцами, пока нижняя
    //    граница расстояния до следующего кольца не превысит найденный минимум.
    const CELL_DEG: f64 = 0.05; // ~5.5 км по меридиану
    let mut grid: CityGrid<'_> = HashMap::new();
    let mut max_abs_lat: f64 = 0.0;
    for &(name, lat, lon) in &centroids {
        let cx = (lon / CELL_DEG).floor() as i64;
        let cy = (lat / CELL_DEG).floor() as i64;
        grid.entry((cx, cy)).or_default().push((name, lat, lon));
        max_abs_lat = max_abs_lat.max(lat.abs());
    }
    let cell_meters = 6_371_000.0 * CELL_DEG.to_radians();

    // 3. Для адресов без города находим ближайший город по сетке
    let mut assigned = 0u64;
    let pb = crate::utils::progress_bar(objects.len() as u64, "Привязка к ближайшему городу");
    for obj in objects.iter_mut() {
        let (lat, lon, city_ref) = match obj {
            GeoObject::Address(addr) => (addr.lat, addr.lon, &mut addr.city),
            GeoObject::Named(named) => (named.lat, named.lon, &mut named.city),
        };
        if city_ref.as_ref().is_none_or(|c| c.is_empty()) {
            let qx = (lon / CELL_DEG).floor() as i64;
            let qy = (lat / CELL_DEG).floor() as i64;
            // Консервативная нижняя граница расстояния до следующего кольца:
            // широтное смещение даёт CELL_DEG, долготное — CELL_DEG·cos(φ).
            // Берём самый маленький cos среди городов и запроса.
            let cos_bound = max_abs_lat.max(lat.abs()).to_radians().cos().max(1e-6);

            let mut best_city: Option<&str> = None;
            let mut best_dist = f64::MAX;

            let mut r: i64 = 0;
            loop {
                for cy in (qy - r)..=(qy + r) {
                    for cx in (qx - r)..=(qx + r) {
                        // только клетки границы текущего кольца
                        if cx != qx - r && cx != qx + r && cy != qy - r && cy != qy + r {
                            continue;
                        }
                        if let Some(cities) = grid.get(&(cx, cy)) {
                            for &(name, clat, clon) in cities {
                                let dist = haversine_approx(lat, lon, clat, clon);
                                if dist < best_dist {
                                    best_dist = dist;
                                    best_city = Some(name);
                                }
                            }
                        }
                    }
                }

                // Дальше не найдём ничего ближе: следующее кольцо дальше `best_dist`.
                if cell_meters * (r as f64) * cos_bound >= best_dist {
                    break;
                }
                r += 1;
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
        if let Some(city) = city_opt
            && !city.is_empty() {
                *city_counts.entry(city.clone()).or_default() += 1;
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
        if let Some(city) = city_ref.as_deref()
            && let Some(fixed) = replacements.get(city) {
                *city_ref = Some(fixed.clone());
                merged += 1;
            }
        pb.inc(1);
    }
    pb.finish_and_clear();

    log::info!(
        "Склеивание опечаток: {} городов исправлено, {} объектов переназначено",
        replacements.len(), merged
    );
}

/// Разбить значение тега по `,`/`;` и взять первый непустой фрагмент.
fn split_first(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        v.split([',', ';'])
            .map(str::trim)
            .find(|p| !p.is_empty())
            .map(str::to_string)
    })
}

/// Ключевые слова типов улиц (с распространёнными падежами и сокращениями).
const STREET_TYPE_HINTS: &[&str] = &[
    "улица", "улицы", "улице", "улицу", "улицей",
    "проспект", "проспекта", "проспекту", "проспектом",
    "переулок", "переулка", "переулку", "переулком",
    "проезд", "проезда", "проезду", "проездом",
    "шоссе", "набережная", "набережной",
    "бульвар", "бульвара", "бульвару", "бульваром",
    "аллея", "аллеи", "площадь", "площади",
    "тупик", "тупика", "линия", "линии",
    "просек", "тракт", "дорога", "дороги",
    "микрорайон", "квартал",
    "ул", "пр-т", "пр-кт", "просп", "пер", "пр-д", "наб", "бул", "б-р", "пл", "ш",
];

/// Похож ли фрагмент на улицу: содержит слово-тип улицы И название
/// (не только сам тип — голое «улица» улицей не считаем).
fn looks_like_street(part: &str) -> bool {
    let words: Vec<String> = part
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-')
                .to_lowercase()
        })
        .collect();
    let has_type = words
        .iter()
        .any(|w| STREET_TYPE_HINTS.contains(&w.as_str()));
    let has_name = words
        .iter()
        .any(|w| !STREET_TYPE_HINTS.contains(&w.as_str()));
    has_type && has_name
}

/// Умное разбиение `addr:street`.
///
/// - Одна часть — возвращаем её как есть.
/// - Несколько частей — ищем ту, что похожа на улицу.
/// - Улицы нет (только регион/населённый пункт/номер дома) — возвращаем `None`,
///   чтобы такой адрес не попал в базу с «улицей» из административного объекта.
fn split_street(value: Option<String>) -> Option<String> {
    let value = value?;
    let parts: Vec<&str> = value
        .split([',', ';'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();

    match parts.as_slice() {
        [] => None,
        [single] => Some((*single).to_string()),
        _ => parts
            .iter()
            .copied()
            .find(|p| looks_like_street(p))
            .map(str::to_string),
    }
}

/// Извлечь Address из тегов.
pub(crate) fn extract_address(tags: &HashMap<String, String>, lat: f64, lon: f64, corrector: Option<&Corrector>, place_types: &HashMap<String, String>) -> Option<Address> {
    let country = tags.get("addr:country").cloned();
    let city = correct_field(
        split_first(
            tags.get("addr:city")
                .or_else(|| tags.get("addr:town"))
                .or_else(|| tags.get("addr:place"))
                .cloned(),
        ),
        corrector,
    );
    let street_tag = tags.get("addr:street").cloned();
    let mut street = correct_field(split_street(street_tag.clone()), corrector);
    let housenumber = tags.get("addr:housenumber").cloned();

    // Тега улицы нет вовсе — пробуем подставить тип населённого пункта.
    // Если же улица указана, но в ней нет части-улицы (только регион/населённый
    // пункт), адрес не используем.
    if street.is_none() && street_tag.is_none()
        && let Some(ref city_name) = city
            && let Some(place_desc) = place_types.get(city_name) {
                street = Some(format!("{} {}", place_desc, city_name));
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
        lat,
        lon,
    })
}

/// Определить русское описание типа населённого пункта по OSM-тегам.
pub(crate) fn place_type_label(tags: &HashMap<String, String>) -> Option<String> {
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
pub(crate) fn extract_named_object(tags: &HashMap<String, String>, lat: f64, lon: f64, corrector: Option<&Corrector>) -> Option<NamedObject> {
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
    fn test_extract_address_splits_comma_street() {
        let mut tags = HashMap::new();
        tags.insert(
            "addr:street".to_string(),
            "улица 1-й Ударной Армии, улица Ленина".to_string(),
        );
        tags.insert("addr:housenumber".to_string(), "3".to_string());
        let addr = extract_address(&tags, 0.0, 0.0, None, &HashMap::new()).unwrap();
        assert_eq!(addr.street.unwrap(), "улица 1-й Ударной Армии");
    }

    #[test]
    fn test_extract_address_drops_region_settlement_street() {
        // Регион + населённый пункт без улицы — адрес не используем, даже если
        // город известен и для него есть тип населённого пункта.
        let mut tags = HashMap::new();
        tags.insert(
            "addr:street".to_string(),
            "Московская область, Наро-Фоминск".to_string(),
        );
        tags.insert("addr:city".to_string(), "Наро-Фоминск".to_string());
        tags.insert("addr:housenumber".to_string(), "1".to_string());
        let mut place_types = HashMap::new();
        place_types.insert("Наро-Фоминск".to_string(), "город".to_string());
        assert!(extract_address(&tags, 0.0, 0.0, None, &place_types).is_none());
    }

    #[test]
    fn test_extract_address_no_street_uses_place_type() {
        // Тега улицы нет вовсе — подставляем тип населённого пункта.
        let mut tags = HashMap::new();
        tags.insert("addr:place".to_string(), "Наро-Фоминск".to_string());
        tags.insert("addr:housenumber".to_string(), "1".to_string());
        let mut place_types = HashMap::new();
        place_types.insert("Наро-Фоминск".to_string(), "город".to_string());
        let addr = extract_address(&tags, 0.0, 0.0, None, &place_types).unwrap();
        assert_eq!(addr.street.unwrap(), "город Наро-Фоминск");
    }

    #[test]
    fn test_split_first() {
        assert_eq!(
            split_first(Some("улица А, улица Б".into())),
            Some("улица А".into())
        );
        assert_eq!(
            split_first(Some("улица А;улица Б".into())),
            Some("улица А".into())
        );
        assert_eq!(
            split_first(Some("  ,  улица А , ".into())),
            Some("улица А".into())
        );
        assert_eq!(split_first(Some("".into())), None);
        assert_eq!(split_first(None), None);
    }

    #[test]
    fn test_split_street_picks_street_part() {
        assert_eq!(
            split_street(Some("Московская область, Наро-Фоминск, улица Ленина, 8".into())),
            Some("улица Ленина".into())
        );
        assert_eq!(
            split_street(Some("улица 1-й Ударной Армии, дом 12/1".into())),
            Some("улица 1-й Ударной Армии".into())
        );
        assert_eq!(
            split_street(Some("Ленинградское шоссе, 88-й километр".into())),
            Some("Ленинградское шоссе".into())
        );
        // одна часть — возвращаем как есть (название без слова-типа — норма)
        assert_eq!(
            split_street(Some("Тверская".into())),
            Some("Тверская".into())
        );
        // нет улицы (регион + населённый пункт) — не используем как улицу
        assert_eq!(
            split_street(Some("Московская область, Наро-Фоминск".into())),
            None
        );
        // голое «улица» после запятой — не считаем улицей
        assert_eq!(
            split_street(Some("17-я Лесная, улица".into())),
            None
        );
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
