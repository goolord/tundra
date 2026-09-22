//! SQLite fallback when the audio container cannot hold tags.
//!
//! Rows are keyed by `cache_key` and stamped with the file's mtime and size, so
//! a different file later saved at the same path does not inherit them. All
//! rows are loaded once; reads never touch the database.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, RwLock};

use rusqlite::Connection;

use crate::locks::{lock, read, write};
use crate::metadata::ManualTagEdits;
use crate::path_util::{FileStamp, cache_key, display_path};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Row {
    fields: ManualTagEdits,
    tag_version: u32,
    /// The instrument came from the tag editor, not the classifier, so
    /// auto-tag must never replace it.
    user_owned: bool,
    /// Whole mtime seconds and size in bytes, as stored; zero in rows written
    /// before stamping.
    stamp: (u64, u64),
}

/// The part of a file's stamp the database stores.
fn file_stamp(path: &Path) -> Option<(u64, u64)> {
    FileStamp::of(path).map(|stamp| (stamp.secs, stamp.len))
}

fn db_path() -> Option<PathBuf> {
    // Tests only ever see the database `with_test_db` points at.
    #[cfg(test)]
    return TEST_DB_PATH.with(|slot| slot.borrow().clone());
    #[cfg(not(test))]
    {
        let dest = crate::app_data::data_dir()?.join("tags.db");
        if let Some(legacy) = crate::app_data::cache_file("tags.db") {
            migrate_legacy_db(&legacy, &dest);
        }
        Some(dest)
    }
}

/// Older builds kept the database in the cache directory. `VACUUM INTO`
/// produces one consistent file including anything still in the WAL, and the
/// source is only removed once that copy is in place.
fn migrate_legacy_db(src: &Path, dest: &Path) {
    if dest.exists() || !src.exists() {
        return;
    }
    let staged = crate::safe_write::unique_sidecar(dest, "atomic");
    let result = Connection::open(src)
        .and_then(|connection| connection.execute("VACUUM INTO ?1", [staged.to_string_lossy()]))
        .map_err(|err| err.to_string())
        .and_then(|_| crate::safe_write::replace_file(&staged, dest).map_err(|err| err.to_string()));
    match result {
        Ok(()) => {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(crate::safe_write::sidecar(src, suffix));
            }
        }
        Err(err) => {
            let _ = std::fs::remove_file(&staged);
            eprintln!("tundra: failed to move tag store to {}: {err}", dest.display());
        }
    }
}

/// Columns added after the first release, with their declarations.
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
        .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect())
        .map_err(fail)?;
    for (column, decl) in COLUMNS {
        if existing.iter().any(|name| name == column) {
            continue;
        }
        // Adding a column and back-filling it commit together, or a failed
        // back-fill would never be retried.
        let transaction = connection.unchecked_transaction().map_err(fail)?;
        transaction.execute(&format!("ALTER TABLE instrument_tags ADD COLUMN {column} {decl}"), []).map_err(fail)?;
        if column == "user_owned" {
            // Rows written before ownership was tracked: any manual field
            // besides the instrument means the tag editor wrote the row.
            transaction
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

/// Every column, in the order `load_rows` reads and `write_row` binds them: the
/// row's own columns, then one per `ManualTagEdits::EDITOR_FIELDS` entry, each
/// named by `TagField::as_str`.
const ROW_COLUMNS: &str =
    "path, tag_version, mtime_secs, size, user_owned, instrument, artist, title, bpm, key, genre, comment";
const FIRST_FIELD_COLUMN: usize = 5;

fn load_rows(connection: &Connection) -> Result<HashMap<PathBuf, Row>, String> {
    let fail = |err: rusqlite::Error| format!("Failed to read tag store: {err}");
    let mut statement = connection.prepare(&format!("SELECT {ROW_COLUMNS} FROM instrument_tags")).map_err(fail)?;
    let rows = statement
        .query_map([], |row| {
            let unsigned = |index| row.get::<_, i64>(index).map(|value| value.max(0) as u64);
            let mut fields = ManualTagEdits::default();
            for (index, field) in ManualTagEdits::EDITOR_FIELDS.into_iter().enumerate() {
                fields.set_field(field, row.get(FIRST_FIELD_COLUMN + index)?);
            }
            let path: String = row.get(0)?;
            let stored =
                Row { fields, tag_version: row.get(1)?, stamp: (unsigned(2)?, unsigned(3)?), user_owned: row.get(4)? };
            Ok((cache_key(Path::new(&path)), stored))
        })
        .map_err(fail)?;
    rows.collect::<Result<_, _>>().map_err(fail)
}

/// The database, opened at start-up only when it already exists, so libraries
/// that never need the fallback never get a database file. `Err` when loading
/// failed: writes are refused so a half-loaded view cannot overwrite rows the
/// database still holds.
type Database = Result<Option<Connection>, String>;

fn connection(database: &mut Database) -> Result<&Connection, String> {
    let slot = database.as_mut().map_err(|err| err.clone())?;
    if slot.is_none() {
        let path = db_path().ok_or("No data directory available")?;
        let connection =
            Connection::open(&path).map_err(|err| format!("Failed to open {}: {err}", display_path(&path)))?;
        prepare_schema(&connection)?;
        *slot = Some(connection);
    }
    Ok(slot.as_ref().expect("opened above"))
}

/// What `modify` does with a row.
enum Change {
    Put(Row),
    Delete,
}

fn write_row(connection: &Connection, key: &Path, change: &Change) -> rusqlite::Result<usize> {
    let stored = key.to_string_lossy();
    let Change::Put(row) = change else {
        return connection.execute("DELETE FROM instrument_tags WHERE path = ?1", [&stored]);
    };
    let (secs, size) = (row.stamp.0 as i64, row.stamp.1 as i64);
    let head: [&dyn rusqlite::ToSql; FIRST_FIELD_COLUMN] = [&stored, &row.tag_version, &secs, &size, &row.user_owned];
    let fields = ManualTagEdits::EDITOR_FIELDS.map(|field| row.fields.field_value(field));
    let values: Vec<&dyn rusqlite::ToSql> = head.into_iter().chain(fields.iter().map(|value| value as _)).collect();
    let placeholders = (1..=values.len()).map(|index| format!("?{index}")).collect::<Vec<_>>().join(", ");
    connection.execute(
        &format!("INSERT OR REPLACE INTO instrument_tags ({ROW_COLUMNS}) VALUES ({placeholders})"),
        values.as_slice(),
    )
}

struct TagStore {
    rows: RwLock<HashMap<PathBuf, Row>>,
    database: Mutex<Database>,
}

fn open_store() -> TagStore {
    let mut database = Ok(None);
    let rows = match db_path() {
        Some(path) if path.exists() => connection(&mut database).and_then(load_rows),
        Some(_) => Ok(HashMap::new()),
        None => Err("No data directory available".to_string()),
    };
    let rows = rows.unwrap_or_else(|err| {
        eprintln!("tundra: failed to load tag store: {err}");
        database = Err(err);
        HashMap::new()
    });
    TagStore { rows: RwLock::new(rows), database: Mutex::new(database) }
}

/// Behind a lock only so `with_test_db` can swap in another database.
static STORE: LazyLock<RwLock<TagStore>> = LazyLock::new(|| RwLock::new(open_store()));

/// The row for `path`, whatever its stamp.
fn stored_row(store: &TagStore, path: &Path) -> Option<Row> {
    read(&store.rows).get(&cache_key(path)).cloned()
}

/// The row for `path` if it still describes the file on disk.
fn current_row(path: &Path) -> Option<Row> {
    stored_row(&read(&STORE), path).filter(|row| file_stamp(path) == Some(row.stamp))
}

/// Read-modify-write the row for `path`, holding the database lock throughout
/// so concurrent edits of one file cannot drop each other's changes. `change`
/// returns `None` to leave the row as it is.
fn modify(path: &Path, change: impl FnOnce(Option<Row>) -> Option<Change>) -> Result<(), String> {
    let store = read(&STORE);
    let mut database = lock(&store.database);
    // Leaving the row alone never opens the database, so a library that needs
    // no fallback never gets a database file.
    let Some(change) = change(stored_row(&store, path)) else {
        return Ok(());
    };
    let key = cache_key(path);
    write_row(connection(&mut database)?, &key, &change)
        .map_err(|err| format!("Failed to save tag for {}: {err}", display_path(path)))?;
    let mut rows = write(&store.rows);
    match change {
        Change::Put(row) => rows.insert(key, row),
        Change::Delete => rows.remove(&key),
    };
    Ok(())
}

/// Like `modify`, starting from the row that matches the file's current stamp.
fn update(path: &Path, change: impl FnOnce(&mut Row)) -> Result<(), String> {
    let stamp = file_stamp(path).unwrap_or_default();
    modify(path, |row| {
        let mut row = row.filter(|row| row.stamp == stamp).unwrap_or_default();
        row.stamp = stamp;
        change(&mut row);
        Some(Change::Put(row))
    })
}

/// Instrument recorded for `path`, if the container could not hold one.
pub fn instrument(path: &Path) -> Option<String> {
    row_instrument(current_row(path)?)
}

/// Instrument the classifier stored, which a newer classifier may replace.
pub fn tundra_instrument(path: &Path) -> Option<String> {
    row_instrument(current_row(path).filter(|row| !row.user_owned)?)
}

fn row_instrument(row: Row) -> Option<String> {
    Some(row.fields.instrument).filter(|instrument| !instrument.is_empty())
}

pub fn tag_version(path: &Path) -> Option<u32> {
    current_row(path).map(|row| row.tag_version)
}

pub fn manual_fields(path: &Path) -> Option<ManualTagEdits> {
    Some(current_row(path)?.fields).filter(|fields| !fields.is_empty())
}

pub fn set_instrument(path: &Path, instrument: &str, tag_version: u32) -> Result<(), String> {
    let instrument = instrument.trim();
    if instrument.is_empty() {
        return Err("Instrument label cannot be empty".into());
    }
    update(path, |row| {
        row.fields.instrument = instrument.to_string();
        row.tag_version = tag_version;
        row.user_owned = false;
    })
}

pub fn set_manual_fields(path: &Path, fields: &ManualTagEdits, tag_version: u32) -> Result<(), String> {
    update(path, |row| {
        let fields = fields.trimmed();
        if fields.instrument != row.fields.instrument {
            row.user_owned = !fields.instrument.is_empty();
        }
        row.fields = fields;
        row.tag_version = tag_version;
    })
}

/// Drop the manual fields after they were written into the file itself. A
/// fallback instrument stays, since the file may still lack one.
pub fn clear_manual_fields(path: &Path) -> Result<(), String> {
    let Some(current) = current_row(path) else {
        return Ok(());
    };
    if current.fields.instrument.trim().is_empty() {
        return modify(path, |_| Some(Change::Delete));
    }
    update(path, |row| {
        row.fields =
            ManualTagEdits { instrument: std::mem::take(&mut row.fields.instrument), ..ManualTagEdits::default() };
    })
}

/// Move a row that matched `previous` onto the file's current stamp. Called after
/// Tundra itself rewrote the file, which changes mtime and size but not identity.
pub(crate) fn restamp(path: &Path, previous: Option<FileStamp>) {
    let (Some(previous), Some(current)) = (previous, FileStamp::of(path)) else {
        return;
    };
    let (previous, current) = ((previous.secs, previous.len), (current.secs, current.len));
    if previous == current {
        return;
    }
    let result = modify(path, |row| {
        let mut row = row.filter(|row| row.stamp == previous)?;
        row.stamp = current;
        Some(Change::Put(row))
    });
    if let Err(err) = result {
        eprintln!("tundra: {err}");
    }
}

#[cfg(test)]
thread_local! {
    static TEST_DB_PATH: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` against an isolated SQLite sidecar database (tests only).
#[cfg(test)]
pub(crate) fn with_test_db<R>(path: PathBuf, f: impl FnOnce() -> R) -> R {
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    let _serial = lock(&TEST_LOCK);
    let reset = || *write(&STORE) = open_store();
    TEST_DB_PATH.with(|slot| *slot.borrow_mut() = Some(path));
    reset();
    let result = f();
    TEST_DB_PATH.with(|slot| *slot.borrow_mut() = None);
    reset();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::ScratchDir;

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
        let field_columns: Vec<_> = ROW_COLUMNS.split(", ").skip(FIRST_FIELD_COLUMN).collect();
        assert_eq!(field_columns, ManualTagEdits::EDITOR_FIELDS.map(|field| field.as_str()));

        let rows = load_rows(&connection).expect("rows");
        let auto = &rows[&cache_key(Path::new("c:/auto.wav"))];
        let manual = &rows[&cache_key(Path::new("c:/manual.wav"))];
        assert_eq!((auto.tag_version, auto.stamp), (0, (0, 0)));
        assert!(!auto.user_owned);
        assert!(manual.user_owned);
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
            .execute("INSERT INTO instrument_tags (path, instrument) VALUES ('c:/kick.wav', 'Kick')", [])
            .expect("insert");

        // `writer` stays open, so the row may still live only in the WAL.
        migrate_legacy_db(&src, &dest);
        drop(writer);

        let rows = load_rows(&Connection::open(&dest).expect("open dest")).expect("rows");
        assert_eq!(rows[&cache_key(Path::new("c:/kick.wav"))].fields.instrument, "Kick");
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
            assert!(set_instrument(&audio, "   ", 1).is_err(), "empty labels are refused");
            set_instrument(&audio, "Kick", 2).expect("set");
            assert_eq!(instrument(&audio).as_deref(), Some("Kick"));
            assert_eq!(tag_version(&audio), Some(2));
        });
        with_test_db(db, || {
            assert_eq!(instrument(&audio).as_deref(), Some("Kick"), "reloaded from disk");
            std::fs::write(&audio, b"short").expect("replace file");
            assert!(instrument(&audio).is_none(), "stamp mismatch must hide stale sidecar row");
            std::fs::remove_file(&audio).expect("remove file");
            assert!(instrument(&audio).is_none(), "missing file hides the row");
        });
    }

    #[test]
    fn manual_instrument_is_user_owned_until_the_classifier_sets_one() {
        let scratch = ScratchDir::new("tag-store-owner");
        let audio = scratch.path().join("kick.wav");
        std::fs::write(&audio, b"audio").expect("write");
        with_test_db(scratch.path().join("tags.db"), || {
            let fields =
                ManualTagEdits { instrument: " Snare ".into(), title: "Crack".into(), ..ManualTagEdits::default() };
            set_manual_fields(&audio, &fields, 1).expect("manual");
            assert_eq!(instrument(&audio).as_deref(), Some("Snare"));
            assert_eq!(tundra_instrument(&audio), None);

            set_instrument(&audio, "Kick", 1).expect("auto");
            assert_eq!(tundra_instrument(&audio).as_deref(), Some("Kick"));

            clear_manual_fields(&audio).expect("clear");
            assert_eq!(instrument(&audio).as_deref(), Some("Kick"));
            assert_eq!(manual_fields(&audio).map(|fields| fields.title), Some(String::new()));
        });
    }

    #[test]
    fn restamp_follows_the_file_only_from_the_previous_stamp() {
        let scratch = ScratchDir::new("tag-store-restamp");
        let audio = scratch.path().join("kick.wav");
        std::fs::write(&audio, b"audio").expect("write");
        with_test_db(scratch.path().join("tags.db"), || {
            set_instrument(&audio, "Kick", 1).expect("set");
            let before = FileStamp::of(&audio);
            std::fs::write(&audio, b"audio, retagged").expect("rewrite");
            restamp(&audio, before);
            assert_eq!(instrument(&audio).as_deref(), Some("Kick"));

            std::fs::write(&audio, b"someone else's file").expect("replace");
            restamp(&audio, before);
            assert!(instrument(&audio).is_none(), "a stale stamp must not carry the row over");
        });
    }
}
