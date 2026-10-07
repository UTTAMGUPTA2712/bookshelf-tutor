//! The learner database (`bookshelf.sqlite`): schema, migrations, device settings, backups.

use std::path::Path;

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppResult;

/// Ordered migrations. Never edit a shipped entry; append a new one.
const MIGRATIONS: &[&str] = &[
    // 1: initial schema
    r#"
    CREATE TABLE device (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );

    CREATE TABLE books (
        fingerprint TEXT PRIMARY KEY,
        title       TEXT NOT NULL,
        author      TEXT,
        track       TEXT NOT NULL DEFAULT 'uncategorized',
        status      TEXT NOT NULL DEFAULT 'core',
        level       TEXT,
        topics      TEXT NOT NULL DEFAULT '[]',
        file_name   TEXT NOT NULL,
        size        INTEGER NOT NULL,
        page_count  INTEGER,
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL,
        device_id   TEXT NOT NULL,
        deleted     INTEGER NOT NULL DEFAULT 0
    );

    -- Where each book lives on THIS machine. Never exported.
    CREATE TABLE book_files (
        path        TEXT PRIMARY KEY,
        fingerprint TEXT NOT NULL,
        size        INTEGER NOT NULL,
        mtime       INTEGER NOT NULL
    );
    CREATE INDEX book_files_fp ON book_files(fingerprint);

    CREATE TABLE progress (
        fingerprint   TEXT PRIMARY KEY,
        status        TEXT NOT NULL DEFAULT 'not-started',
        last_page     INTEGER NOT NULL DEFAULT 1,
        chapters_done TEXT NOT NULL DEFAULT '[]',
        created_at    TEXT NOT NULL,
        updated_at    TEXT NOT NULL,
        device_id     TEXT NOT NULL,
        deleted       INTEGER NOT NULL DEFAULT 0
    );

    CREATE TABLE providers (
        id             TEXT PRIMARY KEY,
        provider_type  TEXT NOT NULL,
        label          TEXT NOT NULL,
        base_url       TEXT NOT NULL,
        enabled_models TEXT NOT NULL DEFAULT '[]',
        options        TEXT NOT NULL DEFAULT '{}',
        created_at     TEXT NOT NULL,
        updated_at     TEXT NOT NULL,
        device_id      TEXT NOT NULL,
        deleted        INTEGER NOT NULL DEFAULT 0
    );

    CREATE TABLE roles (
        role       TEXT PRIMARY KEY,
        chain      TEXT NOT NULL DEFAULT '[]',
        updated_at TEXT NOT NULL,
        device_id  TEXT NOT NULL
    );

    CREATE TABLE attempts (
        id               TEXT PRIMARY KEY,
        kind             TEXT NOT NULL,
        book_fingerprint TEXT,
        chapter_id       TEXT,
        chapter_title    TEXT,
        status           TEXT NOT NULL,
        model            TEXT NOT NULL DEFAULT '{}',
        questions        TEXT NOT NULL DEFAULT '[]',
        overall          TEXT,
        created_at       TEXT NOT NULL,
        updated_at       TEXT NOT NULL,
        device_id        TEXT NOT NULL,
        deleted          INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX attempts_book ON attempts(book_fingerprint, created_at);
    "#,
];

pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn open(path: &Path) -> AppResult<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> AppResult<()> {
    let current: usize = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i + 1)?;
        tx.commit()?;
    }
    Ok(())
}

pub fn get_setting(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM device WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO device(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Stable per-machine id, created on first launch.
pub fn device_id(conn: &Connection) -> AppResult<String> {
    if let Some(id) = get_setting(conn, "device_id")? {
        return Ok(id);
    }
    let id = uuid::Uuid::new_v4().to_string();
    set_setting(conn, "device_id", &id)?;
    Ok(id)
}

/// Online backup into `backups/`, at most once per day, keeping the newest `keep` files.
pub fn daily_backup(conn: &Connection, backup_dir: &Path, keep: usize) -> AppResult<()> {
    std::fs::create_dir_all(backup_dir)?;
    let name = format!("bookshelf-{}.sqlite", Utc::now().format("%Y-%m-%d"));
    let target = backup_dir.join(&name);
    if !target.exists() {
        conn.backup(rusqlite::MAIN_DB, &target, None)?;
    }
    let mut files: Vec<_> = std::fs::read_dir(backup_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("bookshelf-") && n.ends_with(".sqlite"))
        })
        .collect();
    files.sort();
    while files.len() > keep {
        let _ = std::fs::remove_file(files.remove(0));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_and_are_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let v: usize = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, MIGRATIONS.len());
        let id1 = device_id(&conn).unwrap();
        assert_eq!(id1, device_id(&conn).unwrap());
    }
}
