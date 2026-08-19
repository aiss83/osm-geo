//! Модуль финализации: сжатие базы, вычисление SHA-256, экспорт метаданных.

use anyhow::{Context, Result};
use log::info;
use sha2::Digest;
use std::io::{Read, Write};
use std::path::Path;

/// Сжать файл базы алгоритмом Zstandard (потоково), вычислить SHA-256 и
/// записать метаданные.
///
/// Чтение и сжатие идут потоково, чтобы не держать весь файл в памяти.
/// Возвращает (размер сжатого файла, SHA-256, JSON метаданных).
pub fn compress_and_export_metadata(
    db_path: &Path,
    output_dir: &Path,
    metadata: &mut Metadata,
) -> Result<(u64, String, String)> {
    let stem = db_path.file_stem().unwrap().to_str().unwrap();
    let ext = db_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");

    let compressed_path = output_dir.join(format!("{}.{}.zst", stem, ext));

    info!("Сжатие базы данных...");

    let mut input = std::fs::File::open(db_path).context("Открытие базы для сжатия")?;
    let mut hasher = sha2::Sha256::new();
    let mut original_size = 0u64;

    let compressed_file = std::fs::File::create(&compressed_path)
        .with_context(|| format!("Создание {}", compressed_path.display()))?;
    let mut encoder = zstd::stream::Encoder::new(compressed_file, 3)
        .context("Создание zstd-энкодера")?;

    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = input.read(&mut buf).context("Чтение базы")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        encoder.write_all(&buf[..n]).context("Zstd-сжатие")?;
        original_size += n as u64;
    }

    let compressed_file = encoder.finish().context("Завершение zstd-сжатия")?;
    let compressed_size = compressed_file
        .metadata()
        .context("Метаданные сжатого файла")?
        .len();

    let sha256 = format!("{:x}", hasher.finalize());
    metadata.sha256 = sha256.clone();
    metadata.compressed_size_bytes = Some(compressed_size);

    let ratio = compressed_size as f64 / original_size.max(1) as f64 * 100.0;
    info!(
        "Сжатие: {:.2} МБ → {:.2} МБ ({:.1}% от исходного)",
        original_size as f64 / (1024.0 * 1024.0),
        compressed_size as f64 / (1024.0 * 1024.0),
        ratio
    );

    let metadata_path = output_dir.join(format!("{}.metadata.json", stem));
    let metadata_json = serde_json::to_string_pretty(metadata)?;
    std::fs::write(&metadata_path, &metadata_json)
        .with_context(|| format!("Запись {}", metadata_path.display()))?;

    info!("Метаданные записаны: {:?}", metadata_path);

    Ok((compressed_size, sha256, metadata_json))
}

/// Метаданные о собранной базе.
#[derive(serde::Serialize)]
pub struct Metadata {
    pub version: String,
    pub country: String,
    pub country_name: String,
    pub build_date: String,
    pub source_pbf: String,
    pub object_count: u64,
    pub address_count: u64,
    pub named_count: u64,
    pub db_size_bytes: u64,
    pub compressed_size_bytes: Option<u64>,
    pub sha256: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_roundtrip() {
        let dir = std::env::temp_dir().join("osm-geo-finalizer-test");
        std::fs::create_dir_all(&dir).unwrap();

        let db_path = dir.join("test.bin");
        let content = b"hello world ".repeat(4096);
        std::fs::write(&db_path, &content).unwrap();

        let mut metadata = Metadata {
            version: "0.0.0".into(),
            country: "RU".into(),
            country_name: "Россия".into(),
            build_date: "2026-01-01".into(),
            source_pbf: "test.pbf".into(),
            object_count: 0,
            address_count: 0,
            named_count: 0,
            db_size_bytes: content.len() as u64,
            compressed_size_bytes: None,
            sha256: String::new(),
        };

        let (compressed_size, sha256, json) =
            compress_and_export_metadata(&db_path, &dir, &mut metadata).unwrap();

        assert_eq!(sha256.len(), 64);
        assert!(compressed_size > 0);
        assert_eq!(metadata.compressed_size_bytes, Some(compressed_size));
        assert!(json.contains("compressed_size_bytes"));

        // Распаковать и сверить содержимое.
        let zst = std::fs::read(dir.join("test.bin.zst")).unwrap();
        let decoded = zstd::decode_all(&zst[..]).unwrap();
        assert_eq!(decoded, content);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
