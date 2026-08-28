//! SQLite fallback when the audio container cannot hold tags.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::UNIX_EPOCH;

use rusqlite::Connection;

pub use crate::metadata::ManualTagEdits as SidecarManualFields;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidecarTag {
    instrument: String,
    artist: String,
    title: String,
    bpm: String,
    key: String,
    genre: String,
    comment: String,
    tag_version: u32,
    mtime_secs: u64,
    size: u64,
}

impl SidecarTag {
    fn manual_fields(&self) -> SidecarManualFields {
        SidecarManualFields {
            instrument: self.instrument.clone(),
            artist: self.artist.clone(),
            title: self.title.clone(),
            bpm: self.bpm.clone(),
            key: self.key.clone(),
            genre: self.genre.clone(),
            comment: self.comment.clone(),
        }
    }

    fn generic_empty(&self) -> bool {
        [
            &self.title,
            &self.artist,
            &self.bpm,
            &self.key,
            &self.genre,
            &self.comment,
        ]
        .iter()
        .all(|value| value.trim().is_empty())
    }
}

enum Store {
    Missing,
    Ready(HashMap<PathBuf, SidecarTag>),
    Failed(String),
}

/// Canonical map key. Same as metadata/dir cache keys so `\\?\` and case match.
fn key(path: &Path) -> PathBuf {
    crate::path_util::cache_key(path.to_path_buf())
}

fn file_stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime_secs = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((mtime_secs, meta.len()))
}

fn stamp_matches(path: &Path, entry: &SidecarTag) -> bool {
    file_stamp(path) == Some((entry.mtime_secs, entry.size))
}

fn cache_db_path() -> Option<PathBuf> {
    crate::path_util::cache_file("tags.db")
}

fn data_db_path() -> Option<PathBuf> {
    crate::path_util::tundra_data_dir().map(|mut dir| {
        dir.push("tags.db");
        dir
    })
}

fn migrate_sqlite_file(src: &Path, dest: &Path) {
    if dest.exists() || !src.exists() {
        return;
    }
    if let Some(parent) = dest.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    if std::fs::copy(src, dest).is_err() {
        return;
    }
    for suffix in ["-wal", "-shm"] {
        let src_side = crate::path_util::sidecar(src, suffix);
        if src_side.exists() {
            let _ = std::fs::copy(&src_side, crate::path_util::sidecar(dest, suffix));
        }
    }
    let _ = std::fs::remove_file(src);
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(crate::path_util::sidecar(src, suffix));
    }
}

fn db_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = test_db_path() {
        return Some(path);
    }
    let dest = data_db_path()?;
    if let Some(src) = cache_db_path() {
        migrate_sqlite_file(&src, &dest);
    }
    Some(dest)
}

#[cfg(test)]
use std::cell::RefCell;

#[cfg(test)]
thread_local! {
    static TEST_DB_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[cfg(test)]
fn test_db_path() -> Option<PathBuf> {
    TEST_DB_PATH.with(|slot| slot.borrow().clone())
}

#[cfg(test)]
static TAG_STORE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
fn reset_cache_for_tests() {
    let mut store = cache()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *store = Store::Missing;
}

/// Run `f` against an isolated SQLite sidecar database (tests only).
#[cfg(test)]
pub(crate) fn with_test_db<F, R>(path: PathBuf, f: F) -> R
where
    F: FnOnce() -> R,
{
    let _lock = TAG_STORE_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    TEST_DB_PATH.with(|slot| {
        *slot.borrow_mut() = Some(path);
        reset_cache_for_tests();
        let result = f();
        reset_cache_for_tests();
        *slot.borrow_mut() = None;
        result
    })
}

fn open() -> Result<Connection, String> {
    let path = db_path().ok_or_else(|| "No data directory available".to_string())?;
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
                 tag_version INTEGER NOT NULL DEFAULT 0,
                 mtime_secs INTEGER NOT NULL DEFAULT 0,
                 size INTEGER NOT NULL DEFAULT 0
             );",
        )
        .map_err(|err| format!("Failed to prepare tag store: {err}"))?;
    ensure_column(
        connection,
        "instrument_tags",
        "tag_version",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        connection,
        "instrument_tags",
        "mtime_secs",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(connection, "instrument_tags", "size", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(
        connection,
        "instrument_tags",
        "title",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        connection,
        "instrument_tags",
        "artist",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(connection, "instrument_tags", "bpm", "TEXT NOT NULL DEFAULT ''")?;
    ensure_column(connection, "instrument_tags", "key", "TEXT NOT NULL DEFAULT ''")?;
    ensure_column(
        connection,
        "instrument_tags",
        "genre",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        connection,
        "instrument_tags",
        "comment",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    Ok(())
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> Result<(), String> {
    if has_column(connection, table, column)? {
        return Ok(());
    }
    connection
        .execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"),
            [],
        )
        .map_err(|err| format!("Failed to migrate tag store: {err}"))?;
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

fn cache() -> &'static RwLock<Store> {
    static CACHE: OnceLock<RwLock<Store>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(init_store()))
}

fn init_store() -> Store {
    let Some(path) = db_path() else {
        let err = "No data directory available".to_string();
        eprintln!("tundra: failed to load tag store: {err}");
        return Store::Failed(err);
    };
    if !path.exists() {
        return Store::Missing;
    }
    match load_all() {
        Ok(map) => Store::Ready(map),
        Err(err) => {
            eprintln!("tundra: failed to load tag store: {err}");
            Store::Failed(err)
        }
    }
}

fn load_all() -> Result<HashMap<PathBuf, SidecarTag>, String> {
    // Most libraries never need the fallback, so don't create a database until
    // something is actually written to it.
    if !db_path().is_some_and(|path| path.exists()) {
        return Ok(HashMap::new());
    }
    let connection = open()?;
    let mut statement = connection
        .prepare(
            "SELECT path, instrument, tag_version, mtime_secs, size, title, artist, bpm, key, genre, comment
             FROM instrument_tags",
        )
        .map_err(|err| format!("Failed to query tag store: {err}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u32>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
            ))
        })
        .map_err(|err| format!("Failed to read tag store: {err}"))?;
    Ok(rows
        .filter_map(Result::ok)
        .map(
            |(
                path,
                instrument,
                tag_version,
                mtime_secs,
                size,
                title,
                artist,
                bpm,
                initial_key,
                genre,
                comment,
            )| {
            (
                key(Path::new(&path)),
                SidecarTag {
                    instrument,
                    artist,
                    title,
                    bpm,
                    key: initial_key,
                    genre,
                    comment,
                    tag_version,
                    mtime_secs: mtime_secs.max(0) as u64,
                    size: size.max(0) as u64,
                },
            )
        },
        )
        .collect())
}

fn cached(path: &Path) -> Option<SidecarTag> {
    let store = cache()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match &*store {
        Store::Ready(map) => map
            .get(&key(path))
            .cloned()
            .filter(|entry| stamp_matches(path, entry)),
        Store::Missing => None,
        Store::Failed(err) => {
            let _ = err;
            None
        }
    }
}

/// Instrument recorded for `path`, if the container could not hold one.
///
/// A poisoned lock is recovered rather than propagated: the map is a plain
/// cache, so a panic elsewhere must not silently disable the fallback.
pub fn instrument(path: &Path) -> Option<String> {
    cached(path).map(|entry| entry.instrument)
}

pub fn tag_version(path: &Path) -> Option<u32> {
    cached(path).map(|entry| entry.tag_version)
}

pub fn manual_fields(path: &Path) -> Option<SidecarManualFields> {
    cached(path).and_then(|entry| {
        let fields = entry.manual_fields();
        (!fields.is_empty()).then_some(fields)
    })
}

fn write_sidecar_entry(path: &Path, entry: SidecarTag) -> Result<(), String> {
    let key = key(path);
    let stored = key.to_string_lossy().into_owned();
    open()?
        .execute(
            "INSERT INTO instrument_tags (
                 path, instrument, tag_version, mtime_secs, size,
                 title, artist, bpm, key, genre, comment
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(path) DO UPDATE SET
                 instrument = excluded.instrument,
                 tag_version = excluded.tag_version,
                 mtime_secs = excluded.mtime_secs,
                 size = excluded.size,
                 title = excluded.title,
                 artist = excluded.artist,
                 bpm = excluded.bpm,
                 key = excluded.key,
                 genre = excluded.genre,
                 comment = excluded.comment",
            (
                &stored,
                &entry.instrument,
                entry.tag_version,
                entry.mtime_secs as i64,
                entry.size as i64,
                &entry.title,
                &entry.artist,
                &entry.bpm,
                &entry.key,
                &entry.genre,
                &entry.comment,
            ),
        )
        .map_err(|err| format!("Failed to save tag for {}: {err}", path.display()))?;

    let mut store = cache()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match &mut *store {
        Store::Ready(map) => {
            map.insert(key, entry);
        }
        Store::Missing | Store::Failed(_) => {
            let mut map = load_all().unwrap_or_default();
            map.insert(key, entry);
            *store = Store::Ready(map);
        }
    }
    Ok(())
}

fn merge_sidecar_entry(path: &Path, update: impl FnOnce(&mut SidecarTag)) -> Result<(), String> {
    let (mtime_secs, size) = file_stamp(path).unwrap_or((0, 0));
    let mut entry = cached(path).unwrap_or(SidecarTag {
        instrument: String::new(),
        artist: String::new(),
        title: String::new(),
        bpm: String::new(),
        key: String::new(),
        genre: String::new(),
        comment: String::new(),
        tag_version: 0,
        mtime_secs,
        size,
    });
    entry.mtime_secs = mtime_secs;
    entry.size = size;
    update(&mut entry);
    write_sidecar_entry(path, entry)
}

pub fn set_instrument(path: &Path, instrument: &str, tag_version: u32) -> Result<(), String> {
    let instrument = instrument.trim();
    if instrument.is_empty() {
        return Err("Instrument label cannot be empty".into());
    }
    merge_sidecar_entry(path, |entry| {
        entry.instrument = instrument.to_string();
        entry.tag_version = tag_version;
    })
}

pub fn set_manual_fields(
    path: &Path,
    fields: &SidecarManualFields,
    tag_version: u32,
) -> Result<(), String> {
    merge_sidecar_entry(path, |entry| {
        entry.instrument = fields.instrument.trim().to_string();
        entry.artist = fields.artist.trim().to_string();
        entry.title = fields.title.trim().to_string();
        entry.bpm = fields.bpm.trim().to_string();
        entry.key = fields.key.trim().to_string();
        entry.genre = fields.genre.trim().to_string();
        entry.comment = fields.comment.trim().to_string();
        entry.tag_version = tag_version;
    })
}

pub fn clear_manual_fields(path: &Path) -> Result<(), String> {
    let Some(entry) = cached(path) else {
        return Ok(());
    };
    if entry.instrument.trim().is_empty() && entry.generic_empty() {
        return remove_sidecar(path);
    }
    merge_sidecar_entry(path, |entry| {
        entry.artist.clear();
        entry.title.clear();
        entry.bpm.clear();
        entry.key.clear();
        entry.genre.clear();
        entry.comment.clear();
    })
}

pub fn remove_sidecar(path: &Path) -> Result<(), String> {
    let cache_key = key(path);
    let stored = cache_key.to_string_lossy().into_owned();
    if db_path().is_some_and(|path| path.exists()) {
        open()?.execute(
            "DELETE FROM instrument_tags WHERE path = ?1",
            [&stored],
        )
        .map_err(|err| format!("Failed to remove tag for {}: {err}", path.display()))?;
    }
    let mut store = cache()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Store::Ready(map) = &mut *store {
        map.remove(&cache_key);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_instrument_and_version_through_sqlite() {
        let scratch = crate::test_fixtures::ScratchDir::new("tundra_tag_store");
        let dir = scratch.path();
        let db = dir.join("tags.db");
        let connection = Connection::open(&db).expect("open db");
        connection
            .execute_batch(
                "CREATE TABLE instrument_tags (
                     path TEXT PRIMARY KEY,
                     instrument TEXT NOT NULL,
                     tag_version INTEGER NOT NULL DEFAULT 0,
                     mtime_secs INTEGER NOT NULL DEFAULT 0,
                     size INTEGER NOT NULL DEFAULT 0
                 );",
            )
            .expect("create table");
        connection
            .execute(
                "INSERT INTO instrument_tags (path, instrument, tag_version, mtime_secs, size)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                ("c:/samples/kick.wav", "Kick", 1_u32, 100_i64, 512_i64),
            )
            .expect("insert");

        let stored: (String, u32, i64, i64) = connection
            .query_row(
                "SELECT instrument, tag_version, mtime_secs, size FROM instrument_tags WHERE path = ?1",
                ["c:/samples/kick.wav"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("select");
        assert_eq!(stored, ("Kick".to_string(), 1, 100, 512));

        drop(connection);
    }

    #[test]
    fn sidecar_key_matches_cache_key_including_verbatim_prefix() {
        let bare = Path::new(r"C:\Samples\kick.wav");
        let verbatim = Path::new(r"\\?\C:\Samples\kick.wav");
        assert_eq!(key(bare), key(verbatim));
        assert_eq!(key(bare), crate::path_util::cache_key(bare.to_path_buf()));
    }

    #[test]
    fn prepare_schema_adds_missing_columns() {
        let scratch = crate::test_fixtures::ScratchDir::new("tundra_tag_store_migrate");
        let dir = scratch.path();
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
        assert!(has_column(&connection, "instrument_tags", "mtime_secs").expect("column"));
        assert!(has_column(&connection, "instrument_tags", "size").expect("column"));
        let version: u32 = connection
            .query_row(
                "SELECT tag_version FROM instrument_tags WHERE path = ?1",
                ["c:/samples/kick.wav"],
                |row| row.get(0),
            )
            .expect("select version");
        assert_eq!(version, 0);
        let stamp: (i64, i64) = connection
            .query_row(
                "SELECT mtime_secs, size FROM instrument_tags WHERE path = ?1",
                ["c:/samples/kick.wav"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("select stamp");
        assert_eq!(stamp, (0, 0));
        prepare_schema(&connection).expect("migrate is idempotent");

        drop(connection);
    }

    #[test]
    fn stamp_mismatch_or_missing_file_hides_sidecar() {
        let scratch = crate::test_fixtures::ScratchDir::new("tundra_tag_store_stamp");
        let dir = scratch.path();
        let audio = dir.join("kick.wav");
        std::fs::write(&audio, b"audio-v1").expect("write");
        let (mtime_secs, size) = file_stamp(&audio).expect("stamp");
        let entry = SidecarTag {
            instrument: "Kick".into(),
            artist: String::new(),
            title: String::new(),
            bpm: String::new(),
            key: String::new(),
            genre: String::new(),
            comment: String::new(),
            tag_version: 1,
            mtime_secs,
            size,
        };
        assert!(stamp_matches(&audio, &entry));

        std::fs::write(&audio, b"audio-v1-replaced").expect("replace");
        assert!(
            !stamp_matches(&audio, &entry),
            "recycled path with new contents must hide the old sidecar"
        );

        let stale = SidecarTag {
            instrument: "Kick".into(),
            artist: String::new(),
            title: String::new(),
            bpm: String::new(),
            key: String::new(),
            genre: String::new(),
            comment: String::new(),
            tag_version: 1,
            mtime_secs: 0,
            size: 0,
        };
        assert!(
            !stamp_matches(&audio, &stale),
            "pre-stamp rows must not match a real file"
        );

        let _ = std::fs::remove_file(&audio);
        assert!(!stamp_matches(&audio, &entry));
    }

    #[test]
    fn migrate_copies_db_and_wal_then_removes_source() {
        let scratch = crate::test_fixtures::ScratchDir::new("tundra_tag_store_cache_migrate");
        let dir = scratch.path();
        let src_dir = dir.join("cache");
        let dest_dir = dir.join("data");
        std::fs::create_dir_all(&src_dir).expect("cache dir");
        let src = src_dir.join("tags.db");
        let dest = dest_dir.join("tags.db");
        std::fs::write(&src, b"sqlite-main").expect("db");
        std::fs::write(crate::path_util::sidecar(&src, "-wal"), b"wal").expect("wal");
        std::fs::write(crate::path_util::sidecar(&src, "-shm"), b"shm").expect("shm");

        migrate_sqlite_file(&src, &dest);

        assert_eq!(std::fs::read(&dest).expect("dest"), b"sqlite-main");
        assert_eq!(
            std::fs::read(crate::path_util::sidecar(&dest, "-wal")).expect("wal"),
            b"wal"
        );
        assert_eq!(
            std::fs::read(crate::path_util::sidecar(&dest, "-shm")).expect("shm"),
            b"shm"
        );
        assert!(!src.exists());
        assert!(!crate::path_util::sidecar(&src, "-wal").exists());
        assert!(!crate::path_util::sidecar(&src, "-shm").exists());

        std::fs::write(&src, b"should-not-overwrite").expect("new cache");
        migrate_sqlite_file(&src, &dest);
        assert_eq!(std::fs::read(&dest).expect("kept dest"), b"sqlite-main");
    }

    #[test]
    fn migrate_noops_when_source_missing() {
        let scratch = crate::test_fixtures::ScratchDir::new("tundra_tag_store_migrate_missing");
        let dir = scratch.path();
        let src = dir.join("missing.db");
        let dest = dir.join("dest.db");

        migrate_sqlite_file(&src, &dest);

        assert!(!dest.exists());
    }

    #[test]
    fn set_instrument_persists_stamp_and_round_trips_through_cache() {
        let scratch = crate::test_fixtures::ScratchDir::new("tundra_tag_store_set");
        let dir = scratch.path();
        let db = dir.join("tags.db");
        let audio = dir.join("kick.wav");
        std::fs::write(&audio, b"audio-v1-bytes").expect("write");
        with_test_db(db, || {
            set_instrument(&audio, "Kick", 2).expect("set");
            assert_eq!(instrument(&audio).as_deref(), Some("Kick"));
            assert_eq!(tag_version(&audio), Some(2));

            std::fs::write(&audio, b"short").expect("replace file");
            assert!(
                instrument(&audio).is_none(),
                "stamp mismatch must hide stale sidecar row"
            );
        });
    }

    #[test]
    fn set_instrument_rejects_empty_label() {
        let scratch = crate::test_fixtures::ScratchDir::new("tundra_tag_store_empty");
        let dir = scratch.path();
        let db = dir.join("tags.db");
        let audio = dir.join("kick.wav");
        std::fs::write(&audio, b"audio").expect("write");
        with_test_db(db, || {
            assert!(set_instrument(&audio, "   ", 1).is_err());
        });
    }
}
