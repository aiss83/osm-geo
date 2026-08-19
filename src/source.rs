//! Абстракция источников гео-данных (PBF, GOL).
//!
//! Позволяет пайплайну геокодирования работать с разными форматами
//! через единый интерфейс [`FeatureSource`].

use std::path::Path;

use anyhow::Result;

use crate::corrector::Corrector;
use crate::model::{Country, CountryBoundary, GeoObject};

/// Вид источника данных.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// OSM PBF (текущий формат).
    Pbf,
    /// GeoDesk GOL (чтение реализуется на этапе 3).
    Gol,
}

/// Единый интерфейс источника гео-объектов.
pub trait FeatureSource {
    /// Установить корректор опечаток. `None` — без коррекции.
    fn set_corrector(&mut self, corrector: Option<Corrector>);

    /// Разобрать файл и вернуть извлечённые объекты (адреса и POI).
    fn parse(&mut self, path: &Path) -> Result<Vec<GeoObject>>;

    /// Страна, определённая при разборе (если удалось).
    fn country(&self) -> Option<Country> {
        None
    }

    /// Граница страны, извлечённая при разборе (если удалось).
    fn boundary(&self) -> Option<CountryBoundary> {
        None
    }
}

/// Определить вид источника по расширению файла.
pub fn detect_source(path: &Path) -> SourceKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    match ext.as_deref() {
        Some("gol") => SourceKind::Gol,
        _ => SourceKind::Pbf,
    }
}
