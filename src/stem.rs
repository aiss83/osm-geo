//! Общая токенизация и стемминг для полнотекстового поиска.
//!
//! Используется и SQLite-индексатором, и компактным форматом, чтобы результаты
//! поиска совпадали между обоими путями.

use rust_stemmers::{Algorithm, Stemmer};

/// Стеммер русского языка (snowball).
pub struct RussianStemmer {
    inner: Stemmer,
}

impl RussianStemmer {
    pub fn new() -> Self {
        Self {
            inner: Stemmer::create(Algorithm::Russian),
        }
    }

    /// Разбить на слова по пробелам, привести к нижнему регистру и стеммировать.
    pub fn stemmed_tokens(&self, text: &str) -> Vec<String> {
        text.split_whitespace()
            .map(|w| self.inner.stem(&clean_word(w)).to_string())
            .filter(|t| !t.is_empty())
            .collect()
    }

    /// Только нижний регистр без стемминга — для номеров домов, почтовых
    /// индексов и категорий.
    pub fn raw_tokens(&self, text: &str) -> Vec<String> {
        text.split_whitespace()
            .map(clean_word)
            .filter(|t| !t.is_empty())
            .collect()
    }
}

/// Нижний регистр + срез обрамляющих не-буквенно-цифровых символов.
///
/// Позволяет искать «Заречье» по токену `зареч`, даже если в OSM имя записано
/// как `"Заречье"`. Внутренние дефисы и слэши сохраняются (`1-й`, `12/1`).
fn clean_word(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stemmed_tokens() {
        let s = RussianStemmer::new();
        assert_eq!(s.stemmed_tokens("Тверская улица"), vec!["тверск", "улиц"]);
        assert_eq!(s.stemmed_tokens("Москва"), vec!["москв"]);
    }

    #[test]
    fn test_raw_tokens_no_stem() {
        let s = RussianStemmer::new();
        assert_eq!(s.raw_tokens("12/1"), vec!["12/1"]);
        assert_eq!(s.raw_tokens("125009"), vec!["125009"]);
    }

    #[test]
    fn test_clean_word_strips_punctuation() {
        let s = RussianStemmer::new();
        assert_eq!(s.stemmed_tokens("\"Заречье\""), vec!["зареч"]);
        assert_eq!(s.stemmed_tokens("(Красная площадь)"), vec!["красн", "площад"]);
        assert_eq!(s.stemmed_tokens("1-й"), vec!["1-й"]);
    }
}
