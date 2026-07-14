//! Модуль финализации: сжатие базы, проверка целостности, экспорт метаданных.

use anyhow::{Context, Result};
use log::info;
use std::io::Read;
use std::path::Path;

/// Сжать SQLite-файл алгоритмом Zstandard, вычислить SHA-256 и записать метаданные.
///
/// Возвращает (размер сжатого файла, SHA-256, метаданные JSON).
/// SHA-256 вычисляется из буфера в памяти, без повторного чтения файла.
pub fn compress_and_export_metadata(
    db_path: &Path,
    output_dir: &Path,
    metadata: &mut Metadata,
) -> Result<(u64, String, String)> {
    // Читаем исходную базу
    info!("Сжатие базы данных...");
    let mut input = Vec::new();
    std::fs::File::open(db_path)
        .context("Открытие базы для сжатия")?
        .read_to_end(&mut input)
        .context("Чтение базы")?;

    // SHA-256 из буфера в памяти (без повторного чтения файла)
    let sha256 = sha256_from_bytes(&input);
    metadata.sha256 = sha256.clone();

    let original_size = input.len();

    // Сжимаем Zstd
    let compressed = zstd::encode_all(&input[..], 3) // уровень 3 — быстрый, хорошее сжатие
        .context("Zstd-сжатие")?;

    let compressed_size = compressed.len();
    let ratio = compressed_size as f64 / original_size as f64 * 100.0;

    // Имя сжатого файла
    let stem = db_path.file_stem().unwrap().to_str().unwrap();
    let compressed_path = output_dir.join(format!("{}.db.zst", stem));

    std::fs::write(&compressed_path, &compressed)
        .context("Запись сжатого файла")?;

    info!(
        "Сжатие: {:.2} МБ → {:.2} МБ ({:.1}% от исходного)",
        original_size as f64 / (1024.0 * 1024.0),
        compressed_size as f64 / (1024.0 * 1024.0),
        ratio
    );

    // Экспорт метаданных JSON
    let metadata_path = output_dir.join(format!("{}.metadata.json", stem));
    let metadata_json = serde_json::to_string_pretty(metadata)?;
    std::fs::write(&metadata_path, &metadata_json)
        .context("Запись метаданных JSON")?;

    info!("Метаданные записаны: {:?}", metadata_path);

    Ok((compressed_size as u64, sha256, metadata_json))
}

/// Метаданные о собранной базе.
#[derive(serde::Serialize)]
pub struct Metadata {
    pub version: String,
    pub region: String,
    pub build_date: String,
    pub source_pbf: String,
    pub object_count: u64,
    pub address_count: u64,
    pub named_count: u64,
    pub db_size_bytes: u64,
    pub compressed_size_bytes: Option<u64>,
    pub sha256: String,
}

/// Вычислить SHA-256 хеш от байтового буфера.
fn sha256_from_bytes(data: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Вычислить SHA-256 хеш файла.
pub fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_file() {
        let path = std::path::Path::new("Cargo.toml");
        let hash = sha256_file(path).unwrap();
        assert_eq!(hash.len(), 64); // SHA-256 = 32 байта = 64 hex-символов
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_sha256_deterministic() {
        let path = std::path::Path::new("Cargo.toml");
        let hash1 = sha256_file(path).unwrap();
        let hash2 = sha256_file(path).unwrap();
        assert_eq!(hash1, hash2);
    }
}
