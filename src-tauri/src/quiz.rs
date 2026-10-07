//! Chapter quizzes: prompt building, grounding checks, grading and persistence.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::index::Chunk;
use crate::llm::Message;

pub const PROMPT_VERSION: &str = "quiz.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub label: String,
    pub page_start: u32,
    pub page_end: u32,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grade {
    pub score: u32,
    pub verdict: String,
    pub correct_points: Vec<String>,
    pub missing_points: Vec<String>,
    pub misconceptions: Vec<String>,
    pub feedback: String,
    pub reread_pages: Vec<u32>,
    pub graded_by: Value,
    pub graded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: String,
    #[serde(rename = "type")]
    pub qtype: String,
    pub prompt: String,
    pub choices: Option<Vec<String>>,
    pub correct_choice: Option<usize>,
    pub reference_answer: String,
    pub key_points: Vec<String>,
    pub sources: Vec<Source>,
    pub answer: Option<String>,
    pub grade: Option<Grade>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overall {
    pub score: u32,
    pub summary: String,
    pub strengths: Vec<String>,
    pub focus_next: Vec<String>,
    pub next_step: String,
    pub reread_pages: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub id: String,
    pub kind: String,
    pub book_fingerprint: Option<String>,
    pub chapter_id: Option<String>,
    pub chapter_title: Option<String>,
    pub status: String,
    pub model: Value,
    pub questions: Vec<Question>,
    pub overall: Option<Overall>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuizOptions {
    pub count: u32,
    pub difficulty: String,
    pub types: Vec<String>,
}

// ---------- generation ----------

pub fn label_sources(chunks: &[Chunk]) -> Vec<Source> {
    chunks
        .iter()
        .enumerate()
        .map(|(i, c)| Source {
            label: format!("S{}", i + 1),
            page_start: c.page_start,
            page_end: c.page_end,
            excerpt: c.text.clone(),
        })
        .collect()
}

fn pages_label(s: &Source) -> String {
    if s.page_start == s.page_end {
        format!("page {}", s.page_start)
    } else {
        format!("pages {}-{}", s.page_start, s.page_end)
    }
}

fn excerpts_block(sources: &[&Source]) -> String {
    sources
        .iter()
        .map(|s| format!("[{}] ({})\n{}", s.label, pages_label(s), s.excerpt.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn type_plan(opts: &QuizOptions) -> String {
    let types: Vec<&str> = if opts.types.is_empty() {
        vec!["mcq", "short", "explain"]
    } else {
        opts.types.iter().map(String::as_str).collect()
    };
    let mut counts = vec![0u32; types.len()];
    for i in 0..opts.count {
        counts[i as usize % types.len()] += 1;
    }
    types.iter().zip(counts).filter(|(_, n)| *n > 0).map(|(t, n)| format!("{n} {t}")).collect::<Vec<_>>().join(", ")
}

pub fn generation_messages(book_title: &str, chapter_title: &str, opts: &QuizOptions, sources: &[Source]) -> Vec<Message> {
    let difficulty = match opts.difficulty.as_str() {
        "easy" => "easy: recall and basic understanding of the main ideas",
        "hard" => "hard: edge cases, trade-offs, and reasoning about consequences",
        _ => "medium: applying the concepts and comparing alternatives",
    };
    let system = r#"You are an experienced computer-science tutor. You write quiz questions that check whether a student truly understood a chapter of a technical book.

Rules:
- Use ONLY facts from the provided excerpts. Every question must be answerable from them.
- Test understanding (why, how, when, trade-offs, what happens if), not trivia such as page numbers, figure numbers, chapter names or author names.
- Every question cites 1-3 excerpt labels (like "S2") in "sources" — the excerpts that contain its answer.
- Types:
  - "mcq": exactly 4 "choices", exactly one correct, plausible distractors; "correct_choice" is the 0-based index of the correct choice.
  - "short": answerable in 1-3 sentences.
  - "explain": asks the student to explain a concept or mechanism in their own words.
- "reference_answer": a complete model answer. "key_points": 2-5 short facts a good answer must contain.
- Write in English.

Reply with a JSON object only, exactly in this shape:
{"questions":[{"type":"mcq","question":"...","choices":["...","...","...","..."],"correct_choice":0,"reference_answer":"...","key_points":["..."],"sources":["S1"]}]}"#;
    let refs: Vec<&Source> = sources.iter().collect();
    let user = format!(
        "Book: {book_title}\nChapter: {chapter_title}\nDifficulty: {difficulty}\nWrite {} questions ({}), ordered from easier to harder.\n\nExcerpts:\n\n{}",
        opts.count,
        type_plan(opts),
        excerpts_block(&refs)
    );
    vec![Message::system(system), Message::user(user)]
}

#[derive(Debug, Deserialize)]
pub struct GenOut {
    #[serde(default)]
    pub questions: Vec<GenQ>,
}

#[derive(Debug, Deserialize)]
pub struct GenQ {
    #[serde(rename = "type", default)]
    pub qtype: String,
    #[serde(default, alias = "prompt")]
    pub question: String,
    #[serde(default)]
    pub choices: Vec<Value>,
    #[serde(default, alias = "answer_index", alias = "correct")]
    pub correct_choice: Option<Value>,
    #[serde(default, alias = "answer")]
    pub reference_answer: Option<Value>,
    #[serde(default)]
    pub key_points: Vec<Value>,
    #[serde(default)]
    pub sources: Vec<Value>,
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// "B. Foo" / "(b) Foo" → "Foo"; the UI adds its own letters.
fn strip_choice_letter(c: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"^\(?[A-Ha-h][.):]\s+").expect("regex"));
    re.replace(c.trim(), "").trim().to_string()
}

fn normalize_type(t: &str) -> Option<&'static str> {
    let t = t.to_lowercase().replace(['-', ' '], "_");
    match t.as_str() {
        "mcq" | "multiple_choice" | "multiplechoice" | "choice" => Some("mcq"),
        "short" | "short_answer" | "shortanswer" => Some("short"),
        "explain" | "explanation" | "open" | "essay" | "long" => Some("explain"),
        _ => None,
    }
}

fn parse_choice_index(v: &Value, choices: &[String]) -> Option<usize> {
    let idx = match v {
        Value::Number(n) => n.as_u64().map(|n| n as usize),
        Value::String(s) => {
            let s = s.trim();
            if let Ok(n) = s.parse::<usize>() {
                Some(n)
            } else if s.len() == 1 && s.chars().next().unwrap().is_ascii_alphabetic() {
                Some((s.to_ascii_uppercase().as_bytes()[0] - b'A') as usize)
            } else {
                choices.iter().position(|c| c.eq_ignore_ascii_case(s))
            }
        }
        _ => None,
    }?;
    (idx < choices.len()).then_some(idx)
}

/// Turns raw model output into questions, dropping any that aren't grounded in the excerpts.
pub fn normalize_generated(out: &GenOut, sources: &[Source], max: usize) -> Vec<Question> {
    let mut qs = Vec::new();
    for g in &out.questions {
        let Some(qtype) = normalize_type(&g.qtype) else { continue };
        let prompt = g.question.trim().to_string();
        if prompt.len() < 8 {
            continue;
        }
        let norm = |t: &str| t.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
        if qs.iter().any(|q: &Question| norm(&q.prompt) == norm(&prompt)) {
            continue; // small models repeat themselves
        }
        let cited: Vec<Source> = g
            .sources
            .iter()
            .filter_map(|s| {
                let label = value_text(s).trim_matches(['[', ']']).to_uppercase();
                let label = if label.starts_with('S') { label } else { format!("S{label}") };
                sources.iter().find(|src| src.label == label).cloned()
            })
            .collect();
        if cited.is_empty() {
            continue; // grounding check: must cite real excerpts
        }
        let (choices, correct_choice) = if qtype == "mcq" {
            let choices: Vec<String> =
                g.choices.iter().map(|c| strip_choice_letter(&value_text(c))).filter(|c| !c.is_empty()).collect();
            let Some(idx) = g.correct_choice.as_ref().and_then(|v| parse_choice_index(v, &choices)) else { continue };
            if choices.len() < 3 {
                continue;
            }
            (Some(choices), Some(idx))
        } else {
            (None, None)
        };
        let mut reference_answer = g.reference_answer.as_ref().map(value_text).unwrap_or_default();
        if reference_answer.is_empty() {
            if let (Some(c), Some(i)) = (&choices, correct_choice) {
                reference_answer = c[i].clone();
            }
        }
        qs.push(Question {
            id: uuid::Uuid::now_v7().to_string(),
            qtype: qtype.to_string(),
            prompt,
            choices,
            correct_choice,
            reference_answer,
            key_points: g.key_points.iter().map(value_text).filter(|k| !k.is_empty()).collect(),
            sources: cited,
            answer: None,
            grade: None,
        });
        if qs.len() >= max {
            break;
        }
    }
    qs
}

// ---------- grading ----------

fn source_pages(sources: &[Source]) -> Vec<u32> {
    let mut pages: Vec<u32> = sources.iter().map(|s| s.page_start).collect();
    pages.sort_unstable();
    pages.dedup();
    pages
}

pub fn is_blank_answer(a: &str) -> bool {
    let a = a.trim().to_lowercase();
    a.is_empty() || a == "i don't know" || a == "idk" || a == "?"
}

/// Grades without calling a model: multiple choice and "I don't know".
pub fn local_grade(q: &Question, answer: &str) -> Option<Grade> {
    let base = |score: u32, verdict: &str, missing: Vec<String>| Grade {
        score,
        verdict: verdict.into(),
        correct_points: vec![],
        missing_points: missing,
        misconceptions: vec![],
        feedback: q.reference_answer.clone(),
        reread_pages: if score == 100 { vec![] } else { source_pages(&q.sources) },
        graded_by: serde_json::json!({ "local": true }),
        graded_at: now(),
    };
    if is_blank_answer(answer) {
        return Some(base(0, "incorrect", q.key_points.clone()));
    }
    if q.qtype == "mcq" {
        let chosen = answer.trim().parse::<usize>().ok();
        let correct = chosen.is_some() && chosen == q.correct_choice;
        let mut g = base(if correct { 100 } else { 0 }, if correct { "correct" } else { "incorrect" }, vec![]);
        if let (false, Some(choices), Some(i)) = (correct, &q.choices, q.correct_choice) {
            g.missing_points = vec![format!("The correct choice was: {}", choices[i])];
        }
        return Some(g);
    }
    None
}

pub fn grade_messages(q: &Question, answer: &str) -> Vec<Message> {
    let system = r#"You are a fair, precise tutor grading a student's answer to a question about a technical book chapter.

Rules:
- Grade ONLY against the excerpts, the reference answer and the key points.
- Give full credit for correct ideas expressed in different words. Do not reward vague or padded answers.
- Be specific: say exactly what is correct, what is missing, and what is wrong (misconceptions).
- "feedback" teaches the right answer in 2-5 sentences, addressed to the student ("you").
- "reread" lists the excerpt labels (like "S1") the student should re-read; empty if the answer is fully correct.

Reply with a JSON object only, exactly in this shape:
{"score":0,"verdict":"correct|partial|incorrect","correct_points":["..."],"missing_points":["..."],"misconceptions":["..."],"feedback":"...","reread":["S1"]}
"score" is 0-100."#;
    let refs: Vec<&Source> = q.sources.iter().collect();
    let key_points = q.key_points.iter().map(|k| format!("- {k}")).collect::<Vec<_>>().join("\n");
    let user = format!(
        "Question ({}): {}\n\nReference answer: {}\n\nKey points:\n{}\n\nExcerpts:\n\n{}\n\nStudent's answer:\n<<<\n{}\n>>>",
        q.qtype,
        q.prompt,
        q.reference_answer,
        key_points,
        excerpts_block(&refs),
        answer.trim()
    );
    vec![Message::system(system), Message::user(user)]
}

#[derive(Debug, Deserialize)]
pub struct GradeOut {
    pub score: Value,
    #[serde(default)]
    pub verdict: String,
    #[serde(default)]
    pub correct_points: Vec<Value>,
    #[serde(default)]
    pub missing_points: Vec<Value>,
    #[serde(default)]
    pub misconceptions: Vec<Value>,
    #[serde(default)]
    pub feedback: String,
    #[serde(default)]
    pub reread: Vec<Value>,
}

pub fn grade_score(out: &GradeOut) -> Option<u32> {
    let n = match &out.score {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().trim_end_matches('%').parse::<f64>().ok(),
        _ => None,
    }?;
    // Some models answer on a 0-1 or 0-10 scale.
    let n = if n <= 1.0 && out.verdict != "incorrect" && n > 0.0 { n * 100.0 } else if n <= 10.0 && n > 1.0 { n * 10.0 } else { n };
    Some(n.clamp(0.0, 100.0).round() as u32)
}

pub fn validate_grade(out: &GradeOut) -> Result<(), String> {
    grade_score(out).ok_or("\"score\" must be a number from 0 to 100")?;
    if out.feedback.trim().is_empty() {
        return Err("\"feedback\" must not be empty".into());
    }
    Ok(())
}

pub fn to_grade(q: &Question, out: &GradeOut, graded_by: Value) -> Grade {
    let score = grade_score(out).unwrap_or(0);
    let verdict = match score {
        80.. => "correct",
        40..=79 => "partial",
        _ => "incorrect",
    };
    let list = |v: &[Value]| v.iter().map(value_text).filter(|s| !s.is_empty()).collect::<Vec<_>>();
    let reread: Vec<Source> = out
        .reread
        .iter()
        .filter_map(|l| {
            let l = value_text(l).trim_matches(['[', ']']).to_uppercase();
            q.sources.iter().find(|s| s.label == l).cloned()
        })
        .collect();
    let reread_pages = if score >= 90 {
        vec![]
    } else if reread.is_empty() {
        source_pages(&q.sources)
    } else {
        source_pages(&reread)
    };
    Grade {
        score,
        verdict: verdict.into(),
        correct_points: list(&out.correct_points),
        missing_points: list(&out.missing_points),
        misconceptions: list(&out.misconceptions),
        feedback: out.feedback.trim().to_string(),
        reread_pages,
        graded_by,
        graded_at: now(),
    }
}

// ---------- overall review ----------

pub fn overall_score(qs: &[Question]) -> u32 {
    let graded: Vec<u32> = qs.iter().filter_map(|q| q.grade.as_ref().map(|g| g.score)).collect();
    if graded.is_empty() {
        0
    } else {
        (graded.iter().sum::<u32>() as f32 / qs.len() as f32).round() as u32
    }
}

pub fn summary_messages(chapter_title: &str, qs: &[Question]) -> Vec<Message> {
    let system = r#"You are a tutor writing a short, concrete review of a student's quiz on one chapter of a technical book. Be honest and encouraging.

Reply with a JSON object only, exactly in this shape:
{"summary":"2-4 sentences","strengths":["..."],"focus_next":["specific concepts to study next"],"next_step":"move_on|review_weak_spots|reread_chapter"}"#;
    let lines = qs
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let g = q.grade.as_ref();
            format!(
                "{}. [{} {}] {}\n   Missing: {}\n   Misconceptions: {}",
                i + 1,
                g.map(|g| g.verdict.as_str()).unwrap_or("unanswered"),
                g.map(|g| g.score).unwrap_or(0),
                q.prompt,
                g.map(|g| g.missing_points.join("; ")).unwrap_or_default(),
                g.map(|g| g.misconceptions.join("; ")).unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let user = format!("Chapter: {chapter_title}\nOverall score: {}/100\n\nResults:\n{lines}", overall_score(qs));
    vec![Message::system(system), Message::user(user)]
}

#[derive(Debug, Deserialize)]
pub struct SummaryOut {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub strengths: Vec<Value>,
    #[serde(default)]
    pub focus_next: Vec<Value>,
    #[serde(default)]
    pub next_step: String,
}

pub fn validate_summary(s: &SummaryOut) -> Result<(), String> {
    if s.summary.trim().is_empty() {
        return Err("\"summary\" must not be empty".into());
    }
    Ok(())
}

fn default_next_step(score: u32) -> &'static str {
    match score {
        80.. => "move_on",
        50..=79 => "review_weak_spots",
        _ => "reread_chapter",
    }
}

fn all_reread_pages(qs: &[Question]) -> Vec<u32> {
    let mut pages: Vec<u32> = qs.iter().filter_map(|q| q.grade.as_ref()).flat_map(|g| g.reread_pages.clone()).collect();
    pages.sort_unstable();
    pages.dedup();
    pages
}

pub fn to_overall(qs: &[Question], s: Option<&SummaryOut>) -> Overall {
    let score = overall_score(qs);
    let list = |v: &[Value]| v.iter().map(value_text).filter(|s| !s.is_empty()).collect::<Vec<_>>();
    match s {
        Some(s) => Overall {
            score,
            summary: s.summary.trim().to_string(),
            strengths: list(&s.strengths),
            focus_next: list(&s.focus_next),
            next_step: match s.next_step.as_str() {
                "move_on" | "review_weak_spots" | "reread_chapter" => s.next_step.clone(),
                _ => default_next_step(score).into(),
            },
            reread_pages: all_reread_pages(qs),
        },
        None => Overall {
            score,
            summary: format!("You scored {score}/100 on this chapter."),
            strengths: vec![],
            focus_next: qs
                .iter()
                .filter_map(|q| q.grade.as_ref())
                .flat_map(|g| g.missing_points.clone())
                .take(5)
                .collect(),
            next_step: default_next_step(score).into(),
            reread_pages: all_reread_pages(qs),
        },
    }
}

// ---------- persistence ----------

pub fn save(conn: &Connection, a: &Attempt, device_id: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO attempts(id, kind, book_fingerprint, chapter_id, chapter_title, status, model, questions, overall,
                              created_at, updated_at, device_id, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0)
         ON CONFLICT(id) DO UPDATE SET status = excluded.status, model = excluded.model, questions = excluded.questions,
             overall = excluded.overall, updated_at = excluded.updated_at, device_id = excluded.device_id",
        params![
            a.id,
            a.kind,
            a.book_fingerprint,
            a.chapter_id,
            a.chapter_title,
            a.status,
            a.model.to_string(),
            serde_json::to_string(&a.questions)?,
            a.overall.as_ref().map(serde_json::to_string).transpose()?,
            a.created_at,
            a.updated_at,
            device_id
        ],
    )?;
    Ok(())
}

fn row_to_attempt(r: &rusqlite::Row) -> rusqlite::Result<Attempt> {
    let model: String = r.get(6)?;
    let questions: String = r.get(7)?;
    let overall: Option<String> = r.get(8)?;
    Ok(Attempt {
        id: r.get(0)?,
        kind: r.get(1)?,
        book_fingerprint: r.get(2)?,
        chapter_id: r.get(3)?,
        chapter_title: r.get(4)?,
        status: r.get(5)?,
        model: serde_json::from_str(&model).unwrap_or(Value::Null),
        questions: serde_json::from_str(&questions).unwrap_or_default(),
        overall: overall.and_then(|o| serde_json::from_str(&o).ok()),
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

const ATTEMPT_COLS: &str =
    "id, kind, book_fingerprint, chapter_id, chapter_title, status, model, questions, overall, created_at, updated_at";

pub fn get(conn: &Connection, id: &str) -> AppResult<Attempt> {
    conn.query_row(&format!("SELECT {ATTEMPT_COLS} FROM attempts WHERE id = ?1 AND deleted = 0"), [id], row_to_attempt)
        .optional()?
        .ok_or_else(|| AppError::NotFound("Quiz not found".into()))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptSummary {
    pub id: String,
    pub book_fingerprint: Option<String>,
    pub book_title: Option<String>,
    pub chapter_id: Option<String>,
    pub chapter_title: Option<String>,
    pub status: String,
    pub score: Option<u32>,
    pub question_count: usize,
    pub answered: usize,
    pub created_at: String,
}

pub fn list(conn: &Connection, fp: Option<&str>) -> AppResult<Vec<AttemptSummary>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ATTEMPT_COLS}, (SELECT title FROM books b WHERE b.fingerprint = a.book_fingerprint)
         FROM attempts a WHERE deleted = 0 AND (?1 IS NULL OR book_fingerprint = ?1)
         ORDER BY created_at DESC LIMIT 500"
    ))?;
    let rows = stmt.query_map([fp], |r| Ok((row_to_attempt(r)?, r.get::<_, Option<String>>(11)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (a, book_title) = row?;
        out.push(AttemptSummary {
            score: a.overall.as_ref().map(|o| o.score),
            question_count: a.questions.len(),
            answered: a.questions.iter().filter(|q| q.grade.is_some()).count(),
            id: a.id,
            book_fingerprint: a.book_fingerprint,
            book_title,
            chapter_id: a.chapter_id,
            chapter_title: a.chapter_title,
            status: a.status,
            created_at: a.created_at,
        });
    }
    Ok(out)
}

pub fn delete(conn: &Connection, id: &str, device_id: &str) -> AppResult<()> {
    conn.execute("UPDATE attempts SET deleted = 1, updated_at = ?2, device_id = ?3 WHERE id = ?1", params![id, now(), device_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources() -> Vec<Source> {
        (1..=3)
            .map(|i| Source { label: format!("S{i}"), page_start: i * 10, page_end: i * 10 + 1, excerpt: "text".into() })
            .collect()
    }

    #[test]
    fn normalizes_and_grounds_questions() {
        let out: GenOut = serde_json::from_str(
            r#"{"questions":[
                {"type":"Multiple Choice","question":"Which pattern decouples X?","choices":["A","B","C","D"],"correct_choice":"B","sources":["S2"]},
                {"type":"short","question":"Why does the observer pattern help?","reference_answer":"Because...","sources":["S9"]},
                {"type":"mcq","question":"Which pattern decouples X ?","choices":["A","B","C","D"],"correct_choice":1,"sources":["S2"]},
                {"type":"explain","question":"Explain the strategy pattern.","key_points":["a","b"],"sources":["[S1]", "3"]}
            ]}"#,
        )
        .unwrap();
        let qs = normalize_generated(&out, &sources(), 10);
        assert_eq!(qs.len(), 2, "question citing S9 must be dropped");
        assert_eq!(qs[0].qtype, "mcq");
        assert_eq!(qs[0].correct_choice, Some(1));
        assert_eq!(qs[0].reference_answer, "B");
        assert_eq!(qs[1].sources.len(), 2);
        assert_eq!(strip_choice_letter("B. The built-in one"), "The built-in one");
        assert_eq!(strip_choice_letter("(c) Foo"), "Foo");
        assert_eq!(strip_choice_letter("A singleton"), "A singleton");
    }

    #[test]
    fn local_grading() {
        let q = Question {
            id: "1".into(),
            qtype: "mcq".into(),
            prompt: "p".into(),
            choices: Some(vec!["a".into(), "b".into(), "c".into()]),
            correct_choice: Some(2),
            reference_answer: "c".into(),
            key_points: vec![],
            sources: sources(),
            answer: None,
            grade: None,
        };
        assert_eq!(local_grade(&q, "2").unwrap().score, 100);
        let wrong = local_grade(&q, "0").unwrap();
        assert_eq!(wrong.verdict, "incorrect");
        assert_eq!(wrong.reread_pages, vec![10, 20, 30]);
        assert_eq!(local_grade(&q, "  ").unwrap().score, 0);
    }

    #[test]
    fn grade_scores_are_normalized() {
        let g = |s: &str| grade_score(&serde_json::from_str::<GradeOut>(&format!(r#"{{"score":{s},"feedback":"x"}}"#)).unwrap());
        assert_eq!(g("85"), Some(85));
        assert_eq!(g("\"70%\""), Some(70));
        assert_eq!(g("0.9"), Some(90));
        assert_eq!(g("7"), Some(70));
        assert_eq!(g("0"), Some(0));
    }
}
