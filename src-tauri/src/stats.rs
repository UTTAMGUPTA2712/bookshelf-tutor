//! Dashboard numbers. Everything is computed from synced records (attempts, reviews, reading log,
//! progress, cards), never stored, so an export/merge round trip always gives the same numbers.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

use crate::error::AppResult;
use crate::quiz::Attempt;

/// Minutes of reading added per heartbeat at most (the reader sends one a minute while you read).
const MAX_TICK_MINUTES: u32 = 5;

pub fn log_reading(conn: &Connection, fp: &str, minutes: u32, now: DateTime<FixedOffset>, device_id: &str) -> AppResult<()> {
    let day = now.date_naive().to_string();
    let id = format!("{device_id}:{fp}:{day}");
    conn.execute(
        "INSERT INTO reading_log(id, fingerprint, day, minutes, updated_at, device_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET minutes = minutes + excluded.minutes, updated_at = excluded.updated_at",
        params![id, fp, day, minutes.clamp(1, MAX_TICK_MINUTES), crate::db::now(), device_id],
    )?;
    Ok(())
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DayActivity {
    pub day: String,
    pub quizzes: u32,
    pub reviews: u32,
    pub minutes: u32,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Streak {
    pub current: u32,
    pub longest: u32,
    pub active_today: bool,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrendPoint {
    pub id: String,
    pub date: String,
    pub score: u32,
    pub title: String,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BookStat {
    pub fingerprint: String,
    pub title: String,
    pub last_page: u32,
    pub page_count: Option<u32>,
    pub chapters_done: u32,
    pub minutes: u32,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageRow {
    pub provider: String,
    pub model: String,
    pub calls: u32,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub today: u32,
    pub week: u32,
    pub total: u32,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Cards {
    pub due_now: u32,
    pub due_tomorrow: u32,
    pub total: u32,
    pub reviewed_today: u32,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub cards: Cards,
    pub streak: Streak,
    /// Oldest first, one entry per day for the last `ACTIVITY_DAYS` days (today last).
    pub activity: Vec<DayActivity>,
    pub score_trend: Vec<TrendPoint>,
    pub reading: Reading,
    pub books: Vec<BookStat>,
    pub usage: Vec<UsageRow>,
}

pub const ACTIVITY_DAYS: i64 = 84;

fn local_day(ts: &str, tz: &FixedOffset) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(ts).ok().map(|d| d.with_timezone(tz).date_naive())
}

fn streak(active: &BTreeSet<NaiveDate>, today: NaiveDate) -> Streak {
    let active_today = active.contains(&today);
    let mut current = 0;
    let mut d = if active_today { today } else { today - Duration::days(1) };
    while active.contains(&d) {
        current += 1;
        d -= Duration::days(1);
    }
    let (mut longest, mut run, mut prev): (u32, u32, Option<NaiveDate>) = (0, 0, None);
    for &d in active {
        run = if prev.is_some_and(|p| d - p == Duration::days(1)) { run + 1 } else { 1 };
        longest = longest.max(run);
        prev = Some(d);
    }
    Streak { current, longest, active_today }
}

fn add_usage(rows: &mut BTreeMap<(String, String), UsageRow>, who: Option<&Value>, usage: Option<&Value>) {
    let Some(who) = who else { return };
    let provider = who.get("provider").and_then(Value::as_str).unwrap_or("?").to_string();
    let model = who.get("model").and_then(Value::as_str).unwrap_or("?").to_string();
    let row = rows.entry((provider.clone(), model.clone())).or_insert(UsageRow {
        provider,
        model,
        calls: 0,
        prompt_tokens: 0,
        completion_tokens: 0,
    });
    row.calls += 1;
    let n = |k: &str| usage.and_then(|u| u.get(k)).and_then(Value::as_u64).unwrap_or(0);
    row.prompt_tokens += n("promptTokens");
    row.completion_tokens += n("completionTokens");
}

/// Model calls recorded on attempts: question writing and every AI grade.
pub fn usage(attempts: &[Attempt]) -> Vec<UsageRow> {
    let mut rows = BTreeMap::new();
    for a in attempts {
        add_usage(&mut rows, a.model.get("question"), a.model.get("usage"));
        for q in &a.questions {
            if let Some(g) = &q.grade {
                add_usage(&mut rows, g.graded_by.get("model"), g.graded_by.get("usage"));
            }
        }
    }
    let mut out: Vec<UsageRow> = rows.into_values().collect();
    out.sort_by(|a, b| b.calls.cmp(&a.calls));
    out
}

pub fn compute(conn: &Connection, attempts: &[Attempt], now: DateTime<FixedOffset>) -> AppResult<Stats> {
    let interviews: Vec<String> = {
        let mut stmt = conn.prepare("SELECT created_at FROM interviews WHERE deleted = 0")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let tz = *now.offset();
    let today = now.date_naive();
    let first = today - Duration::days(ACTIVITY_DAYS - 1);
    let mut active: BTreeSet<NaiveDate> = BTreeSet::new();
    let mut days: BTreeMap<NaiveDate, DayActivity> = BTreeMap::new();
    fn day(days: &mut BTreeMap<NaiveDate, DayActivity>, d: NaiveDate, first: NaiveDate, today: NaiveDate) -> Option<&mut DayActivity> {
        (d >= first && d <= today).then(|| {
            days.entry(d).or_insert_with(|| DayActivity { day: d.to_string(), quizzes: 0, reviews: 0, minutes: 0 })
        })
    }

    for created in attempts.iter().map(|a| &a.created_at).chain(&interviews) {
        if let Some(d) = local_day(created, &tz) {
            active.insert(d);
            if let Some(x) = day(&mut days, d, first, today) {
                x.quizzes += 1;
            }
        }
    }
    let mut reviewed_today = 0;
    {
        let mut stmt = conn.prepare("SELECT reviewed_at FROM reviews")?;
        for ts in stmt.query_map([], |r| r.get::<_, String>(0))? {
            if let Some(d) = local_day(&ts?, &tz) {
                active.insert(d);
                reviewed_today += u32::from(d == today);
                if let Some(x) = day(&mut days, d, first, today) {
                    x.reviews += 1;
                }
            }
        }
    }
    let mut reading = Reading { today: 0, week: 0, total: 0 };
    let mut minutes_by_book: HashMap<String, u32> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT fingerprint, day, minutes FROM reading_log")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, u32>(2)?)))? {
            let (fp, d, m) = row?;
            *minutes_by_book.entry(fp).or_default() += m;
            reading.total += m;
            let Ok(d) = d.parse::<NaiveDate>() else { continue };
            if m > 0 {
                active.insert(d);
            }
            if d == today {
                reading.today += m;
            }
            if today - d < Duration::days(7) && d <= today {
                reading.week += m;
            }
            if let Some(x) = day(&mut days, d, first, today) {
                x.minutes += m;
            }
        }
    }
    let activity = (0..ACTIVITY_DAYS)
        .map(|i| first + Duration::days(i))
        .map(|d| days.remove(&d).unwrap_or(DayActivity { day: d.to_string(), quizzes: 0, reviews: 0, minutes: 0 }))
        .collect();

    let mut trend: Vec<TrendPoint> = attempts
        .iter()
        .filter(|a| a.kind == "chapter" && a.status == "completed")
        .filter_map(|a| {
            Some(TrendPoint {
                id: a.id.clone(),
                date: a.created_at.clone(),
                score: a.overall.as_ref()?.score,
                title: a.chapter_title.clone().unwrap_or_default(),
            })
        })
        .collect();
    trend.sort_by(|a, b| a.date.cmp(&b.date));
    let trend = trend.split_off(trend.len().saturating_sub(20));

    let mut books = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT p.fingerprint, b.title, p.last_page, b.page_count, p.chapters_done
             FROM progress p JOIN books b ON b.fingerprint = p.fingerprint
             WHERE p.deleted = 0 AND b.deleted = 0 AND p.status != 'finished'
             ORDER BY p.updated_at DESC LIMIT 6",
        )?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, String>(4)?,
            ))
        })? {
            let (fp, title, last_page, page_count, done) = row?;
            books.push(BookStat {
                minutes: minutes_by_book.get(&fp).copied().unwrap_or(0),
                fingerprint: fp,
                title,
                last_page: last_page as u32,
                page_count: page_count.map(|n| n as u32),
                chapters_done: serde_json::from_str::<Vec<String>>(&done).map(|v| v.len() as u32).unwrap_or(0),
            });
        }
    }

    let utc_now = now.with_timezone(&Utc);
    let end_of_tomorrow = (today + Duration::days(2))
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(tz).earliest())
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or(utc_now + Duration::days(2));
    let due_now = crate::cards::count_due(conn, utc_now)?;
    let cards = Cards {
        due_now,
        due_tomorrow: crate::cards::count_due(conn, end_of_tomorrow)? - due_now,
        total: conn.query_row("SELECT COUNT(*) FROM cards WHERE deleted = 0", [], |r| r.get(0))?,
        reviewed_today,
    };

    Ok(Stats {
        cards,
        streak: streak(&active, today),
        activity,
        score_trend: trend,
        reading,
        books,
        usage: usage(attempts),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cards, db, quiz, transfer};

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn streaks() {
        let days: BTreeSet<NaiveDate> = ["2026-10-01", "2026-10-02", "2026-10-03", "2026-10-07", "2026-10-08"]
            .into_iter()
            .map(d)
            .collect();
        assert_eq!(streak(&days, d("2026-10-09")), Streak { current: 2, longest: 3, active_today: false });
        assert_eq!(streak(&days, d("2026-10-08")), Streak { current: 2, longest: 3, active_today: true });
        assert_eq!(streak(&days, d("2026-10-10")).current, 0);
    }

    fn seed(conn: &Connection, dev: &str, now: DateTime<FixedOffset>) {
        let mut a = crate::assessment::new_attempt(crate::skills::track("fundamentals").unwrap(), false);
        a.kind = "chapter".into();
        a.status = "completed".into();
        a.chapter_title = Some("Hash Tables".into());
        a.model = serde_json::json!({ "question": { "provider": "Ollama", "model": "qwen" }, "usage": { "promptTokens": 100, "completionTokens": 50 } });
        a.overall = Some(quiz::Overall {
            score: 64,
            summary: String::new(),
            strengths: vec![],
            focus_next: vec![],
            next_step: String::new(),
            reread_pages: vec![],
        });
        a.created_at = "2026-10-08T09:00:00.000Z".into();
        quiz::save(conn, &a, dev).unwrap();
        let id = cards::create(conn, &cards::NewCard { front: "What is a hash?".into(), back: "x".into(), ..Default::default() }, now.with_timezone(&Utc), dev).unwrap();
        cards::review(conn, &id, 3, now.with_timezone(&Utc), dev).unwrap();
        log_reading(conn, "book", 3, now, dev).unwrap();
        log_reading(conn, "book", 2, now, dev).unwrap();
    }

    #[test]
    fn numbers_survive_export_and_merge() {
        let now = DateTime::parse_from_rfc3339("2026-10-09T12:00:00+05:30").unwrap();
        let linux = Connection::open_in_memory().unwrap();
        db::migrate(&linux).unwrap();
        seed(&linux, "linux", now);
        let stats = compute(&linux, &quiz::all(&linux).unwrap(), now).unwrap();
        assert_eq!(stats.streak, Streak { current: 2, longest: 2, active_today: true });
        assert_eq!(stats.reading, Reading { today: 5, week: 5, total: 5 });
        assert_eq!(stats.cards.reviewed_today, 1);
        assert_eq!(stats.score_trend.len(), 1);
        assert_eq!(stats.usage[0].prompt_tokens, 100);
        assert_eq!(stats.activity.len(), ACTIVITY_DAYS as usize);
        assert_eq!(stats.activity.last().unwrap().reviews, 1);

        let mut mac = Connection::open_in_memory().unwrap();
        db::migrate(&mac).unwrap();
        let file = transfer::build_export(&linux, "linux").unwrap();
        transfer::apply(&mut mac, &file, transfer::ImportMode::Merge).unwrap();
        transfer::apply(&mut mac, &file, transfer::ImportMode::Merge).unwrap(); // idempotent
        assert_eq!(compute(&mac, &quiz::all(&mac).unwrap(), now).unwrap(), stats);

        // The Mac reads too: per-device rows add up instead of overwriting each other.
        log_reading(&mac, "book", 4, now, "mac").unwrap();
        let back = transfer::build_export(&mac, "mac").unwrap();
        let mut linux = linux;
        transfer::apply(&mut linux, &back, transfer::ImportMode::Merge).unwrap();
        assert_eq!(compute(&linux, &quiz::all(&linux).unwrap(), now).unwrap().reading.today, 9);
    }
}
