//! Модель данных: Address (иерархический адрес) и NamedObject (POI/объект).
//!
//! Данные сохраняются в компактном бинарном формате (см. `DATABASE.md`).
//! В компактном формате Address не содержит `country` и `postcode`.

/// Страна, к которой относится весь файл `.bin`.
///
/// `code` — ISO 3166-1 alpha-2 (`RU`), `name` — человекочитаемое название
/// (`Россия`). `name` может быть пустым, если не удалось определить текст.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Country {
    pub code: String,
    pub name: String,
}

/// Граница страны: bounding box + набор полигонов.
///
/// `polygons` — список полигонов; каждый полигон — список колец; каждое
/// кольцо — список точек `(lat, lon)` в `f32`. Первое кольцо полигона —
/// внешнее, остальные — внутренние «дырки».
#[derive(Debug, Clone, Default)]
pub struct CountryBoundary {
    pub min_lat: f32,
    pub min_lon: f32,
    pub max_lat: f32,
    pub max_lon: f32,
    pub polygons: Vec<Vec<Vec<(f32, f32)>>>,
}

/// Иерархический адрес, привязанный к дому/зданию/участку.
#[derive(Debug, Clone)]
pub struct Address {
    pub country: Option<String>,
    pub city: Option<String>,
    pub street: Option<String>,
    pub housenumber: Option<String>,
    pub lat: f64,
    pub lon: f64,
}

/// Именованный объект (POI) с координатной привязкой к городу и стране.
#[derive(Debug, Clone)]
pub struct NamedObject {
    pub name: String,
    pub category: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub lat: f64,
    pub lon: f64,
}

/// Унифицированный объект: либо Address, либо NamedObject.
#[derive(Debug, Clone)]
pub enum GeoObject {
    Address(Address),
    Named(NamedObject),
}

impl GeoObject {
    pub fn lat_lon(&self) -> (f64, f64) {
        match self {
            GeoObject::Address(a) => (a.lat, a.lon),
            GeoObject::Named(n) => (n.lat, n.lon),
        }
    }

    pub fn as_address(&self) -> Option<&Address> {
        match self {
            GeoObject::Address(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_named(&self) -> Option<&NamedObject> {
        match self {
            GeoObject::Named(n) => Some(n),
            _ => None,
        }
    }
}
