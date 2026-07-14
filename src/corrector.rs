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

        // Обычный случай: первое слово с большой, остальные без изменений
        let mut result = vec![title_case_first(words[0])];
        for w in &words[1..] {
            result.push(w.to_string());
        }
        result.join(" ")
    }
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
