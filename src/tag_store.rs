//! SQLite sidecar for instrument labels the audio container itself cannot hold.
//!
//! Native container tags stay the primary store because they travel with the
//! file. This store is a fallback: written only when a native write fails (a
//! read-only sample library, an unsupported container layout), and read only
//! when no native tag is present. Search consults it through the same
//! `instrument` lookup as everything else, so `instrument:Kick` matches
//! sidecar-tagged files identically to natively tagged ones.
//!
//! Lookups happen once per file during a scan, so the table is mirrored in an
//! in-memory map. The map is small: it only ever holds files whose container
//! refused a write.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use rusqlite::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidecarTag {
    instrument: String,
    tag_version: u32,
}

/// Canonical map key. Same as metadata/dir cache keys so `\\?\` and case match.
fn key(path: &Path) -> PathBuf {
    crate::path_util::cache_key(path.to_path_buf())
}

fn db_path() -> Option<PathBuf> {
    let mut dir = dirs::cache_dir()?;
    dir.push("tundra");
    std::fs::create_dir_all(&dir).ok()?;
    dir.push("tags.db");
    Some(dir)
}

fn open() -> Result<Connection, String> {
    let path = db_path().ok_or_else(|| "No cache directory available".to_string())?;
    let connection = Connection::open(&path)
        .map_err(|err| format!("Failed to open {}: {err}", path.display()))?;
    prepare_schema(&connection)?;
    Ok(connection)
}

fn prepare_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS instrument_tags (
                 path TEXT PRIMARY KEY,
                 instrument TEXT NOT NULL,
                 tag_version INTEGER NOT NULL DEFAULT 0
             );",
        )
        .map_err(|err| format!("Failed to prepare tag store: {err}"))?;
    if !has_column(connection, "instrument_tags", "tag_version")? {
        connection
            .execute(
                "ALTER TABLE instrument_tags ADD COLUMN tag_version INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|err| format!("Failed to migrate tag store: {err}"))?;
    }
    Ok(())
}

fn has_column(connection: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|err| format!("Failed to inspect {table}: {err}"))?;
    let mut rows = statement
        .query([])
        .map_err(|err| format!("Failed to read {table} schema: {err}"))?;
    while let Some(row) = rows
        .next()
        .map_err(|err| format!("Failed to read {table} schema: {err}"))?
    {
        let name: String = row
            .get(1)
            .map_err(|err| format!("Failed to read {table} column name: {err}"))?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

type Cache = RwLock<HashMap<PathBuf, SidecarTag>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(load_all().unwrap_or_default()))
}

fn load_all() -> Result<HashMap<PathBuf, SidecarTag>, String> {
    // Most libraries never need the fallback, so don't create a database until
    // something is actually written to it.
    if !db_path().is_some_and(|path| path.exists()) {
        return Ok(HashMap::new());
    }
    let connection = open()?;
    let mut statement = connection
        .prepare("SELECT path, instrument, tag_version FROM instrument_tags")
        .map_err(|err| format!("Failed to query tag store: {err}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u32>(2)?,
            ))
        })
        .map_err(|err| format!("Failed to read tag store: {err}"))?;
    Ok(rows
        .filter_map(Result::ok)
        .map(|(path, instrument, tag_version)| {
            (
                key(Path::new(&path)),
                SidecarTag {
                    instrument,
                    tag_version,
                },
            )
        })
        .collect())
}

fn cached(path: &Path) -> Option<SidecarTag> {
    let cache = cache()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.is_empty() {
        return None;
    }
    cache.get(&key(path)).cloned()
}

/// Instrument recorded for `path`, if the container could not hold one.
///
/// A poisoned lock is recovered rather than propagated: the map is a plain
/// cache, so a panic elsewhere must not silently disable the fallback.
pub fn instrument(path: &Path) -> Option<String> {
    cached(path).map(|entry| entry.instrument)
}

/// Tundra tag version recorded for `path` in the sidecar store.
pub fn tag_version(path: &Path) -> Option<u32> {
    cached(path).map(|entry| entry.tag_version)
}

/// Records `instrument` for `path`, replacing any previous entry.
pub fn set_instrument(path: &Path, instrument: &str, tag_version: u32) -> Result<(), String> {
    let instrument = instrument.trim();
    if instrument.is_empty() {
        return Err("Instrument label cannot be empty".into());
    }
    let key = key(path);
    let stored = key.to_string_lossy().into_owned();

    open()?
        .execute(
            "INSERT INTO instrument_tags (path, instrument, tag_version) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET
                 instrument = excluded.instrument,
                 tag_version = excluded.tag_version",
            (&stored, instrument, tag_version),
        )
        .map_err(|err| format!("Failed to save tag for {}: {err}", path.display()))?;

    cache()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            key,
            SidecarTag {
                instrument: instrument.to_string(),
                tag_version,
            },
        );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_instrument_and_version_through_sqlite() {
        let dir = std::env::temp_dir().join(format!(
            "tundra_tag_store_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db = dir.join("tags.db");
        let connection = Connection::open(&db).expect("open db");
        connection
            .execute_batch(
                "CREATE TABLE instrument_tags (
                     path TEXT PRIMARY KEY,
                     instrument TEXT NOT NULL,
                     tag_version INTEGER NOT NULL DEFAULT 0
                 );",
            )
            .expect("create table");
        connection
            .execute(
                "INSERT INTO instrument_tags (path, instrument, tag_version) VALUES (?1, ?2, ?3)",
                ("c:/samples/kick.wav", "Kick", 1_u32),
            )
            .expect("insert");

        let stored: (String, u32) = connection
            .query_row(
                "SELECT instrument, tag_version FROM instrument_tags WHERE path = ?1",
                ["c:/samples/kick.wav"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("select");
        assert_eq!(stored, ("Kick".to_string(), 1));

        drop(connection);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sidecar_key_matches_cache_key_including_verbatim_prefix() {
        let bare = Path::new(r"C:\Samples\kick.wav");
        let verbatim = Path::new(r"\\?\C:\Samples\kick.wav");
        assert_eq!(key(bare), key(verbatim));
        assert_eq!(key(bare), crate::path_util::cache_key(bare.to_path_buf()));
    }

    #[test]
    fn prepare_schema_adds_missing_tag_version_column() {
        let dir = std::env::temp_dir().join(format!(
            "tundra_tag_store_migrate_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db = dir.join("tags.db");
        let connection = Connection::open(&db).expect("open db");
        connection
            .execute_batch(
                "CREATE TABLE instrument_tags (
                     path TEXT PRIMARY KEY,
                     instrument TEXT NOT NULL
                 );",
            )
            .expect("create pre-version table");
        connection
            .execute(
                "INSERT INTO instrument_tags (path, instrument) VALUES (?1, ?2)",
                ("c:/samples/kick.wav", "Kick"),
            )
            .expect("insert");

        prepare_schema(&connection).expect("migrate");
        assert!(has_column(&connection, "instrument_tags", "tag_version").expect("column"));
        let version: u32 = connection
            .query_row(
                "SELECT tag_version FROM instrument_tags WHERE path = ?1",
                ["c:/samples/kick.wav"],
                |row| row.get(0),
            )
            .expect("select version");
        assert_eq!(version, 0);
        prepare_schema(&connection).expect("migrate is idempotent");

        drop(connection);
        let _ = std::fs::remove_dir_all(dir);
    }
}
