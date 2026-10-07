//! The book library: folder scan, fingerprints, curated metadata, adding/removing books.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::pdf;

#[derive(Debug, Clone, Deserialize)]
struct CatalogEntry {
    file: String,
    title: String,
    author: Option<String>,
    track: String,
    status: String,
    level: Option<String>,
    #[serde(default)]
    topics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub label: String,
}

#[derive(Deserialize)]
struct Catalog {
    tracks: Vec<Track>,
    books: Vec<CatalogEntry>,
}

fn catalog() -> &'static Catalog {
    static CATALOG: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../resources/books.json")).expect("resources/books.json is valid")
    })
}

pub fn tracks() -> Vec<Track> {
    catalog().tracks.clone()
}

fn catalog_lookup(file_name: &str) -> Option<&'static CatalogEntry> {
    catalog().books.iter().find(|b| b.file.eq_ignore_ascii_case(file_name))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookProgress {
    pub status: String,
    pub last_page: u32,
    pub chapters_done: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookView {
    pub fingerprint: String,
    pub title: String,
    pub author: Option<String>,
    pub track: String,
    pub status: String,
    pub level: Option<String>,
    pub topics: Vec<String>,
    pub file_name: String,
    pub size: u64,
    pub page_count: Option<u32>,
    /// Location on this machine; `None` = known from another PC but not found here.
    pub path: Option<String>,
    pub progress: Option<BookProgress>,
}

pub fn fingerprint(path: &Path) -> AppResult<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_mmap_rayon(path)?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn file_stat(path: &Path) -> AppResult<(u64, i64)> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok((meta.len(), mtime))
}

/// Fingerprint with a cache keyed by path + size + mtime, so rescans don't rehash.
fn cached_fingerprint(conn: &Connection, path: &Path) -> AppResult<String> {
    let (size, mtime) = file_stat(path)?;
    let key = path.to_string_lossy();
    let cached: Option<String> = conn
        .query_row(
            "SELECT fingerprint FROM book_files WHERE path = ?1 AND size = ?2 AND mtime = ?3",
            params![key, size as i64, mtime],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(fp) = cached {
        return Ok(fp);
    }
    let fp = fingerprint(path)?;
    conn.execute(
        "INSERT INTO book_files(path, fingerprint, size, mtime) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET fingerprint = excluded.fingerprint, size = excluded.size, mtime = excluded.mtime",
        params![key, fp, size as i64, mtime],
    )?;
    Ok(fp)
}

fn title_from_filename(file_name: &str) -> String {
    let stem = file_name.strip_suffix(".pdf").or_else(|| file_name.strip_suffix(".PDF")).unwrap_or(file_name);
    stem.replace(['_', '.'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// PDF metadata titles are often junk ("Microsoft Word - x.doc", "untitled").
fn usable_meta_title(t: &str) -> bool {
    let l = t.to_lowercase();
    t.len() > 3 && !l.contains(".doc") && !l.contains("untitled") && !l.ends_with(".pdf") && !l.starts_with("microsoft")
}

fn ensure_book_row(conn: &Connection, fp: &str, path: &Path, device_id: &str, undelete: bool) -> AppResult<()> {
    let exists: Option<i64> = conn
        .query_row("SELECT deleted FROM books WHERE fingerprint = ?1", [fp], |r| r.get(0))
        .optional()?;
    let ts = now();
    match exists {
        Some(1) if undelete => {
            conn.execute(
                "UPDATE books SET deleted = 0, updated_at = ?2, device_id = ?3 WHERE fingerprint = ?1",
                params![fp, ts, device_id],
            )?;
        }
        Some(_) => {}
        None => {
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("book.pdf").to_string();
            let (size, _) = file_stat(path)?;
            let info = pdf::read_info(path).unwrap_or_default();
            let (title, author, track, status, level, topics) = match catalog_lookup(&file_name) {
                Some(c) => (
                    c.title.clone(),
                    c.author.clone(),
                    c.track.clone(),
                    c.status.clone(),
                    c.level.clone(),
                    c.topics.clone(),
                ),
                None => (
                    info.title.clone().filter(|t| usable_meta_title(t)).unwrap_or_else(|| title_from_filename(&file_name)),
                    info.author.clone(),
                    "uncategorized".to_string(),
                    "core".to_string(),
                    None,
                    vec![],
                ),
            };
            conn.execute(
                "INSERT INTO books(fingerprint, title, author, track, status, level, topics, file_name, size, page_count,
                                   created_at, updated_at, device_id, deleted)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12, 0)",
                params![
                    fp,
                    title,
                    author,
                    track,
                    status,
                    level,
                    serde_json::to_string(&topics)?,
                    file_name,
                    size as i64,
                    (info.page_count > 0).then_some(info.page_count),
                    ts,
                    device_id
                ],
            )?;
        }
    }
    Ok(())
}

fn is_pdf(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// Walks the library folder, registers new PDFs, forgets files that disappeared from it.
pub fn scan(conn: &Connection, root: &Path, device_id: &str) -> AppResult<()> {
    if !root.is_dir() {
        return Err(AppError::NotFound(format!("Library folder not found: {}", root.display())));
    }
    let mut seen = HashSet::new();
    for entry in WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.') && e.file_name() != "node_modules")
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !entry.file_type().is_file() || !is_pdf(path) {
            continue;
        }
        let fp = cached_fingerprint(conn, path)?;
        ensure_book_row(conn, &fp, path, device_id, false)?;
        seen.insert(path.to_string_lossy().to_string());
    }

    // Drop locations inside the library that no longer exist (linked files elsewhere are kept).
    let root_str = root.to_string_lossy().to_string();
    let mut stmt = conn.prepare("SELECT path FROM book_files")?;
    let stale: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .filter(|p| (p.starts_with(&root_str) && !seen.contains(p)) || !Path::new(p).exists())
        .collect();
    for p in stale {
        conn.execute("DELETE FROM book_files WHERE path = ?1", [p])?;
    }
    Ok(())
}

pub fn list(conn: &Connection) -> AppResult<Vec<BookView>> {
    let mut stmt = conn.prepare(
        "SELECT b.fingerprint, b.title, b.author, b.track, b.status, b.level, b.topics, b.file_name, b.size, b.page_count,
                (SELECT path FROM book_files f WHERE f.fingerprint = b.fingerprint ORDER BY path LIMIT 1),
                p.status, p.last_page, p.chapters_done
         FROM books b LEFT JOIN progress p ON p.fingerprint = b.fingerprint AND p.deleted = 0
         WHERE b.deleted = 0
         ORDER BY b.title COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |r| {
        let topics: String = r.get(6)?;
        let p_status: Option<String> = r.get(11)?;
        let progress = match p_status {
            Some(status) => Some(BookProgress {
                status,
                last_page: r.get::<_, i64>(12)? as u32,
                chapters_done: serde_json::from_str(&r.get::<_, String>(13)?).unwrap_or_default(),
            }),
            None => None,
        };
        Ok(BookView {
            fingerprint: r.get(0)?,
            title: r.get(1)?,
            author: r.get(2)?,
            track: r.get(3)?,
            status: r.get(4)?,
            level: r.get(5)?,
            topics: serde_json::from_str(&topics).unwrap_or_default(),
            file_name: r.get(7)?,
            size: r.get::<_, i64>(8)? as u64,
            page_count: r.get::<_, Option<i64>>(9)?.map(|n| n as u32),
            path: r.get(10)?,
            progress,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get(conn: &Connection, fp: &str) -> AppResult<BookView> {
    list(conn)?
        .into_iter()
        .find(|b| b.fingerprint == fp)
        .ok_or_else(|| AppError::NotFound("Book not found".into()))
}

pub fn path_of(conn: &Connection, fp: &str) -> AppResult<PathBuf> {
    let mut stmt = conn.prepare("SELECT path FROM book_files WHERE fingerprint = ?1 ORDER BY path")?;
    let paths: Vec<String> = stmt.query_map([fp], |r| r.get(0))?.filter_map(|r| r.ok()).collect();
    paths
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
        .ok_or_else(|| AppError::NotFound("This book's file isn't on this computer. Add it again or rescan the library.".into()))
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AddMode {
    Copy,
    Link,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddResult {
    pub added: Vec<String>,
    pub duplicates: Vec<Duplicate>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Duplicate {
    pub file: String,
    pub existing_title: String,
}

fn unique_target(root: &Path, file_name: &str) -> PathBuf {
    let target = root.join(file_name);
    if !target.exists() {
        return target;
    }
    let stem = Path::new(file_name).file_stem().and_then(|s| s.to_str()).unwrap_or("book");
    (2..)
        .map(|n| root.join(format!("{stem} ({n}).pdf")))
        .find(|p| !p.exists())
        .expect("some free name")
}

pub fn add(conn: &Connection, root: &Path, paths: &[PathBuf], mode: AddMode, device_id: &str) -> AppResult<AddResult> {
    let mut result = AddResult::default();
    for src in paths {
        let label = src.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
        if !is_pdf(src) || !src.is_file() {
            result.errors.push(format!("{label}: not a PDF file"));
            continue;
        }
        let fp = match fingerprint(src) {
            Ok(fp) => fp,
            Err(e) => {
                result.errors.push(format!("{label}: {e}"));
                continue;
            }
        };
        let existing: Option<(String, i64)> = conn
            .query_row("SELECT title, deleted FROM books WHERE fingerprint = ?1", [&fp], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        if let Some((title, 0)) = &existing {
            if path_of(conn, &fp).is_ok() {
                result.duplicates.push(Duplicate { file: label, existing_title: title.clone() });
                continue;
            }
        }
        let location = match mode {
            AddMode::Copy if !src.starts_with(root) => {
                let target = unique_target(root, &label);
                if let Err(e) = std::fs::copy(src, &target) {
                    result.errors.push(format!("{label}: copy failed: {e}"));
                    continue;
                }
                target
            }
            _ => src.clone(),
        };
        let (size, mtime) = file_stat(&location)?;
        conn.execute(
            "INSERT INTO book_files(path, fingerprint, size, mtime) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET fingerprint = excluded.fingerprint, size = excluded.size, mtime = excluded.mtime",
            params![location.to_string_lossy(), fp, size as i64, mtime],
        )?;
        ensure_book_row(conn, &fp, &location, device_id, true)?;
        result.added.push(fp);
    }
    Ok(result)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookPatch {
    pub title: Option<String>,
    pub author: Option<String>,
    pub track: Option<String>,
    pub status: Option<String>,
    pub level: Option<String>,
}

pub fn update(conn: &Connection, fp: &str, patch: &BookPatch, device_id: &str) -> AppResult<()> {
    let n = conn.execute(
        "UPDATE books SET title = COALESCE(?2, title), author = COALESCE(?3, author), track = COALESCE(?4, track),
                          status = COALESCE(?5, status), level = COALESCE(?6, level), updated_at = ?7, device_id = ?8
         WHERE fingerprint = ?1",
        params![fp, patch.title, patch.author, patch.track, patch.status, patch.level, now(), device_id],
    )?;
    if n == 0 {
        return Err(AppError::NotFound("Book not found".into()));
    }
    Ok(())
}

pub fn remove(conn: &Connection, root: Option<&Path>, fp: &str, delete_file: bool, delete_progress: bool, device_id: &str) -> AppResult<()> {
    let ts = now();
    if delete_file {
        if let (Some(root), Ok(path)) = (root, path_of(conn, fp)) {
            // Only ever delete copies the app manages inside the library folder.
            if path.starts_with(root) {
                std::fs::remove_file(&path)?;
            }
        }
    }
    conn.execute("UPDATE books SET deleted = 1, updated_at = ?2, device_id = ?3 WHERE fingerprint = ?1", params![fp, ts, device_id])?;
    conn.execute("DELETE FROM book_files WHERE fingerprint = ?1", [fp])?;
    if delete_progress {
        conn.execute("UPDATE progress SET deleted = 1, updated_at = ?2, device_id = ?3 WHERE fingerprint = ?1", params![fp, ts, device_id])?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressPatch {
    pub last_page: Option<u32>,
    pub status: Option<String>,
    pub chapter_done: Option<String>,
    pub chapter_undone: Option<String>,
}

pub fn save_progress(conn: &Connection, fp: &str, patch: &ProgressPatch, device_id: &str) -> AppResult<BookProgress> {
    let ts = now();
    conn.execute(
        "INSERT INTO progress(fingerprint, status, last_page, chapters_done, created_at, updated_at, device_id, deleted)
         VALUES (?1, 'reading', 1, '[]', ?2, ?2, ?3, 0)
         ON CONFLICT(fingerprint) DO NOTHING",
        params![fp, ts, device_id],
    )?;
    let (status, last_page, done): (String, i64, String) = conn.query_row(
        "SELECT status, last_page, chapters_done FROM progress WHERE fingerprint = ?1",
        [fp],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let mut done: Vec<String> = serde_json::from_str(&done).unwrap_or_default();
    if let Some(c) = &patch.chapter_done {
        if !done.contains(c) {
            done.push(c.clone());
        }
    }
    if let Some(c) = &patch.chapter_undone {
        done.retain(|d| d != c);
    }
    let mut status = patch.status.clone().unwrap_or(status);
    if status == "not-started" {
        status = "reading".into();
    }
    let last_page = patch.last_page.map(|p| p as i64).unwrap_or(last_page);
    conn.execute(
        "UPDATE progress SET status = ?2, last_page = ?3, chapters_done = ?4, updated_at = ?5, device_id = ?6, deleted = 0
         WHERE fingerprint = ?1",
        params![fp, status, last_page, serde_json::to_string(&done)?, ts, device_id],
    )?;
    Ok(BookProgress { status, last_page: last_page as u32, chapters_done: done })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_titles() {
        assert_eq!(title_from_filename("Python_Cookbook_3rd_Edition.pdf"), "Python Cookbook 3rd Edition");
        assert!(!usable_meta_title("Microsoft Word - notes.doc"));
        assert!(usable_meta_title("Effective C++"));
    }

    #[test]
    fn catalog_parses_and_matches() {
        assert!(tracks().iter().any(|t| t.id == "fundamentals"));
        assert_eq!(catalog_lookup("head-first-design-patterns.pdf").unwrap().status, "start-here");
    }
}
