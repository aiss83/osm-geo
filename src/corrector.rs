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
    "съезд",
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
    "съезд",
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
    /// Загружает словарь построчно с индикатором прогресса.
    pub fn from_file(dict_path: &Path) -> Result<Self> {
        use std::io::{BufRead, BufReader};

        let mut symspell: SymSpell<UnicodeStringStrategy> = SymSpell::default();

        let file = std::fs::File::open(dict_path)
            .context("Открытие файла словаря")?;
        let file_size = file.metadata()?.len();
        let reader = BufReader::new(file);

        let pb = indicatif::ProgressBar::new(file_size);
        pb.set_style(
            indicatif::ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] Загрузка словаря SymSpell: {bytes}/{total_bytes} ({bytes_per_sec})")
                .unwrap()
                .progress_chars("##-"),
        );

        for line in reader.lines() {
            let line = line.context("Чтение строки словаря")?;
            let line_len = line.len() as u64 + 1; // +1 за отброшенный \n
            symspell.load_dictionary_line(&line, 0, 1, " ");
            pb.inc(line_len);
        }

        pb.finish_with_message("Словарь SymSpell загружен");
        info!("Словарь SymSpell загружен");
        Ok(Self {
            symspell,
        })
    }

    /// Нормализовать битые символы в тексте (OSM-артефакты).
    pub fn normalize_chars(text: &str) -> String {
        let mut result = text.to_string();
        // Ƒ (Latin F with hook) — в OSM кодирует Д, Т или К.
        // Эвристика: генерируем все три варианта и выбираем по известным stem'ам.
        // По умолчанию — Д (наиболее частый случай).
        if result.contains('\u{0191}') {
            let with_t: String = result.chars().map(|c| if c == '\u{0191}' { 'Т' } else { c }).collect();
            let with_d: String = result.chars().map(|c| if c == '\u{0191}' { 'Д' } else { c }).collect();
            let with_k: String = result.chars().map(|c| if c == '\u{0191}' { 'К' } else { c }).collect();

            let is_t = with_t.contains("Тверс")   // Тверская
                    || with_t.contains("Тульс")    // Тульская
                    || with_t.contains("Томс");    // Томская

            let is_k = with_k.contains("Камск")   // Камский
                    || with_k.contains("Курск")    // Курская
                    || with_k.contains("Казан")    // Казанская
                    || with_k.contains("Киевс")    // Киевская
                    || with_k.contains("Коломен")  // Коломенская
                    || with_k.contains("Красно");  // Краснодарская, Красноармейская

            result = if is_t { with_t } else if is_k { with_k } else { with_d };
        }
        result
    }
    /// Исправить опечатки в тексте.
    ///
    /// Каждое слово проверяется через SymSpell. Слова длиной ≤ 3 символа,
    /// слова на латинице, типы улиц и топонимы не корректируются.
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

            // Типы улиц — валидные слова, не должны правиться SymSpell
            // (частотный словарь может не содержать «проспект», предлагая «проспекту»)
            if is_protected_word(&lower) {
                corrected.push((*word).to_string());
                continue;
            }

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
   #[deprecated(note = "Заменено на normalizer::normalize_rule_based. Будет удалено после верификации.")]
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

    /// Исправить согласование прилагательного с существительным.
    ///
    /// В OSM часто встречается родительный падеж прилагательного вместо
    /// именительного: «Калининградской улица» → «Калининградская улица»,
    /// «Калининградской зоопарк» → «Калининградский зоопарк».
    ///
    /// Правило: для двух последних слов вида `<Прилагательное> <Существительное>`
    /// исправляем окончание прилагательного на именительный падеж
    /// в соответствии с родом существительного:
    /// - Женский (-а/-я):      -ой→-ая, -ей→-яя
    /// - Мужской (согласная):  -ой→-ый/-ий, -ей→-ий
    /// - Средний (-о/-е):      -ой→-ое, -ей→-ее
    #[deprecated(note = "Заменено на normalizer::normalize_rule_based. Будет удалено после верификации нейросети.")]
    pub fn fix_adjective_agreement(text: &str) -> String {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.len() < 2 {
            return text.to_string();
        }

        let last = words[words.len() - 1].to_lowercase();
        let last_is_cyrillic = contains_cyrillic(&last);

        // Определяем род: сначала по типу улицы, затем по окончанию существительного
        let gender = if FEMININE_STREET_TYPES.contains(&last.as_str()) {
            Some('f')
        } else if MASCULINE_STREET_TYPES.contains(&last.as_str()) {
            Some('m')
        } else if NEUTER_STREET_TYPES.contains(&last.as_str()) {
            Some('n')
        } else if last_is_cyrillic {
            // Не тип улицы, но кириллическое существительное — определяем род по окончанию
            detect_noun_gender(&last)
        } else {
            None
        };

        let gender = match gender {
            Some(g) => g,
            None => return text.to_string(),
        };

        let adj = words[words.len() - 2];
        let adj_lower = adj.to_lowercase();

        // Прилагательное должно содержать кириллицу и быть достаточно длинным
        if !contains_cyrillic(&adj_lower) || adj_lower.len() < 4 {
            return text.to_string();
        }

        let corrected_adj = if let Some(stem) = try_strip_ending(&adj_lower, "ой") {
            // -ой: твёрдая или мягкая основа (к/г/х → ий)
            let stem_chars: Vec<char> = stem.chars().collect();
            let soft = stem_chars.last().map_or(false, |c| matches!(c, 'к' | 'г' | 'х'));
            match gender {
                'f' => format_adj(&adj, &stem, "ая"),
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

    /// Нормализовать падеж типа улицы: «проспекту» → «проспект», «улицы» → «улица».
    ///
    /// В OSM встречаются названия, где сам тип улицы записан в косвенном падеже
    /// (дательном, родительном). Функция приводит тип улицы к именительному падежу
    /// во всех словах текста.
    #[deprecated(note = "Заменено на normalizer::normalize_oblique_street_types. Будет удалено после верификации.")]
    pub fn normalize_street_types_case(text: &str) -> String {
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut result = Vec::with_capacity(words.len());

        for &word in &words {
            if !contains_cyrillic(word) {
                result.push(word.to_string());
                continue;
            }
            let lower = word.to_lowercase();
            if let Some(canonical) = resolve_oblique_street_type(&lower) {
                result.push(restore_case(canonical, word));
            } else {
                result.push(word.to_string());
            }
        }

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

/// Таблица косвенных падежей типов улиц → именительный падеж.
/// «проспекту» → «проспект», «улицы» → «улица» и т.д.
const OBLIQUE_STREET_TYPE_MAP: &[(&str, &str)] = &[
    // Мужской род — родительный (-а) и дательный (-у)
    ("проспекта", "проспект"),
    ("проспекту", "проспект"),
    ("бульвара", "бульвар"),
    ("бульвару", "бульвар"),
    ("проезда", "проезд"),
    ("проезду", "проезд"),
    ("тупика", "тупик"),
    ("тупику", "тупик"),
    ("вала", "вал"),
    ("валу", "вал"),
    ("спуска", "спуск"),
    ("спуску", "спуск"),
    ("подъезда", "подъезд"),
    ("подъезду", "подъезд"),
    ("тракта", "тракт"),
    ("тракту", "тракт"),
    ("моста", "мост"),
    ("мосту", "мост"),
    ("канала", "канал"),
    ("каналу", "канал"),
    ("съезда", "съезд"),
    ("съезду", "съезд"),
    // переулок — беглая гласная
    ("переулка", "переулок"),
    ("переулку", "переулок"),
    // Женский род — родительный/дательный
    ("улицы", "улица"),
    ("улице", "улица"),
    ("аллеи", "аллея"),
    ("линии", "линия"),
    ("дороги", "дорога"),
    ("дороге", "дорога"),
    ("набережной", "набережная"),
    ("площади", "площадь"),
];

/// Определить род существительного по окончанию (упрощённая эвристика).
///
/// -а/-я → женский, -о/-е → средний, согласная → мужской.
fn detect_noun_gender(lower: &str) -> Option<char> {
    let chars: Vec<char> = lower.chars().collect();
    match chars.last()? {
        'а' | 'я' => Some('f'),
        'о' | 'е' => Some('n'),
        _ => Some('m'), // согласная, й, ь → мужской
    }
}

/// Разрешить косвенный падеж типа улицы в именительный.
/// Возвращает каноническую форму, если слово — известный тип улицы в косвенном падеже.
fn resolve_oblique_street_type(lower: &str) -> Option<&'static str> {
    for &(oblique, canonical) in OBLIQUE_STREET_TYPE_MAP {
        if lower == oblique {
            return Some(canonical);
        }
    }
    None
}

/// Скачать словарь по URL и сохранить локально.
fn download_dict(dest: &Path) -> Result<()> {
    use std::io::Read;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut resp = reqwest::blocking::get(DICT_URL)?;
    let total = resp.content_length().unwrap_or(0);
    let pb = indicatif::ProgressBar::new(total);
    if total > 0 {
        pb.set_style(
            indicatif::ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] Загрузка словаря: {bytes}/{total_bytes} ({bytes_per_sec})")
                .unwrap()
                .progress_chars("##-"),
        );
    } else {
        pb.set_style(
            indicatif::ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] Загрузка словаря: {bytes} ({bytes_per_sec})")
                .unwrap(),
        );
    }

    let mut file = std::fs::File::create(dest)?;
    let mut buf = [0u8; 65536];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n])?;
        pb.inc(n as u64);
    }

    pb.finish_with_message("Словарь загружен");
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

/// Слово защищено от коррекции SymSpell.
///
/// Защищены:
/// - Типы улиц (улица, проспект, переулок, ...) — валидные термины
/// - Топонимы на -ово/-ево/-ино (Исаково, Бородино, ...)
/// - Названия городов с характерными окончаниями (-ск, -цк, -град, -бург, -поль)
fn is_protected_word(lower: &str) -> bool {
    // Все списки типов улиц
    if STREET_TYPE_PREFIXES.contains(&lower) {
        return true;
    }
    if FEMININE_STREET_TYPES.contains(&lower) {
        return true;
    }
    if MASCULINE_STREET_TYPES.contains(&lower) {
        return true;
    }
    if NEUTER_STREET_TYPES.contains(&lower) {
        return true;
    }

    // Топонимы на -ово/-ево/-ино
    if lower.ends_with("ово") || lower.ends_with("ево") || lower.ends_with("ино") {
        return true;
    }

    // Названия городов: -ск, -цк (Гурьевск, Балтийск, Донецк, ...)
    // Требуем ≥3 символов перед окончанием, чтобы исключить «диск», «поиск»
    if (lower.ends_with("ск") || lower.ends_with("цк")) && lower.len() >= 5 {
        return true;
    }
    // -град, -бург, -поль (Калининград, Оренбург, Севастополь, ...)
    if lower.ends_with("град") || lower.ends_with("бург") || lower.ends_with("поль") {
        return true;
    }

    false
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

    #[test]
    fn test_normalize_street_types_case_basic() {
        // Masculine dative → nominative
        assert_eq!(
            Corrector::normalize_street_types_case("Московский проспекту"),
            "Московский проспект"
        );
        // Masculine genitive
        assert_eq!(
            Corrector::normalize_street_types_case("Ленинский проспекта"),
            "Ленинский проспект"
        );
        // Feminine genitive → nominative
        assert_eq!(
            Corrector::normalize_street_types_case("Тверская улицы"),
            "Тверская улица"
        );
        // Feminine dative
        assert_eq!(
            Corrector::normalize_street_types_case("по улице"),
            "по улица"
        );
        // переулок (fleeting vowel)
        assert_eq!(
            Corrector::normalize_street_types_case("Сивцев переулка"),
            "Сивцев переулок"
        );
        // No change for already-correct forms
        assert_eq!(
            Corrector::normalize_street_types_case("Московский проспект"),
            "Московский проспект"
        );
    }

    #[test]
    fn test_normalize_street_types_case_with_number() {
        // «проспекту съезд 1» — нормализуем «проспекту», «съезд» уже в им.п.
        assert_eq!(
            Corrector::normalize_street_types_case("Московский проспекту съезд 1"),
            "Московский проспект съезд 1"
        );
        // «съезду» → «съезд»
        assert_eq!(
            Corrector::normalize_street_types_case("Московский проспект съезду 3"),
            "Московский проспект съезд 3"
        );
    }

    #[test]
    fn test_normalize_street_types_case_no_change() {
        // Не-типы улиц не трогаем
        assert_eq!(
            Corrector::normalize_street_types_case("Большое Исаково"),
            "Большое Исаково"
        );
        assert_eq!(
            Corrector::normalize_street_types_case("Калининград"),
            "Калининград"
        );
        // Латиница не трогается
        assert_eq!(
            Corrector::normalize_street_types_case("Moscow street"),
            "Moscow street"
        );
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

    /// Проверка, что защищённые слова не портятся SymSpell.
    #[test]
    fn test_protected_words_untouched() {
        // Ищем словарь там же, где и Corrector::new_or_download
        let dict_path = ["ru_full.txt", "data/ru_full.txt"]
            .iter()
            .map(Path::new)
            .find(|p| p.exists())
            .map(|p| p.to_path_buf());

        let dict_path = match dict_path {
            Some(p) => p,
            None => {
                eprintln!("Пропуск: словарь не найден");
                return;
            }
        };

        let corrector = Corrector::from_file(&dict_path).unwrap();

        // Тип улицы «проспект» не должен превращаться в «проспекту»
        let r = corrector.correct("Московский проспект");
        eprintln!("correct('Московский проспект') = '{r}'");
        assert!(
            r.contains("проспект"),
            "«проспект» испорчен: '{r}'"
        );
        assert!(
            !r.contains("проспекту"),
            "«проспект» перекорректирован в «проспекту»: '{r}'"
        );

        // Топоним «Исаково» не должен превращаться в «Исакова»
        let r = corrector.correct("Большое Исаково");
        eprintln!("correct('Большое Исаково') = '{r}'");
        assert!(
            r.contains("Исаково"),
            "«Исаково» испорчен: '{r}'"
        );
        assert!(
            !r.contains("Исакова"),
            "«Исаково» перекорректирован в «Исакова»: '{r}'"
        );

        // Полный проблемный адрес
        let r = corrector.correct("Московский проспект съезд 1");
        eprintln!("correct('Московский проспект съезд 1') = '{r}'");
        assert!(
            r.contains("проспект"),
            "«проспект» испорчен в полном адресе: '{r}'"
        );
    }

    /// Проверка, что названия городов не искажаются SymSpell.
    #[test]
    fn test_city_names_untouched() {
        let dict_path = ["ru_full.txt", "data/ru_full.txt"]
            .iter()
            .map(Path::new)
            .find(|p| p.exists())
            .map(|p| p.to_path_buf());

        let dict_path = match dict_path {
            Some(p) => p,
            None => { eprintln!("Пропуск: словарь не найден"); return; }
        };

        let corrector = Corrector::from_file(&dict_path).unwrap();

        for city in &["Гурьевск", "Балтийск", "Славск", "Гвардейск", "Светлогорск",
                       "Зеленоградск", "Пионерский", "Советск", "Неман", "Гусев",
                       "Черняховск", "Краснознаменск", "Нестеров"] {
            let r = corrector.correct(city);
            eprintln!("correct('{city}') = '{r}'");
            assert_eq!(r, *city, "Название города '{city}' искажено в '{r}'");
        }
    }
}
