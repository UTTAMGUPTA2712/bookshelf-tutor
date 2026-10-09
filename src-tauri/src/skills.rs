//! Topic taxonomy, chapter→topic mapping, the skill map and the roadmap.
//!
//! The skill map is never stored: it is recomputed from attempts (the evidence), so merging exports
//! from several machines can't produce conflicting mastery numbers.

use std::collections::{HashMap, HashSet};

use chrono::{Datelike, Duration, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::library::BookView;
use crate::pdf::Chapter;
use crate::quiz::Attempt;

// ---------- taxonomy ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Topic {
    pub id: String,
    pub label: String,
    pub description: String,
    pub prereqs: Vec<String>,
    #[serde(default)]
    pub book_tags: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackTopics {
    pub id: String,
    pub label: String,
    pub book_tags: Vec<String>,
    /// Listed so that every prerequisite comes before the topics that need it.
    pub topics: Vec<Topic>,
}

#[derive(Deserialize)]
struct Taxonomy {
    tracks: Vec<TrackTopics>,
}

fn taxonomy() -> &'static Taxonomy {
    static T: std::sync::OnceLock<Taxonomy> = std::sync::OnceLock::new();
    T.get_or_init(|| serde_json::from_str(include_str!("../resources/taxonomy.json")).expect("resources/taxonomy.json is valid"))
}

pub fn tracks() -> &'static [TrackTopics] {
    &taxonomy().tracks
}

pub fn track(id: &str) -> AppResult<&'static TrackTopics> {
    tracks()
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| AppError::NotFound(format!("There is no skill map for the track “{id}”")))
}

pub fn topic(id: &str) -> Option<&'static Topic> {
    tracks().iter().flat_map(|t| &t.topics).find(|t| t.id == id)
}

/// Topic graph depth: 0 for topics without prerequisites.
fn layers(track: &TrackTopics) -> HashMap<&str, u32> {
    let mut out: HashMap<&str, u32> = HashMap::new();
    for t in &track.topics {
        let d = t.prereqs.iter().filter_map(|p| out.get(p.as_str())).map(|d| d + 1).max().unwrap_or(0);
        out.insert(&t.id, d);
    }
    out
}

// ---------- chapter → topic mapping ----------

fn norm_tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '+')
        .filter(|w| !w.is_empty())
        .map(|w| {
            // crude plural folding: "classes" → "class", "tables" → "table"
            if w.len() > 4 && w.ends_with("sses") {
                w[..w.len() - 2].to_string()
            } else if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") {
                w[..w.len() - 1].to_string()
            } else {
                w.to_string()
            }
        })
        .collect()
}

fn title_matches(title: &str, keywords: &[String]) -> bool {
    let words = norm_tokens(title);
    keywords.iter().any(|k| {
        let kw = norm_tokens(k);
        !kw.is_empty() && words.windows(kw.len()).any(|w| w == kw.as_slice())
    })
}

/// One FTS5 expression matching any keyword (each keyword as a phrase).
fn fts_expr(keywords: &[String]) -> Option<String> {
    let phrases: Vec<String> = keywords
        .iter()
        .map(|k| {
            k.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
        })
        .filter(|p| !p.is_empty())
        .map(|p| format!("\"{p}\""))
        .collect();
    (!phrases.is_empty()).then(|| phrases.join(" OR "))
}

/// Covers, front/back matter, index letters and chapter summaries never teach a topic.
fn is_back_matter(title: &str) -> bool {
    let t = title.trim().to_lowercase();
    if t.chars().filter(|c| c.is_alphanumeric()).count() <= 2 {
        return true; // "A", "B" … index sections
    }
    const EXACT: &[&str] = &["index", "references", "summary", "contributors", "credits"];
    const CONTAINS: &[&str] = &[
        "front matter", "front-matter", "back matter", "back-matter",
        "contents", "bibliograph", "preface", "foreword", "prologue", "acknowledg", "about the author", "copyright",
        "glossary", "colophon", "cover", "this book", "this edition", "chapter summary", "recommended reading",
        "reading list", "further reading", "reference sites", "other books", "参考书目", "词汇表", "目录",
    ];
    EXACT.contains(&t.as_str()) || t.ends_with(" index") || CONTAINS.iter().any(|x| t.contains(x))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterMatch {
    pub chapter: Chapter,
    pub score: f32,
    pub title_match: bool,
}

const MATCH_THRESHOLD: f32 = 0.6;
/// Chapters found by text alone (no title match) must score at least this to be suggested.
const STRONG_BODY: f32 = 1.0;

/// The chapters worth suggesting: those named after the topic if any, else clear text matches.
fn best_matches(matches: &[ChapterMatch]) -> Vec<&ChapterMatch> {
    let titled: Vec<&ChapterMatch> = matches.iter().filter(|m| m.title_match).collect();
    if !titled.is_empty() {
        return titled;
    }
    matches.iter().filter(|m| m.score >= STRONG_BODY).collect()
}

/// How strongly each chapter of an indexed book covers a topic: title keywords plus how much more often
/// the chapter mentions the topic's keywords than the book as a whole does.
pub fn topic_chapters(idx: &Connection, fp: &str, topic: &Topic) -> AppResult<Vec<ChapterMatch>> {
    let chapters = crate::index::chapters(idx, fp)?;
    let Some(expr) = fts_expr(&topic.keywords) else { return Ok(vec![]) };
    if chapters.is_empty() {
        return Ok(vec![]);
    }
    let mut sizes: HashMap<u32, u32> = HashMap::new();
    let mut stmt = idx.prepare_cached("SELECT chapter_idx, COUNT(*) FROM chunks WHERE fingerprint = ?1 GROUP BY chapter_idx")?;
    for row in stmt.query_map([fp], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?)))? {
        let (c, n) = row?;
        sizes.insert(c, n);
    }
    let mut hits: HashMap<u32, u32> = HashMap::new();
    let mut stmt = idx.prepare_cached(
        "SELECT c.chapter_idx, COUNT(*) FROM chunks_fts JOIN chunks c ON c.id = chunks_fts.rowid
         WHERE chunks_fts MATCH ?1 AND c.fingerprint = ?2 GROUP BY c.chapter_idx",
    )?;
    for row in stmt.query_map(params![expr, fp], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?)))? {
        let (c, n) = row?;
        hits.insert(c, n);
    }
    let total_chunks: u32 = sizes.values().sum();
    let total_hits: u32 = hits.values().sum();
    let base = if total_chunks > 0 { total_hits as f32 / total_chunks as f32 } else { 0.0 };

    let chapters: Vec<Chapter> = chapters.into_iter().filter(|c| !is_back_matter(&c.title)).collect();
    // When no chapter is named after the topic and the whole book is about it, fall back to
    // chapters that mention it nearly everywhere.
    let any_title = chapters.iter().any(|c| title_matches(&c.title, &topic.keywords));
    let mut out = Vec::new();
    for ch in chapters {
        let n = sizes.get(&ch.idx).copied().unwrap_or(0);
        if n == 0 {
            continue;
        }
        let frac = hits.get(&ch.idx).copied().unwrap_or(0) as f32 / n as f32;
        let lift = if base > 0.0 { frac / base } else { 0.0 };
        // Tiny chapters say little: a two-chunk "chapter" full of keywords is usually a summary page.
        let size_weight = (n as f32 / 4.0).min(1.0);
        let body = if base >= 0.5 {
            if !any_title && frac >= 0.9 && n >= 3 { 0.6 } else { 0.0 }
        } else if frac >= 0.3 && lift >= 1.5 {
            (lift.min(4.0) - 1.0) * frac * size_weight
        } else {
            0.0
        };
        let title_match = title_matches(&ch.title, &topic.keywords);
        let score = body + if title_match { 2.0 } else { 0.0 };
        if score >= MATCH_THRESHOLD {
            out.push(ChapterMatch { chapter: ch, score, title_match });
        }
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

/// Topics a book can teach: those whose book tags overlap the book's tags.
fn book_topics(book: &BookView) -> Vec<&'static Topic> {
    let tags: HashSet<&str> = book.topics.iter().map(String::as_str).collect();
    tracks()
        .iter()
        .flat_map(|t| &t.topics)
        .filter(|t| t.book_tags.iter().any(|g| tags.contains(g.as_str())))
        .collect()
}

/// The (at most 3) topics a chapter quiz is evidence for.
pub fn chapter_topics(idx: &Connection, book: &BookView, chapter_idx: u32) -> AppResult<Vec<String>> {
    let mut scored = Vec::new();
    for t in book_topics(book) {
        let matches = topic_chapters(idx, &book.fingerprint, t)?;
        if let Some(m) = best_matches(&matches).into_iter().find(|m| m.chapter.idx == chapter_idx) {
            scored.push((m.score, t.id.clone()));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut ids: Vec<String> = Vec::new();
    for (_, id) in scored {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.truncate(3);
    Ok(ids)
}

// ---------- mastery ----------

fn difficulty_level(d: Option<&str>) -> f32 {
    match d {
        Some("easy") => 25.0,
        Some("hard") => 85.0,
        _ => 55.0,
    }
}

/// What one graded question says about mastery: a right answer shows you're above its level,
/// a wrong one that you're below it.
pub fn question_estimate(difficulty: Option<&str>, score: u32) -> f32 {
    let level = difficulty_level(difficulty);
    let s = score as f32 / 100.0;
    (s * (level + 15.0) + (1.0 - s) * (level - 30.0)).clamp(0.0, 100.0)
}

#[derive(Debug, Clone)]
pub struct Evidence {
    pub topic: String,
    pub estimate: f32,
    pub weight: f32,
    pub at: String,
}

pub fn evidence_from(a: &Attempt) -> Vec<Evidence> {
    match a.kind.as_str() {
        "assessment" => {
            let mut by_topic: HashMap<&str, Vec<f32>> = HashMap::new();
            for q in &a.questions {
                if let (Some(t), Some(g)) = (&q.topic, &q.grade) {
                    by_topic.entry(t).or_default().push(question_estimate(q.difficulty.as_deref(), g.score));
                }
            }
            by_topic
                .into_iter()
                .map(|(t, es)| Evidence {
                    topic: t.to_string(),
                    estimate: es.iter().sum::<f32>() / es.len() as f32,
                    weight: (es.len() as f32 / 2.0).min(1.0),
                    at: a.updated_at.clone(),
                })
                .collect()
        }
        _ => {
            let Some(o) = a.overall.as_ref().filter(|_| a.status == "completed") else { return vec![] };
            let graded = a.questions.iter().filter(|q| q.grade.is_some()).count();
            a.topics
                .iter()
                .map(|t| Evidence {
                    topic: t.clone(),
                    estimate: o.score as f32,
                    weight: (graded as f32 / 5.0).min(1.0),
                    at: a.updated_at.clone(),
                })
                .collect()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mastery {
    pub mastery: u32,
    pub confidence: f32,
    pub evidence: u32,
}

/// Newer evidence counts more; confidence grows with the amount of evidence.
pub fn combine(evidence: &[&Evidence]) -> Option<Mastery> {
    let mut ev: Vec<&&Evidence> = evidence.iter().filter(|e| e.weight > 0.0).collect();
    if ev.is_empty() {
        return None;
    }
    ev.sort_by(|a, b| b.at.cmp(&a.at));
    let (mut num, mut den, mut total) = (0.0f32, 0.0f32, 0.0f32);
    for (i, e) in ev.iter().enumerate() {
        let w = e.weight * 0.75f32.powi(i as i32);
        num += w * e.estimate;
        den += w;
        total += e.weight;
    }
    Some(Mastery {
        mastery: (num / den).round() as u32,
        confidence: 1.0 - 0.5f32.powf(total),
        evidence: ev.len() as u32,
    })
}

pub fn mastery_map(attempts: &[Attempt]) -> HashMap<String, Mastery> {
    let evidence: Vec<Evidence> = attempts.iter().flat_map(evidence_from).collect();
    let mut by_topic: HashMap<&str, Vec<&Evidence>> = HashMap::new();
    for e in &evidence {
        by_topic.entry(&e.topic).or_default().push(e);
    }
    by_topic.into_iter().filter_map(|(t, ev)| combine(&ev).map(|m| (t.to_string(), m))).collect()
}

pub fn status_of(m: Option<&Mastery>) -> &'static str {
    match m.map(|m| m.mastery) {
        None => "untested",
        Some(0..50) => "weak",
        Some(50..75) => "learning",
        Some(_) => "strong",
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicView {
    pub id: String,
    pub label: String,
    pub description: String,
    pub prereqs: Vec<String>,
    pub layer: u32,
    pub mastery: Option<u32>,
    pub confidence: f32,
    pub evidence: u32,
    pub status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillMap {
    pub track: String,
    pub label: String,
    pub topics: Vec<TopicView>,
    /// Latest skill test for this track (finished or not).
    pub last_assessment: Option<AssessmentRef>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssessmentRef {
    pub id: String,
    pub status: String,
    pub score: Option<u32>,
    pub summary: Option<String>,
    pub strengths: Vec<String>,
    pub focus_next: Vec<String>,
    pub created_at: String,
}

pub fn assessment_track(a: &Attempt) -> Option<&str> {
    a.model.get("track").and_then(|v| v.as_str())
}

pub fn skill_map(track_id: &str, attempts: &[Attempt]) -> AppResult<SkillMap> {
    let track = track(track_id)?;
    let mastery = mastery_map(attempts);
    let layer = layers(track);
    let topics = track
        .topics
        .iter()
        .map(|t| {
            let m = mastery.get(&t.id);
            TopicView {
                id: t.id.clone(),
                label: t.label.clone(),
                description: t.description.clone(),
                prereqs: t.prereqs.clone(),
                layer: layer[t.id.as_str()],
                mastery: m.map(|m| m.mastery),
                confidence: m.map(|m| m.confidence).unwrap_or(0.0),
                evidence: m.map(|m| m.evidence).unwrap_or(0),
                status: status_of(m).into(),
            }
        })
        .collect();
    let last_assessment = attempts
        .iter()
        .filter(|a| a.kind == "assessment" && assessment_track(a) == Some(track_id))
        .max_by(|a, b| a.created_at.cmp(&b.created_at))
        .map(|a| AssessmentRef {
            id: a.id.clone(),
            status: a.status.clone(),
            score: a.overall.as_ref().map(|o| o.score),
            summary: a.overall.as_ref().map(|o| o.summary.clone()),
            strengths: a.overall.as_ref().map(|o| o.strengths.clone()).unwrap_or_default(),
            focus_next: a.overall.as_ref().map(|o| o.focus_next.clone()).unwrap_or_default(),
            created_at: a.created_at.clone(),
        });
    Ok(SkillMap { track: track.id.clone(), label: track.label.clone(), topics, last_assessment })
}

// ---------- roadmap ----------

/// Topics above this mastery are considered learned and get no roadmap step.
const LEARNED: u32 = 75;

fn status_priority(status: &str) -> f32 {
    match status {
        "start-here" => 4.0,
        "core" => 3.0,
        "reference" => 2.0,
        "later" => 1.0,
        _ => 0.0,
    }
}

fn in_progress(b: &BookView) -> bool {
    b.progress.as_ref().is_some_and(|p| p.status == "reading" || p.last_page > 1 || !p.chapters_done.is_empty())
}

/// Books the roadmap may point to for a topic: on this PC, never outdated, tags overlapping the topic's.
pub fn candidate_books<'a>(topic: &Topic, books: &'a [BookView]) -> Vec<&'a BookView> {
    let mut out: Vec<&BookView> = books
        .iter()
        .filter(|b| b.path.is_some() && b.status != "outdated" && status_priority(&b.status) > 0.0)
        .filter(|b| b.topics.iter().any(|t| topic.book_tags.contains(t)))
        .collect();
    out.sort_by(|a, b| book_rank(b).total_cmp(&book_rank(a)));
    out
}

fn book_rank(b: &BookView) -> f32 {
    status_priority(&b.status) + if in_progress(b) { 1.5 } else { 0.0 }
}

/// Every book a track's roadmap could use (to index them up front).
pub fn track_books<'a>(track: &TrackTopics, books: &'a [BookView]) -> Vec<&'a BookView> {
    let mut seen = HashSet::new();
    track
        .topics
        .iter()
        .flat_map(|t| candidate_books(t, books))
        .filter(|b| seen.insert(b.fingerprint.clone()))
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookRef {
    pub fingerprint: String,
    pub title: String,
    pub status: String,
}

impl From<&BookView> for BookRef {
    fn from(b: &BookView) -> Self {
        BookRef { fingerprint: b.fingerprint.clone(), title: b.title.clone(), status: b.status.clone() }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepChapter {
    pub idx: u32,
    pub id: String,
    pub title: String,
    pub start_page: u32,
    pub end_page: u32,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub topic_id: String,
    pub label: String,
    pub mastery: Option<u32>,
    pub status: String,
    pub reason: String,
    pub book: Option<BookRef>,
    pub chapters: Vec<StepChapter>,
    /// Other books that also cover the topic.
    pub alternatives: Vec<BookRef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeekItem {
    pub topic_id: String,
    pub topic_label: String,
    pub book: BookRef,
    pub chapter: StepChapter,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Week {
    pub number: u32,
    pub starts: String,
    pub items: Vec<WeekItem>,
    pub pages: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Roadmap {
    pub track: String,
    pub label: String,
    pub pace: u32,
    pub steps: Vec<Step>,
    pub weeks: Vec<Week>,
    /// Topics already learned (mastery ≥ 75).
    pub learned: Vec<String>,
    /// Candidate books that must be indexed before their chapters can be suggested.
    pub needs_index: Vec<BookRef>,
}

/// Matched chapters per (topic, book), from the index. Books that aren't indexed have no entry.
pub type ChapterIndex = HashMap<(String, String), Vec<ChapterMatch>>;

pub fn chapter_index(idx: &Connection, track: &TrackTopics, books: &[BookView]) -> AppResult<(ChapterIndex, Vec<BookRef>)> {
    let mut out = ChapterIndex::new();
    let mut needs_index = Vec::new();
    for b in track_books(track, books) {
        if !crate::index::status(idx, &b.fingerprint)?.indexed {
            needs_index.push(BookRef::from(b));
            continue;
        }
        for t in &track.topics {
            if candidate_books(t, std::slice::from_ref(b)).is_empty() {
                continue;
            }
            out.insert((t.id.clone(), b.fingerprint.clone()), topic_chapters(idx, &b.fingerprint, t)?);
        }
    }
    Ok((out, needs_index))
}

/// Weakest available topic first, never before its prerequisites; each step points to chapters in
/// the best book on this PC; chapters are then spread over weeks at the chosen pace.
pub fn roadmap(
    track_id: &str,
    attempts: &[Attempt],
    books: &[BookView],
    chapters: &ChapterIndex,
    needs_index: Vec<BookRef>,
    pace: u32,
) -> AppResult<Roadmap> {
    let track = track(track_id)?;
    let mastery = mastery_map(attempts);
    let pace = pace.clamp(1, 20);

    let learned: Vec<String> = track
        .topics
        .iter()
        .filter(|t| mastery.get(&t.id).is_some_and(|m| m.mastery >= LEARNED))
        .map(|t| t.id.clone())
        .collect();
    // Untested topics sort as "probably weak", after topics known to be weak.
    let sort_key = |t: &Topic| mastery.get(&t.id).map(|m| m.mastery as f32).unwrap_or(45.0);

    let mut placed: HashSet<&str> = learned.iter().map(String::as_str).collect();
    let mut remaining: Vec<&Topic> = track.topics.iter().filter(|t| !placed.contains(t.id.as_str())).collect();
    let mut order: Vec<&Topic> = Vec::new();
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .enumerate()
            .filter(|(_, t)| t.prereqs.iter().all(|p| placed.contains(p.as_str())))
            .min_by(|(_, a), (_, b)| sort_key(a).total_cmp(&sort_key(b)))
            .map(|(i, _)| i)
            .unwrap_or(0); // only reachable with a broken taxonomy; keep going in listed order
        let t = remaining.remove(ready);
        placed.insert(&t.id);
        order.push(t);
    }

    let done_chapters = |fp: &str| -> HashSet<String> {
        books
            .iter()
            .find(|b| b.fingerprint == fp)
            .and_then(|b| b.progress.as_ref())
            .map(|p| p.chapters_done.iter().cloned().collect())
            .unwrap_or_default()
    };

    let mut steps = Vec::new();
    for t in order {
        let m = mastery.get(&t.id);
        let unlocks: Vec<&str> = track
            .topics
            .iter()
            .filter(|o| o.prereqs.contains(&t.id))
            .map(|o| o.label.as_str())
            .take(2)
            .collect();
        let mut reason = match m {
            None => "Not tested yet.".to_string(),
            Some(m) if m.mastery < 50 => format!("Weak spot — mastery {}%.", m.mastery),
            Some(m) => format!("Almost there — mastery {}%.", m.mastery),
        };
        if !unlocks.is_empty() {
            reason.push_str(&format!(" Needed for {}.", unlocks.join(" and ")));
        }

        // Best book with matching chapters; ties go to start-here/core books and books in progress.
        let candidates = candidate_books(t, books);
        let mut with_chapters: Vec<(&BookView, Vec<&ChapterMatch>)> = candidates
            .iter()
            .filter_map(|b| {
                let best = best_matches(chapters.get(&(t.id.clone(), b.fingerprint.clone()))?);
                (!best.is_empty()).then_some((*b, best))
            })
            .collect();
        with_chapters.sort_by(|(a, ac), (b, bc)| {
            let rank = |bk: &BookView, cs: &[&ChapterMatch]| book_rank(bk) + 0.5 * cs[0].score.min(3.0);
            rank(b, bc).total_cmp(&rank(a, ac))
        });
        let (book, step_chapters) = match with_chapters.first() {
            Some((b, matches)) => {
                let done = done_chapters(&b.fingerprint);
                let mut picked: Vec<&ChapterMatch> =
                    matches.iter().copied().filter(|c| !done.contains(&c.chapter.id)).take(3).collect();
                if picked.is_empty() {
                    picked = matches.iter().copied().take(2).collect(); // all read already: suggest a re-read
                }
                picked.sort_by_key(|c| c.chapter.idx);
                let chs = picked
                    .into_iter()
                    .map(|c| StepChapter {
                        idx: c.chapter.idx,
                        id: c.chapter.id.clone(),
                        title: c.chapter.title.clone(),
                        start_page: c.chapter.start_page,
                        end_page: c.chapter.end_page,
                        done: done.contains(&c.chapter.id),
                    })
                    .collect();
                (Some(BookRef::from(*b)), chs)
            }
            // Not indexed (or nothing matched): still name the best book.
            None => (candidates.first().map(|b| BookRef::from(*b)), vec![]),
        };
        let alternatives = candidates
            .iter()
            .filter(|b| book.as_ref().is_none_or(|x| x.fingerprint != b.fingerprint))
            .filter(|b| chapters.get(&(t.id.clone(), b.fingerprint.clone())).is_some_and(|c| !best_matches(c).is_empty()))
            .take(2)
            .map(|b| BookRef::from(*b))
            .collect();
        steps.push(Step {
            topic_id: t.id.clone(),
            label: t.label.clone(),
            mastery: m.map(|m| m.mastery),
            status: status_of(m).into(),
            reason,
            book,
            chapters: step_chapters,
            alternatives,
        });
    }

    let weeks = weekly_plan(&steps, pace);
    Ok(Roadmap { track: track.id.clone(), label: track.label.clone(), pace, steps, weeks, learned, needs_index })
}

fn weekly_plan(steps: &[Step], pace: u32) -> Vec<Week> {
    let monday = {
        let today = Utc::now().date_naive();
        today - Duration::days(today.weekday().num_days_from_monday() as i64)
    };
    let items: Vec<WeekItem> = steps
        .iter()
        .filter_map(|s| s.book.as_ref().map(|b| (s, b)))
        .flat_map(|(s, b)| {
            s.chapters.iter().filter(|c| !c.done).map(move |c| WeekItem {
                topic_id: s.topic_id.clone(),
                topic_label: s.label.clone(),
                book: b.clone(),
                chapter: c.clone(),
            })
        })
        .collect();
    items
        .chunks(pace as usize)
        .take(12)
        .enumerate()
        .map(|(i, chunk)| Week {
            number: i as u32 + 1,
            starts: (monday + Duration::weeks(i as i64)).to_string(),
            pages: chunk.iter().map(|w| w.chapter.end_page.saturating_sub(w.chapter.start_page) + 1).sum(),
            items: chunk.to_vec(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quiz::{Grade, Overall, Question};

    #[test]
    fn taxonomy_is_consistent() {
        let mut all_ids = HashSet::new();
        for track in tracks() {
            let mut seen = HashSet::new();
            assert!(!track.topics.is_empty());
            for t in &track.topics {
                for p in &t.prereqs {
                    assert!(seen.contains(p.as_str()), "{}: prereq {p} must be listed earlier in {}", t.id, track.id);
                }
                assert!(!t.keywords.is_empty() && !t.book_tags.is_empty(), "{} needs keywords and bookTags", t.id);
                assert!(all_ids.insert(t.id.clone()), "duplicate topic id {}", t.id);
                seen.insert(t.id.as_str());
            }
        }
        assert_eq!(layers(track("fundamentals").unwrap())["graphs"], 4);
    }

    #[test]
    fn title_and_fts_helpers() {
        let kw = vec!["hash table".to_string(), "observer".to_string(), "i/o".to_string()];
        assert!(title_matches("3.4 Hash Tables", &kw));
        assert!(title_matches("Keeping your Objects in the Know: the Observer Pattern", &kw));
        assert!(!title_matches("Hashing it out with tables", &kw));
        assert!(title_matches("System-Level I/O", &kw));
        assert_eq!(fts_expr(&kw).unwrap(), "\"hash table\" OR \"observer\" OR \"i o\"");
        assert!(is_back_matter("Index") && is_back_matter("Front matter") && !is_back_matter("Indexing"));
        assert!(is_back_matter("B") && is_back_matter("Back Cover") && is_back_matter("4.9 Chapter Summary"));
        assert!(!is_back_matter("appendix C: Probability refresher"));
        assert!(title_matches("Chapter 8: Gaining Proficiency with Classes and Objects", &["class".to_string()]));
        assert!(title_matches("Processes and Threads", &["process".to_string()]));
    }

    fn grade(score: u32) -> Option<Grade> {
        Some(Grade {
            score,
            verdict: "x".into(),
            correct_points: vec![],
            missing_points: vec![],
            misconceptions: vec![],
            feedback: String::new(),
            reread_pages: vec![],
            graded_by: serde_json::Value::Null,
            graded_at: String::new(),
        })
    }

    fn q(topic: &str, difficulty: &str, score: u32) -> Question {
        Question {
            id: format!("{topic}-{difficulty}"),
            qtype: "mcq".into(),
            prompt: String::new(),
            choices: None,
            correct_choice: None,
            reference_answer: String::new(),
            key_points: vec![],
            sources: vec![],
            answer: Some("0".into()),
            grade: grade(score),
            topic: Some(topic.into()),
            difficulty: Some(difficulty.into()),
        }
    }

    pub fn attempt(kind: &str, at: &str, questions: Vec<Question>, topics: Vec<&str>, score: Option<u32>) -> Attempt {
        Attempt {
            id: at.into(),
            kind: kind.into(),
            book_fingerprint: None,
            chapter_id: None,
            chapter_title: None,
            status: "completed".into(),
            model: serde_json::json!({ "track": "fundamentals" }),
            questions,
            overall: score.map(|s| Overall {
                score: s,
                summary: String::new(),
                strengths: vec![],
                focus_next: vec![],
                next_step: String::new(),
                reread_pages: vec![],
            }),
            topics: topics.into_iter().map(String::from).collect(),
            created_at: at.into(),
            updated_at: at.into(),
        }
    }

    #[test]
    fn mastery_from_tests_and_quizzes() {
        assert_eq!(question_estimate(Some("medium"), 100), 70.0);
        assert_eq!(question_estimate(Some("hard"), 100), 100.0);
        assert_eq!(question_estimate(Some("easy"), 0), 0.0);

        let test = attempt(
            "assessment",
            "2026-10-01",
            vec![q("hashing", "medium", 100), q("hashing", "hard", 100), q("graphs", "medium", 0), q("graphs", "easy", 0)],
            vec![],
            Some(50),
        );
        let m = mastery_map(std::slice::from_ref(&test));
        assert_eq!(m["hashing"].mastery, 85);
        assert_eq!(m["graphs"].mastery, 13);
        assert_eq!(status_of(m.get("graphs")), "weak");
        assert_eq!(status_of(m.get("trees")), "untested");

        // A later, good chapter quiz on graphs pulls mastery up and raises confidence.
        let quiz = attempt("chapter", "2026-10-05", (0..5).map(|_| q("x", "medium", 90)).collect(), vec!["graphs"], Some(90));
        let m2 = mastery_map(&[test, quiz]);
        assert!(m2["graphs"].mastery > 50, "{:?}", m2["graphs"]);
        assert!(m2["graphs"].confidence > m["graphs"].confidence);
    }

    fn book(fp: &str, status: &str, tags: &[&str]) -> BookView {
        BookView {
            fingerprint: fp.into(),
            title: fp.into(),
            author: None,
            track: "fundamentals".into(),
            status: status.into(),
            level: None,
            topics: tags.iter().map(|s| s.to_string()).collect(),
            file_name: format!("{fp}.pdf"),
            size: 1,
            page_count: Some(100),
            path: Some(format!("/books/{fp}.pdf")),
            progress: None,
        }
    }

    fn ch(idx: u32, title: &str) -> ChapterMatch {
        ChapterMatch {
            chapter: Chapter { idx, id: format!("p{}", idx * 10), title: title.into(), depth: 0, start_page: idx * 10, end_page: idx * 10 + 9 },
            score: 2.0,
            title_match: true,
        }
    }

    #[test]
    fn roadmap_respects_prereqs_status_and_pace() {
        let books = vec![
            book("algs", "start-here", &["algorithms"]),
            book("clrs", "reference", &["algorithms"]),
            book("old", "outdated", &["algorithms"]),
        ];
        let mut idx = ChapterIndex::new();
        for b in ["algs", "clrs", "old"] {
            idx.insert(("hashing".into(), b.into()), vec![ch(3, "Hash Tables"), ch(4, "Symbol tables")]);
            idx.insert(("graphs".into(), b.into()), vec![ch(5, "Graphs")]);
        }
        // Strong on the basics, weak on graphs, the rest untested.
        let test = attempt(
            "assessment",
            "2026-10-01",
            vec![
                q("complexity", "medium", 100),
                q("complexity", "hard", 100),
                q("linear-structures", "medium", 100),
                q("linear-structures", "hard", 100),
                q("graphs", "medium", 0),
                q("graphs", "easy", 0),
            ],
            vec![],
            Some(60),
        );
        let r = roadmap("fundamentals", &[test], &books, &idx, vec![], 2).unwrap();
        assert!(r.learned.contains(&"complexity".to_string()));
        let pos = |id: &str| r.steps.iter().position(|s| s.topic_id == id);
        assert!(pos("complexity").is_none());
        // graphs is the weakest topic but needs trees first, which needs recursion and hashing.
        assert!(pos("trees").unwrap() < pos("graphs").unwrap());
        assert!(pos("hashing").unwrap() < pos("trees").unwrap());
        for s in &r.steps {
            if let Some(b) = &s.book {
                assert_ne!(b.fingerprint, "old", "outdated books are never suggested");
            }
        }
        let hashing = &r.steps[pos("hashing").unwrap()];
        assert_eq!(hashing.book.as_ref().unwrap().fingerprint, "algs", "start-here beats reference");
        assert_eq!(hashing.chapters.len(), 2);
        assert_eq!(hashing.alternatives.len(), 1);
        assert!(r.weeks.iter().all(|w| w.items.len() <= 2));
        assert_eq!(r.weeks.iter().map(|w| w.items.len()).sum::<usize>(), 3);
    }
}
