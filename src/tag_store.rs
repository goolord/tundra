//! SQLite fallback when the audio container cannot hold tags.
//!
//! Rows are keyed by `cache_key` and stamped with the file's mtime and size, so
//! a different file later saved at the same path does not inherit them. All
//! rows are loaded once; reads never touch the database.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard, RwLock};
use std::time::UNIX_EPOCH;

use rusqlite::Connection;

pub use crate::metadata::ManualTagEdits as SidecarManualFields;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SidecarTag {
    instrument: String,
    artist: String,
    title: String,
    bpm: String,
    key: String,
    genre: String,
    comment: String,
    tag_version: u32,
    /// The instrument came from the tag editor, not the classifier, so
    /// auto-tag must never replace it.
    user_owned: bool,
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
}

/// Canonical map key. Same as metadata/dir cache keys so `\\?\` and case match.
fn key(path: &Path) -> PathBuf {
    crate::path_util::cache_key(path.to_path_buf())
}

pub(crate) fn file_stamp(path: &Path) -> Option<(u64, u64)> {
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

fn db_path() -> Option<PathBuf> {
    // Tests only ever see the database `with_test_db` points at.
    #[cfg(test)]
    return test_db_path();
    #[cfg(not(test))]
    real_db_path()
}

#[cfg(not(test))]
fn real_db_path() -> Option<PathBuf> {
    let dest = crate::path_util::tundra_data_dir()?.join("tags.db");
    if let Some(legacy) = crate::path_util::cache_file("tags.db") {
        migrate_legacy_db(&legacy, &dest);
    }
    Some(dest)
}

/// Older builds kept the database in the cache directory. `VACUUM INTO`
/// produces one consistent file including anything still in the WAL, and the
/// source is only removed once that copy is in place.
fn migrate_legacy_db(src: &Path, dest: &Path) {
    if dest.exists() || !src.exists() {
        return;
    }
    let staged = crate::path_util::unique_sidecar(dest, "atomic");
    let copied = Connection::open(src).and_then(|connection| {
        connection.execute("VACUUM INTO ?1", [staged.to_string_lossy()])
    });
    let result = copied
        .map_err(|err| err.to_string())
        .and_then(|_| crate::path_util::replace_file(&staged, dest).map_err(|err| err.to_string()));
    match result {
        Ok(()) => {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(crate::path_util::sidecar(src, suffix));
            }
        }
        Err(err) => {
            let _ = std::fs::remove_file(&staged);
            eprintln!("tundra: failed to move tag store to {}: {err}", dest.display());
        }
    }
}

const COLUMNS: [(&str, &str); 10] = [
    ("tag_version", "INTEGER NOT NULL DEFAULT 0"),
    ("mtime_secs", "INTEGER NOT NULL DEFAULT 0"),
    ("size", "INTEGER NOT NULL DEFAULT 0"),
    ("title", "TEXT NOT NULL DEFAULT ''"),
    ("artist", "TEXT NOT NULL DEFAULT ''"),
    ("bpm", "TEXT NOT NULL DEFAULT ''"),
    ("key", "TEXT NOT NULL DEFAULT ''"),
    ("genre", "TEXT NOT NULL DEFAULT ''"),
    ("comment", "TEXT NOT NULL DEFAULT ''"),
    ("user_owned", "INTEGER NOT NULL DEFAULT 0"),
];

fn prepare_schema(connection: &Connection) -> Result<(), String> {
    let fail = |err: rusqlite::Error| format!("Failed to prepare tag store: {err}");
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS instrument_tags (
                 path TEXT PRIMARY KEY,
                 instrument TEXT NOT NULL
             );",
        )
        .map_err(fail)?;
    let existing: Vec<String> = connection
        .prepare("SELECT name FROM pragma_table_info('instrument_tags')")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()
        })
        .map_err(fail)?;
    for (column, decl) in COLUMNS {
        if existing.iter().any(|name| name == column) {
            continue;
        }
        // Adding a column and back-filling it commit together, or a failed
        // back-fill would never be retried.
        let transaction = connection.unchecked_transaction().map_err(fail)?;
        transaction
            .execute(&format!("ALTER TABLE instrument_tags ADD COLUMN {column} {decl}"), [])
            .map_err(fail)?;
        if column == "user_owned" {
            // Rows written before ownership was tracked: any manual field
            // besides the instrument means the tag editor wrote the row.
            connection
                .execute(
                    "UPDATE instrument_tags SET user_owned = 1
                     WHERE instrument != '' AND (artist != '' OR title != '' OR bpm != ''
                         OR key != '' OR genre != '' OR comment != '')",
                    [],
                )
                .map_err(fail)?;
        }
        transaction.commit().map_err(fail)?;
    }
    Ok(())
}

fn load_rows(connection: &Connection) -> Result<HashMap<PathBuf, SidecarTag>, String> {
    let mut statement = connection
        .prepare(
            "SELECT path, instrument, tag_version, mtime_secs, size, title, artist, bpm, key,
                    genre, comment, user_owned
             FROM instrument_tags",
        )
        .map_err(|err| format!("Failed to query tag store: {err}"))?;
    let rows = statement
        .query_map([], |row| {
            let path: String = row.get(0)?;
            Ok((
                key(Path::new(&path)),
                SidecarTag {
                    instrument: row.get(1)?,
                    tag_version: row.get(2)?,
                    mtime_secs: row.get::<_, i64>(3)?.max(0) as u64,
                    size: row.get::<_, i64>(4)?.max(0) as u64,
                    title: row.get(5)?,
                    artist: row.get(6)?,
                    bpm: row.get(7)?,
                    key: row.get(8)?,
                    genre: row.get(9)?,
                    comment: row.get(10)?,
                    user_owned: row.get(11)?,
                },
            ))
        })
        .map_err(|err| format!("Failed to read tag store: {err}"))?;
    rows.collect::<Result<_, _>>()
        .map_err(|err| format!("Failed to read tag store: {err}"))
}

/// The open database. Opened at start-up only when it already exists, so
/// libraries that never need the fallback never get a database file.
struct Database {
    connection: Option<Connection>,
    error: Option<String>,
}

impl Database {
    fn connection(&mut self) -> Result<&Connection, String> {
        if let Some(err) = &self.error {
            return Err(err.clone());
        }
        if self.connection.is_none() {
            let path = db_path().ok_or_else(|| "No data directory available".to_string())?;
            let connection = Connection::open(&path)
                .map_err(|err| format!("Failed to open {}: {err}", path.display()))?;
            prepare_schema(&connection)?;
            self.connection = Some(connection);
        }
        Ok(self.connection.as_ref().expect("opened above"))
    }
}

struct TagStore {
    rows: RwLock<HashMap<PathBuf, SidecarTag>>,
    database: Mutex<Database>,
}

fn open_store() -> TagStore {
    let mut database = Database {
        connection: None,
        error: None,
    };
    let rows = match db_path() {
        Some(path) if path.exists() => database.connection().and_then(load_rows),
        Some(_) => Ok(HashMap::new()),
        None => Err("No data directory available".to_string()),
    };
    let rows = rows.unwrap_or_else(|err| {
        // Refuse writes rather than let a half-loaded view overwrite rows the
        // database still holds.
        eprintln!("tundra: failed to load tag store: {err}");
        database.error = Some(err);
        HashMap::new()
    });
    TagStore {
        rows: RwLock::new(rows),
        database: Mutex::new(database),
    }
}

static STORE: LazyLock<RwLock<TagStore>> = LazyLock::new(|| RwLock::new(open_store()));

fn with_store<R>(f: impl FnOnce(&TagStore) -> R) -> R {
    let store = STORE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&store)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn cached(path: &Path) -> Option<SidecarTag> {
    with_store(|store| {
        let rows = store
            .rows
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        rows.get(&key(path))
            .filter(|entry| stamp_matches(path, entry))
            .cloned()
    })
}

/// Instrument recorded for `path`, if the container could not hold one.
pub fn instrument(path: &Path) -> Option<String> {
    cached(path)
        .map(|entry| entry.instrument)
        .filter(|instrument| !instrument.is_empty())
}

/// Instrument the classifier stored, which a newer classifier may replace.
pub fn tundra_instrument(path: &Path) -> Option<String> {
    cached(path)
        .filter(|entry| !entry.user_owned && !entry.instrument.is_empty())
        .map(|entry| entry.instrument)
}

pub fn tag_version(path: &Path) -> Option<u32> {
    cached(path).map(|entry| entry.tag_version)
}

pub fn manual_fields(path: &Path) -> Option<SidecarManualFields> {
    cached(path)
        .map(|entry| entry.manual_fields())
        .filter(|fields| !fields.is_empty())
}

fn save(path: &Path, entry: Option<SidecarTag>) -> Result<(), String> {
    with_store(|store| {
        let mut database = lock(&store.database);
        write_row(store, &mut database, path, entry)
    })
}

/// Write one row and mirror it in memory. The caller holds the database lock.
fn write_row(
    store: &TagStore,
    database: &mut Database,
    path: &Path,
    entry: Option<SidecarTag>,
) -> Result<(), String> {
    let row_key = key(path);
    let stored = row_key.to_string_lossy().into_owned();
    {
        let connection = database.connection()?;
        let result = match &entry {
            Some(entry) => connection.execute(
                "INSERT INTO instrument_tags (
                     path, instrument, tag_version, mtime_secs, size,
                     title, artist, bpm, key, genre, comment, user_owned
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
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
                     comment = excluded.comment,
                     user_owned = excluded.user_owned",
                rusqlite::params![
                    stored,
                    entry.instrument,
                    entry.tag_version,
                    entry.mtime_secs as i64,
                    entry.size as i64,
                    entry.title,
                    entry.artist,
                    entry.bpm,
                    entry.key,
                    entry.genre,
                    entry.comment,
                    entry.user_owned,
                ],
            ),
            None => connection.execute("DELETE FROM instrument_tags WHERE path = ?1", [&stored]),
        };
        result.map_err(|err| format!("Failed to save tag for {}: {err}", path.display()))?;

    }
    let mut rows = store
        .rows
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match entry {
        Some(entry) => rows.insert(row_key, entry),
        None => rows.remove(&row_key),
    };
    Ok(())
}

/// Read-modify-write one row against the file's current stamp, holding the
/// database lock throughout so concurrent edits of one file cannot drop
/// each other's fields.
fn update(path: &Path, change: impl FnOnce(&mut SidecarTag)) -> Result<(), String> {
    with_store(|store| {
        let mut database = lock(&store.database);
        let (mtime_secs, size) = file_stamp(path).unwrap_or((0, 0));
        let mut entry = store
            .rows
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key(path))
            .filter(|entry| (entry.mtime_secs, entry.size) == (mtime_secs, size))
            .cloned()
            .unwrap_or_default();
        entry.mtime_secs = mtime_secs;
        entry.size = size;
        change(&mut entry);
        write_row(store, &mut database, path, Some(entry))
    })
}

pub fn set_instrument(path: &Path, instrument: &str, tag_version: u32) -> Result<(), String> {
    let instrument = instrument.trim();
    if instrument.is_empty() {
        return Err("Instrument label cannot be empty".into());
    }
    update(path, |entry| {
        entry.instrument = instrument.to_string();
        entry.tag_version = tag_version;
        entry.user_owned = false;
    })
}

pub fn set_manual_fields(
    path: &Path,
    fields: &SidecarManualFields,
    tag_version: u32,
) -> Result<(), String> {
    update(path, |entry| {
        let instrument = fields.instrument.trim();
        if instrument != entry.instrument {
            entry.user_owned = !instrument.is_empty();
        }
        entry.instrument = instrument.to_string();
        entry.artist = fields.artist.trim().to_string();
        entry.title = fields.title.trim().to_string();
        entry.bpm = fields.bpm.trim().to_string();
        entry.key = fields.key.trim().to_string();
        entry.genre = fields.genre.trim().to_string();
        entry.comment = fields.comment.trim().to_string();
        entry.tag_version = tag_version;
    })
}

/// Drop the manual fields after they were written into the file itself. A
/// fallback instrument stays, since the file may still lack one.
pub fn clear_manual_fields(path: &Path) -> Result<(), String> {
    let Some(entry) = cached(path) else {
        return Ok(());
    };
    if entry.instrument.trim().is_empty() {
        return save(path, None);
    }
    update(path, |entry| {
        entry.artist.clear();
        entry.title.clear();
        entry.bpm.clear();
        entry.key.clear();
        entry.genre.clear();
        entry.comment.clear();
    })
}

/// Move a row that matched `previous` onto the file's current stamp. Called after
/// Tundra itself rewrote the file, which changes mtime and size but not identity.
pub(crate) fn restamp(path: &Path, previous: (u64, u64)) {
    let Some(current) = file_stamp(path) else {
        return;
    };
    if current == previous {
        return;
    }
    let entry = with_store(|store| {
        let rows = store
            .rows
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        rows.get(&key(path)).cloned()
    });
    let Some(mut entry) = entry.filter(|entry| (entry.mtime_secs, entry.size) == previous) else {
        return;
    };
    (entry.mtime_secs, entry.size) = current;
    if let Err(err) = save(path, Some(entry)) {
        eprintln!("tundra: {err}");
    }
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
static TAG_STORE_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Run `f` against an isolated SQLite sidecar database (tests only).
#[cfg(test)]
pub(crate) fn with_test_db<F, R>(path: PathBuf, f: F) -> R
where
    F: FnOnce() -> R,
{
    let _lock = lock(&TAG_STORE_TEST_LOCK);
    let reset = || {
        *STORE
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = open_store();
    };
    TEST_DB_PATH.with(|slot| {
        *slot.borrow_mut() = Some(path);
        reset();
        let result = f();
        *slot.borrow_mut() = None;
        reset();
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::ScratchDir;

    #[test]
    fn sidecar_key_matches_cache_key_including_verbatim_prefix() {
        let bare = Path::new(r"C:\Samples\kick.wav");
        let verbatim = Path::new(r"\\?\C:\Samples\kick.wav");
        assert_eq!(key(bare), key(verbatim));
        assert_eq!(key(bare), crate::path_util::cache_key(bare.to_path_buf()));
    }

    #[test]
    fn prepare_schema_migrates_old_tables_and_infers_ownership() {
        let scratch = ScratchDir::new("tag-store-migrate");
        let connection = Connection::open(scratch.path().join("tags.db")).expect("open db");
        connection
            .execute_batch(
                "CREATE TABLE instrument_tags (path TEXT PRIMARY KEY, instrument TEXT NOT NULL,
                     title TEXT NOT NULL DEFAULT '');
                 INSERT INTO instrument_tags VALUES ('c:/auto.wav', 'Kick', '');
                 INSERT INTO instrument_tags VALUES ('c:/manual.wav', 'Snare', 'Crack');",
            )
            .expect("old schema");

        prepare_schema(&connection).expect("migrate");
        prepare_schema(&connection).expect("migrate is idempotent");

        let rows = load_rows(&connection).expect("rows");
        let auto = &rows[&key(Path::new("c:/auto.wav"))];
        let manual = &rows[&key(Path::new("c:/manual.wav"))];
        assert_eq!((auto.tag_version, auto.mtime_secs, auto.size), (0, 0, 0));
        assert!(!auto.user_owned);
        assert!(manual.user_owned);
    }

    #[test]
    fn stamp_mismatch_or_missing_file_hides_sidecar() {
        let scratch = ScratchDir::new("tag-store-stamp");
        let audio = scratch.path().join("kick.wav");
        std::fs::write(&audio, b"audio-v1").expect("write");
        let (mtime_secs, size) = file_stamp(&audio).expect("stamp");
        let entry = SidecarTag {
            instrument: "Kick".into(),
            mtime_secs,
            size,
            ..SidecarTag::default()
        };
        assert!(stamp_matches(&audio, &entry));

        std::fs::write(&audio, b"audio-v1-replaced").expect("replace");
        assert!(
            !stamp_matches(&audio, &entry),
            "recycled path with new contents must hide the old sidecar"
        );
        assert!(
            !stamp_matches(&audio, &SidecarTag::default()),
            "pre-stamp rows must not match a real file"
        );

        let _ = std::fs::remove_file(&audio);
        assert!(!stamp_matches(&audio, &entry));
    }

    #[test]
    fn legacy_database_moves_with_its_wal_contents() {
        let scratch = ScratchDir::new("tag-store-legacy");
        let src = scratch.path().join("cache").join("tags.db");
        let dest = scratch.path().join("data").join("tags.db");
        std::fs::create_dir_all(src.parent().unwrap()).expect("cache dir");
        std::fs::create_dir_all(dest.parent().unwrap()).expect("data dir");
        let writer = Connection::open(&src).expect("open src");
        prepare_schema(&writer).expect("schema");
        writer
            .execute(
                "INSERT INTO instrument_tags (path, instrument) VALUES ('c:/kick.wav', 'Kick')",
                [],
            )
            .expect("insert");

        // `writer` stays open, so the row may still live only in the WAL.
        migrate_legacy_db(&src, &dest);
        drop(writer);

        let rows = load_rows(&Connection::open(&dest).expect("open dest")).expect("rows");
        assert_eq!(rows[&key(Path::new("c:/kick.wav"))].instrument, "Kick");
        // Windows cannot delete a database another connection holds open.
        #[cfg(unix)]
        assert!(!src.exists());

        std::fs::write(&src, b"stale").expect("new cache file");
        migrate_legacy_db(&src, &dest);
        assert!(load_rows(&Connection::open(&dest).expect("dest")).is_ok());
    }

    #[test]
    fn set_instrument_persists_and_survives_a_reload() {
        let scratch = ScratchDir::new("tag-store-set");
        let db = scratch.path().join("tags.db");
        let audio = scratch.path().join("kick.wav");
        std::fs::write(&audio, b"audio-v1-bytes").expect("write");
        with_test_db(db.clone(), || {
            assert!(instrument(&audio).is_none());
            set_instrument(&audio, "Kick", 2).expect("set");
            assert_eq!(instrument(&audio).as_deref(), Some("Kick"));
            assert_eq!(tag_version(&audio), Some(2));
        });
        with_test_db(db, || {
            assert_eq!(instrument(&audio).as_deref(), Some("Kick"), "reloaded from disk");
            std::fs::write(&audio, b"short").expect("replace file");
            assert!(
                instrument(&audio).is_none(),
                "stamp mismatch must hide stale sidecar row"
            );
        });
    }

    #[test]
    fn manual_instrument_is_user_owned_until_the_classifier_sets_one() {
        let scratch = ScratchDir::new("tag-store-owner");
        let audio = scratch.path().join("kick.wav");
        std::fs::write(&audio, b"audio").expect("write");
        with_test_db(scratch.path().join("tags.db"), || {
            let fields = SidecarManualFields {
                instrument: "Snare".into(),
                ..SidecarManualFields::default()
            };
            set_manual_fields(&audio, &fields, 1).expect("manual");
            assert_eq!(instrument(&audio).as_deref(), Some("Snare"));
            assert_eq!(tundra_instrument(&audio), None);

            set_instrument(&audio, "Kick", 1).expect("auto");
            assert_eq!(tundra_instrument(&audio).as_deref(), Some("Kick"));

            clear_manual_fields(&audio).expect("clear");
            assert_eq!(instrument(&audio).as_deref(), Some("Kick"));
        });
    }

    #[test]
    fn set_instrument_rejects_empty_label() {
        let scratch = ScratchDir::new("tag-store-empty");
        let audio = scratch.path().join("kick.wav");
        std::fs::write(&audio, b"audio").expect("write");
        with_test_db(scratch.path().join("tags.db"), || {
            assert!(set_instrument(&audio, "   ", 1).is_err());
        });
    }
}
