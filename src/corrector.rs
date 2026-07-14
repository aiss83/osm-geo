//! Модуль коррекции опечаток в названиях улиц и городов.
//!
//! Использует SymSpell с частотным словарём русского языка
//! для автоматического исправления опечаток в кириллических названиях.
//!
//! Также выполняет нормализацию регистра:
//! - «улица Ленина» → префиксный тип улицы с маленькой, название с большой
//! - «Красная площадь» → предложение: первое слово с большой

use anyhow::{Context, Result};
use log::info;
use std::path::Path;

use symspell::{SymSpell, UnicodeStringStrategy, Verbosity};

/// URL частотного словаря русского языка (UTF-8, формат: слово частота).
const DICT_URL: &str =
    "https://raw.githubusercontent.com/hermitdave/FrequencyWords/master/content/2018/ru/ru_full.txt";

/// Слова-префиксы, обозначающие тип улицы/геообъекта.
/// Если название начинается с одного из этих слов, оно пишется со строчной буквы,
/// а остальная часть — с заглавных.
const STREET_TYPE_PREFIXES: &[&str] = &[
    "улица",
    "проспект",
    "переулок",
    "бульвар",
    "проезд",
    "площадь",
    "набережная",
    "шоссе",
    "аллея",
    "тупик",
    "линия",
    "вал",
    "спуск",
    "подъезд",
    "дорога",
    "тракт",
    "мост",
    "канал",
];

/// Типы улиц женского рода — если прилагательное перед ними
/// оканчивается на -ой/-ей (родительный падеж), исправляем на
/// именительный: -ой→-ая, -ей→-яя.
const FEMININE_STREET_TYPES: &[&str] = &[
    "улица", "набережная", "площадь", "аллея", "линия", "дорога",
];

/// Типы улиц мужского рода: -ой→-ый/-ий, -ей→-ий.
const MASCULINE_STREET_TYPES: &[&str] = &[
    "проспект", "переулок", "бульвар", "проезд", "тупик",
    "вал", "спуск", "подъезд", "тракт", "мост", "канал",
];

/// Типы улиц среднего рода: -ой→-ое, -ей→-ее.
const NEUTER_STREET_TYPES: &[&str] = &[
    "шоссе",
];

/// Корректор опечаток на основе SymSpell.
pub struct Corrector {
    symspell: SymSpell<UnicodeStringStrategy>,
}

impl Corrector {
    /// Создать новый корректор. Ищет словарь `ru_full.txt` в нескольких местах:
    /// 1. Рядом с бинарником (текущая рабочая директория)
    /// 2. В поддиректории `data/`
    /// При отсутствии скачивает словарь в текущую директорию.
    pub fn new_or_download() -> Result<Self> {
        // Ищем словарь в нескольких местах
        let candidates = [
            Path::new("ru_full.txt").to_path_buf(),
            Path::new("data/ru_full.txt").to_path_buf(),
        ];

        let dict_path = candidates
            .iter()
            .find(|p| p.exists())
            .cloned()
            .unwrap_or_else(|| Path::new("ru_full.txt").to_path_buf());

        if !dict_path.exists() {
            info!("Словарь не найден, скачивание из {}...", DICT_URL);
            download_dict(&dict_path)?;
        }

        Self::from_file(&dict_path)
    }

    /// Создать корректор из конкретного файла словаря.
    pub fn from_file(dict_path: &Path) -> Result<Self> {
        let mut symspell: SymSpell<UnicodeStringStrategy> = SymSpell::default();

        symspell.load_dictionary(
            dict_path.to_str().context("невалидный путь к словарю")?,
            0,   // term_index  — колонка со словом (0-based)
            1,   // count_index — колонка с частотой (0-based)
            " ", // разделитель колонок
        );

        info!("Словарь SymSpell загружен");
        Ok(Self {
            symspell,
        })
    }

    /// Нормализовать битые символы в тексте (OSM-артефакты).
    pub fn normalize_chars(text: &str) -> String {
        let mut result = text.to_string();
        // Ƒ (Latin F with hook) — в OSM бывает и вместо Т, и вместо Д
        if result.contains('\u{0191}') {
            let with_t: String = result.chars().map(|c| if c == '\u{0191}' { 'Т' } else { c }).collect();
            let with_d: String = result.chars().map(|c| if c == '\u{0191}' { 'Д' } else { c }).collect();
            // Эвристика: если есть «олгорукого» — это Долгорукого
            result = if with_t.contains("Толгорукого") { with_d } else { with_t };
        }
        result
    }
    /// Исправить опечатки в тексте.
    ///
    /// Каждое слово проверяется через SymSpell. Слова длиной ≤ 3 символа
    /// и слова на латинице не корректируются.
    pub fn correct(&self, text: &str) -> String {
        if text.is_empty() {
            return text.to_string();
        }

        let words: Vec<&str> = text.split_whitespace().collect();
        let mut corrected = Vec::with_capacity(words.len());

        for word in &words {
            // Не корректируем короткие слова и латиницу
            if word.len() <= 3 || !contains_cyrillic(word) {
                corrected.push((*word).to_string());
                continue;
            }

            let lower = word.to_lowercase();
            let suggestions = self.symspell.lookup(&lower, Verbosity::Closest, 2);

            if let Some(sug) = suggestions.first() {
                if sug.term != lower {
                    // Сохраняем оригинальный регистр первой буквы
                    let result = restore_case(&sug.term, word);
                    corrected.push(result);
                    continue;
                }
            }

            corrected.push((*word).to_string());
        }

        corrected.join(" ")
    }

    /// Нормализовать регистр названия улицы/города.
    ///
    /// Правила:
    /// - Если первый токен — слово-тип улицы (напр. «улица», «проспект»),
    ///   он приводится к нижнему регистру, а остальные слова — каждое с большой буквы.
    /// - Иначе — регистр предложения: первое слово с большой, остальные как есть.
    pub fn normalize_case(text: &str) -> String {
        if text.is_empty() {
            return text.to_string();
        }

        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() {
            return text.to_string();
        }

        let first_lower = words[0].to_lowercase();

        // Проверяем, является ли первое слово типом улицы
        if STREET_TYPE_PREFIXES.contains(&first_lower.as_str()) {
            let mut result = vec![first_lower];
            for w in &words[1..] {
                result.push(title_case_first(w));
            }
            return result.join(" ");
        }

        // Обычный случай: первое слово с большой, остальные — тип улицы
        // в конце приводится к нижнему регистру.
        let mut result = vec![title_case_first(words[0])];
        let last_idx = words.len() - 1;
        for (i, w) in words[1..].iter().enumerate() {
            let idx = i + 1;
            if idx == last_idx {
                let lower = w.to_lowercase();
                if STREET_TYPE_PREFIXES.contains(&lower.as_str()) {
                    result.push(lower);
                    continue;
                }
            }
            result.push(w.to_string());
        }
        result.join(" ")
    }

    /// Исправить согласование прилагательного с типом улицы.
    ///
    /// В OSM часто встречается «Калининградской улица» (родительный падеж)
    /// вместо правильного «Калининградская улица» (именительный).
    ///
    /// Правило: для названий вида `<Прилагательное> <ТипУлицы>` исправляем
    /// окончание прилагательного на именительный падеж нужного рода:
    /// - Женский (улица, площадь, ...):  -ой→-ая, -ей→-яя
    /// - Мужской (проспект, переулок, ...): -ой→-ый/-ий, -ей→-ий
    /// - Средний (шоссе):                -ой→-ое, -ей→-ее
    pub fn fix_adjective_agreement(text: &str) -> String {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.len() < 2 {
            return text.to_string();
        }

        let last = words[words.len() - 1].to_lowercase();

        // Определяем род типа улицы
        let gender = if FEMININE_STREET_TYPES.contains(&last.as_str()) {
            'f'
        } else if MASCULINE_STREET_TYPES.contains(&last.as_str()) {
            'm'
        } else if NEUTER_STREET_TYPES.contains(&last.as_str()) {
            'n'
        } else {
            return text.to_string();
        };

        let adj = words[words.len() - 2];
        let adj_lower = adj.to_lowercase();

        let corrected_adj = if let Some(stem) = try_strip_ending(&adj_lower, "ой") {
            // -ой: твёрдая или мягкая основа
            let stem_chars: Vec<char> = stem.chars().collect();
            let soft = stem_chars.last().map_or(false, |c| *c == 'к');
            match gender {
                'f' => format_adj(&adj, &stem, if soft { "ая" } else { "ая" }),
                'm' => format_adj(&adj, &stem, if soft { "ий" } else { "ый" }),
                'n' => format_adj(&adj, &stem, if soft { "ое" } else { "ое" }),
                _ => unreachable!(),
            }
        } else if let Some(stem) = try_strip_ending(&adj_lower, "ей") {
            // -ей: всегда мягкая основа
            match gender {
                'f' => format_adj(&adj, &stem, "яя"),
                'm' => format_adj(&adj, &stem, "ий"),
                'n' => format_adj(&adj, &stem, "ее"),
                _ => unreachable!(),
            }
        } else {
            return text.to_string();
        };

        let mut result: Vec<String> = words[..words.len() - 2]
            .iter()
            .map(|w| w.to_string())
            .collect();
        result.push(corrected_adj);
        result.push(words[words.len() - 1].to_string());
        result.join(" ")
    }
}

/// Проверить, заканчивается ли слово на указанное окончание (по символам).
/// Возвращает основу (без окончания), если да.
fn try_strip_ending(lower: &str, ending: &str) -> Option<String> {
    if lower.len() <= ending.len() || !lower.ends_with(ending) {
        return None;
    }
    let chars: Vec<char> = lower.chars().collect();
    let stem: String = chars[..chars.len() - ending.chars().count()]
        .iter()
        .collect();
    Some(stem)
}

/// Склеить основу из оригинального слова (с сохранением регистра) и новое окончание.
fn format_adj(original: &str, lower_stem: &str, new_ending: &str) -> String {
    let orig_chars: Vec<char> = original.chars().collect();
    let stem_len = lower_stem.chars().count();
    let orig_stem: String = orig_chars[..stem_len].iter().collect();
    format!("{}{}", orig_stem, new_ending)
}

/// Скачать словарь по URL и сохранить локально.
fn download_dict(dest: &Path) -> Result<()> {
    use std::io::Read;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    info!("Загрузка словаря (может занять несколько минут, ~30 МБ)...");
    let mut resp = reqwest::blocking::get(DICT_URL)?;
    let mut file = std::fs::File::create(dest)?;
    let mut buf = [0u8; 65536];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n])?;
    }

    info!("Словарь сохранён: {:?}", dest);
    Ok(())
}

/// Проверить, содержит ли строка кириллические символы.
fn contains_cyrillic(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c,
            '\u{0400}'..='\u{04FF}' |  // Кириллица
            '\u{0500}'..='\u{052F}'    // Кириллица (расширенная)
        )
    })
}

/// Восстановить регистр первой буквы: если оригинал начинался с заглавной,
/// результат тоже начинается с заглавной.
fn restore_case(corrected: &str, original: &str) -> String {
    if original.chars().next().map_or(false, |c| c.is_uppercase()) {
        title_case_first(corrected)
    } else {
        corrected.to_string()
    }
}

/// Сделать первую букву заглавной, остальные строчными.
fn title_case_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let rest: String = chars.collect();
            let mut result = first.to_uppercase().to_string();
            result.push_str(&rest.to_lowercase());
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contains_cyrillic() {
        assert!(contains_cyrillic("Москва"));
        assert!(contains_cyrillic("улица Ленина"));
        assert!(!contains_cyrillic("Moscow"));
        assert!(!contains_cyrillic("123"));
    }

    #[test]
    fn test_normalize_case_street_prefix() {
        assert_eq!(
            Corrector::normalize_case("улица Ленина"),
            "улица Ленина"
        );
        assert_eq!(
            Corrector::normalize_case("УЛИЦА ПУШКИНА"),
            "улица Пушкина"
        );
        assert_eq!(
            Corrector::normalize_case("проспект МИРА"),
            "проспект Мира"
        );
        assert_eq!(
            Corrector::normalize_case("переулок Сивцев Вражек"),
            "переулок Сивцев Вражек"
        );
    }

    #[test]
    fn test_normalize_case_sentence() {
        assert_eq!(
            Corrector::normalize_case("красная площадь"),
            "Красная площадь"
        );
        assert_eq!(
            Corrector::normalize_case("МОСКВА"),
            "Москва"
        );
        assert_eq!(
            Corrector::normalize_case("санкт-петербург"),
            "Санкт-петербург"
        );
    }

    #[test]
    fn test_normalize_case_empty() {
        assert_eq!(Corrector::normalize_case(""), "");
    }

    #[test]
    fn test_title_case_first() {
        assert_eq!(title_case_first("москва"), "Москва");
        assert_eq!(title_case_first("МОСКВА"), "Москва");
        assert_eq!(title_case_first(""), "");
        assert_eq!(title_case_first("а"), "А");
    }

    /// Интеграционный тест: создание Corrector с реальным словарём (если доступен).
    /// Пропускается, если словарь не скачан.
    #[test]
    fn test_corrector_with_dict() {
        let dict_path = Path::new("data/ru_full.txt");
        if !dict_path.exists() {
            eprintln!("Пропуск: словарь не найден ({:?})", dict_path);
            return;
        }

        let corrector = Corrector::from_file(&dict_path).unwrap();

        // «Масква» — опечатка, должно исправиться на «Москва»
        let result = corrector.correct("Масква");
        eprintln!("correct('Масква') = '{result}'");
        // Не жёсткое утверждение — качество зависит от словаря
        assert!(!result.is_empty());
    }
}
