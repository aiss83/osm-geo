//! Транслитерация кириллицы в латиницу.
//!
//! Используется на стороне сборщика: транслитерированная форма
//! названия индексируется наряду с оригиналом, чтобы запросы
//! латиницей (напр. «Moskva») находили объекты «Москва».
//!
//! Схема транслитерации — гибридная, приближенная к тому, что
//! используют Яндекс.Карты / Google Maps для русского языка.

/// Транслитерировать русский текст в латиницу.
///
/// Возвращает `None` если текст уже на латинице (нет кириллических символов).
pub fn transliterate(text: &str) -> Option<String> {
    if !contains_cyrillic(text) {
        return None;
    }

    let mut result = String::with_capacity(text.len() * 2);
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        match ch {
            'А' => result.push('A'),     'а' => result.push('a'),
            'Б' => result.push('B'),     'б' => result.push('b'),
            'В' => result.push('V'),     'в' => result.push('v'),
            'Г' => result.push('G'),     'г' => result.push('g'),
            'Д' => result.push('D'),     'д' => result.push('d'),
            'Е' => {
                if i == 0 || is_prev_vowel_or_sign(&chars, i) {
                    result.push_str("Ye");
                } else {
                    result.push('E');
                }
            }
            'е' => {
                if i == 0 || is_prev_vowel_or_sign(&chars, i) {
                    result.push_str("ye");
                } else {
                    result.push('e');
                }
            }
            'Ё' => result.push_str("Yo"),
            'ё' => result.push_str("yo"),
            'Ж' => result.push_str("Zh"), 'ж' => result.push_str("zh"),
            'З' => result.push('Z'),     'з' => result.push('z'),
            'И' => result.push('I'),     'и' => result.push('i'),
            'Й' => result.push('Y'),     'й' => result.push('y'),
            'К' => result.push('K'),     'к' => result.push('k'),
            'Л' => result.push('L'),     'л' => result.push('l'),
            'М' => result.push('M'),     'м' => result.push('m'),
            'Н' => result.push('N'),     'н' => result.push('n'),
            'О' => result.push('O'),     'о' => result.push('o'),
            'П' => result.push('P'),     'п' => result.push('p'),
            'Р' => result.push('R'),     'р' => result.push('r'),
            'С' => result.push('S'),     'с' => result.push('s'),
            'Т' => result.push('T'),     'т' => result.push('t'),
            'У' => result.push('U'),     'у' => result.push('u'),
            'Ф' => result.push('F'),     'ф' => result.push('f'),
            'Х' => result.push_str("Kh"), 'х' => result.push_str("kh"),
            'Ц' => result.push_str("Ts"), 'ц' => result.push_str("ts"),
            'Ч' => result.push_str("Ch"), 'ч' => result.push_str("ch"),
            'Ш' => result.push_str("Sh"), 'ш' => result.push_str("sh"),
            'Щ' => result.push_str("Shch"), 'щ' => result.push_str("shch"),
            'Ъ' | 'ъ' => {}, // твёрдый знак опускаем
            'Ы' => result.push('Y'),     'ы' => result.push('y'),
            'Ь' | 'ь' => {}, // мягкий знак опускаем
            'Э' => result.push('E'),     'э' => result.push('e'),
            'Ю' => result.push_str("Yu"),
            'ю' => result.push_str("yu"),
            'Я' => result.push_str("Ya"),
            'я' => result.push_str("ya"),
            // Пробелы, дефисы, цифры — как есть
            ' ' | '-' | '0'..='9' => result.push(ch),
            // Прочие латинские символы — как есть
            c if c.is_ascii_alphanumeric() || c == '\'' => result.push(c),
            // Остальное — пропускаем
            _ => {}
        }
        i += 1;
    }

    // Убираем trailing/leading пробелы
    let result = result.trim().to_string();
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Проверить, содержит ли текст кириллические символы.
fn contains_cyrillic(text: &str) -> bool {
    text.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c))
}

/// Предыдущий символ — гласная, ь или ъ?
fn is_prev_vowel_or_sign(chars: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    matches!(
        chars[i - 1],
        'а' | 'е' | 'ё' | 'и' | 'о' | 'у' | 'ы' | 'э' | 'ю' | 'я'
            | 'А' | 'Е' | 'Ё' | 'И' | 'О' | 'У' | 'Ы' | 'Э' | 'Ю' | 'Я'
            | 'ь' | 'Ь' | 'ъ' | 'Ъ' | ' ' | '-'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_words() {
        assert_eq!(transliterate("Москва").unwrap(), "Moskva");
        assert_eq!(transliterate("Россия").unwrap(), "Rossiya");
        assert_eq!(transliterate("Санкт-Петербург").unwrap(), "Sankt-Peterburg");
    }

    #[test]
    fn test_special_letters() {
        assert_eq!(transliterate("Щёлково").unwrap(), "Shchyolkovo");
        assert_eq!(transliterate("Химки").unwrap(), "Khimki");
        assert_eq!(transliterate("Царицыно").unwrap(), "Tsaritsyno");
        assert_eq!(transliterate("Черёмушки").unwrap(), "Cheryomushki");
        assert_eq!(transliterate("Жуковский").unwrap(), "Zhukovskiy");
    }

    #[test]
    fn test_ye_rule() {
        assert_eq!(transliterate("Егорьевск").unwrap(), "Yegoryevsk");
        assert_eq!(transliterate("Полежаевская").unwrap(), "Polezhayevskaya");
        assert_eq!(transliterate("Переделкино").unwrap(), "Peredelkino");
    }

    #[test]
    fn test_ya_rule() {
        assert_eq!(transliterate("Ясенево").unwrap(), "Yasenevo");
        assert_eq!(transliterate("Красноярск").unwrap(), "Krasnoyarsk");
        assert_eq!(transliterate("Полянка").unwrap(), "Polyanka");
    }

    #[test]
    fn test_no_cyrillic() {
        assert_eq!(transliterate("Moscow"), None);
        assert_eq!(
            transliterate("Санкт-Петербург и Moscow").unwrap(),
            "Sankt-Peterburg i Moscow"
        );
    }

    #[test]
    fn test_street_names() {
        assert_eq!(
            transliterate("улица Тверская").unwrap(),
            "ulitsa Tverskaya"
        );
        assert_eq!(
            transliterate("Ленинский проспект").unwrap(),
            "Leninskiy prospekt"
        );
    }

    #[test]
    fn test_empty() {
        assert_eq!(transliterate(""), None);
    }

    #[test]
    fn test_uppercase_preservation() {
        assert_eq!(transliterate("МОСКВА").unwrap(), "MOSKVA");
        assert_eq!(transliterate("Красная Площадь").unwrap(), "Krasnaya Ploshchad");
    }
}