//! The teacher: chapter lessons in two steps, written from the chapter's own text.
//!
//! Step 1 — a concise summary with every keyword in bold, plus a glossary of all the terms (with pages).
//! Long chapters are read in parts (one call each, sized to the model's context) and the notes are
//! combined in one more call. The parts also yield the chapter's topics, with their page ranges.
//!
//! Step 2 — each topic taught on demand in three layers: in plain words, in industry terms, in short.
//!
//! Lessons are stored in `bookshelf.sqlite` (and exported), so a lesson written on one PC is ready
//! on the other.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::index::Chunk;
use crate::llm::Message;
use crate::quiz::Source;

pub const PROMPT_VERSION: &str = "lesson.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Term {
    pub term: String,
    pub definition: String,
    pub page: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicLesson {
    /// Markdown.
    pub plain: String,
    pub industry: String,
    pub recap: String,
    pub terms: Vec<Term>,
    pub written_by: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Topic {
    pub title: String,
    pub page_start: u32,
    pub page_end: u32,
    pub lesson: Option<TopicLesson>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lesson {
    /// `<fingerprint>:<chapter id>` — the same on every PC.
    pub id: String,
    pub fingerprint: String,
    pub chapter_id: String,
    pub chapter_idx: u32,
    pub chapter_title: String,
    pub book_title: String,
    pub page_start: u32,
    pub page_end: u32,
    pub overview: String,
    /// Markdown bullets with **keywords** in bold.
    pub summary: Vec<String>,
    pub terms: Vec<Term>,
    pub topics: Vec<Topic>,
    pub model: Value,
    pub created_at: String,
    pub updated_at: String,
}

pub fn lesson_id(fp: &str, chapter_id: &str) -> String {
    format!("{fp}:{chapter_id}")
}

// ---------- step 1: summary, keywords, topics ----------

/// Splits a chapter into consecutive parts that each fit `budget` characters.
pub fn split_parts(chunks: &[Chunk], budget: usize) -> Vec<Vec<Chunk>> {
    let mut parts: Vec<Vec<Chunk>> = Vec::new();
    let mut size = 0;
    for c in chunks {
        if parts.is_empty() || (size + c.text.len() > budget && !parts.last().is_some_and(|p| p.is_empty())) {
            parts.push(Vec::new());
            size = 0;
        }
        size += c.text.len();
        parts.last_mut().expect("pushed").push(c.clone());
    }
    parts
}

fn excerpts_block(sources: &[Source]) -> String {
    sources
        .iter()
        .map(|s| format!("[{}] (pages {}-{})\n{}", s.label, s.page_start, s.page_end, s.excerpt.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn outline_messages(book: &str, chapter: &str, part: usize, parts: usize, sources: &[Source]) -> Vec<Message> {
    let scope = if parts == 1 {
        "the whole chapter".to_string()
    } else {
        format!("part {} of {} of the chapter (other parts are handled separately)", part + 1, parts)
    };
    let system = r#"You are an excellent teacher who helps a busy engineer learn a technical book without reading every page.

From the excerpts, write:
- "overview": 2-3 sentences: what this is about and why it matters.
- "summary": 5-10 concise bullet points that together cover ALL the important ideas. Wrap every technical keyword in **bold**.
- "terms": EVERY technical term, acronym, jargon word and named concept that appears (aim for completeness — this builds the reader's vocabulary). Each with a one-sentence plain definition and the excerpt label ("S1") where it appears.
- "topics": the 2-5 teachable topics, in reading order, each with a short title and the excerpt labels it covers.

Use ONLY the excerpts. Write in English.

Reply with a JSON object only, exactly in this shape:
{"overview":"...","summary":["..."],"terms":[{"term":"...","definition":"...","source":"S1"}],"topics":[{"title":"...","sources":["S1","S2"]}]}"#;
    let user = format!("Book: {book}\nChapter: {chapter}\nThese excerpts are {scope}.\n\nExcerpts:\n\n{}", excerpts_block(sources));
    vec![Message::system(system), Message::user(user)]
}

#[derive(Debug, Default, Deserialize)]
pub struct OutlineOut {
    #[serde(default)]
    pub overview: String,
    #[serde(default)]
    pub summary: Vec<Value>,
    #[serde(default)]
    pub terms: Vec<Value>,
    #[serde(default)]
    pub topics: Vec<Value>,
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn find_source<'a>(label: &Value, sources: &'a [Source]) -> Option<&'a Source> {
    let l = text(label).trim_matches(['[', ']']).to_uppercase();
    let l = if l.starts_with('S') { l } else { format!("S{l}") };
    sources.iter().find(|s| s.label == l)
}

pub fn validate_outline(o: &OutlineOut) -> Result<(), String> {
    if o.summary.iter().all(|s| text(s).is_empty()) {
        return Err("\"summary\" needs bullet points".into());
    }
    if o.topics.is_empty() {
        return Err("\"topics\" needs at least one topic with \"title\" and \"sources\"".into());
    }
    Ok(())
}

/// Bullet text without a leading "- ", "* " or "• " (the UI draws its own). "**bold**" stays intact.
pub fn bullet(s: &str) -> String {
    let s = s.trim();
    for marker in ["- ", "* ", "• ", "•"] {
        if let Some(rest) = s.strip_prefix(marker) {
            return rest.trim().to_string();
        }
    }
    s.to_string()
}

/// One part's notes, with source labels turned into pages.
pub struct PartNotes {
    pub overview: String,
    pub summary: Vec<String>,
    pub terms: Vec<Term>,
    pub topics: Vec<Topic>,
}

pub fn part_notes(o: &OutlineOut, sources: &[Source]) -> PartNotes {
    let terms = o
        .terms
        .iter()
        .filter_map(|t| {
            let term = t.get("term").map(text).filter(|s| !s.is_empty())?;
            let definition = t.get("definition").map(text).filter(|s| !s.is_empty())?;
            let page = t.get("source").and_then(|s| find_source(s, sources)).map(|s| s.page_start);
            Some(Term { term, definition, page })
        })
        .collect();
    let first = sources.first().map(|s| s.page_start).unwrap_or(1);
    let last = sources.last().map(|s| s.page_end).unwrap_or(first);
    let topics = o
        .topics
        .iter()
        .filter_map(|t| {
            let title = t.get("title").map(text).filter(|s| !s.is_empty())?;
            let cited: Vec<&Source> = t
                .get("sources")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|l| find_source(l, sources)).collect())
                .unwrap_or_default();
            let (start, end) = if cited.is_empty() {
                (first, last)
            } else {
                (cited.iter().map(|s| s.page_start).min().unwrap_or(first), cited.iter().map(|s| s.page_end).max().unwrap_or(last))
            };
            Some(Topic { title, page_start: start, page_end: end, lesson: None })
        })
        .collect();
    PartNotes {
        overview: o.overview.trim().to_string(),
        summary: o.summary.iter().map(|s| bullet(&text(s))).filter(|s| !s.is_empty()).collect(),
        terms,
        topics,
    }
}

/// Keeps the first definition of each term (case-insensitive), in order of appearance.
pub fn merge_terms(lists: impl IntoIterator<Item = Vec<Term>>) -> Vec<Term> {
    let mut out: Vec<Term> = Vec::new();
    for t in lists.into_iter().flatten() {
        let key = t.term.to_lowercase();
        if !out.iter().any(|x| x.term.to_lowercase() == key) {
            out.push(t);
        }
    }
    out
}

pub fn combine_messages(chapter: &str, notes: &[PartNotes]) -> Vec<Message> {
    let system = r#"You are an excellent teacher. Combine the notes from the parts of one chapter into one concise summary.

- "overview": 2-3 sentences: what the chapter is about and why it matters.
- "summary": 6-12 concise bullet points covering ALL the important ideas across the parts, in order. Keep every technical keyword in **bold**.

Reply with a JSON object only, exactly in this shape:
{"overview":"...","summary":["..."]}"#;
    let body = notes
        .iter()
        .enumerate()
        .map(|(i, n)| format!("Part {}:\n{}", i + 1, n.summary.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n")))
        .collect::<Vec<_>>()
        .join("\n\n");
    vec![Message::system(system), Message::user(format!("Chapter: {chapter}\n\n{body}"))]
}

#[derive(Debug, Default, Deserialize)]
pub struct CombineOut {
    #[serde(default)]
    pub overview: String,
    #[serde(default)]
    pub summary: Vec<Value>,
}

pub fn validate_combine(c: &CombineOut) -> Result<(), String> {
    if c.summary.iter().all(|s| text(s).is_empty()) {
        return Err("\"summary\" needs bullet points".into());
    }
    Ok(())
}

pub struct ChapterInfo<'a> {
    pub fingerprint: &'a str,
    pub book_title: &'a str,
    pub chapter: &'a crate::pdf::Chapter,
}

/// Puts the parts together. `combined` (overview, bullets) is used when the chapter had several parts.
pub fn build_lesson(info: &ChapterInfo, notes: Vec<PartNotes>, combined: Option<(String, Vec<String>)>, model: Value) -> Lesson {
    let (overview, summary) = match combined {
        Some(c) => c,
        None => {
            let overview = notes.first().map(|n| n.overview.clone()).unwrap_or_default();
            (overview, notes.iter().flat_map(|n| n.summary.clone()).collect())
        }
    };
    let mut topics: Vec<Topic> = Vec::new();
    for t in notes.iter().flat_map(|n| n.topics.clone()) {
        // Parts sometimes continue the same topic: merge neighbours with the same title.
        match topics.last_mut() {
            Some(prev) if prev.title.eq_ignore_ascii_case(&t.title) => prev.page_end = prev.page_end.max(t.page_end),
            _ => topics.push(t),
        }
    }
    let ts = now();
    Lesson {
        id: lesson_id(info.fingerprint, &info.chapter.id),
        fingerprint: info.fingerprint.into(),
        chapter_id: info.chapter.id.clone(),
        chapter_idx: info.chapter.idx,
        chapter_title: info.chapter.title.clone(),
        book_title: info.book_title.into(),
        page_start: info.chapter.start_page,
        page_end: info.chapter.end_page,
        overview,
        summary,
        terms: merge_terms(notes.into_iter().map(|n| n.terms)),
        topics,
        model,
        created_at: ts.clone(),
        updated_at: ts,
    }
}

// ---------- step 2: teaching one topic ----------

/// The chapter chunks that overlap a topic's pages.
pub fn topic_chunks(chunks: &[Chunk], t: &Topic) -> Vec<Chunk> {
    let hit: Vec<Chunk> = chunks.iter().filter(|c| c.page_end >= t.page_start && c.page_start <= t.page_end).cloned().collect();
    if hit.is_empty() { chunks.to_vec() } else { hit }
}

pub fn teach_messages(l: &Lesson, topic: &Topic, sources: &[Source]) -> Vec<Message> {
    let system = r#"You are a patient, excellent teacher. Teach ONE topic from a technical book chapter, using the excerpts, in three layers:

1. "plain": explain it in everyday language for someone new to it — use an analogy or a familiar situation, short sentences, no jargon (or explain any jargon right away). 1-3 short paragraphs.
2. "industry": now explain the SAME idea the way a professional engineer would: the precise technical terms (in **bold**), how it is used in real systems and codebases, common pitfalls and trade-offs. If the excerpts contain code or a worked example, include a short one in a fenced code block. 1-4 paragraphs.
3. "recap": 3-6 bullet points ("- ...") that summarize the topic, keywords in **bold**.
Also "terms": the key technical terms of this topic, each with a one-sentence definition.

Stay faithful to the excerpts; do not invent facts the book does not support. Markdown is allowed inside the strings. Write in English.

Reply with a JSON object only, exactly in this shape:
{"plain":"...","industry":"...","recap":"- ...\n- ...","terms":[{"term":"...","definition":"..."}]}"#;
    let user = format!(
        "Book: {}\nChapter: {}\nTopic: {} (pages {}-{})\n\nExcerpts:\n\n{}",
        l.book_title,
        l.chapter_title,
        topic.title,
        topic.page_start,
        topic.page_end,
        excerpts_block(sources)
    );
    vec![Message::system(system), Message::user(user)]
}

#[derive(Debug, Default, Deserialize)]
pub struct TeachOut {
    #[serde(default)]
    pub plain: String,
    #[serde(default)]
    pub industry: String,
    #[serde(default)]
    pub recap: Value,
    #[serde(default)]
    pub terms: Vec<Value>,
}

pub fn validate_teach(t: &TeachOut) -> Result<(), String> {
    if t.plain.trim().len() < 40 {
        return Err("\"plain\" must explain the topic in everyday language".into());
    }
    if t.industry.trim().len() < 40 {
        return Err("\"industry\" must explain the topic in professional terms".into());
    }
    Ok(())
}

pub fn to_topic_lesson(t: &TeachOut, page: u32, written_by: Value) -> TopicLesson {
    let recap = match &t.recap {
        Value::Array(items) => items.iter().map(|i| format!("- {}", bullet(&text(i)))).collect::<Vec<_>>().join("\n"),
        other => text(other),
    };
    TopicLesson {
        plain: t.plain.trim().to_string(),
        industry: t.industry.trim().to_string(),
        recap,
        terms: t
            .terms
            .iter()
            .filter_map(|x| {
                Some(Term {
                    term: x.get("term").map(text).filter(|s| !s.is_empty())?,
                    definition: x.get("definition").map(text).filter(|s| !s.is_empty())?,
                    page: Some(page),
                })
            })
            .collect(),
        written_by,
        created_at: now(),
    }
}

/// Adds a topic's new terms to the chapter glossary.
pub fn absorb_terms(l: &mut Lesson, idx: usize) {
    let extra = l.topics[idx].lesson.as_ref().map(|t| t.terms.clone()).unwrap_or_default();
    l.terms = merge_terms([std::mem::take(&mut l.terms), extra]);
}

// ---------- persistence ----------

pub fn save(conn: &Connection, l: &Lesson, device_id: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO lessons(id, fingerprint, chapter_id, chapter_title, data, created_at, updated_at, device_id, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0)
         ON CONFLICT(id) DO UPDATE SET chapter_title = excluded.chapter_title, data = excluded.data,
             updated_at = excluded.updated_at, device_id = excluded.device_id, deleted = 0",
        params![l.id, l.fingerprint, l.chapter_id, l.chapter_title, serde_json::to_string(l)?, l.created_at, l.updated_at, device_id],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> AppResult<Option<Lesson>> {
    let data: Option<String> = conn
        .query_row("SELECT data FROM lessons WHERE id = ?1 AND deleted = 0", [id], |r| r.get(0))
        .optional()?;
    Ok(data.and_then(|d| serde_json::from_str(&d).ok()))
}

pub fn require(conn: &Connection, id: &str) -> AppResult<Lesson> {
    get(conn, id)?.ok_or_else(|| AppError::NotFound("Lesson not found — summarize the chapter first".into()))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LessonSummary {
    pub id: String,
    pub fingerprint: String,
    pub book_title: String,
    pub chapter_idx: u32,
    pub chapter_title: String,
    pub terms: usize,
    pub topics: usize,
    pub topics_taught: usize,
    pub updated_at: String,
}

pub fn list(conn: &Connection) -> AppResult<Vec<LessonSummary>> {
    let mut stmt = conn.prepare("SELECT data FROM lessons WHERE deleted = 0 ORDER BY updated_at DESC")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows
        .filter_map(|r| r.ok())
        .filter_map(|d| serde_json::from_str::<Lesson>(&d).ok())
        .map(|l| LessonSummary {
            topics_taught: l.topics.iter().filter(|t| t.lesson.is_some()).count(),
            topics: l.topics.len(),
            terms: l.terms.len(),
            id: l.id,
            fingerprint: l.fingerprint,
            book_title: l.book_title,
            chapter_idx: l.chapter_idx,
            chapter_title: l.chapter_title,
            updated_at: l.updated_at,
        })
        .collect())
}

pub fn delete(conn: &Connection, id: &str, device_id: &str) -> AppResult<()> {
    conn.execute("UPDATE lessons SET deleted = 1, updated_at = ?2, device_id = ?3 WHERE id = ?1", params![id, now(), device_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, pdf::Chapter};

    fn chunk(id: i64, p: u32, len: usize) -> Chunk {
        Chunk { id, chapter_idx: 0, page_start: p, page_end: p + 1, text: "x".repeat(len) }
    }

    fn src(label: &str, p: u32) -> Source {
        Source { label: label.into(), page_start: p, page_end: p + 1, excerpt: String::new() }
    }

    #[test]
    fn parts_fit_the_budget() {
        let chunks: Vec<Chunk> = (0..10).map(|i| chunk(i, 10 + i as u32 * 2, 1000)).collect();
        let parts = split_parts(&chunks, 3500);
        assert_eq!(parts.iter().map(Vec::len).collect::<Vec<_>>(), [3, 3, 3, 1]);
        assert_eq!(split_parts(&chunks, 100_000).len(), 1);
        // A single chunk bigger than the budget still gets its own part.
        assert_eq!(split_parts(&[chunk(1, 1, 9000), chunk(2, 3, 10)], 5000).len(), 2);
    }

    #[test]
    fn notes_merge_into_a_lesson() {
        let a: OutlineOut = serde_json::from_str(
            r#"{"overview":"About hashing.","summary":["- **Hash tables** map keys to buckets."],
                "terms":[{"term":"Hash function","definition":"Maps a key to an index.","source":"S2"},{"term":"","definition":"x"}],
                "topics":[{"title":"Hash functions","sources":["S1","S2"]},{"title":"Separate chaining","sources":["S9"]}]}"#,
        )
        .unwrap();
        let b: OutlineOut = serde_json::from_str(
            r#"{"summary":["**Linear probing** keeps entries in one array."],
                "terms":[{"term":"hash function","definition":"dup","source":"S1"},{"term":"Load factor","definition":"N/M.","source":"S1"}],
                "topics":[{"title":"separate chaining","sources":["S1"]},{"title":"Linear probing","sources":["S1"]}]}"#,
        )
        .unwrap();
        let na = part_notes(&a, &[src("S1", 460), src("S2", 462)]);
        let nb = part_notes(&b, &[src("S1", 470)]);
        assert_eq!(na.terms[0].page, Some(462));
        assert_eq!((na.topics[0].page_start, na.topics[0].page_end), (460, 463));
        assert_eq!((na.topics[1].page_start, na.topics[1].page_end), (460, 463), "unknown labels → the whole part");
        assert_eq!(na.summary[0], "**Hash tables** map keys to buckets.");
        assert_eq!(bullet("* **Bold** first"), "**Bold** first");

        let chapter = Chapter { idx: 7, id: "p460".into(), title: "3.4 Hash Tables".into(), depth: 0, start_page: 460, end_page: 480 };
        let info = ChapterInfo { fingerprint: "fp", book_title: "Algorithms", chapter: &chapter };
        let l = build_lesson(&info, vec![na, nb], Some(("Combined.".into(), vec!["One".into()])), Value::Null);
        assert_eq!(l.id, "fp:p460");
        assert_eq!(l.terms.len(), 2, "duplicate term dropped");
        assert_eq!(l.topics.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Hash functions", "Separate chaining", "Linear probing"]);
        assert_eq!(l.topics[1].page_end, 471, "continued topic merged across parts");
        assert_eq!(l.overview, "Combined.");
    }

    #[test]
    fn topic_lessons_and_storage() {
        let conn = Connection::open_in_memory().unwrap();
        db::migrate(&conn).unwrap();
        let chapter = Chapter { idx: 0, id: "p1".into(), title: "Intro".into(), depth: 0, start_page: 1, end_page: 9 };
        let info = ChapterInfo { fingerprint: "fp", book_title: "B", chapter: &chapter };
        let outline: OutlineOut =
            serde_json::from_str(r#"{"overview":"o","summary":["s"],"terms":[],"topics":[{"title":"T","sources":["S1"]}]}"#).unwrap();
        let mut l = build_lesson(&info, vec![part_notes(&outline, &[src("S1", 3)])], None, Value::Null);
        let chunks = vec![chunk(1, 1, 10), chunk(2, 3, 10), chunk(3, 7, 10)];
        assert_eq!(topic_chunks(&chunks, &l.topics[0]).iter().map(|c| c.id).collect::<Vec<_>>(), [2]);

        let out: TeachOut = serde_json::from_str(
            r#"{"plain":"Think of a library card catalog that tells you which shelf to walk to.","industry":"A **hash function** maps keys to bucket indexes in O(1) on average.","recap":["**Hashing** is fast","Collisions happen"],"terms":[{"term":"Bucket","definition":"A slot."}]}"#,
        )
        .unwrap();
        assert!(validate_teach(&out).is_ok());
        l.topics[0].lesson = Some(to_topic_lesson(&out, 3, Value::Null));
        absorb_terms(&mut l, 0);
        assert_eq!(l.topics[0].lesson.as_ref().unwrap().recap, "- **Hashing** is fast\n- Collisions happen");
        assert_eq!(l.terms[0].term, "Bucket");

        save(&conn, &l, "dev").unwrap();
        let back = get(&conn, "fp:p1").unwrap().unwrap();
        assert!(back.topics[0].lesson.is_some());
        assert_eq!(list(&conn).unwrap()[0].topics_taught, 1);
        delete(&conn, "fp:p1", "dev").unwrap();
        assert!(get(&conn, "fp:p1").unwrap().is_none());
    }
}
