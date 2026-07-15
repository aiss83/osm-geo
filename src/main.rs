//! CLI osm-geo — ETL-пайплайн для подготовки офлайн-базы геокодирования.
//!
//! Основные команды:
//!   build   — сборка базы для региона
//!   query   — тестовый поиск в собранной базе
//!   info    — вывод метаданных о базе

mod compact;
mod corrector;
mod dedup;
mod downloader;
mod finalizer;
mod indexer;
mod model;
mod parser;
mod translit;

use anyhow::Result;
use clap::{Parser, Subcommand};
use log::info;
use rusqlite::Connection;
use std::io::Write;
use std::path::PathBuf;


#[derive(Parser)]
#[command(name = "osm-geo")]
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
    /// Сборка базы данных для региона
    Build {
        /// PBF-файл (локальный путь) или регион Geofabrik (напр. russia/central-fed-district)
        #[arg(short, long)]
        input: String,

        /// Путь к выходному файлу (по умолчанию: {stem}.db или {stem}.bin)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Код региона (напр. RU-CFD)
        #[arg(short, long)]
        region: Option<String>,

        /// Формат вывода: sqlite (по умолчанию) или compact
        #[arg(short, long, default_value = "sqlite")]
        format: String,
    },

    /// Тестовый поиск в собранной базе
    Query {
        /// Путь к SQLite-базе
        #[arg(short, long)]
        db: PathBuf,

        /// Поисковый запрос
        query: String,
    },

    /// Вывод метаданных о собранной базе
    Info {
        /// Путь к SQLite-базе
        #[arg(short, long)]
        db: PathBuf,
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
            region,
            format,
        } => cmd_build(&input, output.as_ref(), region.as_deref(), &format),
        Commands::Query { db, query } => cmd_query(&db, &query),
        Commands::Info { db } => cmd_info(&db),
        Commands::List { region } => cmd_list(region.as_deref()),
    }
}

/// Команда build: парсинг PBF → запись в SQLite или компактный бинарный формат.
fn cmd_build(
    input: &str,
    output: Option<&PathBuf>,
    region: Option<&str>,
    format: &str,
) -> Result<()> {
    info!("=== Сборка базы данных ===");

    // 1. Разрешаем входной файл
    let input_path = resolve_input(input)?;
    info!("Входной файл: {:?}", input_path);

    // 2. Определяем выходной файл
    let default_ext = if format == "compact" { "bin" } else { "db" };
    let output_path = match output {
        Some(p) => p.to_path_buf(),
        None => {
            let stem = downloader::derive_output_stem(&input_path);
            PathBuf::from(format!("{}.{}", stem, default_ext))
        }
    };
    info!("Выходной файл: {:?}", output_path);

    // 3. Парсинг PBF
    let corrector = corrector::Corrector::new_or_download()
        .map_err(|e| {
            log::warn!("Не удалось загрузить корректор опечаток: {} (продолжаем без коррекции)", e);
            e
        })
        .ok();
    let mut parser = parser::PbfParser::new();
    if let Some(c) = corrector {
        parser = parser.with_corrector(c);
    }
    let mut objects = parser.parse_file(&input_path)?;
    info!("Извлечено {} объектов", objects.len());

    // 4. Привязка адресов без города к ближайшему городу
    parser::infer_missing_cities(&mut objects);

    // 4b. Склеивание городов-опечаток с каноническими названиями
    parser::merge_typo_cities(&mut objects);

    // 5. Дедупликация
    let objects = dedup::deduplicate(objects);
    let addr_count = objects.iter().filter(|o| o.as_address().is_some()).count();
    let named_count = objects.iter().filter(|o| o.as_named().is_some()).count();
    let count = objects.len() as u64;

    info!(
        "После дедупликации: {} объектов (адресов: {}, POI: {})",
        count, addr_count, named_count
    );

    // 6. Запись в выбранном формате
    let file_size = if format == "compact" {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut writer = compact::CompactWriter::new();
        writer.build(&objects, &output_path, region.unwrap_or("unknown"), timestamp)?;
        std::fs::metadata(&output_path)?.len()
    } else {
        let mut idx =
            indexer::Indexer::create(&output_path)?.with_progress(count);
        if let Some(region) = region {
            idx.set_meta("region", region)?;
        }
        idx.set_meta("source", &input_path.display().to_string())?;
        idx.set_meta("build_date", &chrono_now())?;
        idx.set_meta("version", env!("CARGO_PKG_VERSION"))?;
        for chunk in objects.chunks(10_000) {
            idx.insert_batch(chunk)?;
        }
        idx.set_meta("object_count", &count.to_string())?;
        idx.set_meta("addr_count", &addr_count.to_string())?;
        idx.set_meta("named_count", &named_count.to_string())?;
        idx.finalize()?;
        indexer::Indexer::db_size(&output_path)?
    };

    info!("=== Сборка завершена ===");
    info!("Объектов: {}", count);
    info!(
        "Размер файла: {:.2} МБ",
        file_size as f64 / (1024.0 * 1024.0)
    );

    // 7. Сжатие и метаданные (для обоих форматов)
    let output_dir = output_path.parent().unwrap_or(std::path::Path::new("."));
    let mut metadata = finalizer::Metadata {
        version: env!("CARGO_PKG_VERSION").to_string(),
        region: region.unwrap_or("unknown").to_string(),
        build_date: chrono_now(),
        source_pbf: input_path.display().to_string(),
        object_count: count,
        address_count: addr_count as u64,
        named_count: named_count as u64,
        db_size_bytes: file_size,
        compressed_size_bytes: None,
        sha256: String::new(), // будет заполнен в compress_and_export_metadata
    };

    let (compressed_size, sha256, _meta_json) =
        finalizer::compress_and_export_metadata(&output_path, output_dir, &mut metadata)?;

    info!("SHA-256: {}", sha256);
    info!(
        "Сжатый размер: {:.2} МБ",
        compressed_size as f64 / (1024.0 * 1024.0)
    );

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

/// Команда query: тестовый поиск в базе.
fn cmd_query(db: &PathBuf, query: &str) -> Result<()> {
    use rusqlite::Connection;
    use rust_stemmers::{Algorithm, Stemmer};

    let conn = Connection::open(db)?;

    // Применяем русский стеммер к запросу
    let stemmer = Stemmer::create(Algorithm::Russian);
    let stemmed: String = query
        .split_whitespace()
        .map(|w| {
            let lower = w.to_lowercase();
            format!("{}*", stemmer.stem(&lower))
        })
        .collect::<Vec<_>>()
        .join(" ");

    println!("Запрос:        {}", query);
    println!("Стемминг:      {}", stemmed);
    println!();

    // Поиск по адресам
    println!("=== Адреса ===");
    match search_fts(&conn, "fts_address", &stemmed) {
        Ok(results) if !results.is_empty() => {
            for (id, _) in results.iter().take(10) {
                print_object(&conn, *id)?;
            }
        }
        _ => println!("  (ничего не найдено)"),
    }

    // Поиск по именованным объектам
    println!("\n=== Объекты ===");
    match search_fts(&conn, "fts_named", &stemmed) {
        Ok(results) if !results.is_empty() => {
            for (id, _) in results.iter().take(10) {
                print_object(&conn, *id)?;
            }
        }
        _ => println!("  (ничего не найдено)"),
    }

    Ok(())
}

/// Поиск в FTS-таблице.
fn search_fts(
    conn: &Connection,
    table: &str,
    query: &str,
) -> Result<Vec<(i64, f64)>> {
    let sql = format!(
        "SELECT rowid, rank FROM {} WHERE {} MATCH ?1 ORDER BY rank LIMIT 20",
        table, table
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([query], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Вывести объект по id.
fn print_object(conn: &Connection, id: i64) -> Result<()> {
    let mut stmt =
        conn.prepare("SELECT type, lat, lon,
            country, city, street, housenumber, postcode,
            name, translit, category
            FROM objects WHERE id = ?1")?;

    let row = stmt.query_row([id], |row| {
        Ok((
            row.get::<_, u8>(0)?,
            row.get::<_, f64>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, Option<String>>(9)?,
            row.get::<_, Option<String>>(10)?,
        ))
    })?;

    let (obj_type, lat, lon,
        country, city, street, housenumber, _postcode,
        name, _translit, category) = row;

    match obj_type {
        0 => {
            let parts: Vec<&str> = [country.as_deref(), city.as_deref(),
                street.as_deref(), housenumber.as_deref()]
                .into_iter()
                .flatten()
                .collect();
            println!("  [Адрес] [{}, {}] {}", lat, lon, parts.join(", "));
        }
        1 => {
            println!("  [Объект] [{}, {}] {} ({})",
                lat, lon,
                name.as_deref().unwrap_or("?"),
                category.as_deref().unwrap_or("без категории"));
        }
        _ => println!("  [?] id={}", id),
    }

    Ok(())
}

/// Команда info: вывод метаданных о базе.
fn cmd_info(db: &PathBuf) -> Result<()> {
    use rusqlite::Connection;

    let conn = Connection::open(db)?;

    println!("База данных: {:?}", db);

    // Размер файла
    let size = std::fs::metadata(db)?.len();
    println!("Размер:       {:.2} МБ", size as f64 / (1024.0 * 1024.0));

    // Метаданные
    println!("\nМетаданные:");
    let mut stmt = conn.prepare("SELECT key, value FROM meta ORDER BY key")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (key, value) = row?;
        println!("  {}: {}", key, value);
    }

    // Статистика
    println!("\nСтатистика:");
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM objects",
        [],
        |row| row.get(0),
    )?;
    println!("  Всего объектов: {}", count);

    let addr_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM objects WHERE type = 0",
        [],
        |row| row.get(0),
    )?;
    println!("  Адресов:        {}", addr_count);

    let named_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM objects WHERE type = 1",
        [],
        |row| row.get(0),
    )?;
    println!("  Объектов (POI): {}", named_count);

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

/// Текущая дата в ISO-формате.
fn chrono_now() -> String {
    // Простая альтернатива chrono: используем std::time
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();

    // Приблизительное преобразование unixtime → YYYY-MM-DD
    let days = secs / 86400;
    // 1970-01-01 + days
    let (y, m, d) = days_to_date(days as i64);
    format!("{:04}-{:02}-{:02}", y, m, d)
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

    let month_days = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut m = 1u32;
    for &md in &month_days {
        if remaining < md as i64 {
            break;
        }
        remaining -= md as i64;
        m += 1;
    }

    let d = (remaining + 1) as u32;
    (y, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}