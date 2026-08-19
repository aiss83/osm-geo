//! Определение страны: из данных файла, по явному флагу или имени файла.

use std::collections::HashMap;

use crate::model::Country;

/// Код alpha-2, русское имя, английское имя для сопоставления с именем файла.
const COUNTRIES: &[(&str, &str, &str)] = &[
    ("RU", "Россия", "russia"),
    ("BY", "Беларусь", "belarus"),
    ("KZ", "Казахстан", "kazakhstan"),
    ("UA", "Украина", "ukraine"),
    ("GE", "Грузия", "georgia"),
    ("AM", "Армения", "armenia"),
    ("AZ", "Азербайджан", "azerbaijan"),
    ("LT", "Литва", "lithuania"),
    ("LV", "Латвия", "latvia"),
    ("EE", "Эстония", "estonia"),
    ("PL", "Польша", "poland"),
    ("FI", "Финляндия", "finland"),
    ("MN", "Монголия", "mongolia"),
    ("CN", "Китай", "china"),
    ("KP", "КНДР", "north korea"),
    ("JP", "Япония", "japan"),
    ("US", "США", "united states"),
];

/// alpha-3 → alpha-2 (для fallback по `ISO3166-1:alpha3`).
const ALPHA3: &[(&str, &str)] = &[
    ("RUS", "RU"),
    ("BLR", "BY"),
    ("KAZ", "KZ"),
    ("UKR", "UA"),
    ("GEO", "GE"),
    ("ARM", "AM"),
    ("AZE", "AZ"),
    ("LTU", "LT"),
    ("LVA", "LV"),
    ("EST", "EE"),
    ("POL", "PL"),
    ("FIN", "FI"),
    ("MNG", "MN"),
    ("CHN", "CN"),
    ("PRK", "KP"),
    ("JPN", "JP"),
    ("USA", "US"),
];

fn code_to_name(code: &str) -> String {
    COUNTRIES
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, name, _)| name.to_string())
        .unwrap_or_default()
}

fn alpha3_to_alpha2(code: &str) -> Option<&'static str> {
    ALPHA3.iter().find(|(a3, _)| *a3 == code).map(|(_, a2)| *a2)
}

fn normalize_code(code: &str) -> String {
    code.trim().to_ascii_uppercase()
}

/// Определить страну по тегам relation `boundary=administrative` + `admin_level=2`.
///
/// Приоритет кода: `ISO3166-1:alpha2` → bare `ISO3166-1` → `ISO3166-1:alpha3`.
/// Имя: `name:ru` → `name:en` → `name` → таблица по коду.
pub fn from_tags(tags: &HashMap<String, String>) -> Option<Country> {
    let name = tags
        .get("name:ru")
        .or_else(|| tags.get("name:en"))
        .or_else(|| tags.get("name"))
        .cloned()
        .unwrap_or_default();

    if let Some(c) = tags.get("ISO3166-1:alpha2") {
        let code = normalize_code(c);
        if code.len() == 2 {
            let name = if name.is_empty() { code_to_name(&code) } else { name };
            return Some(Country { code, name });
        }
    }

    if let Some(c) = tags.get("ISO3166-1") {
        let code = normalize_code(c);
        if code.len() == 2 {
            let name = if name.is_empty() { code_to_name(&code) } else { name };
            return Some(Country { code, name });
        }
        if let Some(a2) = alpha3_to_alpha2(&code) {
            let code = a2.to_string();
            let name = if name.is_empty() { code_to_name(&code) } else { name };
            return Some(Country { code, name });
        }
    }

    if let Some(c) = tags.get("ISO3166-1:alpha3") {
        let code = normalize_code(c);
        if let Some(a2) = alpha3_to_alpha2(&code) {
            let code = a2.to_string();
            let name = if name.is_empty() { code_to_name(&code) } else { name };
            return Some(Country { code, name });
        }
    }

    None
}

/// Определить страну по явному alpha-2 коду (`--country RU`).
pub fn from_code(code: &str) -> Option<Country> {
    let code = normalize_code(code);
    if code.is_empty() {
        return None;
    }
    let name = code_to_name(&code);
    Some(Country { code, name })
}

/// Попытаться определить страну по имени входного файла.
///
/// Из `russia-latest.osm.pbf` / `/path/belarus.gol` извлекается первый токен
/// имени и сопоставляется с английским/русским названием.
pub fn from_filename(file_name: &str) -> Option<Country> {
    let base = file_name.rsplit('/').next().unwrap_or(file_name);
    let stem = base.split('.').next().unwrap_or(base);
    let lower = stem.to_lowercase();

    for &(code, name, en) in COUNTRIES {
        if lower == en || lower.starts_with(en) || lower.contains(en) {
            return Some(Country {
                code: code.to_string(),
                name: name.to_string(),
            });
        }
    }

    for &(code, name, _) in COUNTRIES {
        if lower.contains(&name.to_lowercase()) {
            return Some(Country {
                code: code.to_string(),
                name: name.to_string(),
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_code() {
        let c = from_code("ru").unwrap();
        assert_eq!(c.code, "RU");
        assert_eq!(c.name, "Россия");
    }

    #[test]
    fn test_from_filename() {
        let c = from_filename("russia-latest.osm.pbf").unwrap();
        assert_eq!(c.code, "RU");
        let c = from_filename("/data/belarus.gol").unwrap();
        assert_eq!(c.code, "BY");
    }

    #[test]
    fn test_from_tags() {
        let mut tags = HashMap::new();
        tags.insert("name".to_string(), "Россия".to_string());
        tags.insert("ISO3166-1".to_string(), "RU".to_string());
        tags.insert("ISO3166-1:alpha2".to_string(), "RU".to_string());
        tags.insert("ISO3166-1:alpha3".to_string(), "RUS".to_string());
        let c = from_tags(&tags).unwrap();
        assert_eq!(c.code, "RU");
        assert_eq!(c.name, "Россия");
    }
}
