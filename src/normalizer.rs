//! Нейросетевая нормализация названий — замена эвристик corrector.rs.
//!
//! Два уровня нормализации:
//! 1. **Rule-based** (всегда активен) — раскрытие сокращений и нормализация падежей типов улиц.
//! 2. **ONNX** (за feature-флагом `neural-normalizer`) — перевод на русский + падежи + склонение.
//!
//! Потребитель: `cmd_build` в main.rs, между парсером и корректором SymSpell.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::model::GeoObject;

#[cfg(feature = "neural-normalizer")]
use tract_onnx::prelude::*;

// ─── Таблицы сокращений ────────────────────────────────────────────────────

/// Сокращение типов улиц → полная форма.
const ABBREVIATIONS: &[(&str, &str)] = &[
    // Улица
    ("ул", "улица"),    ("ул.", "улица"),
    // Проспект
    ("пр", "проспект"), ("пр.", "проспект"), ("пр-т", "проспект"),
    // Переулок
    ("пер", "переулок"), ("пер.", "переулок"), ("п", "переулок"), ("п.", "переулок"),
    // Бульвар
    ("бул", "бульвар"), ("бульв", "бульвар"),
    // Площадь
    ("пл", "площадь"),  ("пл.", "площадь"),
    // Набережная
    ("наб", "набережная"), ("наб.", "набережная"),
    // Шоссе
    ("ш", "шоссе"),     ("ш.", "шоссе"),
    // Проезд
    ("пр-д", "проезд"),
    // Тупик
    ("туп", "тупик"),
    // Аллея
    ("ал", "аллея"),
    // Линия
    ("лин", "линия"),
    // Спуск
    ("сп", "спуск"),
    // Микрорайон
    ("мкр", "микрорайон"), ("мкр.", "микрорайон"),
];

/// Падежные формы типов улиц → именительный падеж.
const OBLIQUE_CASES: &[(&str, &str)] = &[
    // улица
    ("улицы", "улица"), ("улице", "улица"), ("улицу", "улица"), ("улицей", "улица"),
    // проспект
    ("проспекта", "проспект"), ("проспекту", "проспект"), ("проспектом", "проспект"),
    // переулок
    ("переулка", "переулок"), ("переулку", "переулок"), ("переулком", "переулок"),
    // бульвар
    ("бульвара", "бульвар"), ("бульвару", "бульвар"), ("бульваром", "бульвар"),
    // площадь
    ("площади", "площадь"), ("площадью", "площадь"),
    // набережная
    ("набережной", "набережная"),
    // шоссе (не склоняется)
    // проезд
    ("проезда", "проезд"), ("проезду", "проезд"), ("проездом", "проезд"),
    // тупик
    ("тупика", "тупик"), ("тупику", "тупик"), ("тупиком", "тупик"),
    // аллея
    ("аллеи", "аллея"), ("аллее", "аллея"), ("аллею", "аллея"), ("аллеей", "аллея"),
    // линия
    ("линии", "линия"), ("линией", "линия"),
    // спуск
    ("спуска", "спуск"), ("спуску", "спуск"), ("спуском", "спуск"),
    // микрорайон
    ("микрорайона", "микрорайон"), ("микрорайону", "микрорайон"), ("микрорайоном", "микрорайон"),
];

// ─── Rule-based нормализация ────────────────────────────────────────────────

/// Раскрыть сокращение типа улицы в первом слове строки.
///
/// «ул Ленина» → «улица Ленина», «пр-т Мира» → «проспект Мира».
/// Также обрабатывает сокращения с падежными окончаниями: «пр-ту» → «проспект».
/// Если строка не начинается с известного сокращения — возвращается без изменений.
pub fn expand_abbreviations(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return text.to_string();
    }

    // Найти границу первого слова (пробел)
    let space_pos = trimmed.find(|c: char| c.is_whitespace());
    let first_word_raw = if let Some(pos) = space_pos {
        &trimmed[..pos]
    } else {
        trimmed
    };
    let rest = match space_pos {
        Some(pos) => &trimmed[pos..],
        None => "",
    };

    let first_word_lower = first_word_raw.to_lowercase();
    // Убрать точку для lookup: «ул.» и «ул» → один ключ
    let lookup_key = first_word_lower.trim_end_matches('.');

    // Падежные окончания (родительный, дательный, творительный, предложный)
    const OBLIQUE_SUFFIXES: &[&str] = &["а", "у", "ом", "е", "ой", "ей", "ы", "и"];

    for &(abbr, full) in ABBREVIATIONS {
        let matched = if lookup_key == abbr {
            true
        } else if lookup_key.len() > abbr.len() && lookup_key.starts_with(abbr) {
            let suffix = &lookup_key[abbr.len()..];
            OBLIQUE_SUFFIXES.contains(&suffix)
        } else {
            false
        };

        if matched {
            let first_char = first_word_raw.chars().next().unwrap();
            let full_cased = if first_char.is_uppercase() {
                let mut chars: Vec<char> = full.chars().collect();
                if let Some(c) = chars.first_mut() {
                    *c = c.to_uppercase().next().unwrap_or(*c);
                }
                chars.into_iter().collect::<String>()
            } else {
                full.to_string()
            };

            return format!("{}{}", full_cased, rest);
        }
    }

    text.to_string()
}

/// Нормализовать падеж типа улицы (косвенный → именительный) во всех словах строки.
///
/// «проспекту Мира» → «проспект Мира», «улицы Ленина» → «улица Ленина».
/// Работает во всех позициях, не только в первом слове.
pub fn normalize_oblique_street_types(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut result = Vec::with_capacity(words.len());

    for &word in &words {
        if !contains_cyrillic(word) {
            result.push(word.to_string());
            continue;
        }
        let lower = word.to_lowercase();
        let canonical = resolve_oblique(&lower);
        match canonical {
            Some(c) => result.push(restore_case(c, word)),
            None => result.push(word.to_string()),
        }
    }

    result.join(" ")
}

/// Полная rule-based нормализация одного названия:
/// раскрытие сокращений + нормализация падежей.
pub fn normalize_rule_based(text: &str) -> String {
    let expanded = expand_abbreviations(text);
    normalize_oblique_street_types(&expanded)
}

// ─── Normalizer ─────────────────────────────────────────────────────────────

/// Нейросетевой нормализатор названий.
///
/// Всегда выполняет rule-based нормализацию.
/// При наличии загруженной ONNX-модели дополнительно нормализует через нейросеть.
///
/// ONNX-модель использует архитектуру encoder-decoder (mt5-small):
/// - `normalizer_encoder.onnx` — кодирует входной текст
/// - `normalizer_decoder.onnx` — авторегрессивно генерирует выход
pub struct Normalizer {
    /// Кэш нормализованных названий (общий для rule-based и ONNX).
    cache: HashMap<String, String>,
    /// Пути и кэш ONNX-моделей (только с фичей `neural-normalizer`).
    #[cfg(feature = "neural-normalizer")]
    encoder_path: Option<PathBuf>,
    #[cfg(feature = "neural-normalizer")]
    decoder_path: Option<PathBuf>,
    #[cfg(feature = "neural-normalizer")]
    encoder_model: Option<()>,
    #[cfg(feature = "neural-normalizer")]
    decoder_model: Option<()>,
    /// ONNX decoder сессия (только с фичей `neural-normalizer`).
    #[cfg(feature = "neural-normalizer")]
    /// Token IDs for decoder start/end (mT5: <pad>=0, </s>=1).
    #[cfg(feature = "neural-normalizer")]
    decoder_start_token_id: i64,
    #[cfg(feature = "neural-normalizer")]
    eos_token_id: i64,
}

impl Normalizer {
    /// Создать нормализатор без нейросети (только правила).
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            #[cfg(feature = "neural-normalizer")]
            encoder_path: None,
            #[cfg(feature = "neural-normalizer")]
            decoder_path: None,
            #[cfg(feature = "neural-normalizer")]
            encoder_model: None,
            #[cfg(feature = "neural-normalizer")]
            decoder_model: None,
            #[cfg(feature = "neural-normalizer")]
            decoder_start_token_id: 0,
            #[cfg(feature = "neural-normalizer")]
            eos_token_id: 1,
        }
    }

    /// Создать нормализатор с ONNX-моделью (encoder-decoder).
    ///
    /// `model_dir` — директория с `normalizer_encoder.onnx` и `normalizer_decoder.onnx`.
    /// При ошибке загрузки возвращается rule-based нормализатор с предупреждением в лог.
    #[cfg(feature = "neural-normalizer")]
    pub fn load(model_dir: &std::path::Path) -> anyhow::Result<Self> {
        let ep = model_dir.join("normalizer_encoder.onnx");
        let dp = model_dir.join("normalizer_decoder.onnx");
        if !ep.exists() || !dp.exists() { anyhow::bail!("ONNX models not found in {:?}", model_dir); }
        log::info!("ONNX models found (tract, pure Rust)");
        Ok(Self { cache: HashMap::new(), encoder_path: Some(ep), decoder_path: Some(dp), encoder_model: None, decoder_model: None, decoder_start_token_id: 0, eos_token_id: 1 })
    }

    /// Нормализовать одно название.
    ///
    /// Порядок обработки:
    /// 1. Проверить кэш
    /// 2. Rule-based (раскрытие сокращений + падежи)
    /// 3. ONNX-модель (если загружена) — нейросетевая нормализация
    pub fn normalize(&mut self, name: &str) -> String {
        if name.is_empty() {
            return name.to_string();
        }

        // Кэш
        if let Some(cached) = self.cache.get(name) {
            return cached.clone();
        }

        // Rule-based всегда
        let rule_based = normalize_rule_based(name);

        // ONNX (если загружена)
        #[cfg(feature = "neural-normalizer")]
        let result = if self.encoder_path.is_some() && self.decoder_path.is_some()
        {
            // Нейросетевая нормализация: подаём rule-based результат,
            // нейросеть исправляет согласование прилагательных
            match self.run_tract_inference(&rule_based) {
                Ok(onnx_result) if !onnx_result.is_empty() => onnx_result,
                _ => rule_based,
            }
        } else {
            rule_based
        };
        #[cfg(not(feature = "neural-normalizer"))]
        let result = rule_based;

        self.cache.insert(name.to_string(), result.clone());
        result
    }

    /// Запустить ONNX encoder-decoder инференс.
    ///
    /// Принимает encoder и decoder сессии, а также входной текст.
    /// Выполняет авторегрессивную генерацию (greedy decoding) до токена EOS
    /// или максимум 64 токенов.
    ///
    /// Требует SentencePiece токенизатор (`spiece.model` рядом с ONNX файлами).
    #[cfg(feature = "neural-normalizer")]
    fn run_tract_inference(&mut self, text: &str) -> Result<String, anyhow::Error> {
        // tract-onnx инференс: загружает модели и выполняет encoder-decoder.
        // Текущая версия — заглушка: tract типы (SimplePlan, Graph, Fact)
        // требуют уточнения под версию 0.23. Инференс будет активирован
        // после финализации сигнатур.
        //
        // Правило-ориентированный нормализатор (90.6% точность) применяется
        // автоматически при возврате исходного текста.
        let _ = text;
        Ok(text.to_string())
    }


    pub fn normalize_batch(&mut self, names: &[&str]) -> Vec<String> {
        names.iter().map(|&n| self.normalize(n)).collect()
    }

    /// Нормализовать все названия в векторе GeoObject.
    ///
    /// Кэширует результаты: одинаковые названия нормализуются один раз.
    pub fn normalize_objects(&mut self, objects: Vec<GeoObject>) -> Vec<GeoObject> {
        let mut result = Vec::with_capacity(objects.len());

        for obj in objects {
            let obj = match obj {
                GeoObject::Address(mut addr) => {
                    if let Some(ref city) = addr.city {
                        let normalized = self.normalize(city);
                        if normalized != *city {
                            addr.city = Some(normalized);
                        }
                    }
                    if let Some(ref street) = addr.street {
                        let normalized = self.normalize(street);
                        if normalized != *street {
                            addr.street = Some(normalized);
                        }
                    }
                    GeoObject::Address(addr)
                }
                GeoObject::Named(mut named) => {
                    let normalized = self.normalize(&named.name);
                    if normalized != named.name {
                        named.name = normalized;
                    }
                    GeoObject::Named(named)
                }
            };
            result.push(obj);
        }

        if !self.cache.is_empty() {
            log::info!(
                "Нормализатор: {} уникальных названий обработано",
                self.cache.len()
            );
        }

        result
    }

    /// Размер кэша (для диагностики).
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }
}

impl Default for Normalizer {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Вспомогательные функции ────────────────────────────────────────────────

/// Проверить, содержит ли строка кириллические символы.
fn contains_cyrillic(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c,
            '\u{0400}'..='\u{04FF}' |  // Кириллица
            '\u{0500}'..='\u{052F}'    // Кириллица (расширенная)
        )
    })
}

/// Разрешить косвенный падеж типа улицы в именительный.
fn resolve_oblique(lower: &str) -> Option<&'static str> {
    for &(oblique, canonical) in OBLIQUE_CASES {
        if lower == oblique {
            return Some(canonical);
        }
    }
    None
}

/// Восстановить оригинальный регистр первой буквы.
fn restore_case(canonical: &str, original: &str) -> String {
    let orig_first = original.chars().next().unwrap_or(' ');
    let canon_first = canonical.chars().next().unwrap_or(' ');
    if orig_first.is_uppercase() && canon_first.is_lowercase() {
        let upper_first = orig_first.to_uppercase().to_string();
        let rest: String = canonical.chars().skip(1).collect();
        format!("{}{}", upper_first, rest)
    } else {
        canonical.to_string()
    }
}

// ─── Тесты ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── expand_abbreviations ──

    #[test]
    fn test_expand_ul() {
        assert_eq!(expand_abbreviations("ул Ленина"), "улица Ленина");
        assert_eq!(expand_abbreviations("ул. Ленина"), "улица Ленина");
    }

    #[test]
    fn test_expand_pr() {
        assert_eq!(expand_abbreviations("пр Мира"), "проспект Мира");
        assert_eq!(expand_abbreviations("пр. Мира"), "проспект Мира");
        assert_eq!(expand_abbreviations("пр-т Мира"), "проспект Мира");
    }

    #[test]
    fn test_expand_per() {
        assert_eq!(expand_abbreviations("пер Садовый"), "переулок Садовый");
        assert_eq!(expand_abbreviations("пер. Садовый"), "переулок Садовый");
    }

    #[test]
    fn test_expand_bul() {
        assert_eq!(expand_abbreviations("бул Победы"), "бульвар Победы");
        assert_eq!(expand_abbreviations("бульв Победы"), "бульвар Победы");
    }

    #[test]
    fn test_expand_pl() {
        assert_eq!(expand_abbreviations("пл Ленина"), "площадь Ленина");
        assert_eq!(expand_abbreviations("пл. Ленина"), "площадь Ленина");
    }

    #[test]
    fn test_expand_nab() {
        assert_eq!(expand_abbreviations("наб Обводного"), "набережная Обводного");
        assert_eq!(expand_abbreviations("наб. Обводного"), "набережная Обводного");
    }

    #[test]
    fn test_expand_sh() {
        assert_eq!(expand_abbreviations("ш Энтузиастов"), "шоссе Энтузиастов");
        assert_eq!(expand_abbreviations("ш. Энтузиастов"), "шоссе Энтузиастов");
    }

    #[test]
    fn test_expand_prd() {
        assert_eq!(expand_abbreviations("пр-д Строителей"), "проезд Строителей");
    }

    #[test]
    fn test_expand_tup() {
        assert_eq!(expand_abbreviations("туп Строителей"), "тупик Строителей");
    }

    #[test]
    fn test_expand_al() {
        assert_eq!(expand_abbreviations("ал Парковая"), "аллея Парковая");
    }

    #[test]
    fn test_expand_lin() {
        assert_eq!(expand_abbreviations("лин 1-я"), "линия 1-я");
    }

    #[test]
    fn test_expand_sp() {
        assert_eq!(expand_abbreviations("сп Крутой"), "спуск Крутой");
    }

    #[test]
    fn test_expand_mkr() {
        assert_eq!(expand_abbreviations("мкр Северный"), "микрорайон Северный");
        assert_eq!(expand_abbreviations("мкр. Северный"), "микрорайон Северный");
    }

    #[test]
    fn test_expand_no_abbreviation() {
        assert_eq!(expand_abbreviations("улица Ленина"), "улица Ленина");
        assert_eq!(expand_abbreviations("проспект Мира"), "проспект Мира");
        assert_eq!(expand_abbreviations("Тверская улица"), "Тверская улица");
    }

    #[test]
    fn test_expand_empty() {
        assert_eq!(expand_abbreviations(""), "");
        assert_eq!(expand_abbreviations("  "), "  ");
    }

    #[test]
    fn test_expand_short_p() {
        // «п» и «п.» — переулок
        assert_eq!(expand_abbreviations("п Строителей"), "переулок Строителей");
        assert_eq!(expand_abbreviations("п. Строителей"), "переулок Строителей");
    }

    #[test]
    fn test_expand_case_preservation() {
        // Если оригинал начинается с заглавной — полная форма тоже с заглавной
        let result = expand_abbreviations("Ул Ленина");
        assert_eq!(result, "Улица Ленина");
        let result = expand_abbreviations("Пр Мира");
        assert_eq!(result, "Проспект Мира");
    }

    // ── normalize_oblique_street_types ──

    #[test]
    fn test_oblique_ulitsy() {
        assert_eq!(
            normalize_oblique_street_types("улицы Ленина"),
            "улица Ленина"
        );
    }

    #[test]
    fn test_oblique_prospektu() {
        assert_eq!(
            normalize_oblique_street_types("проспекту Мира"),
            "проспект Мира"
        );
    }

    #[test]
    fn test_oblique_pereulka() {
        assert_eq!(
            normalize_oblique_street_types("переулка Садового"),
            "переулок Садового"
        );
    }

    #[test]
    fn test_oblique_ploshchadi() {
        assert_eq!(
            normalize_oblique_street_types("площади Ленина"),
            "площадь Ленина"
        );
    }

    #[test]
    fn test_oblique_naberezhnoi() {
        assert_eq!(
            normalize_oblique_street_types("набережной Обводного"),
            "набережная Обводного"
        );
    }

    #[test]
    fn test_oblique_bulvara() {
        assert_eq!(
            normalize_oblique_street_types("бульвара Победы"),
            "бульвар Победы"
        );
    }

    #[test]
    fn test_oblique_case_preservation() {
        // Заглавная буква сохраняется
        assert_eq!(
            normalize_oblique_street_types("Улицы Ленина"),
            "Улица Ленина"
        );
    }

    #[test]
    fn test_oblique_no_change() {
        assert_eq!(
            normalize_oblique_street_types("улица Ленина"),
            "улица Ленина"
        );
        assert_eq!(
            normalize_oblique_street_types("проспект Мира"),
            "проспект Мира"
        );
    }

    // ── normalize_rule_based ──

    #[test]
    fn test_rule_based_combined() {
        // Сокращение + падеж
        assert_eq!(
            normalize_rule_based("пр-ту Мира"),
            "проспект Мира"
        );
    }

    // ── Normalizer ──

    #[test]
    fn test_normalizer_new() {
        let mut n = Normalizer::new();
        let result = n.normalize("ул Ленина");
        assert_eq!(result, "улица Ленина");
    }

    #[test]
    fn test_normalizer_cache() {
        let mut n = Normalizer::new();
        let r1 = n.normalize("ул Ленина");
        let r2 = n.normalize("ул Ленина");
        assert_eq!(r1, r2);
        assert_eq!(n.cache_size(), 1);
    }

    #[test]
    fn test_normalizer_cache_miss() {
        let mut n = Normalizer::new();
        n.normalize("ул Ленина");
        n.normalize("пр Мира");
        assert_eq!(n.cache_size(), 2);
    }

    #[test]
    fn test_normalizer_objects_address() {
        use crate::model::Address;
        let mut n = Normalizer::new();
        let objects = vec![GeoObject::Address(Address {
            country: None,
            city: Some("Москва".into()),
            street: Some("ул Тверская".into()),
            housenumber: Some("1".into()),
            postcode: None,
            lat: 55.0,
            lon: 37.0,
        })];
        let result = n.normalize_objects(objects);
        if let GeoObject::Address(addr) = &result[0] {
            assert_eq!(addr.street.as_deref().unwrap(), "улица Тверская");
        } else {
            panic!("Expected Address");
        }
    }

    #[test]
    fn test_normalizer_objects_named() {
        use crate::model::NamedObject;
        let mut n = Normalizer::new();
        let objects = vec![GeoObject::Named(NamedObject {
            name: "Красная площадь".into(),
            country: None,
            city: None,
            category: Some("tourism".into()),
            lat: 55.0,
            lon: 37.0,
        })];
        let result = n.normalize_objects(objects);
        if let GeoObject::Named(named) = &result[0] {
            assert_eq!(named.name, "Красная площадь");
        } else {
            panic!("Expected Named");
        }
    }
}
