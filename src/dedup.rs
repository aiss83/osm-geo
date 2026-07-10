//! Дедупликация объектов перед индексацией.
//!
//! Удаляет дубликаты адресов (одинаковый city+street+housenumber)
//! и POI (одинаковое name + близкие координаты).

use std::collections::HashSet;

use indicatif::{ProgressBar, ProgressStyle};
use log::info;

use crate::model::GeoObject;

pub fn deduplicate(objects: Vec<GeoObject>) -> Vec<GeoObject> {
    let original = objects.len();
    let mut seen_addresses: HashSet<AddressKey> = HashSet::new();
    let mut seen_named: Vec<(String, f64, f64)> = Vec::new();
    let mut result = Vec::with_capacity(original);
    let total = original as u64;

    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] Дедупликация: {pos}/{len}",
            )
            .unwrap()
            .progress_chars("##-"),
    );

    for obj in objects {
        pb.inc(1);

        match obj {
            GeoObject::Address(ref addr) => {
                let key = AddressKey::from_address(addr);
                if seen_addresses.insert(key) {
                    result.push(obj);
                }
            }
            GeoObject::Named(ref named) => {
                let is_dup = seen_named.iter().any(|(n, lat, lon)| {
                    n == &named.name
                        && haversine_approx(named.lat, named.lon, *lat, *lon) < 100.0
                });

                if !is_dup {
                    seen_named.push((named.name.clone(), named.lat, named.lon));
                    result.push(obj);
                }
            }
        }
    }

    pb.finish_and_clear();

    let removed = original - result.len();
    if removed > 0 {
        info!(
            "Дедупликация: удалено {} дубликатов, осталось {}",
            removed, result.len()
        );
    }

    result
}

#[derive(Hash, Eq, PartialEq)]
struct AddressKey {
    country: Option<String>,
    city: Option<String>,
    street: Option<String>,
    housenumber: Option<String>,
}

impl AddressKey {
    fn from_address(addr: &crate::model::Address) -> Self {
        Self {
            country: normalize_key(addr.country.as_deref()),
            city: normalize_key(addr.city.as_deref()),
            street: normalize_key(addr.street.as_deref()),
            housenumber: normalize_key(addr.housenumber.as_deref()),
        }
    }
}

fn normalize_key(s: Option<&str>) -> Option<String> {
    s.map(|s| s.trim().to_lowercase().replace("  ", " "))
}

fn haversine_approx(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6_371_000.0;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    r * c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Address, NamedObject};

    #[test]
    fn test_dedup_addresses() {
        let addr1 = GeoObject::Address(Address {
            country: Some("Россия".into()),
            city: Some("Москва".into()),
            street: Some("Тверская".into()),
            housenumber: Some("1".into()),
            postcode: None,
            lat: 55.0,
            lon: 37.0,
        });
        let addr2 = GeoObject::Address(Address {
            country: Some("Россия".into()),
            city: Some("Москва".into()),
            street: Some("Тверская".into()),
            housenumber: Some("1".into()),
            postcode: Some("125009".into()),
            lat: 55.1,
            lon: 37.1,
        });
        let addr3 = GeoObject::Address(Address {
            country: Some("Россия".into()),
            city: Some("Москва".into()),
            street: Some("Тверская".into()),
            housenumber: Some("3".into()),
            postcode: None,
            lat: 55.0,
            lon: 37.0,
        });

        let result = deduplicate(vec![addr1, addr2, addr3]);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_dedup_named_same_location() {
        let obj1 = GeoObject::Named(NamedObject {
            name: "Кафе".into(),
            translit: None,
            category: Some("amenity".into()),
            lat: 55.0,
            lon: 37.0,
        });
        let obj2 = GeoObject::Named(NamedObject {
            name: "Кафе".into(),
            translit: None,
            category: Some("amenity".into()),
            lat: 55.0003,
            lon: 37.0003,
        });
        let obj3 = GeoObject::Named(NamedObject {
            name: "Ресторан".into(),
            translit: None,
            category: Some("amenity".into()),
            lat: 55.0,
            lon: 37.0,
        });

        let result = deduplicate(vec![obj1, obj2, obj3]);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_haversine() {
        let d = haversine_approx(55.7539, 37.6208, 55.7520, 37.6175);
        assert!(d > 100.0 && d < 1000.0);
    }
}
