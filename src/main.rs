//! CLI osm-geo — ETL-пайплайн для подготовки офлайн-базы геокодирования.
//!
//! Основные команды:
//!   build   — сборка базы для страны
//!   convert — конвертация OSM PBF в GeoDesk GOL

mod compact;
mod boundary;
mod corrector;
mod country;
mod dedup;
mod downloader;
mod finalizer;
mod fts;
mod gol;
#[cfg(feature = "gol-ffi")]
mod gol_ffi;
mod model;
mod normalizer;
mod parser;
mod source;
mod stem;
mod utils;

use anyhow::Result;
use clap::{Parser, Subcommand};
use log::info;
use std::io::Write;
use std::path::PathBuf;


#[derive(Parser)]
#[command(name = "osm-geo")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "ETL-пайплайн для подготовки офлайн-базы геокодирования из OSM")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Уровень логирования (off, error, warn, info, debug, trace)
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Сборка базы данных для страны
    Build {
        /// PBF-файл (локальный путь) или страна Geofabrik (напр. russia)
        #[arg(short, long)]
        input: String,

        /// Путь к выходному файлу (по умолчанию: {stem}.bin)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Код страны ISO 3166-1 alpha-2 (напр. RU). Если не задан —
        /// определяется по имени файла.
        #[arg(long)]
        country: Option<String>,

        /// Источник данных: auto (по расширению), pbf или gol
        #[arg(long, default_value = "auto")]
        source: String,
    },

    /// Конвертировать OSM PBF в GeoDesk GOL
    Convert {
        /// Входной PBF-файл (локальный путь или регион Geofabrik)
        #[arg(short, long)]
        input: String,

        /// Выходной GOL-файл (по умолчанию: {stem}.gol)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Путь к исполняемому файлу `gol` (по умолчанию — из PATH/кэша)
        #[arg(long)]
        gol_bin: Option<PathBuf>,
    },

    /// Список доступных для скачивания регионов Geofabrik
    List {
        /// Код региона (напр. russia). Без аргумента — верхний уровень.
        region: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Настройка логирования
    let log_level = match cli.log_level.to_lowercase().as_str() {
        "off" => log::LevelFilter::Off,
        "error" => log::LevelFilter::Error,
        "warn" => log::LevelFilter::Warn,
        "info" => log::LevelFilter::Info,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    };
    env_logger::Builder::new()
        .filter_level(log_level)
        .format_timestamp_millis()
        .init();

    match cli.command {
        Commands::Build {
            input,
            output,
            country,
            source,
        } => cmd_build(&input, output.as_ref(), country.as_deref(), &source),
        Commands::Convert {
            input,
            output,
            gol_bin,
        } => cmd_convert(&input, output.as_ref(), gol_bin.as_ref()),
        Commands::List { region } => cmd_list(region.as_deref()),
    }
}

/// Команда build: парсинг PBF → запись в компактный бинарный формат.
fn cmd_build(
    input: &str,
    output: Option<&PathBuf>,
    country_flag: Option<&str>,
    source_kind: &str,
) -> Result<()> {
    info!("=== Сборка базы данных ===");

    // 1. Разрешаем входной файл
    let input_path = resolve_input(input)?;
    info!("Входной файл: {:?}", input_path);

    // 2. Определяем базовое имя выходного файла (без расширения)
    let output_stem = match output {
        Some(p) => {
            let s = p.to_string_lossy();
            // Отрезаем известное расширение, если указано
            let trimmed = s.strip_suffix(".bin").unwrap_or(&s);
            PathBuf::from(trimmed)
        }
        None => {
            let stem = downloader::derive_output_stem(&input_path);
            PathBuf::from(stem)
        }
    };
    info!("Выходной файл (stem): {:?}", output_stem);

    // 3. Парсинг источника (PBF или GOL)
    let corrector = corrector::Corrector::new_or_download()
        .map_err(|e| {
            log::warn!("Не удалось загрузить корректор опечаток: {} (продолжаем без коррекции)", e);
            e
        })
        .ok();

    let kind = match source_kind {
        "auto" => source::detect_source(&input_path),
        "pbf" => source::SourceKind::Pbf,
        "gol" => source::SourceKind::Gol,
        other => anyhow::bail!("Неизвестный --source: '{}'. Допустимо: auto, pbf, gol", other),
    };

    let mut src: Box<dyn source::FeatureSource> = match kind {
        source::SourceKind::Pbf => Box::new(parser::PbfParser::new()),
        #[cfg(feature = "gol-ffi")]
        source::SourceKind::Gol => Box::new(gol_ffi::GolFfiSource::new()),
        #[cfg(not(feature = "gol-ffi"))]
        source::SourceKind::Gol => Box::new(gol::GolSource::new()),
    };
    src.set_corrector(corrector);
    let mut objects = src.parse(&input_path)?;
    info!("Извлечено {} объектов", objects.len());

    // 3a. Определяем страну: данные файла → --country → имя файла.
    let detected = src.country();
    let boundary = src.boundary();
    let mut country = detected
        .or_else(|| country_flag.and_then(country::from_code))
        .or_else(|| country::from_filename(&input_path.to_string_lossy()))
        .unwrap_or_else(|| {
            log::warn!("Не удалось определить страну; сохраняю пустой код");
            model::Country::default()
        });
    if country.name.is_empty() && !country.code.is_empty() {
        if let Some(c) = country::from_code(&country.code) {
            country.name = c.name;
        }
    }
    info!("Страна: {} ({})", country.code, country.name);

    // Унифицируем страну у всех объектов — для консистентной дедупликации.
    for obj in &mut objects {
        let slot = match obj {
            model::GeoObject::Address(a) => &mut a.country,
            model::GeoObject::Named(n) => &mut n.country,
        };
        *slot = if country.code.is_empty() {
            None
        } else {
            Some(country.code.clone())
        };
    }

    // 4. Привязка адресов без города к ближайшему городу
    parser::infer_missing_cities(&mut objects);

    // 4b. Склеивание городов-опечаток с каноническими названиями
    parser::merge_typo_cities(&mut objects);

    // 4c. Нормализация названий (нейросеть при наличии ONNX, всегда rule-based)
    {
        #[cfg(feature = "neural-normalizer")]
        let mut normalizer = {
            let model_dir = std::path::PathBuf::from("models");
            match normalizer::Normalizer::load(&model_dir) {
                Ok(n) => {
                    info!("Нейросетевой нормализатор активирован");
                    n
                }
                Err(e) => {
                    log::warn!("ONNX-модель не загружена: {} (использую rule-based)", e);
                    normalizer::Normalizer::new()
                }
            }
        };
        #[cfg(not(feature = "neural-normalizer"))]
        let mut normalizer = normalizer::Normalizer::new();

        objects = normalizer.normalize_objects(objects);
    }

    // 5. Дедупликация
    let objects = dedup::deduplicate(objects);
    let addr_count = objects.iter().filter(|o| o.as_address().is_some()).count();
    let named_count = objects.iter().filter(|o| o.as_named().is_some()).count();
    let count = objects.len() as u64;

    info!(
        "После дедупликации: {} объектов (адресов: {}, POI: {})",
        count, addr_count, named_count
    );

    // 6. Запись компактного формата
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let output_path = output_stem.with_extension("bin");
    info!("Запись в компактном формате: {:?}", output_path);

    let mut writer = compact::CompactWriter::new();
    writer.build(&objects, &output_path, &country, boundary.as_ref(), timestamp)?;
    let file_size = std::fs::metadata(&output_path)?.len();

    info!(
        "  Размер: {:.2} МБ",
        file_size as f64 / (1024.0 * 1024.0)
    );

    // 7. Сжатие и метаданные
    let output_dir = output_path.parent().unwrap_or(std::path::Path::new("."));
    let mut metadata = finalizer::Metadata {
        version: env!("CARGO_PKG_VERSION").to_string(),
        country: country.code.clone(),
        country_name: country.name.clone(),
        build_date: crate::utils::today_iso(),
        source_pbf: input_path.display().to_string(),
        object_count: count,
        address_count: addr_count as u64,
        named_count: named_count as u64,
        db_size_bytes: file_size,
        compressed_size_bytes: None,
        sha256: String::new(),
    };

    let (compressed_size, sha256, _meta_json) =
        finalizer::compress_and_export_metadata(&output_path, output_dir, &mut metadata)?;

    info!("  SHA-256: {}", sha256);
    info!(
        "  Сжатый размер: {:.2} МБ",
        compressed_size as f64 / (1024.0 * 1024.0)
    );

    info!("=== Сборка завершена ===");
    info!("Объектов: {}", count);

    Ok(())
}

/// Команда convert: конвертация OSM PBF → GeoDesk GOL.
fn cmd_convert(input: &str, output: Option<&PathBuf>, gol_bin: Option<&PathBuf>) -> Result<()> {
    info!("=== Конвертация PBF → GOL ===");

    // 1. Разрешаем входной файл (локальный, регион Geofabrik или URL)
    let input_path = resolve_input(input)?;
    info!("Входной PBF: {:?}", input_path);

    // 2. Выходной GOL-файл
    let output_path = match output {
        Some(p) => p.clone(),
        None => {
            let stem = downloader::derive_output_stem(&input_path);
            PathBuf::from(format!("{}.gol", stem))
        }
    };

    // 3. Находим/устанавливаем утилиту gol
    let tool = match gol_bin {
        Some(p) => gol::GolTool::new(p.clone()),
        None => gol::GolTool::find_or_install()?,
    };

    // 4. Конвертация
    tool.build(&input_path, &output_path)?;
    info!("Создан GOL: {:?}", output_path);
    info!("=== Конвертация завершена ===");
    Ok(())
}

/// Разрешить входной параметр: локальный файл или загрузка из Geofabrik.
/// Автоматически распаковывает .gz/.zst после скачивания.
fn resolve_input(input: &str) -> Result<PathBuf> {
    let path = if downloader::is_existing_file(input) {
        PathBuf::from(input)
    } else if downloader::looks_like_region(input) {
        let cache_dir = dirs_next().unwrap_or_else(|| PathBuf::from("data"));
        downloader::download_pbf(input, &cache_dir)?
    } else if input.starts_with("http://") || input.starts_with("https://") {
        let cache_dir = dirs_next().unwrap_or_else(|| PathBuf::from("data"));
        let filename = input.rsplit('/').next().unwrap_or("download.osm.pbf");
        let dest = cache_dir.join(filename);
        download_full_url(input, &dest)?;
        dest
    } else {
        anyhow::bail!(
            "Файл не найден: '{}'. Используйте существующий файл, регион Geofabrik (напр. russia/central-fed-district) или URL.",
            input
        )
    };

    // Авто-распаковка .gz / .zst
    downloader::decompress_if_needed(&path)
}

/// Директория кэша: data/ в рабочей директории.
fn dirs_next() -> Option<PathBuf> {
    Some(PathBuf::from("data"))
}

/// Простая загрузка полного URL (для произвольных источников).
fn download_full_url(url: &str, dest: &PathBuf) -> Result<()> {
    use std::io::Read;
    info!("Загрузка из URL: {}", url);
    let mut resp = reqwest::blocking::get(url)?;
    let mut file = std::fs::File::create(dest)?;
    let mut buf = [0u8; 65536];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 { break; }
        file.write_all(&buf[..n])?;
    }
    info!("Загрузка завершена: {:?}", dest);
    Ok(())
}

/// Команда list: список регионов Geofabrik.
fn cmd_list(region: Option<&str>) -> Result<()> {
    let region = region.unwrap_or("");
    let suffix = if region.is_empty() {
        String::new()
    } else {
        format!(" для {}", region)
    };
    println!("Загрузка списка регионов{}...\n", suffix);

    let regions = downloader::list_regions(region)?;

    if regions.is_empty() {
        println!("  (ничего не найдено)");
        return Ok(());
    }

    // Заголовок таблицы
    println!("{:<50} {:>12}", "Регион", "Размер");
    println!("{}", "-".repeat(64));

    for r in &regions {
        let size_str = if r.size_mb >= 1024.0 {
            format!("{:.1} GB", r.size_mb / 1024.0)
        } else {
            format!("{:.0} MB", r.size_mb)
        };
        println!("{:<50} {:>12}", r.name, size_str);
    }

    println!("\nДля скачивания: osm-geo build --input {}", 
        regions.first().map(|r| r.subpath.as_str()).unwrap_or("REGION"));
    Ok(())
}
