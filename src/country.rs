//! Определение страны: из данных файла, по явному флагу или имени файла.

use std::collections::HashMap;

use crate::model::Country;

/// alpha-2, русское имя, английское имя (для сопоставления с именем файла),
/// числовой код ISO 3166-1 numeric.
const COUNTRIES: &[(&str, &str, &str, &str)] = &[
    ("RU", "Россия", "russia", "643"),
    ("BY", "Беларусь", "belarus", "112"),
    ("KZ", "Казахстан", "kazakhstan", "398"),
    ("UA", "Украина", "ukraine", "804"),
    ("GE", "Грузия", "georgia", "268"),
    ("AM", "Армения", "armenia", "051"),
    ("AZ", "Азербайджан", "azerbaijan", "031"),
    ("LT", "Литва", "lithuania", "440"),
    ("LV", "Латвия", "latvia", "428"),
    ("EE", "Эстония", "estonia", "233"),
    ("PL", "Польша", "poland", "616"),
    ("FI", "Финляндия", "finland", "246"),
    ("MN", "Монголия", "mongolia", "496"),
    ("CN", "Китай", "china", "156"),
    ("KP", "КНДР", "north korea", "408"),
    ("JP", "Япония", "japan", "392"),
    ("US", "США", "united states", "840"),
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
        .find(|(c, _, _, _)| *c == code)
        .map(|(_, name, _, _)| name.to_string())
        .unwrap_or_default()
}

/// Английское имя страны по alpha-2 коду (для имён файлов).
fn english_name(code: &str) -> Option<&'static str> {
    COUNTRIES
        .iter()
        .find(|(c, _, _, _)| *c == code)
        .map(|(_, _, en, _)| *en)
}

/// Числовой код ISO 3166-1 numeric по alpha-2 коду (строка из трёх цифр).
fn numeric_code(code: &str) -> Option<&'static str> {
    COUNTRIES
        .iter()
        .find(|(c, _, _, _)| *c == code)
        .map(|(_, _, _, num)| *num)
}

/// Привести имя страны к безопасному виду для имени файла:
/// строчные ASCII-буквы, разделители → дефис.
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        let alnum = ch.is_ascii_alphanumeric();
        if alnum {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
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

    for &(code, name, en, _) in COUNTRIES {
        if lower == en || lower.starts_with(en) || lower.contains(en) {
            return Some(Country {
                code: code.to_string(),
                name: name.to_string(),
            });
        }
    }

    for &(code, name, _, _) in COUNTRIES {
        if lower.contains(&name.to_lowercase()) {
            return Some(Country {
                code: code.to_string(),
                name: name.to_string(),
            });
        }
    }

    None
}

/// Сформировать имя выходного файла компактного формата:
/// `{страна}-{код страны}-{числовой код страны}.osmg`.
///
/// Страна берётся английским именем (строчными, через дефис), код страны —
/// alpha-2 в нижнем регистре, числовой код — ISO 3166-1 numeric (три цифры).
/// Возвращает `None`, если код страны неизвестен.
pub fn output_filename(country: &Country) -> Option<String> {
    if country.code.is_empty() {
        return None;
    }

    let name = english_name(&country.code)
        .map(slugify)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| country.code.to_ascii_lowercase());
    let code = country.code.to_ascii_lowercase();
    let numeric = numeric_code(&country.code).unwrap_or("000");

    Some(format!("{}-{}-{}.osmg", name, code, numeric))
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

    #[test]
    fn test_output_filename() {
        let c = from_code("RU").unwrap();
        assert_eq!(output_filename(&c).unwrap(), "russia-ru-643.osmg");

        let us = from_code("US").unwrap();
        assert_eq!(output_filename(&us).unwrap(), "united-states-us-840.osmg");

        let am = from_code("AM").unwrap();
        assert_eq!(output_filename(&am).unwrap(), "armenia-am-051.osmg");
    }

    #[test]
    fn test_output_filename_unknown_code() {
        assert!(output_filename(&Country::default()).is_none());

        let c = Country { code: "XX".into(), name: String::new() };
        assert_eq!(output_filename(&c).unwrap(), "xx-xx-000.osmg");
    }
}
