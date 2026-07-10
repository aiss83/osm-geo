//! Модуль загрузки PBF-файлов с Geofabrik.
//!
//! Поддерживает:
//! - Загрузку по имени региона (напр. `russia/central-fed-district`)
//! - Кэширование в локальной директории
//! - Проверку целостности файла (Content-Length)
//! - Докачку оборванных загрузок (HTTP Range)

use anyhow::{Context, Result};
use log::{info, warn};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Базовый URL Geofabrik для загрузки PBF.
const GEOFABRIK_BASE: &str = "https://download.geofabrik.de";

/// Загрузить PBF-файл для указанного региона Geofabrik.
///
/// `region` — путь региона, например `"russia/central-fed-district"`.
/// `cache_dir` — директория для кэширования загруженных файлов.
///
/// Возвращает путь к загруженному (или закэшированному) файлу.
pub fn download_pbf(region: &str, cache_dir: &Path) -> Result<PathBuf> {
    let url = format!("{}/{}-latest.osm.pbf", GEOFABRIK_BASE, region);
    let filename = format!("{}-latest.osm.pbf", region.replace('/', "-"));
    let dest = cache_dir.join(&filename);

    // Создаём кэш-директорию
    std::fs::create_dir_all(cache_dir)
        .context("Создание кэш-директории")?;

    info!("Загрузка PBF: {}", url);
    info!("Кэш-файл: {:?}", dest);

    // Проверяем, есть ли уже полный файл
    if dest.exists() {
        let size = std::fs::metadata(&dest)?.len();
        if size > 0 {
            // Проверяем, совпадает ли размер с серверным
            match get_content_length(&url) {
                Ok(expected) if size == expected => {
                    info!(
                        "Файл уже загружен ({:.1} МБ), пропускаем загрузку",
                        size as f64 / (1024.0 * 1024.0)
                    );
                    return Ok(dest);
                }
                Ok(expected) => {
                    info!(
                        "Размер не совпадает: локально {:.1} МБ, сервер {:.1} МБ. Докачка...",
                        size as f64 / (1024.0 * 1024.0),
                        expected as f64 / (1024.0 * 1024.0)
                    );
                    // Если локальный файл больше серверного — удаляем и качаем заново
                    if size >= expected {
                        std::fs::remove_file(&dest)?;
                    }
                    // Иначе — пытаемся докачать
                    return resume_download(&url, &dest, size, expected);
                }
                Err(_) => {
                    warn!("Не удалось получить размер файла с сервера, перекачиваем заново");
                    std::fs::remove_file(&dest)?;
                }
            }
        }
    }

    // Полная загрузка
    download_full(&url, &dest)
}

/// Получить ожидаемый размер файла с сервера (Content-Length).
fn get_content_length(url: &str) -> Result<u64> {
    let client = reqwest::blocking::Client::new();
    let resp = client.head(url).send()?;

    resp.headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .context("Не удалось получить Content-Length")
}

/// Полная загрузка файла.
fn download_full(url: &str, dest: &Path) -> Result<PathBuf> {
    let client = reqwest::blocking::Client::new();
    let resp = client.get(url).send()?;

    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(dest)?;
    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 65536]; // 64 КБ буфер

    let mut body = resp;
    loop {
        let n = body.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;
        if total > 0 && downloaded % (1024 * 1024 * 10) < 65536 {
            // Прогресс каждые ~10 МБ
            info!(
                "Загрузка: {:.1} / {:.1} МБ ({:.0}%)",
                downloaded as f64 / (1024.0 * 1024.0),
                total as f64 / (1024.0 * 1024.0),
                if total > 0 {
                    downloaded as f64 / total as f64 * 100.0
                } else {
                    0.0
                }
            );
        }
    }

    info!(
        "Загрузка завершена: {:.1} МБ",
        downloaded as f64 / (1024.0 * 1024.0)
    );
    Ok(dest.to_path_buf())
}

/// Докачка файла с позиции `existing_size`.
fn resume_download(url: &str, dest: &Path, existing_size: u64, _expected_size: u64) -> Result<PathBuf> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(url)
        .header("Range", format!("bytes={}-", existing_size))
        .send()?;

    let status = resp.status();
    if !status.is_success() && status.as_u16() != 206 {
        // Сервер не поддерживает Range — качаем заново
        warn!("Сервер не поддерживает докачку, загружаем заново");
        std::fs::remove_file(dest)?;
        return download_full(url, dest);
    }

    info!("Докачка с позиции {:.1} МБ", existing_size as f64 / (1024.0 * 1024.0));
    let mut file = std::fs::OpenOptions::new().append(true).open(dest)?;

    let mut downloaded = existing_size;
    let mut buf = [0u8; 65536];
    let mut body = resp;

    loop {
        let n = body.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;
    }

    info!(
        "Докачка завершена: {:.1} МБ",
        downloaded as f64 / (1024.0 * 1024.0)
    );
    Ok(dest.to_path_buf())
}

/// Проверить, является ли строка путём к существующему файлу.
pub fn is_existing_file(path: &str) -> bool {
    Path::new(path).is_file()
}

/// Проверить, похожа ли строка на регион Geofabrik (содержит `/`).
pub fn looks_like_region(path: &str) -> bool {
    path.contains('/') && !path.starts_with("http://") && !path.starts_with("https://")
}

/// Распаковать файл, если он сжат (.gz или .zst).
/// Определяет формат по MAGIC BYTES (а не по расширению) —
/// работает даже если файл переименован.
/// Возвращает путь к распакованному .pbf.
pub fn decompress_if_needed(compressed: &Path) -> Result<PathBuf> {
    // Читаем первые 4 байта для определения формата
    let mut magic = [0u8; 4];
    if let Ok(mut f) = std::fs::File::open(compressed) {
        use std::io::Read;
        let _ = f.read_exact(&mut magic);
    }

    let name = compressed.to_str().unwrap_or("");

    // Gzip: magic = 1F 8B
    if magic[0] == 0x1F && magic[1] == 0x8B {
        let dest = if name.ends_with(".gz") {
            PathBuf::from(name.strip_suffix(".gz").unwrap())
        } else {
            PathBuf::from(format!("{}.decompressed", name))
        };
        if dest.exists() {
            info!("Распакованный файл уже существует: {:?}", dest);
            return Ok(dest);
        }
        info!("Распаковка gzip (magic 1F 8B): {:?} → {:?}", compressed, dest);
        let input = std::fs::File::open(compressed)?;
        let mut decoder = flate2::read::GzDecoder::new(input);
        let mut output = std::fs::File::create(&dest)?;
        std::io::copy(&mut decoder, &mut output)?;
        info!("Распаковка завершена: {:?} ({:.1} МБ)", dest,
            std::fs::metadata(&dest)?.len() as f64 / (1024.0 * 1024.0));
        return Ok(dest);
    }

    // Zstd: magic = 28 B5 2F FD
    if magic == [0x28, 0xB5, 0x2F, 0xFD] {
        let dest = if name.ends_with(".zst") {
            PathBuf::from(name.strip_suffix(".zst").unwrap())
        } else {
            PathBuf::from(format!("{}.decompressed", name))
        };
        if dest.exists() {
            info!("Распакованный файл уже существует: {:?}", dest);
            return Ok(dest);
        }
        info!("Распаковка zstd (magic 28 B5 2F FD): {:?} → {:?}", compressed, dest);
        let input = std::fs::read(compressed)?;
        let output = zstd::decode_all(&input[..])?;
        std::fs::write(&dest, output)?;
        info!("Распаковка завершена: {:?} ({:.1} МБ)", dest,
            std::fs::metadata(&dest)?.len() as f64 / (1024.0 * 1024.0));
        return Ok(dest);
    }

    // Расширение предполагает архив, но magic bytes не совпадают → ошибка
    if name.ends_with(".gz") || name.ends_with(".zst") {
        anyhow::bail!(
            "Файл имеет расширение архива, но не является сжатым (magic bytes не совпадают): {:?}",
            compressed
        );
    }

    // Не сжатый файл
    Ok(compressed.to_path_buf())
}

/// Извлечь stem из имени файла для авто-генерации выходного имени.
///
/// Примеры:
/// - `central-fed-district-latest.osm.pbf` → `central-fed-district`
/// - `/path/to/russia-latest.osm.pbf` → `russia`
pub fn derive_output_stem(input_path: &Path) -> String {
    let filename = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    // Убираем суффиксы .osm и -latest
    let name = filename
        .strip_suffix(".osm")
        .unwrap_or(filename);

    let name = name
        .strip_suffix("-latest")
        .unwrap_or(name);

    name.to_string()
}

/// Регион Geofabrik с именем и размером.
#[derive(Debug, Clone)]
pub struct RegionInfo {
    pub name: String,
    pub subpath: String, // напр. "russia/central-fed-district"
    pub size_mb: f64,
}

/// Получить список подрегионов для указанного региона Geofabrik.
/// Если region пустой — показывает верхний уровень (континенты/страны).
pub fn list_regions(region: &str) -> Result<Vec<RegionInfo>> {
    let url = if region.is_empty() {
        "https://download.geofabrik.de/index.html".to_string()
    } else {
        format!("https://download.geofabrik.de/{}.html", region)
    };

    let html = reqwest::blocking::get(&url)?.text()?;
    Ok(parse_region_table(&html, region))
}

/// Парсинг HTML-таблицы подрегионов Geofabrik.
fn parse_region_table(html: &str, parent: &str) -> Vec<RegionInfo> {
    let mut regions = Vec::new();

    // Ищем href="...-latest.osm.pbf" ... (SIZE&nbsp;MB) или (SIZE MB)
    let re = regex_lite::Regex::new(
        r#"<a href="([^"]*-latest\.osm\.pbf)"[^>]*>\[\.osm\.pbf\]</a>.*?\(([^)]+)\)"#,
    );

    let Ok(re) = re else {
        return regions;
    };

    for caps in re.captures_iter(html) {
        let href = caps.get(1).unwrap().as_str();
        let size_str = caps.get(2).unwrap().as_str();

        // Извлекаем имя региона из href: "russia/central-fed-district-latest.osm.pbf"
        let subpath = href
            .strip_suffix("-latest.osm.pbf")
            .unwrap_or(href)
            .to_string();

        // Имя — последний компонент пути, с заменой дефисов на пробелы
        let name = subpath
            .rsplit('/')
            .next()
            .unwrap_or(&subpath)
            .replace('-', " ");

        let size_mb = parse_size(size_str);

        if parent.is_empty() || subpath.starts_with(&format!("{}/", parent)) {
            regions.push(RegionInfo {
                name,
                subpath,
                size_mb,
            });
        }
    }

    // Если родитель не пустой, фильтруем только прямых потомков
    if !parent.is_empty() {
        let depth = parent.matches('/').count() + 1;
        regions.retain(|r| r.subpath.matches('/').count() == depth);
    }

    regions
}

/// Парсинг размера: "828 MB" → 828.0, "38.4 MB" → 38.4, "1.2 GB" → 1228.8
fn parse_size(s: &str) -> f64 {
    let s = s.trim().replace("&nbsp;", " ");
    if let Some(mb) = s.strip_suffix(" MB") {
        mb.trim().parse().unwrap_or(0.0)
    } else if let Some(gb) = s.strip_suffix(" GB") {
        gb.trim().parse::<f64>().unwrap_or(0.0) * 1024.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_output_stem() {
        assert_eq!(
            derive_output_stem(Path::new("central-fed-district-latest.osm.pbf")),
            "central-fed-district"
        );
        assert_eq!(
            derive_output_stem(Path::new("/data/russia-latest.osm.pbf")),
            "russia"
        );
        assert_eq!(
            derive_output_stem(Path::new("moscow.osm.pbf")),
            "moscow"
        );
    }

    #[test]
    fn test_looks_like_region() {
        assert!(looks_like_region("russia/central-fed-district"));
        assert!(!looks_like_region("russia-latest.osm.pbf"));
        assert!(!looks_like_region("https://example.com/file.pbf"));
    }

    #[test]
    fn test_is_existing_file() {
        assert!(is_existing_file("Cargo.toml"));
        assert!(!is_existing_file("nonexistent.pbf"));
    }

    #[test]
    fn test_parse_size_mb() {
        assert!((parse_size("828 MB") - 828.0).abs() < 0.01);
        assert!((parse_size("38.4 MB") - 38.4).abs() < 0.01);
    }

    #[test]
    fn test_parse_size_gb() {
        assert!((parse_size("1.2 GB") - 1228.8).abs() < 0.1);
    }

    #[test]
    fn test_parse_region_table() {
        let html = r#"<a href="russia/central-fed-district-latest.osm.pbf">[.osm.pbf]</a> (828 MB)"#;
        let regions = parse_region_table(html, "russia");
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].name, "central fed district");
        assert_eq!(regions[0].subpath, "russia/central-fed-district");
        assert!((regions[0].size_mb - 828.0).abs() < 0.01);
    }
}
