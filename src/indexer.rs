//! Модуль индексации: запись GeoObject в SQLite-базу с FTS5 и B-tree.
//!
//! Все поля хранятся в плоских колонках (без BLOB).
//! FTS5 — contentless таблицы с пре-стеммингом на стороне сборщика.

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use log::info;
use rusqlite::{params, Connection};
use rust_stemmers::{Algorithm, Stemmer};
use std::path::Path;

use crate::model::GeoObject;

pub struct Indexer {
    conn: Connection,
    stemmer: Stemmer,
    count: u64,
    progress: Option<ProgressBar>,
}

impl Indexer {
    pub fn create(path: &Path) -> Result<Self> {
        info!("Создание базы данных: {:?}", path);

        if path.exists() {
            std::fs::remove_file(path)?;
        }

        let conn = Connection::open(path)?;

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA mmap_size = 268435456;
             PRAGMA cache_size = -65536;
             PRAGMA page_size = 4096;
             PRAGMA auto_vacuum = INCREMENTAL;",
        )?;

        let stemmer = Stemmer::create(Algorithm::Russian);

        let indexer = Self {
            conn,
            stemmer,
            count: 0,
            progress: None,
        };

        indexer.init_schema()?;
        Ok(indexer)
    }

    pub fn with_progress(mut self, total: u64) -> Self {
        let pb = ProgressBar::new(total);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
                )
                .unwrap()
                .progress_chars("##-"),
        );
        self.progress = Some(pb);
        self
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS objects (
                id          INTEGER PRIMARY KEY,
                type        INTEGER NOT NULL,   -- 0 = Address, 1 = NamedObject
                lat         REAL    NOT NULL,
                lon         REAL    NOT NULL,
                -- Address
                country      TEXT,
                city         TEXT,
                street       TEXT,
                housenumber  TEXT,
                postcode     TEXT,
                -- NamedObject
                name         TEXT,
                translit     TEXT,
                category     TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_objects_lat_lon ON objects(lat, lon);

            CREATE VIRTUAL TABLE IF NOT EXISTS fts_named USING fts5(
                name, translit, category,
                tokenize = 'unicode61',
                content  = '',
                prefix   = '2 3 4'
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS fts_address USING fts5(
                country, city, street, housenumber, postcode,
                tokenize = 'unicode61',
                content  = '',
                prefix   = '2 3 4'
            );

            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;
        Ok(())
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.prepare_cached(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        )?.execute(params![key, value])?;
        Ok(())
    }

    pub fn insert_batch(&mut self, objects: &[GeoObject]) -> Result<()> {
        self.conn.execute("BEGIN", [])?;

        for obj in objects {
            self.insert_one(obj)?;
        }

        self.conn.execute("COMMIT", [])?;

        if let Some(ref pb) = self.progress {
            pb.inc(objects.len() as u64);
        }

        Ok(())
    }

    fn insert_one(&mut self, obj: &GeoObject) -> Result<()> {
        let (lat, lon) = obj.lat_lon();
        let obj_type = obj.object_type() as u8;

        match obj {
            GeoObject::Address(addr) => {
                self.conn.prepare_cached(
                    "INSERT INTO objects (type, lat, lon,
                        country, city, street, housenumber, postcode)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )?.execute(params![
                    obj_type, lat, lon,
                    addr.country.as_deref(),
                    addr.city.as_deref(),
                    addr.street.as_deref(),
                    addr.housenumber.as_deref(),
                    addr.postcode.as_deref(),
                ])?;

                let id = self.conn.last_insert_rowid();

                let country = self.stem_text(addr.country.as_deref().unwrap_or(""));
                let city = self.stem_text(addr.city.as_deref().unwrap_or(""));
                let street = self.stem_text(addr.street.as_deref().unwrap_or(""));
                let housenumber = addr.housenumber.as_deref().unwrap_or("");
                let postcode = addr.postcode.as_deref().unwrap_or("");

                self.conn.prepare_cached(
                    "INSERT INTO fts_address (rowid, country, city, street, housenumber, postcode)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?.execute(params![id, country, city, street, housenumber, postcode])?;
            }
            GeoObject::Named(obj) => {
                self.conn.prepare_cached(
                    "INSERT INTO objects (type, lat, lon,
                        name, translit, category)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?.execute(params![
                    obj_type, lat, lon,
                    obj.name.as_str(),
                    obj.translit.as_deref(),
                    obj.category.as_deref(),
                ])?;

                let id = self.conn.last_insert_rowid();

                let name = self.stem_text(&obj.name);
                let translit = obj
                    .translit
                    .as_deref()
                    .map(|t| self.stem_text(t))
                    .unwrap_or_default();
                let category = obj.category.as_deref().unwrap_or("");

                self.conn.prepare_cached(
                    "INSERT INTO fts_named (rowid, name, translit, category)
                     VALUES (?1, ?2, ?3, ?4)",
                )?.execute(params![id, name, translit, category])?;
            }
        }

        self.count += 1;
        Ok(())
    }

    fn stem_text(&self, text: &str) -> String {
        text.split_whitespace()
            .map(|word| {
                let lower = word.to_lowercase();
                self.stemmer.stem(&lower).to_string()
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn finalize(&self) -> Result<()> {
        if let Some(ref pb) = self.progress {
            pb.finish_with_message("Индексация завершена");
        }

        info!("Оптимизация базы данных...");
        self.conn.execute("ANALYZE", [])?;

        self.conn.execute(
            "INSERT INTO fts_named(fts_named) VALUES('optimize')",
            [],
        )?;
        self.conn.execute(
            "INSERT INTO fts_address(fts_address) VALUES('optimize')",
            [],
        )?;

        info!("Дефрагментация (incremental_vacuum)...");
        self.conn.execute("PRAGMA optimize", [])?;
        self.conn.execute("PRAGMA incremental_vacuum", [])?;

        info!("Оптимизация завершена");
        Ok(())
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn db_size(path: &Path) -> Result<u64> {
        let meta = std::fs::metadata(path)?;
        Ok(meta.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Address, NamedObject};

    fn test_db_path() -> std::path::PathBuf {
        std::path::PathBuf::from("/tmp/test_osm_geo.db")
    }

    fn make_address() -> GeoObject {
        GeoObject::Address(Address {
            country: Some("Россия".into()),
            city: Some("Москва".into()),
            street: Some("Тверская".into()),
            housenumber: Some("1".into()),
            postcode: Some("125009".into()),
            lat: 55.7558,
            lon: 37.6173,
        })
    }

    fn make_named() -> GeoObject {
        GeoObject::Named(NamedObject {
            name: "Красная площадь".into(),
            translit: Some("Krasnaya ploshchad".into()),
            category: Some("tourism".into()),
            lat: 55.7539,
            lon: 37.6208,
        })
    }

    #[test]
    fn test_create_and_insert() {
        let path = test_db_path();
        let mut indexer = Indexer::create(&path).unwrap();

        indexer.insert_batch(&[make_address(), make_named()]).unwrap();
        indexer.set_meta("region", "RU-MOW").unwrap();
        indexer.set_meta("version", "0.1.0").unwrap();
        indexer.finalize().unwrap();

        assert!(path.exists());
        let size = Indexer::db_size(&path).unwrap();
        assert!(size > 0);

        let conn = Connection::open(&path).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_address WHERE fts_address MATCH 'москв* тверск*'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_named WHERE fts_named MATCH 'красн* площад*'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let _ = std::fs::remove_file(&path);
    }
}
