//! Модель данных: Address (иерархический адрес) и NamedObject (POI/объект).
//!
//! Соответствует разделу 2.2.2 ТЗ.
//!
//! BLOB не используется — все поля хранятся в плоских колонках SQLite.
//! NamedObject содержит только русское имя (name:ru/name) и транслитерацию.

/// Тип сущности в базе.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectType {
    Address = 0,
    NamedObject = 1,
}

/// Иерархический адрес, привязанный к дому/зданию/участку.
#[derive(Debug, Clone)]
pub struct Address {
    pub country: Option<String>,
    pub city: Option<String>,
    pub street: Option<String>,
    pub housenumber: Option<String>,
    pub postcode: Option<String>,
    pub lat: f64,
    pub lon: f64,
}

/// Именованный объект без жёсткой адресной привязки.
/// Только русское название + транслитерация.
#[derive(Debug, Clone)]
pub struct NamedObject {
    pub name: String,
    pub translit: Option<String>,
    pub category: Option<String>,
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
    pub fn object_type(&self) -> ObjectType {
        match self {
            GeoObject::Address(_) => ObjectType::Address,
            GeoObject::Named(_) => ObjectType::NamedObject,
        }
    }

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
