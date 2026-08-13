//! Общие утилиты: расстояние Левенштейна, формула Хаверсина, работа с датами.

use std::time::SystemTime;

use indicatif::{ProgressBar, ProgressStyle};

/// Создать прогресс-бар единого стиля для этапов пост-обработки.
///
/// `label` выводится в начале строки; прогресс считается по `len` элементов.
pub fn progress_bar(len: u64, label: &str) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] {msg}: {percent:>3}% [{bar:40}] {pos}/{len}, ETA {eta}",
            )
            .unwrap()
            .progress_chars("##-"),
    );
    pb.set_message(label.to_string());
    pb
}

/// Расстояние Левенштейна между двумя строками (edit distance).
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let n = a_chars.len();
    let m = b_chars.len();

    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr = vec![0usize; m + 1];

    for i in 1..=n {
        curr[0] = i;
        for j in 1..=m {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1)           // удаление
                .min(curr[j - 1] + 1)          // вставка
                .min(prev[j - 1] + cost);      // замена
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[m]
}

/// Приближённое расстояние Хаверсина в метрах (для сравнения, не для навигации).
pub fn haversine_approx(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6_371_000.0;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    r * c
}

/// Текущая дата в ISO-формате (YYYY-MM-DD).
///
/// Использует только `std::time::SystemTime` — без внешних зависимостей.
/// Достаточно для метаданных сборки; для точных календарных вычислений
/// используйте специализированный крейт (`time` или `chrono`).
pub fn today_iso() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();

    // Приблизительное преобразование unixtime → YYYY-MM-DD.
    // Корректно для всех дат с 1970 по 2100 год.
    let days = secs / 86400;
    let (y, m, d) = days_to_date(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Грубое преобразование дней от 1970-01-01 в (year, month, day).
fn days_to_date(days: i64) -> (i64, u32, u32) {
    let mut y = 1970i64;
    let mut remaining = days;

    loop {
        let year_days = if is_leap(y) { 366 } else { 365 };
        if remaining < year_days {
            break;
        }
        remaining -= year_days;
        y += 1;
    }

    static MONTH_DAYS_LEAP: [i64; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    static MONTH_DAYS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    let month_days = if is_leap(y) { &MONTH_DAYS_LEAP } else { &MONTH_DAYS };

    let mut m = 1u32;
    for &md in month_days {
        if remaining < md {
            break;
        }
        remaining -= md;
        m += 1;
    }

    let d = (remaining + 1) as u32;
    (y, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levenshtein_same() {
        assert_eq!(levenshtein_distance("Москва", "Москва"), 0);
    }

    #[test]
    fn test_levenshtein_one_char() {
        assert_eq!(levenshtein_distance("Москва", "Москвa"), 1); // а vs a (лат.)
        assert_eq!(levenshtein_distance("Москва", "Мсква"), 1);
    }

    #[test]
    fn test_levenshtein_two_chars() {
        assert_eq!(levenshtein_distance("Москва", "Мскв"), 2);
    }

    #[test]
    fn test_haversine() {
        let d = haversine_approx(55.7539, 37.6208, 55.7520, 37.6175);
        assert!(d > 100.0 && d < 1000.0);
    }

    #[test]
    fn test_haversine_same_point() {
        let d = haversine_approx(55.0, 37.0, 55.0, 37.0);
        assert_eq!(d, 0.0);
    }

    #[test]
    fn test_today_iso_format() {
        let date = today_iso();
        assert_eq!(date.len(), 10);
        assert_eq!(date.chars().nth(4), Some('-'));
        assert_eq!(date.chars().nth(7), Some('-'));
    }
}
