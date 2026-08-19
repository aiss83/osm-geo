//! Интеграция с GeoDesk GOL.
//!
//! - Этап 1: конвертация OSM PBF → GOL через утилиту `gol` ([`GolTool`]).
//! - Этап 3: чтение GOL как источника гео-объектов ([`GolSource`]) через
//!   `gol query -f pbf` + повторное использование PBF-парсера.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use log::info;

use crate::corrector::Corrector;
use crate::model::{Country, CountryBoundary, GeoObject};
use crate::source::FeatureSource;

const GOL_VERSION: &str = "2.3.2";
const GOL_BASE_URL: &str = "https://github.com/clarisma/geodesk-gol/releases/download";

/// GOQL для адресных фич.
///
/// В gol 2.3.2 есть баги движка запросов: смешивание ключей `addr:*` с другими
/// ключами в одном multi-selector-запросе даёт неверный результат, а унарный
/// тест `[addr:housenumber]` занижает результат для тегов с числовыми значениями.
/// Поэтому адреса выбираются отдельным запросом, а его PBF-вывод объединяется
/// с запросом POI/мест (см. [`GolSource::parse`]).
const GOL_ADDR_QUERY: &str = "*[addr:street], *[addr:city], *[addr:town], *[addr:place]";

/// GOQL для POI и типов мест. `place`/`landuse=allotments` нужны для
/// подстановки улиц у адресов без `addr:street`.
const GOL_POI_QUERY: &str = "*[historic], *[tourism], *[place], *[landuse=allotments]";

/// Обёртка над self-contained исполняемым файлом `gol` (GeoDesk GOL Tool).
pub struct GolTool {
    binary: PathBuf,
}

impl GolTool {
    /// Создать обёртку для конкретного бинаря.
    pub fn new(binary: PathBuf) -> Self {
        Self { binary }
    }

    /// Найти `gol` в PATH или в локальном кэше; при отсутствии — скачать и
    /// распаковать платформенную сборку.
    pub fn find_or_install() -> Result<Self> {
        let name = binary_name();

        // 1. Локальный бинарь в каталоге gol/ проекта (если пользователь положил его сам)
        let local = Path::new("gol").join(name);
        if local.is_file() {
            return Ok(Self { binary: local });
        }

        // 2. В PATH
        if let Some(path) = find_in_path(name) {
            return Ok(Self { binary: path });
        }

        let cache_dir = cache_dir();
        let cached = cache_dir.join(name);
        if cached.is_file() {
            return Ok(Self { binary: cached });
        }

        Self::install(&cache_dir)
    }

    /// Скачать и распаковать `gol` в кэш-директорию.
    fn install(cache_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(cache_dir).context("Создание каталога инструментов")?;

        let url = download_url()?;
        let archive = cache_dir.join(format!("gol-{}.zip", GOL_VERSION));

        info!("Скачивание GOL Tool {}: {}", GOL_VERSION, url);
        download(&url, &archive)?;

        info!("Распаковка {:?}", archive);
        extract_zip(&archive, cache_dir)?;

        let binary = find_executable(cache_dir, binary_name()).with_context(|| {
            format!(
                "Не найден бинарь '{}' после распаковки. \
                 Установите GOL Tool вручную (https://www.geodesk.com/download).",
                binary_name()
            )
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&binary)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&binary, perms)?;
        }

        Ok(Self { binary })
    }

    /// Конвертировать `pbf` → `gol`.
    ///
    /// Используется `--waynode-ids` (`-w`), чтобы сохранить ноды геометрии
    /// way/relation — это необходимо для корректного расчёта центроидов при
    /// последующем чтении GOL через `gol query -f pbf`.
    pub fn build(&self, pbf: &Path, gol: &Path) -> Result<()> {
        info!(
            "Конвертация {} → {} (gol build)",
            pbf.display(),
            gol.display()
        );

        let status = Command::new(&self.binary)
            .arg("build")
            .arg("--waynode-ids")
            .arg(gol)
            .arg(pbf)
            .status()
            .with_context(|| format!("Запуск '{}'", self.binary.display()))?;

        if !status.success() {
            bail!("gol build завершился с ошибкой (статус {:?})", status.code());
        }
        Ok(())
    }

    /// Выгрузить результаты GOQL-запроса из GOL в формате OSM PBF.
    ///
    /// Используется как «путь A» из плана: экспортируем нужные фичи в PBF и
    /// отдаём их существующему PBF-парсеру. Для полной геометрии way/relation
    /// GOL должен быть собран с `--waynode-ids` (это делает [`Self::build`]).
    pub fn query_to_pbf(&self, gol: &Path, query: &str, out: &Path) -> Result<()> {
        info!("gol query {} '{}' -f pbf", gol.display(), query);

        let output = File::create(out)
            .with_context(|| format!("Создание файла {}", out.display()))?;

        let status = Command::new(&self.binary)
            .arg("query")
            .arg(gol)
            .arg(query)
            .arg("-f")
            .arg("pbf")
            .stdout(Stdio::from(output))
            .stderr(Stdio::inherit())
            .status()
            .with_context(|| format!("Запуск '{}'", self.binary.display()))?;

        if !status.success() {
            bail!("gol query завершился с ошибкой (статус {:?})", status.code());
        }
        Ok(())
    }

    /// Выгрузить результаты GOQL-запроса из GOL в формате GeoJSON (stdout).
    pub fn query_geojson(&self, gol: &Path, query: &str) -> Result<String> {
        let output = Command::new(&self.binary)
            .arg("query")
            .arg(gol)
            .arg(query)
            .arg("-f")
            .arg("geojson")
            .output()
            .with_context(|| format!("Запуск '{}'", self.binary.display()))?;

        if !output.status.success() {
            bail!(
                "gol query завершился с ошибкой (статус {:?})",
                output.status.code()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// Источник гео-объектов из GOL.
///
/// Экспортирует GOL во временный PBF (`gol query '*' -f pbf`) и повторно
/// использует PBF-парсер, поэтому извлечение адресов/POI, сбор типов мест и
/// расчёт центроидов полностью совпадают с прямым разбором PBF.
pub struct GolSource {
    corrector: Option<Corrector>,
    country: Option<Country>,
    boundary: Option<CountryBoundary>,
}

impl GolSource {
    pub fn new() -> Self {
        Self {
            corrector: None,
            country: None,
            boundary: None,
        }
    }
}

impl FeatureSource for GolSource {
    fn set_corrector(&mut self, corrector: Option<Corrector>) {
        self.corrector = corrector;
    }

    fn parse(&mut self, path: &Path) -> Result<Vec<GeoObject>> {
        let tool = GolTool::find_or_install()?;
        let temp = temp_pbf_path(path, "addr");
        let temp_poi = temp_pbf_path(path, "poi");

        // Два отдельных запроса из-за багов multi-selector в gol 2.3.2,
        // затем объединяем PBF-потоки: PBF — последовательность независимых
        // блоков, поэтому конкатенация даёт валидный файл.
        tool.query_to_pbf(path, GOL_ADDR_QUERY, &temp)?;
        tool.query_to_pbf(path, GOL_POI_QUERY, &temp_poi)?;
        append_file(&temp_poi, &temp)?;

        let mut parser = crate::parser::PbfParser::new();
        parser.set_corrector(self.corrector.take());
        let result = parser.parse_file(&temp);

        self.country = parser.country();
        self.boundary = parser.boundary();

        // Граница страны из GOL (temp-PBF запросов не содержит admin_level=2).
        if self.boundary.is_none()
            && let Some(c) = &self.country
            && !c.code.is_empty() {
                let query = format!("*[\"ISO3166-1:alpha2\"={}]", c.code);
                if let Ok(json) = tool.query_geojson(path, &query) {
                    self.boundary = crate::boundary::parse_geojson_boundary(&json);
                }
            }

        // Временные PBF больше не нужны; ошибки удаления игнорируем.
        let _ = std::fs::remove_file(&temp);
        let _ = std::fs::remove_file(&temp_poi);

        result
    }

    fn country(&self) -> Option<Country> {
        self.country.clone()
    }

    fn boundary(&self) -> Option<CountryBoundary> {
        self.boundary.clone()
    }
}

// ---- вспомогательные функции ----

fn binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "gol.exe"
    } else {
        "gol"
    }
}

fn cache_dir() -> PathBuf {
    Path::new("data").join("tools")
}

/// Временный PBF-файл для экспорта GOL (уникальный на процесс).
fn temp_pbf_path(gol: &Path, tag: &str) -> PathBuf {
    let stem = gol
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("gol");
    let name = format!("osm-geo-{}-{}-{}.osm.pbf", stem, std::process::id(), tag);
    std::env::temp_dir().join(name)
}

/// Дописать содержимое `src` в конец `dst`.
fn append_file(src: &Path, dst: &Path) -> Result<()> {
    let mut out = std::fs::OpenOptions::new()
        .append(true)
        .open(dst)
        .with_context(|| format!("Открытие {}", dst.display()))?;
    let mut inp = File::open(src).with_context(|| format!("Открытие {}", src.display()))?;
    std::io::copy(&mut inp, &mut out).context("Объединение PBF")?;
    Ok(())
}

fn download_url() -> Result<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let asset = match (os, arch) {
        ("macos", _) => format!("gol-{}-macos.zip", GOL_VERSION),
        ("linux", "x86_64") => format!("gol-{}-linux.zip", GOL_VERSION),
        _ => bail!(
            "Автоустановка GOL Tool не поддерживается для {}/{}. \
             Скачайте вручную с https://www.geodesk.com/download и положите в {}",
            os,
            arch,
            cache_dir().display()
        ),
    };

    Ok(format!("{}/v{}/{}", GOL_BASE_URL, GOL_VERSION, asset))
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Рекурсивно найти исполняемый файл `name` внутри `dir`.
fn find_executable(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
                return Some(path);
            }
        }
    }
    None
}

fn download(url: &str, dest: &Path) -> Result<()> {
    let mut resp = reqwest::blocking::get(url).context("Запрос на скачивание")?;
    if !resp.status().is_success() {
        bail!("Скачивание не удалось: HTTP {}", resp.status());
    }

    let mut file = File::create(dest).context("Создание файла")?;
    let mut buf = [0u8; 65536];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
    }
    Ok(())
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive).context("Открытие архива gol")?;
    let mut zip = zip::ZipArchive::new(file).context("Чтение zip-архива gol")?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).context("Чтение записи архива")?;
        // `enclosed_name` защищает от path-traversal (например, от `../`).
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let out_path = dest.join(relative);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).context("Создание каталога")?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).context("Создание каталога")?;
            }
            let mut out = File::create(&out_path).context("Создание файла")?;
            std::io::copy(&mut entry, &mut out).context("Распаковка файла")?;
        }
    }
    Ok(())
}
