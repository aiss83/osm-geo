//! FFI-привязки к `libgolffi` — C ABI поверх libgeodesk.
//!
//! Позволяет читать GOL напрямую, без подпроцесса `gol` и без промежуточного PBF.
//! Доступно только при сборке с feature-флагом `gol-ffi` (см. `build.rs`).

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;

use anyhow::{bail, Context, Result};
use log::info;

use crate::corrector::Corrector;
use crate::model::{Country, CountryBoundary, GeoObject};
use crate::parser::{extract_address, extract_named_object, place_type_label};
use crate::source::FeatureSource;

// Опак-типы C ABI.
#[repr(C)]
struct GolFeatures {
    _private: [u8; 0],
}

#[repr(C)]
struct GolFeature {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn gol_open(path: *const c_char) -> *mut GolFeatures;
    fn gol_close(f: *mut GolFeatures);
    fn gol_iterate(f: *const GolFeatures) -> *mut GolFeature;
    fn gol_count(f: *const GolFeatures) -> u64;
    fn gol_next(it: *mut GolFeature) -> i32;
    fn gol_free(it: *mut GolFeature);
    fn gol_type(it: *const GolFeature) -> i32;
    fn gol_lon(it: *const GolFeature) -> f64;
    fn gol_lat(it: *const GolFeature) -> f64;
    fn gol_tag(it: *mut GolFeature, key: *const c_char) -> *const c_char;
}

/// Ключи тегов, которые нужны пайплайну извлечения.
const TAG_KEYS: &[&str] = &[
    "addr:country",
    "addr:city",
    "addr:town",
    "addr:place",
    "addr:street",
    "addr:housenumber",
    "historic",
    "tourism",
    "name",
    "name:ru",
    "name:en",
    "place",
    "landuse",
    "boundary",
    "admin_level",
    "ISO3166-1",
    "ISO3166-1:alpha2",
    "ISO3166-1:alpha3",
];

const FEATURE_RELATION: i32 = 2;

/// Прочитать нужные теги текущей фичи в `HashMap<String, String>`.
fn read_tags(it: *mut GolFeature, keys: &[CString]) -> HashMap<String, String> {
    let mut tags = HashMap::new();
    for (i, key) in TAG_KEYS.iter().enumerate() {
        let value = unsafe { gol_tag(it, keys[i].as_ptr()) };
        if !value.is_null() {
            let s = unsafe { CStr::from_ptr(value) }.to_string_lossy().into_owned();
            tags.insert((*key).to_string(), s);
        }
    }
    tags
}

/// Пройтись по всем фичам коллекции, вызывая `f(type, lat, lon, tags)`.
fn for_each_feature(
    lib: *const GolFeatures,
    total: u64,
    label: &str,
    mut f: impl FnMut(i32, f64, f64, HashMap<String, String>),
) -> Result<()> {
    let it = unsafe { gol_iterate(lib) };
    if it.is_null() {
        bail!("gol_iterate: не удалось создать итератор");
    }

    let pb = crate::utils::progress_bar(total, label);
    let keys: Vec<CString> = TAG_KEYS
        .iter()
        .map(|k| CString::new(*k).expect("тег без NUL"))
        .collect();

    loop {
        let has_next = unsafe { gol_next(it) };
        if has_next == 0 {
            break;
        }
        let feature_type = unsafe { gol_type(it) };
        let lon = unsafe { gol_lon(it) };
        let lat = unsafe { gol_lat(it) };
        let tags = read_tags(it, &keys);
        f(feature_type, lat, lon, tags);
        pb.inc(1);
    }

    pb.finish_and_clear();
    unsafe { gol_free(it) };
    Ok(())
}

/// Источник гео-объектов из GOL через FFI (прямое чтение, без PBF).
pub struct GolFfiSource {
    corrector: Option<Corrector>,
    country: Option<Country>,
    boundary: Option<CountryBoundary>,
}

impl GolFfiSource {
    pub fn new() -> Self {
        Self {
            corrector: None,
            country: None,
            boundary: None,
        }
    }

    fn parse_opened(&mut self, lib: *const GolFeatures) -> Result<Vec<GeoObject>> {
        // Общее число фич — для точных прогресс-баров (один лишний проход).
        let total = unsafe { gol_count(lib) };

        // Проход 1: собираем типы мест и определяем страну.
        let mut place_types: HashMap<String, String> = HashMap::new();
        let mut addr_country_counts: HashMap<String, u64> = HashMap::new();
        let mut admin_country: Option<Country> = None;
        for_each_feature(lib, total, "Сбор типов мест из GOL", |_, _, _, tags| {
            if let Some(c) = tags.get("addr:country")
                && !c.is_empty() {
                    *addr_country_counts.entry(c.clone()).or_default() += 1;
                }
            if tags.get("boundary").map(|s| s.as_str()) == Some("administrative")
                && tags.get("admin_level").map(|s| s.as_str()) == Some("2")
                && let Some(c) = crate::country::from_tags(&tags) {
                    admin_country = Some(c);
                }
            if let Some(place_name) = tags.get("name").or_else(|| tags.get("name:ru"))
                && let Some(pt) = place_type_label(&tags) {
                    place_types.entry(place_name.clone()).or_insert(pt);
                }
        })?;

        let majority = {
            let mut best: Option<(u64, String)> = None;
            for (val, count) in &addr_country_counts {
                let code = val.trim().to_ascii_uppercase();
                if code.len() == 2 && code.chars().all(|c| c.is_ascii_uppercase())
                    && best.as_ref().map_or(true, |(bc, _)| *count > *bc) {
                        best = Some((*count, code));
                    }
            }
            best.map(|(_, code)| code)
        };
        self.country = majority
            .and_then(|code| crate::country::from_code(&code))
            .or(admin_country);

        // Проход 2: извлекаем адреса и POI.
        let mut objects = Vec::new();
        for_each_feature(lib, total, "Извлечение объектов из GOL", |feature_type, lat, lon, tags| {
            if feature_type == FEATURE_RELATION {
                // В текущем PBF-парсере relations дают только named-объекты.
                if let Some(obj) = extract_named_object(&tags, lat, lon, self.corrector.as_ref()) {
                    objects.push(GeoObject::Named(obj));
                }
            } else {
                let has_address = tags.contains_key("addr:housenumber")
                    || tags.contains_key("addr:street")
                    || tags.contains_key("addr:city");
                let has_name = tags.contains_key("name") || tags.contains_key("name:ru");

                // Точная копия логики PbfParser::process_element.
                if has_address
                    && let Some(addr) =
                        extract_address(&tags, lat, lon, self.corrector.as_ref(), &place_types)
                {
                    objects.push(GeoObject::Address(addr));
                } else if has_name
                    && let Some(obj) =
                        extract_named_object(&tags, lat, lon, self.corrector.as_ref())
                {
                    objects.push(GeoObject::Named(obj));
                }
            }
        })?;

        let addr_count = objects.iter().filter(|o| o.as_address().is_some()).count();
        let named_count = objects.iter().filter(|o| o.as_named().is_some()).count();
        info!(
            "GOL(FFI): {} фич, сырых: адресов={}, POI={}",
            total, addr_count, named_count
        );

        Ok(objects)
    }
}

impl FeatureSource for GolFfiSource {
    fn set_corrector(&mut self, corrector: Option<Corrector>) {
        self.corrector = corrector;
    }

    fn parse(&mut self, path: &Path) -> Result<Vec<GeoObject>> {
        let c_path = CString::new(path.to_string_lossy().as_bytes())
            .context("путь к GOL содержит NUL")?;

        let lib = unsafe { gol_open(c_path.as_ptr()) };
        if lib.is_null() {
            bail!("Не удалось открыть GOL: {}", path.display());
        }

        let result = self.parse_opened(lib);
        unsafe { gol_close(lib) };

        // Граница страны: GOL-FII не даёт геометрию, используем `gol query -f geojson`.
        if self.boundary.is_none()
            && let Some(c) = &self.country
            && !c.code.is_empty()
            && let Ok(tool) = crate::gol::GolTool::find_or_install() {
                let query = format!("*[\"ISO3166-1:alpha2\"={}]", c.code);
                if let Ok(json) = tool.query_geojson(path, &query) {
                    self.boundary = crate::boundary::parse_geojson_boundary(&json);
                }
            }

        info!("Чтение GOL (FFI) завершено");
        result
    }

    fn country(&self) -> Option<Country> {
        self.country.clone()
    }

    fn boundary(&self) -> Option<CountryBoundary> {
        self.boundary.clone()
    }
}
