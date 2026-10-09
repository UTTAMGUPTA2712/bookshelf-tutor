//! Spaced-repetition cards: FSRS scheduling, the review queue, and cards made from quiz mistakes.
//!
//! The scheduler is FSRS-5 with its published default weights, at day granularity: "Again" brings a
//! card back in 10 minutes, every other rating schedules it whole days ahead at 90% target recall.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::quiz::{Attempt, Question};

// ---------- FSRS ----------

const W: [f64; 19] = [
    0.40255, 1.18385, 3.173, 15.69105, 7.1949, 0.5345, 1.4604, 0.0046, 1.54575, 0.1192, 1.01925, 1.9395, 0.11, 0.29605,
    2.2698, 0.2315, 2.9898, 0.51655, 0.6621,
];
const DECAY: f64 = -0.5;
const FACTOR: f64 = 19.0 / 81.0;
const RETENTION: f64 = 0.9;
const RELEARN_MINUTES: i64 = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub due: String,
    pub stability: f64,
    pub difficulty: f64,
    pub reps: u32,
    pub lapses: u32,
    /// "new" | "learning" | "review" | "relearning"
    pub state: String,
    pub last_review: Option<String>,
}

fn parse(ts: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(ts).ok().map(|d| d.with_timezone(&Utc))
}

fn fmt(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn retrievability(days: f64, stability: f64) -> f64 {
    (1.0 + FACTOR * days / stability.max(0.01)).powf(DECAY)
}

fn interval_days(stability: f64) -> f64 {
    let ivl = stability / FACTOR * (RETENTION.powf(1.0 / DECAY) - 1.0);
    ivl.round().clamp(1.0, 36500.0)
}

fn init_difficulty(g: u8) -> f64 {
    (W[4] - (W[5] * (g as f64 - 1.0)).exp() + 1.0).clamp(1.0, 10.0)
}

fn next_difficulty(d: f64, g: u8) -> f64 {
    let delta = -W[6] * (g as f64 - 3.0);
    let d2 = d + delta * (10.0 - d) / 9.0;
    (W[7] * init_difficulty(4) + (1.0 - W[7]) * d2).clamp(1.0, 10.0)
}

fn recall_stability(d: f64, s: f64, r: f64, g: u8) -> f64 {
    let hard = if g == 2 { W[15] } else { 1.0 };
    let easy = if g == 4 { W[16] } else { 1.0 };
    s * (1.0 + W[8].exp() * (11.0 - d) * s.powf(-W[9]) * ((W[10] * (1.0 - r)).exp() - 1.0) * hard * easy)
}

fn forget_stability(d: f64, s: f64, r: f64) -> f64 {
    (W[11] * d.powf(-W[12]) * ((s + 1.0).powf(W[13]) - 1.0) * (W[14] * (1.0 - r)).exp()).min(s)
}

fn short_term_stability(s: f64, g: u8) -> f64 {
    s * (W[17] * (g as f64 - 3.0 + W[18])).exp()
}

/// The card's next state after a rating (1 = Again, 2 = Hard, 3 = Good, 4 = Easy).
pub fn schedule(cur: &Schedule, rating: u8, at: DateTime<Utc>) -> Schedule {
    let g = rating.clamp(1, 4);
    let (stability, difficulty) = match cur.last_review.as_deref().and_then(parse) {
        None => (W[g as usize - 1], init_difficulty(g)),
        Some(last) => {
            let days = (at - last).num_seconds().max(0) as f64 / 86_400.0;
            let s = if days < 1.0 {
                short_term_stability(cur.stability, g)
            } else {
                let r = retrievability(days, cur.stability);
                if g == 1 {
                    forget_stability(cur.difficulty, cur.stability, r)
                } else {
                    recall_stability(cur.difficulty, cur.stability, r, g)
                }
            };
            (s.clamp(0.01, 36500.0), next_difficulty(cur.difficulty, g))
        }
    };
    let (due, state, lapses) = if g == 1 {
        let state = if cur.state == "new" || cur.state == "learning" { "learning" } else { "relearning" };
        let lapses = cur.lapses + u32::from(cur.state == "review");
        (at + Duration::minutes(RELEARN_MINUTES), state, lapses)
    } else {
        (at + Duration::days(interval_days(stability) as i64), "review", cur.lapses)
    };
    Schedule {
        due: fmt(due),
        stability,
        difficulty,
        reps: cur.reps + 1,
        lapses,
        state: state.into(),
        last_review: Some(fmt(at)),
    }
}

/// "10m", "3d", "2mo", "1.2y" — the wait each rating would give.
pub fn preview(cur: &Schedule, at: DateTime<Utc>) -> [String; 4] {
    [1u8, 2, 3, 4].map(|g| {
        let next = schedule(cur, g, at);
        let mins = parse(&next.due).map(|d| (d - at).num_minutes()).unwrap_or(0);
        match mins {
            m if m < 60 => format!("{m}m"),
            m if m < 60 * 24 => format!("{}h", m / 60),
            m if m < 60 * 24 * 30 => format!("{}d", (m as f64 / 1440.0).round()),
            m if m < 60 * 24 * 365 => format!("{}mo", (m as f64 / 43_200.0).round()),
            m => format!("{:.1}y", m as f64 / 525_600.0),
        }
    })
}

// ---------- storage ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub id: String,
    pub front: String,
    pub back: String,
    pub topic_id: Option<String>,
    pub attempt_id: Option<String>,
    pub book_fingerprint: Option<String>,
    pub book_title: Option<String>,
    pub page: Option<u32>,
    #[serde(flatten)]
    pub schedule: Schedule,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewCard {
    pub front: String,
    pub back: String,
    pub topic_id: Option<String>,
    pub attempt_id: Option<String>,
    pub question_id: Option<String>,
    pub book_fingerprint: Option<String>,
    pub page: Option<u32>,
}

const CARD_COLS: &str = "c.id, c.front, c.back, c.topic_id, c.attempt_id, c.book_fingerprint, c.page, c.due, c.stability,
    c.difficulty, c.reps, c.lapses, c.state, c.last_review, c.created_at,
    (SELECT title FROM books b WHERE b.fingerprint = c.book_fingerprint)";

fn row_to_card(r: &rusqlite::Row) -> rusqlite::Result<Card> {
    Ok(Card {
        id: r.get(0)?,
        front: r.get(1)?,
        back: r.get(2)?,
        topic_id: r.get(3)?,
        attempt_id: r.get(4)?,
        book_fingerprint: r.get(5)?,
        page: r.get::<_, Option<i64>>(6)?.map(|p| p as u32),
        schedule: Schedule {
            due: r.get(7)?,
            stability: r.get(8)?,
            difficulty: r.get(9)?,
            reps: r.get::<_, i64>(10)? as u32,
            lapses: r.get::<_, i64>(11)? as u32,
            state: r.get(12)?,
            last_review: r.get(13)?,
        },
        created_at: r.get(14)?,
        book_title: r.get(15)?,
    })
}

fn clean(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>().trim_end())
    }
}

/// New cards are first due at `due` (callers pass "tomorrow" for cards made from mistakes).
pub fn create(conn: &Connection, c: &NewCard, due: DateTime<Utc>, device_id: &str) -> AppResult<String> {
    let front = clean(&c.front, 1000);
    let back = clean(&c.back, 4000);
    if front.is_empty() || back.is_empty() {
        return Err(AppError::Invalid("A card needs a front and a back".into()));
    }
    let id = uuid::Uuid::now_v7().to_string();
    let ts = now();
    conn.execute(
        "INSERT INTO cards(id, front, back, topic_id, attempt_id, question_id, book_fingerprint, page, due, state,
                           created_at, updated_at, device_id, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'new', ?10, ?10, ?11, 0)",
        params![id, front, back, c.topic_id, c.attempt_id, c.question_id, c.book_fingerprint, c.page, fmt(due), ts, device_id],
    )?;
    Ok(id)
}

pub fn get(conn: &Connection, id: &str) -> AppResult<Card> {
    conn.query_row(&format!("SELECT {CARD_COLS} FROM cards c WHERE c.id = ?1 AND c.deleted = 0"), [id], row_to_card)
        .optional()?
        .ok_or_else(|| AppError::NotFound("Card not found".into()))
}

pub fn update_text(conn: &Connection, id: &str, front: &str, back: &str, device_id: &str) -> AppResult<Card> {
    let (front, back) = (clean(front, 1000), clean(back, 4000));
    if front.is_empty() || back.is_empty() {
        return Err(AppError::Invalid("A card needs a front and a back".into()));
    }
    conn.execute(
        "UPDATE cards SET front = ?2, back = ?3, updated_at = ?4, device_id = ?5 WHERE id = ?1",
        params![id, front, back, now(), device_id],
    )?;
    get(conn, id)
}

pub fn delete(conn: &Connection, id: &str, device_id: &str) -> AppResult<()> {
    conn.execute("UPDATE cards SET deleted = 1, updated_at = ?2, device_id = ?3 WHERE id = ?1", params![id, now(), device_id])?;
    Ok(())
}

pub fn list(conn: &Connection, query: Option<&str>) -> AppResult<Vec<Card>> {
    let like = query.map(|q| format!("%{}%", q.trim()));
    let mut stmt = conn.prepare(&format!(
        "SELECT {CARD_COLS} FROM cards c WHERE c.deleted = 0 AND (?1 IS NULL OR c.front LIKE ?1 OR c.back LIKE ?1)
         ORDER BY c.created_at DESC LIMIT 1000"
    ))?;
    let rows = stmt.query_map([like], row_to_card)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Cards due now, oldest-due first, at most `limit`.
pub fn due(conn: &Connection, at: DateTime<Utc>, limit: u32) -> AppResult<Vec<Card>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CARD_COLS} FROM cards c WHERE c.deleted = 0 AND c.due <= ?1 ORDER BY c.due LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![fmt(at), limit], row_to_card)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn count_due(conn: &Connection, before: DateTime<Utc>) -> AppResult<u32> {
    Ok(conn.query_row("SELECT COUNT(*) FROM cards WHERE deleted = 0 AND due <= ?1", [fmt(before)], |r| r.get(0))?)
}

/// Applies a rating, logs the review, returns the updated card.
pub fn review(conn: &Connection, id: &str, rating: u8, at: DateTime<Utc>, device_id: &str) -> AppResult<Card> {
    if !(1..=4).contains(&rating) {
        return Err(AppError::Invalid("Rating must be 1-4".into()));
    }
    let card = get(conn, id)?;
    let next = schedule(&card.schedule, rating, at);
    let ts = fmt(at);
    let ivl = parse(&next.due).map(|d| (d - at).num_minutes() as f64 / 1440.0).unwrap_or(0.0);
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE cards SET due = ?2, stability = ?3, difficulty = ?4, reps = ?5, lapses = ?6, state = ?7, last_review = ?8,
                          updated_at = ?8, device_id = ?9
         WHERE id = ?1",
        params![id, next.due, next.stability, next.difficulty, next.reps, next.lapses, next.state, ts, device_id],
    )?;
    tx.execute(
        "INSERT INTO reviews(id, card_id, rating, state, interval_days, reviewed_at, updated_at, device_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)",
        params![uuid::Uuid::now_v7().to_string(), id, rating, card.schedule.state, ivl, ts, device_id],
    )?;
    tx.commit()?;
    get(conn, id)
}

// ---------- cards from quiz mistakes ----------

/// Below this score a question counts as a mistake worth a card.
pub const MISTAKE_BELOW: u32 = 80;

pub fn mistakes(a: &Attempt) -> Vec<(usize, &Question)> {
    a.questions
        .iter()
        .enumerate()
        .filter(|(_, q)| q.grade.as_ref().is_some_and(|g| g.score < MISTAKE_BELOW))
        .collect()
}

fn question_page(q: &Question) -> Option<u32> {
    q.grade
        .as_ref()
        .and_then(|g| g.reread_pages.first().copied())
        .or_else(|| q.sources.first().map(|s| s.page_start))
}

fn base_card(a: &Attempt, q: &Question) -> NewCard {
    NewCard {
        topic_id: q.topic.clone().or_else(|| a.topics.first().cloned()),
        attempt_id: Some(a.id.clone()),
        question_id: Some(q.id.clone()),
        book_fingerprint: a.book_fingerprint.clone(),
        page: question_page(q),
        ..Default::default()
    }
}

/// The plain fallback card: the question on the front, the right answer on the back.
pub fn fallback_card(a: &Attempt, q: &Question) -> NewCard {
    let mut back = match (&q.choices, q.correct_choice) {
        (Some(c), Some(i)) if q.qtype == "mcq" => {
            let choice = c.get(i).cloned().unwrap_or_default();
            if q.reference_answer.trim().is_empty() || q.reference_answer.trim() == choice.trim() {
                choice
            } else {
                format!("{choice}\n\n{}", q.reference_answer.trim())
            }
        }
        _ => q.reference_answer.trim().to_string(),
    };
    if back.is_empty() {
        back = q.key_points.join("\n");
    }
    NewCard { front: q.prompt.clone(), back, ..base_card(a, q) }
}

/// Cards the model wrote (`{"q": 1, "front": "...", "back": "..."}`, `q` = 1-based question number) for
/// missed questions, at most two per question; every missed question without one gets the fallback card.
pub fn cards_for_mistakes(a: &Attempt, llm_cards: &[Value]) -> Vec<NewCard> {
    let missed = mistakes(a);
    let mut out = Vec::new();
    for (i, q) in &missed {
        let mine: Vec<NewCard> = llm_cards
            .iter()
            .filter(|c| c.get("q").and_then(Value::as_u64) == Some(*i as u64 + 1))
            .filter_map(|c| {
                let front = c.get("front")?.as_str()?.trim();
                let back = c.get("back")?.as_str()?.trim();
                (front.len() >= 8 && !back.is_empty() && front.len() <= 400)
                    .then(|| NewCard { front: front.into(), back: back.into(), ..base_card(a, q) })
            })
            .take(2)
            .collect();
        if mine.is_empty() {
            out.push(fallback_card(a, q));
        } else {
            out.extend(mine);
        }
    }
    out
}

/// A card made from this source item (e.g. one glossary term of a lesson) already exists.
pub fn exists(conn: &Connection, attempt_id: &str, question_id: &str) -> AppResult<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM cards WHERE attempt_id = ?1 AND question_id = ?2 AND deleted = 0 LIMIT 1",
            [attempt_id, question_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

pub fn has_cards_for(conn: &Connection, attempt_id: &str) -> AppResult<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM cards WHERE attempt_id = ?1 LIMIT 1", [attempt_id], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Start of the next local day, as UTC: cards made from today's mistakes are due tomorrow.
pub fn tomorrow_start() -> DateTime<Utc> {
    let local = chrono::Local::now();
    let midnight = (local.date_naive() + Duration::days(1)).and_hms_opt(0, 0, 0).expect("valid time");
    midnight
        .and_local_timezone(chrono::Local)
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now() + Duration::hours(12))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn new_schedule() -> Schedule {
        Schedule {
            due: "2026-10-09T00:00:00.000Z".into(),
            stability: 0.0,
            difficulty: 0.0,
            reps: 0,
            lapses: 0,
            state: "new".into(),
            last_review: None,
        }
    }

    fn t(s: &str) -> DateTime<Utc> {
        parse(s).unwrap()
    }

    #[test]
    fn fsrs_first_reviews_and_growth() {
        let at = t("2026-10-09T10:00:00Z");
        let s0 = new_schedule();
        assert_eq!(preview(&s0, at), ["10m", "1d", "3d", "16d"].map(String::from));
        let again = schedule(&s0, 1, at);
        assert_eq!(again.state, "learning");
        assert_eq!(again.lapses, 0);

        // Good, then Good again on the due day: the interval grows.
        let good = schedule(&s0, 3, at);
        assert_eq!(good.state, "review");
        let first_gap = t(&good.due) - at;
        let later = t(&good.due);
        let good2 = schedule(&good, 3, later);
        assert!(t(&good2.due) - later > first_gap * 2, "{good2:?}");
        assert!(good2.stability > good.stability);

        // Forgetting a review card: lapse, short relearn, lower stability.
        let lapse = schedule(&good2, 1, t(&good2.due));
        assert_eq!((lapse.state.as_str(), lapse.lapses), ("relearning", 1));
        assert!(lapse.stability < good2.stability);
        assert!(lapse.difficulty > good2.difficulty);
        assert_eq!(t(&lapse.due) - t(&good2.due), Duration::minutes(10));
        // Easy beats Good beats Hard.
        let [_, h, g, e] = [1u8, 2, 3, 4].map(|r| schedule(&good2, r, t(&good2.due)).stability);
        assert!(h < g && g < e);
    }

    fn graded(prompt: &str, score: u32, qtype: &str) -> Question {
        Question {
            id: prompt.into(),
            qtype: qtype.into(),
            prompt: prompt.into(),
            choices: (qtype == "mcq").then(|| vec!["A heap".into(), "A stack".into()]),
            correct_choice: (qtype == "mcq").then_some(1),
            reference_answer: "Because LIFO.".into(),
            key_points: vec![],
            sources: vec![crate::quiz::Source { label: "S1".into(), page_start: 42, page_end: 43, excerpt: String::new() }],
            answer: Some("x".into()),
            grade: Some(crate::quiz::Grade {
                score,
                verdict: String::new(),
                correct_points: vec![],
                missing_points: vec![],
                misconceptions: vec![],
                feedback: String::new(),
                reread_pages: vec![],
                graded_by: Value::Null,
                graded_at: String::new(),
            }),
            topic: None,
            difficulty: None,
        }
    }

    #[test]
    fn mistakes_become_cards_due_tomorrow() {
        let conn = Connection::open_in_memory().unwrap();
        db::migrate(&conn).unwrap();
        let a = Attempt {
            id: "att".into(),
            kind: "chapter".into(),
            book_fingerprint: Some("fp".into()),
            chapter_id: None,
            chapter_title: None,
            status: "completed".into(),
            model: Value::Null,
            questions: vec![
                graded("Which structure backs function calls?", 0, "mcq"),
                graded("Why is recursion depth limited?", 100, "short"),
                graded("What happens on stack overflow?", 40, "short"),
            ],
            overall: None,
            topics: vec!["recursion".into()],
            created_at: String::new(),
            updated_at: String::new(),
        };
        let llm = serde_json::json!([
            {"q": 3, "front": "What error does unbounded recursion cause?", "back": "A stack overflow."},
            {"q": 2, "front": "This one was answered correctly", "back": "ignored"},
            {"q": 3, "front": "short", "back": "too short front, dropped"}
        ]);
        let cards = cards_for_mistakes(&a, llm.as_array().unwrap());
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].front, "Which structure backs function calls?");
        assert_eq!(cards[0].back, "A stack\n\nBecause LIFO.");
        assert_eq!(cards[1].front, "What error does unbounded recursion cause?");
        assert_eq!((cards[1].page, cards[1].topic_id.as_deref()), (Some(42), Some("recursion")));

        let tomorrow = tomorrow_start();
        for c in &cards {
            create(&conn, c, tomorrow, "dev").unwrap();
        }
        assert!(has_cards_for(&conn, "att").unwrap());
        assert!(due(&conn, Utc::now(), 50).unwrap().is_empty(), "nothing due today");
        let next_day = tomorrow + Duration::hours(1);
        let queue = due(&conn, next_day, 50).unwrap();
        assert_eq!(queue.len(), 2);

        let reviewed = review(&conn, &queue[0].id, 3, next_day, "dev").unwrap();
        assert_eq!(reviewed.schedule.reps, 1);
        assert_eq!(due(&conn, next_day, 50).unwrap().len(), 1);
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM reviews", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }
}
