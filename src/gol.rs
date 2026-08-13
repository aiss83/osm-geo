//! Интеграция с GeoDesk GOL.
//!
//! - Этап 1: конвертация OSM PBF → GOL через утилиту `gol` ([`GolTool`]).
//! - Этап 2: заглушка источника [`GolSource`] (чтение GOL — на этапе 3).

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use log::info;

use crate::corrector::Corrector;
use crate::model::GeoObject;
use crate::source::FeatureSource;

const GOL_VERSION: &str = "2.3.2";
const GOL_BASE_URL: &str = "https://github.com/clarisma/geodesk-gol/releases/download";

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
}

/// Источник гео-объектов из GOL.
///
/// Чтение будет реализовано на этапе 3; сейчас это заглушка, чтобы
/// унифицированный интерфейс и автоопределение формата уже работали.
pub struct GolSource {
    corrector: Option<Corrector>,
}

impl GolSource {
    pub fn new() -> Self {
        Self { corrector: None }
    }
}

impl FeatureSource for GolSource {
    fn set_corrector(&mut self, corrector: Option<Corrector>) {
        self.corrector = corrector;
    }

    fn parse(&mut self, path: &Path) -> Result<Vec<GeoObject>> {
        let _ = self.corrector.as_ref();
        bail!(
            "Чтение GOL-файла ({:?}) будет реализовано на этапе 3",
            path
        );
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
