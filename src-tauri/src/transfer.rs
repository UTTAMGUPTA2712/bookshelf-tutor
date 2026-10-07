//! Progress export/import between machines. One JSON file, versioned, checksummed.
//! Device-local data (file paths, API keys, settings) is never included.

use std::path::Path;

use rusqlite::{types::ValueRef, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::db::now;
use crate::error::{AppError, AppResult};

pub const FORMAT: &str = "bookshelf-tutor-export";
pub const SCHEMA_VERSION: u32 = 1;

struct Table {
    name: &'static str,
    key: &'static str,
    columns: &'static [&'static str],
    /// TEXT columns that hold JSON; exported as real JSON instead of strings.
    json_columns: &'static [&'static str],
}

const TABLES: &[Table] = &[
    Table {
        name: "books",
        key: "fingerprint",
        columns: &[
            "fingerprint", "title", "author", "track", "status", "level", "topics", "file_name", "size", "page_count",
            "created_at", "updated_at", "device_id", "deleted",
        ],
        json_columns: &["topics"],
    },
    Table {
        name: "progress",
        key: "fingerprint",
        columns: &["fingerprint", "status", "last_page", "chapters_done", "created_at", "updated_at", "device_id", "deleted"],
        json_columns: &["chapters_done"],
    },
    Table {
        name: "providers",
        key: "id",
        columns: &[
            "id", "provider_type", "label", "base_url", "enabled_models", "options", "created_at", "updated_at", "device_id",
            "deleted",
        ],
        json_columns: &["enabled_models", "options"],
    },
    Table { name: "roles", key: "role", columns: &["role", "chain", "updated_at", "device_id"], json_columns: &["chain"] },
    Table {
        name: "attempts",
        key: "id",
        columns: &[
            "id", "kind", "book_fingerprint", "chapter_id", "chapter_title", "status", "model", "questions", "overall",
            "created_at", "updated_at", "device_id", "deleted",
        ],
        json_columns: &["model", "questions", "overall"],
    },
];

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFile {
    pub format: String,
    pub schema_version: u32,
    pub app_version: String,
    pub exported_at: String,
    pub device_id: String,
    pub data: Map<String, Value>,
    pub checksum: String,
}

fn checksum(data: &Map<String, Value>) -> AppResult<String> {
    Ok(format!("blake3:{}", blake3::hash(serde_json::to_string(data)?.as_bytes()).to_hex()))
}

fn read_table(conn: &Connection, t: &Table) -> AppResult<Vec<Value>> {
    let sql = format!("SELECT {} FROM {}", t.columns.join(", "), t.name);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut obj = Map::new();
        for (i, col) in t.columns.iter().enumerate() {
            let v = match row.get_ref(i)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(n) => Value::from(n),
                ValueRef::Real(f) => Value::from(f),
                ValueRef::Text(s) => {
                    let s = String::from_utf8_lossy(s).to_string();
                    if t.json_columns.contains(col) {
                        serde_json::from_str(&s).unwrap_or(Value::String(s))
                    } else {
                        Value::String(s)
                    }
                }
                ValueRef::Blob(_) => Value::Null,
            };
            obj.insert(col.to_string(), v);
        }
        out.push(Value::Object(obj));
    }
    Ok(out)
}

pub fn build_export(conn: &Connection, device_id: &str) -> AppResult<ExportFile> {
    let mut data = Map::new();
    for t in TABLES {
        data.insert(t.name.to_string(), Value::Array(read_table(conn, t)?));
    }
    Ok(ExportFile {
        format: FORMAT.into(),
        schema_version: SCHEMA_VERSION,
        app_version: env!("CARGO_PKG_VERSION").into(),
        exported_at: now(),
        device_id: device_id.into(),
        checksum: checksum(&data)?,
        data,
    })
}

pub fn write_export(conn: &Connection, device_id: &str, path: &Path) -> AppResult<()> {
    let file = build_export(conn, device_id)?;
    let tmp = path.with_extension("json.partial");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&file)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn read_export(path: &Path) -> AppResult<ExportFile> {
    let bytes = std::fs::read(path)?;
    let file: ExportFile = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Invalid(format!("This isn't a Bookshelf Tutor export file ({e})")))?;
    if file.format != FORMAT {
        return Err(AppError::Invalid("This isn't a Bookshelf Tutor export file".into()));
    }
    if file.schema_version > SCHEMA_VERSION {
        return Err(AppError::Invalid(format!(
            "This file was made by a newer version of the app (format {}). Update the app first.",
            file.schema_version
        )));
    }
    // Older schema versions would be migrated here, one step at a time.
    if checksum(&file.data)? != file.checksum {
        return Err(AppError::Invalid("The file is damaged or was edited (checksum mismatch)".into()));
    }
    Ok(file)
}

#[derive(Debug, Default, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TableChanges {
    pub table: String,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub from_device: String,
    pub same_device: bool,
    pub exported_at: String,
    pub app_version: String,
    pub changes: Vec<TableChanges>,
}

fn local_updated_at(conn: &Connection, t: &Table, key: &Value) -> AppResult<Option<String>> {
    let key = match key {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let sql = format!("SELECT updated_at FROM {} WHERE {} = ?1", t.name, t.key);
    match conn.query_row(&sql, [key], |r| r.get::<_, String>(0)) {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// What a merge would do, table by table. Newer `updated_at` wins.
fn plan(conn: &Connection, file: &ExportFile) -> AppResult<Vec<(usize, Vec<Value>, TableChanges)>> {
    let mut out = Vec::new();
    for (ti, t) in TABLES.iter().enumerate() {
        let rows = file.data.get(t.name).and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let mut changes = TableChanges { table: t.name.into(), ..Default::default() };
        let mut to_write = Vec::new();
        for row in rows {
            let (Some(key), Some(updated)) = (row.get(t.key), row.get("updated_at").and_then(|v| v.as_str())) else {
                continue;
            };
            match local_updated_at(conn, t, key)? {
                None => {
                    changes.added += 1;
                    to_write.push(row.clone());
                }
                Some(local) if updated > local.as_str() => {
                    changes.updated += 1;
                    to_write.push(row.clone());
                }
                Some(_) => changes.unchanged += 1,
            }
        }
        out.push((ti, to_write, changes));
    }
    Ok(out)
}

pub fn preview(conn: &Connection, file: &ExportFile, device_id: &str) -> AppResult<ImportPreview> {
    Ok(ImportPreview {
        from_device: file.device_id.clone(),
        same_device: file.device_id == device_id,
        exported_at: file.exported_at.clone(),
        app_version: file.app_version.clone(),
        changes: plan(conn, file)?.into_iter().map(|(_, _, c)| c).collect(),
    })
}

fn upsert(conn: &Connection, t: &Table, row: &Value) -> AppResult<()> {
    let placeholders: Vec<String> = (1..=t.columns.len()).map(|i| format!("?{i}")).collect();
    let sql = format!("INSERT OR REPLACE INTO {} ({}) VALUES ({})", t.name, t.columns.join(", "), placeholders.join(", "));
    let values: Vec<rusqlite::types::Value> = t
        .columns
        .iter()
        .map(|c| match row.get(*c).unwrap_or(&Value::Null) {
            Value::Null => rusqlite::types::Value::Null,
            Value::Bool(b) => rusqlite::types::Value::Integer(*b as i64),
            Value::Number(n) => n
                .as_i64()
                .map(rusqlite::types::Value::Integer)
                .unwrap_or_else(|| rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0))),
            Value::String(s) => rusqlite::types::Value::Text(s.clone()),
            other => rusqlite::types::Value::Text(other.to_string()),
        })
        .collect();
    conn.execute(&sql, rusqlite::params_from_iter(values))?;
    Ok(())
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    Merge,
    Replace,
}

pub fn apply(conn: &mut Connection, file: &ExportFile, mode: ImportMode) -> AppResult<Vec<TableChanges>> {
    let tx = conn.transaction()?;
    let result = if mode == ImportMode::Replace {
        let mut changes = Vec::new();
        for t in TABLES {
            tx.execute(&format!("DELETE FROM {}", t.name), [])?;
            let rows = file.data.get(t.name).and_then(|v| v.as_array()).cloned().unwrap_or_default();
            for row in &rows {
                upsert(&tx, t, row)?;
            }
            changes.push(TableChanges { table: t.name.into(), added: rows.len(), ..Default::default() });
        }
        changes
    } else {
        let planned = plan(&tx, file)?;
        let mut changes = Vec::new();
        for (ti, rows, c) in planned {
            for row in &rows {
                upsert(&tx, &TABLES[ti], row)?;
            }
            changes.push(c);
        }
        changes
    };
    tx.commit()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use rusqlite::params;

    fn db_with(device: &str) -> Connection {
        let c = Connection::open_in_memory().unwrap();
        db::migrate(&c).unwrap();
        db::set_setting(&c, "device_id", device).unwrap();
        c
    }

    fn add_progress(c: &Connection, fp: &str, page: i64, ts: &str, dev: &str) {
        c.execute(
            "INSERT OR REPLACE INTO progress(fingerprint, status, last_page, chapters_done, created_at, updated_at, device_id, deleted)
             VALUES (?1, 'reading', ?2, '[\"p1\"]', ?3, ?3, ?4, 0)",
            params![fp, page, ts, dev],
        )
        .unwrap();
    }

    fn page(c: &Connection, fp: &str) -> i64 {
        c.query_row("SELECT last_page FROM progress WHERE fingerprint = ?1", [fp], |r| r.get(0)).unwrap()
    }

    #[test]
    fn merge_keeps_newest_and_is_idempotent() {
        let linux = db_with("linux");
        let mut mac = db_with("mac");
        add_progress(&linux, "book-a", 50, "2026-10-07T10:00:00.000Z", "linux");
        add_progress(&linux, "book-b", 10, "2026-10-01T10:00:00.000Z", "linux");
        add_progress(&mac, "book-b", 99, "2026-10-05T10:00:00.000Z", "mac");

        let file = build_export(&linux, "linux").unwrap();
        // round-trip through JSON like a real file
        let file: ExportFile = serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
        assert_eq!(checksum(&file.data).unwrap(), file.checksum);

        let p = preview(&mac, &file, "mac").unwrap();
        let progress = p.changes.iter().find(|c| c.table == "progress").unwrap();
        assert_eq!((progress.added, progress.updated, progress.unchanged), (1, 0, 1));

        apply(&mut mac, &file, ImportMode::Merge).unwrap();
        assert_eq!(page(&mac, "book-a"), 50);
        assert_eq!(page(&mac, "book-b"), 99, "mac's newer progress must win");
        let done: String = mac.query_row("SELECT chapters_done FROM progress WHERE fingerprint='book-a'", [], |r| r.get(0)).unwrap();
        assert_eq!(done, "[\"p1\"]", "JSON columns survive the round trip");

        let again = apply(&mut mac, &file, ImportMode::Merge).unwrap();
        assert!(again.iter().all(|c| c.added == 0 && c.updated == 0));
    }

    #[test]
    fn replace_and_tamper_detection() {
        let linux = db_with("linux");
        let mut mac = db_with("mac");
        add_progress(&mac, "only-on-mac", 3, "2026-10-07T10:00:00.000Z", "mac");
        add_progress(&linux, "book-a", 7, "2026-10-01T10:00:00.000Z", "linux");
        let file = build_export(&linux, "linux").unwrap();
        apply(&mut mac, &file, ImportMode::Replace).unwrap();
        let n: i64 = mac.query_row("SELECT COUNT(*) FROM progress", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // device settings are untouched by replace
        assert_eq!(db::get_setting(&mac, "device_id").unwrap().as_deref(), Some("mac"));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.json");
        write_export(&linux, "linux", &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap().replace("\"last_page\": 7", "\"last_page\": 8");
        std::fs::write(&path, text).unwrap();
        assert!(matches!(read_export(&path), Err(AppError::Invalid(_))));
    }
}
