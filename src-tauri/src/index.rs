//! The rebuildable search index (`index.sqlite`): chapters, text chunks, FTS5 keyword search.
//! Nothing here is exported — it can always be regenerated from the PDFs.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::db::now;
use crate::error::AppResult;
use crate::pdf::{self, Chapter};

/// Bump to force re-indexing after changes to extraction or chunking.
pub const INDEX_VERSION: i64 = 2;

const CHUNK_CHARS: usize = 3200; // ~800 tokens
const OVERLAP_CHARS: usize = 400;

pub fn open(path: &Path) -> AppResult<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS indexed_books (
            fingerprint TEXT PRIMARY KEY,
            version     INTEGER NOT NULL,
            page_count  INTEGER NOT NULL,
            text_pages  INTEGER NOT NULL,
            indexed_at  TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS chapters (
            fingerprint TEXT NOT NULL,
            idx         INTEGER NOT NULL,
            id          TEXT NOT NULL,
            title       TEXT NOT NULL,
            depth       INTEGER NOT NULL,
            start_page  INTEGER NOT NULL,
            end_page    INTEGER NOT NULL,
            PRIMARY KEY (fingerprint, idx)
        );
        CREATE TABLE IF NOT EXISTS chunks (
            id          INTEGER PRIMARY KEY,
            fingerprint TEXT NOT NULL,
            chapter_idx INTEGER NOT NULL,
            page_start  INTEGER NOT NULL,
            page_end    INTEGER NOT NULL,
            text        TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS chunks_book ON chunks(fingerprint, chapter_idx);
        CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
            text, content='chunks', content_rowid='id', tokenize='porter unicode61'
        );
        "#,
    )?;
    Ok(conn)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub indexed: bool,
    pub page_count: u32,
    /// Pages with a text layer. Near zero = scanned book, quizzes unavailable.
    pub text_pages: u32,
    pub chapters: u32,
}

pub fn status(conn: &Connection, fp: &str) -> AppResult<IndexStatus> {
    let row: Option<(i64, i64, i64)> = conn
        .query_row(
            "SELECT version, page_count, text_pages FROM indexed_books WHERE fingerprint = ?1",
            [fp],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let chapters: i64 = conn.query_row("SELECT COUNT(*) FROM chapters WHERE fingerprint = ?1", [fp], |r| r.get(0))?;
    Ok(match row {
        Some((v, pc, tp)) if v == INDEX_VERSION => IndexStatus { indexed: true, page_count: pc as u32, text_pages: tp as u32, chapters: chapters as u32 },
        _ => IndexStatus { indexed: false, page_count: 0, text_pages: 0, chapters: 0 },
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub id: i64,
    pub chapter_idx: u32,
    pub page_start: u32,
    pub page_end: u32,
    pub text: String,
}

/// Splits each chapter's pages into overlapping chunks that never cross chapter boundaries.
pub fn chunk_pages(pages: &[String], chapters: &[Chapter]) -> Vec<Chunk> {
    let mut out = Vec::new();
    for ch in chapters {
        let mut buf = String::new();
        let mut buf_start = ch.start_page;
        let mut last_page = ch.start_page;
        let mut lines: Vec<(u32, &str)> = Vec::new();
        for page in ch.start_page..=ch.end_page {
            if let Some(text) = pages.get(page as usize - 1) {
                for line in text.lines() {
                    lines.push((page, line));
                }
            }
        }
        for (page, line) in lines {
            if buf.is_empty() {
                buf_start = page;
            }
            buf.push_str(line);
            buf.push('\n');
            last_page = page;
            if buf.len() >= CHUNK_CHARS {
                out.push(Chunk { id: 0, chapter_idx: ch.idx, page_start: buf_start, page_end: page, text: buf.clone() });
                // carry an overlap tail into the next chunk
                let mut cut = buf.len().saturating_sub(OVERLAP_CHARS);
                while !buf.is_char_boundary(cut) {
                    cut += 1;
                }
                let tail = buf[cut..].to_string();
                buf = tail;
                buf_start = page;
            }
        }
        if buf.trim().len() > 40 {
            out.push(Chunk { id: 0, chapter_idx: ch.idx, page_start: buf_start, page_end: last_page, text: buf });
        }
    }
    out
}

pub struct Prepared {
    page_count: u32,
    text_pages: u32,
    chapters: Vec<Chapter>,
    chunks: Vec<Chunk>,
}

/// The slow part (PDF extraction + chunking). Needs no database lock.
pub fn prepare(path: &Path, on_progress: impl FnMut(u32, u32)) -> AppResult<Prepared> {
    let extracted = pdf::extract(path, on_progress)?;
    let page_count = extracted.pages.len() as u32;
    let text_pages = extracted.pages.iter().filter(|p| p.len() > 50).count() as u32;
    let chapters = pdf::detect_chapters(&extracted.outline, &extracted.pages);
    let chunks = chunk_pages(&extracted.pages, &chapters);
    Ok(Prepared { page_count, text_pages, chapters, chunks })
}

/// Replaces any previous index for the book with the prepared one.
pub fn store(conn: &mut Connection, fp: &str, p: &Prepared) -> AppResult<IndexStatus> {
    let Prepared { page_count, text_pages, chapters, chunks } = p;
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO chunks_fts(chunks_fts, rowid, text) SELECT 'delete', id, text FROM chunks WHERE fingerprint = ?1",
        [fp],
    )?;
    tx.execute("DELETE FROM chunks WHERE fingerprint = ?1", [fp])?;
    tx.execute("DELETE FROM chapters WHERE fingerprint = ?1", [fp])?;
    {
        let mut ins_ch = tx.prepare(
            "INSERT INTO chapters(fingerprint, idx, id, title, depth, start_page, end_page) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for c in chapters {
            ins_ch.execute(params![fp, c.idx, c.id, c.title, c.depth, c.start_page, c.end_page])?;
        }
        let mut ins_chunk = tx.prepare(
            "INSERT INTO chunks(fingerprint, chapter_idx, page_start, page_end, text) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        let mut ins_fts = tx.prepare("INSERT INTO chunks_fts(rowid, text) VALUES (?1, ?2)")?;
        for c in chunks {
            ins_chunk.execute(params![fp, c.chapter_idx, c.page_start, c.page_end, c.text])?;
            ins_fts.execute(params![tx.last_insert_rowid(), c.text])?;
        }
    }
    tx.execute(
        "INSERT INTO indexed_books(fingerprint, version, page_count, text_pages, indexed_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(fingerprint) DO UPDATE SET version = excluded.version, page_count = excluded.page_count,
             text_pages = excluded.text_pages, indexed_at = excluded.indexed_at",
        params![fp, INDEX_VERSION, page_count, text_pages, now()],
    )?;
    tx.commit()?;
    status(conn, fp)
}

pub fn chapters(conn: &Connection, fp: &str) -> AppResult<Vec<Chapter>> {
    let mut stmt = conn.prepare(
        "SELECT idx, id, title, depth, start_page, end_page FROM chapters WHERE fingerprint = ?1 ORDER BY idx",
    )?;
    let rows = stmt.query_map([fp], |r| {
        Ok(Chapter {
            idx: r.get(0)?,
            id: r.get(1)?,
            title: r.get(2)?,
            depth: r.get(3)?,
            start_page: r.get(4)?,
            end_page: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn chapter_chunks(conn: &Connection, fp: &str, chapter_idx: u32) -> AppResult<Vec<Chunk>> {
    let mut stmt = conn.prepare(
        "SELECT id, chapter_idx, page_start, page_end, text FROM chunks
         WHERE fingerprint = ?1 AND chapter_idx = ?2 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![fp, chapter_idx], |r| {
        Ok(Chunk { id: r.get(0)?, chapter_idx: r.get(1)?, page_start: r.get(2)?, page_end: r.get(3)?, text: r.get(4)? })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub chunk_id: i64,
    pub chapter_idx: u32,
    pub page_start: u32,
    pub page_end: u32,
    pub snippet: String,
}

/// Turns free text into a safe FTS5 query: quoted terms joined with OR.
fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.chars().count() >= 2)
        .take(16)
        .map(|t| format!("\"{}\"", t.to_lowercase()))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

pub fn search(conn: &Connection, fp: &str, query: &str, chapter_idx: Option<u32>, limit: u32) -> AppResult<Vec<SearchHit>> {
    let Some(q) = fts_query(query) else { return Ok(vec![]) };
    let mut stmt = conn.prepare(
        "SELECT c.id, c.chapter_idx, c.page_start, c.page_end,
                snippet(chunks_fts, 0, '«', '»', '…', 24)
         FROM chunks_fts JOIN chunks c ON c.id = chunks_fts.rowid
         WHERE chunks_fts MATCH ?1 AND c.fingerprint = ?2 AND (?3 IS NULL OR c.chapter_idx = ?3)
         ORDER BY bm25(chunks_fts) LIMIT ?4",
    )?;
    let rows = stmt.query_map(params![q, fp, chapter_idx, limit], |r| {
        Ok(SearchHit {
            chunk_id: r.get(0)?,
            chapter_idx: r.get(1)?,
            page_start: r.get(2)?,
            page_end: r.get(3)?,
            snippet: r.get::<_, String>(4)?.replace('\n', " "),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Picks chunks spread evenly across a chapter so the whole chapter is covered within a budget.
pub fn select_context(chunks: &[Chunk], budget_chars: usize) -> Vec<Chunk> {
    let total: usize = chunks.iter().map(|c| c.text.len()).sum();
    if total <= budget_chars || chunks.is_empty() {
        return chunks.to_vec();
    }
    let avg = total / chunks.len();
    let n = (budget_chars / avg.max(1)).clamp(1, chunks.len());
    let step = chunks.len() as f64 / n as f64;
    (0..n).map(|i| chunks[((i as f64 + 0.5) * step) as usize].clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_respect_chapters_and_size() {
        let pages: Vec<String> = (0..10).map(|i| format!("page {i} ").repeat(200)).collect();
        let chapters = vec![
            Chapter { idx: 0, id: "p1".into(), title: "A".into(), depth: 0, start_page: 1, end_page: 4 },
            Chapter { idx: 1, id: "p5".into(), title: "B".into(), depth: 0, start_page: 5, end_page: 10 },
        ];
        let chunks = chunk_pages(&pages, &chapters);
        assert!(chunks.iter().all(|c| c.text.len() < CHUNK_CHARS * 2));
        assert!(chunks.iter().filter(|c| c.chapter_idx == 0).all(|c| c.page_end <= 4));
        assert!(chunks.iter().filter(|c| c.chapter_idx == 1).all(|c| c.page_start >= 5));
    }

    #[test]
    fn fts_query_is_sanitized() {
        assert_eq!(fts_query("what's \"NEAR\" a-b?").unwrap(), "\"what\" OR \"near\"");
        assert!(fts_query("?!").is_none());
    }

    #[test]
    fn context_selection_spreads() {
        let chunks: Vec<Chunk> = (0..10)
            .map(|i| Chunk { id: i, chapter_idx: 0, page_start: i as u32, page_end: i as u32, text: "x".repeat(1000) })
            .collect();
        let sel = select_context(&chunks, 3000);
        assert_eq!(sel.len(), 3);
        assert!(sel[0].id < sel[1].id && sel[1].id < sel[2].id);
        assert!(sel[2].id >= 7);
    }
}
