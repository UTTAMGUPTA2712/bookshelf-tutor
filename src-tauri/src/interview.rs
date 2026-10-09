//! Mock interviews: an interviewer character asks planned questions and keeps probing every key point
//! from new angles until you have clearly shown it (or clearly can't), then writes a report.
//!
//! Flow: one call plans questions of mixed difficulty (grounded in book excerpts when there are any).
//! For each candidate message, a strict **judge** call works out the intent (answer, clarifying
//! question, "repeat that", thinking aloud, skip, …) and marks the key points; Rust then **decides**
//! the next move under fixed rules (tries per point, turns per question, free turns for clarifying);
//! finally the **interviewer** call says that move in character. "Fast" mode folds judge and voice
//! into one call for slow local models. Obvious cases (hint button, "I don't know", "repeat", "skip")
//! need no model call at all. The next main question is picked adaptively from a pool: harder after
//! strong answers, easier after weak ones.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::llm::Message;
use crate::quiz::Source;

pub const PROMPT_VERSION: &str = "interview.v2";
/// A point not verified after this many follow-ups on it is closed (partial or missing).
const MAX_POINT_PROBES: u32 = 2;
const HINT_PENALTY: u32 = 8;
/// Clarifying questions, "repeat that", thinking aloud: free per question up to this many.
const MAX_FREE_TURNS: u32 = 3;

// ---------- interviewers ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Persona {
    pub id: String,
    pub name: String,
    pub role: String,
    pub style: String,
    pub probing: String,
    pub hints: String,
    pub challenges: String,
}

#[derive(Deserialize)]
struct PersonaFile {
    personas: Vec<Persona>,
}

pub fn personas() -> &'static [Persona] {
    static P: std::sync::OnceLock<Vec<Persona>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        serde_json::from_str::<PersonaFile>(include_str!("../resources/interviewers.json"))
            .expect("resources/interviewers.json is valid")
            .personas
    })
}

pub fn persona(id: &str) -> &'static Persona {
    personas().iter().find(|p| p.id == id).unwrap_or(&personas()[0])
}

/// A different interviewer each time: one not met in the recent interviews (most recent first),
/// picked pseudo-randomly from `seed`.
pub fn pick_persona(recent: &[String], seed: u64) -> &'static Persona {
    let all = personas();
    let window = recent.len().min(all.len().saturating_sub(1));
    let fresh: Vec<&Persona> = all.iter().filter(|p| !recent[..window].contains(&p.id)).collect();
    let pool: Vec<&Persona> = if fresh.is_empty() { all.iter().collect() } else { fresh };
    pool[(seed % pool.len() as u64) as usize]
}

// ---------- records ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    /// "chapter" | "book" | "topics" | "general"
    pub source: String,
    pub book_fingerprint: Option<String>,
    pub chapter_idx: Option<u32>,
    pub track: Option<String>,
    #[serde(default)]
    pub topic_ids: Vec<String>,
    /// "junior" | "mid" | "senior"
    pub level: String,
    pub minutes: u32,
    /// None = rotate automatically.
    pub persona_id: Option<String>,
    /// "thorough" (separate judge + voice calls) | "fast" (one call) | "auto" (decided at start).
    #[serde(default)]
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointState {
    pub text: String,
    /// "untested" | "verified" | "partial" | "missing"
    pub status: String,
    pub probes: u32,
    /// Angles already used to probe this point (why, example, edge_case, …).
    #[serde(default)]
    pub angles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IQuestion {
    pub id: String,
    /// "concept" | "scenario" | "design" | "code"
    pub kind: String,
    /// "easy" | "medium" | "hard"
    #[serde(default = "medium")]
    pub difficulty: String,
    /// 1-based position in the interview once asked; spare questions that were never needed stay None.
    #[serde(default)]
    pub order: Option<u32>,
    pub topic: Option<String>,
    pub prompt: String,
    pub code: Option<String>,
    pub reference: String,
    pub sources: Vec<Source>,
    pub points: Vec<PointState>,
    pub turns: u32,
    pub hints: u32,
    #[serde(default)]
    pub free_turns: u32,
    #[serde(default)]
    pub last_focus: Option<usize>,
    pub done: bool,
    pub score: Option<u32>,
}

fn medium() -> String {
    "medium".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    /// "interviewer" | "candidate"
    pub role: String,
    /// interviewer: "opening" | "question" | "probe" | "challenge" | "hint" | "clarify" | "repeat" | "wait" |
    /// "redirect" | "transition" | "closing"; candidate: "answer" | "hint_request" | "dont_know"
    pub kind: String,
    pub text: String,
    pub question: usize,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionNote {
    pub question: usize,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub overall: u32,
    /// "strong" | "good" | "needs_work"
    pub verdict: String,
    /// Share of key points shown, 0-100.
    pub coverage: u32,
    /// 1-5, from the interviewer.
    pub depth: Option<u32>,
    pub clarity: Option<u32>,
    pub summary: String,
    pub strengths: Vec<String>,
    pub gaps: Vec<String>,
    pub best_moment: Option<String>,
    pub weakest_moment: Option<String>,
    pub notes: Vec<QuestionNote>,
    pub reread_pages: Vec<u32>,
    pub written_by: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interview {
    pub id: String,
    pub title: String,
    pub persona_id: String,
    pub setup: Setup,
    pub questions: Vec<IQuestion>,
    pub transcript: Vec<Turn>,
    /// Index of the question being discussed.
    pub current: usize,
    /// "in_progress" | "wrapping_up" | "completed"
    pub status: String,
    pub report: Option<Report>,
    pub topics: Vec<String>,
    pub model: Value,
    pub created_at: String,
    pub updated_at: String,
}

pub fn question_count(minutes: u32) -> usize {
    match minutes {
        0..=20 => 4,
        21..=35 => 7,
        _ => 10,
    }
}

/// Extra questions planned beyond `question_count`, so the next one can be picked by difficulty.
pub fn spare_count(count: usize) -> usize {
    (count / 3).max(2)
}

/// How many easy / medium / hard questions to plan.
pub fn difficulty_mix(level: &str, total: usize) -> (usize, usize, usize) {
    let (e, m) = match level {
        "junior" => (0.4, 0.4),
        "senior" => (0.2, 0.4),
        _ => (0.34, 0.33),
    };
    let easy = ((total as f32 * e).round() as usize).max(1);
    let med = ((total as f32 * m).round() as usize).max(1);
    (easy, med, total.saturating_sub(easy + med).max(1))
}

/// Candidate turns allowed per question before the interviewer must move on.
pub fn turn_cap(level: &str) -> u32 {
    match level {
        "junior" => 4,
        "senior" => 7,
        _ => 5,
    }
}

// ---------- planning ----------

fn persona_block(p: &Persona) -> String {
    format!(
        "You are {}, {}.\nPersonality: {}\nHow you probe: {}\nHints: {}\nYou challenge answers that sound right: {}.",
        p.name, p.role, p.style, p.probing, p.hints, p.challenges
    )
}

fn excerpts_block(sources: &[Source]) -> String {
    sources
        .iter()
        .map(|s| format!("[{}] (pages {}-{})\n{}", s.label, s.page_start, s.page_end, s.excerpt.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub struct PlanInput<'a> {
    pub persona: &'a Persona,
    pub level: &'a str,
    pub count: usize,
    /// What the interview is about, e.g. "Head First Design Patterns — Chapter 2: the Observer Pattern".
    pub subject: &'a str,
    /// (id, label) of topics the questions may be tagged with.
    pub topics: &'a [(String, String)],
    pub sources: &'a [Source],
}

pub fn plan_messages(i: &PlanInput) -> Vec<Message> {
    let mix = difficulty_mix(i.level, i.count + spare_count(i.count));
    let grounding = if i.sources.is_empty() {
        "Use your own expert knowledge of the subject."
    } else {
        "Base every question ONLY on the provided book excerpts and cite 1-3 excerpt labels (like \"S2\") in \"sources\"."
    };
    let system = format!(
        r#"{persona}

You are preparing a technical job interview for a {level}-level software engineer. You will ask the questions yourself, in character.

Rules:
- Write exactly {total} questions: {easy} "easy" (warm-up: core idea), {med} "medium" (apply it), {hard} "hard" (edge cases, trade-offs, reasoning about consequences). Cover different parts of the subject. {grounding}
- Mix kinds: "concept" (explain how/why something works), "scenario" (apply it to a realistic situation), "design" (only for mid/senior: choose and justify an approach), "code" (only if the subject is about programming: predict the output, find the bug, or write a small function; put the code in "code").
- "question": exactly what you will say to the candidate, in your own voice. One question, no sub-lists.
- "points": 3-6 short, checkable key points that a COMPLETE answer must show. These are what you will verify one by one.
- "reference": a model answer in 2-4 sentences.
- "topic": one of the given topic ids that the question tests, if any fit.
- "opening": 1-2 sentences to greet the candidate and introduce yourself, in character. Do not ask a question in it.
- Write in English.

Reply with a JSON object only, exactly in this shape:
{{"opening":"...","questions":[{{"difficulty":"easy","kind":"concept","topic":"...","question":"...","code":null,"points":["..."],"reference":"...","sources":["S1"]}}]}}"#,
        persona = persona_block(i.persona),
        level = i.level,
        total = i.count + spare_count(i.count),
        easy = mix.0,
        med = mix.1,
        hard = mix.2,
        grounding = grounding,
    );
    let topics = if i.topics.is_empty() {
        "(none)".to_string()
    } else {
        i.topics.iter().map(|(id, l)| format!("{id} = {l}")).collect::<Vec<_>>().join("\n")
    };
    let mut user = format!("Subject: {}\nTopic ids:\n{topics}", i.subject);
    if !i.sources.is_empty() {
        user.push_str(&format!("\n\nExcerpts:\n\n{}", excerpts_block(i.sources)));
    }
    vec![Message::system(system), Message::user(user)]
}

#[derive(Debug, Deserialize)]
pub struct PlanOut {
    #[serde(default)]
    pub opening: String,
    #[serde(default)]
    pub questions: Vec<PlanQ>,
}

#[derive(Debug, Deserialize)]
pub struct PlanQ {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub difficulty: String,
    #[serde(default)]
    pub topic: Option<Value>,
    #[serde(default, alias = "prompt")]
    pub question: String,
    #[serde(default)]
    pub code: Option<Value>,
    #[serde(default, alias = "key_points")]
    pub points: Vec<Value>,
    #[serde(default, alias = "reference_answer")]
    pub reference: Option<Value>,
    #[serde(default)]
    pub sources: Vec<Value>,
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub fn normalize_plan(out: &PlanOut, sources: &[Source], topics: &[(String, String)], default_topic: Option<&str>, max: usize) -> Vec<IQuestion> {
    let mut qs = Vec::new();
    for p in &out.questions {
        let prompt = p.question.trim().to_string();
        let points: Vec<PointState> = p
            .points
            .iter()
            .map(text)
            .filter(|t| !t.is_empty())
            .take(6)
            .map(|t| PointState { text: t, status: "untested".into(), probes: 0, angles: vec![] })
            .collect();
        if prompt.len() < 10 || points.len() < 2 {
            continue;
        }
        let cited: Vec<Source> = p
            .sources
            .iter()
            .filter_map(|s| {
                let l = text(s).trim_matches(['[', ']']).to_uppercase();
                let l = if l.starts_with('S') { l } else { format!("S{l}") };
                sources.iter().find(|x| x.label == l).cloned()
            })
            .collect();
        if !sources.is_empty() && cited.is_empty() {
            continue; // grounding check
        }
        let kind = match p.kind.trim().to_lowercase().as_str() {
            k @ ("concept" | "scenario" | "design" | "code") => k.to_string(),
            _ => "concept".into(),
        };
        let topic = p
            .topic
            .as_ref()
            .map(text)
            .filter(|t| topics.iter().any(|(id, _)| id == t))
            .or_else(|| default_topic.map(String::from));
        let code = p.code.as_ref().map(text).filter(|c| c.len() > 3 && c != "null");
        let difficulty = match p.difficulty.trim().to_lowercase().as_str() {
            "easy" | "warm-up" | "warmup" => "easy",
            "hard" | "difficult" => "hard",
            _ => "medium",
        };
        qs.push(IQuestion {
            id: uuid::Uuid::now_v7().to_string(),
            kind,
            difficulty: difficulty.into(),
            order: None,
            topic,
            prompt,
            code,
            reference: p.reference.as_ref().map(text).unwrap_or_default(),
            sources: cited,
            points,
            turns: 0,
            hints: 0,
            free_turns: 0,
            last_focus: None,
            done: false,
            score: None,
        });
        if qs.len() >= max {
            break;
        }
    }
    qs
}

pub fn validate_plan(out: &PlanOut, sources: &[Source], want: usize) -> Result<(), String> {
    let n = normalize_plan(out, sources, &[], None, want + spare_count(want)).len();
    if n < want.div_ceil(2) {
        return Err(format!(
            "only {n} usable questions — each needs \"question\", 3-6 \"points\"{}",
            if sources.is_empty() { "" } else { " and \"sources\" citing the S-labels" }
        ));
    }
    Ok(())
}

fn question_text(q: &IQuestion) -> String {
    match &q.code {
        Some(code) => format!("{}\n\n```\n{}\n```", q.prompt, code.trim_matches('`').trim()),
        None => q.prompt.clone(),
    }
}

fn interviewer(text: impl Into<String>, kind: &str, question: usize) -> Turn {
    Turn { role: "interviewer".into(), kind: kind.into(), text: text.into(), question, at: now() }
}

pub fn new_interview(setup: Setup, persona: &Persona, title: String, opening: &str, questions: Vec<IQuestion>, topics: Vec<String>, model: Value) -> Interview {
    let ts = now();
    let opening = if opening.trim().is_empty() {
        format!("Hi, I'm {}, {}. Let's get started.", persona.name, persona.role)
    } else {
        opening.trim().to_string()
    };
    let mut iv = Interview {
        id: uuid::Uuid::now_v7().to_string(),
        title,
        persona_id: persona.id.clone(),
        setup,
        questions,
        transcript: vec![interviewer(opening, "opening", 0)],
        current: 0,
        status: "in_progress".into(),
        report: None,
        topics,
        model,
        created_at: ts.clone(),
        updated_at: ts,
    };
    ask_next(&mut iv);
    iv
}

// ---------- adaptive question choice ----------

fn difficulty_num(d: &str) -> f32 {
    match d {
        "easy" => 1.0,
        "hard" => 3.0,
        _ => 2.0,
    }
}

pub fn asked_count(iv: &Interview) -> usize {
    iv.questions.iter().filter(|q| q.order.is_some()).count()
}

/// Where the next question should sit (1 = easy … 3 = hard): starts a little below the level and
/// moves up after a strong answer (≥ 80), down after a weak one (< 50).
pub fn target_difficulty(iv: &Interview) -> f32 {
    let mut t: f32 = match iv.setup.level.as_str() {
        "junior" => 1.0,
        "senior" => 2.0,
        _ => 1.5,
    };
    let mut done: Vec<&IQuestion> = iv.questions.iter().filter(|q| q.done && q.order.is_some()).collect();
    done.sort_by_key(|q| q.order);
    for q in done {
        match q.score.unwrap_or(0) {
            80.. => t += 1.0,
            0..50 => t -= 1.0,
            _ => {}
        }
        t = t.clamp(1.0, 3.0);
    }
    t
}

/// The unasked question closest to the target difficulty (ties: easier first, then a topic and kind
/// not covered yet). None once enough questions have been asked.
pub fn pick_next(iv: &Interview) -> Option<usize> {
    if asked_count(iv) >= question_count(iv.setup.minutes) {
        return None;
    }
    let target = target_difficulty(iv);
    let used_topics: Vec<&Option<String>> = iv.questions.iter().filter(|q| q.order.is_some()).map(|q| &q.topic).collect();
    let used_kinds: Vec<&String> = iv.questions.iter().filter(|q| q.order.is_some()).map(|q| &q.kind).collect();
    iv.questions
        .iter()
        .enumerate()
        .filter(|(_, q)| q.order.is_none())
        .min_by(|(i, a), (j, b)| {
            let key = |q: &IQuestion| {
                let d = difficulty_num(&q.difficulty);
                (
                    ((d - target).abs() * 10.0) as i32,
                    d as i32,
                    used_topics.contains(&&q.topic) as i32,
                    used_kinds.contains(&&q.kind) as i32,
                )
            };
            key(a).cmp(&key(b)).then(i.cmp(j))
        })
        .map(|(i, _)| i)
}

/// Asks the next main question, or closes the interview when there is none.
fn ask_next(iv: &mut Interview) {
    match pick_next(iv) {
        Some(i) => {
            iv.questions[i].order = Some(asked_count(iv) as u32 + 1);
            iv.current = i;
            let text = question_text(&iv.questions[i]);
            iv.transcript.push(interviewer(text, "question", i));
        }
        None => {
            let idx = iv.current;
            iv.transcript.push(interviewer(
                "That's all the questions I have. Thanks for your time — I'll write up my notes now.",
                "closing",
                idx,
            ));
            iv.status = "wrapping_up".into();
        }
    }
}

// ---------- the conversation ----------

/// What the judge (or the rules, for obvious cases) concluded about the candidate's message.
#[derive(Debug, Default, Deserialize)]
pub struct JudgeOut {
    #[serde(default)]
    pub intent: String,
    #[serde(default)]
    pub points: Vec<Value>,
    #[serde(default)]
    pub focus: Option<Value>,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub angle: String,
    #[serde(default)]
    pub note: String,
    /// Fast mode only: the interviewer's line, written in the same call.
    #[serde(default)]
    pub message: String,
}

const INTENTS: &[&str] = &["answer", "clarify", "repeat", "thinking", "skip", "answer_request", "off_topic"];
const ANGLES: &[(&str, &str)] = &[
    ("why", "ask why it works that way"),
    ("how", "ask exactly how it works, step by step"),
    ("example", "ask for a concrete example"),
    ("edge_case", "ask about an edge case or a failure"),
    ("compare", "ask them to compare it with an alternative"),
    ("what_if", "change one condition and ask what happens"),
    ("define", "ask for a precise definition"),
    ("estimate", "ask for numbers: cost, size, time"),
];

fn persona_line(iv: &Interview) -> String {
    persona_block(persona(&iv.persona_id))
}

fn question_history(iv: &Interview) -> String {
    iv.transcript
        .iter()
        .filter(|t| t.question == iv.current && t.kind != "opening")
        .map(|t| format!("{}: {}", if t.role == "interviewer" { "Interviewer" } else { "Candidate" }, t.text))
        .collect::<Vec<_>>()
        .join("\n")
}

fn open_points(q: &IQuestion) -> Vec<usize> {
    q.points
        .iter()
        .enumerate()
        .filter(|(_, p)| p.status != "verified" && p.status != "missing" && p.probes < MAX_POINT_PROBES)
        .map(|(i, _)| i)
        .collect()
}

/// Cases that need no model: the hint and "I don't know" buttons, "repeat that", "skip".
pub fn quick_judgement(iv: &Interview, kind: &str, text: &str) -> Option<JudgeOut> {
    let q = &iv.questions[iv.current];
    let t = text.trim().to_lowercase();
    let t = t.trim_end_matches(['.', '!', '?']);
    let out = |intent: &str, action: &str, points: Vec<Value>| JudgeOut {
        intent: intent.into(),
        action: action.into(),
        points,
        ..Default::default()
    };
    if kind == "hint_request" {
        return Some(out("answer_request", "hint", vec![]));
    }
    if kind == "dont_know" {
        // The point just asked about is missing; on the main question, everything not yet shown is.
        let missing: Vec<Value> = match q.last_focus {
            Some(f) => vec![serde_json::json!({ "i": f + 1, "status": "missing" })],
            None => (0..q.points.len())
                .filter(|i| q.points[*i].status == "untested")
                .map(|i| serde_json::json!({ "i": i + 1, "status": "missing" }))
                .collect(),
        };
        return Some(out("answer", "probe", missing));
    }
    const REPEAT: &[&str] = &["repeat", "repeat that", "repeat the question", "can you repeat", "can you repeat that", "can you repeat the question", "pardon", "say that again", "come again", "sorry, what"];
    const SKIP: &[&str] = &["skip", "skip this", "skip this one", "next", "next question", "pass", "let's move on", "move on"];
    if REPEAT.contains(&t) {
        return Some(out("repeat", "", vec![]));
    }
    if SKIP.contains(&t) {
        return Some(out("skip", "move_on", vec![]));
    }
    None
}

pub fn judge_messages(iv: &Interview, candidate_text: &str, fast: bool) -> Vec<Message> {
    let q = &iv.questions[iv.current];
    let turns_left = turn_cap(&iv.setup.level).saturating_sub(q.turns + 1);
    let who = if fast {
        format!("{}\n\nYou are running a live technical interview and also judging the answers.", persona_line(iv))
    } else {
        "You are the strict, fair evaluator behind a live technical interview. You do not talk to the candidate; you decide what the interviewer does next.".to_string()
    };
    let angles = ANGLES.iter().map(|(a, d)| format!("\"{a}\" ({d})")).collect::<Vec<_>>().join(", ");
    let message_rule = if fast {
        "\n6. \"message\": what you say next, 1-3 spoken sentences in character. For clarify: explain what the question means without giving away the answer. Never reveal the reference answer, never say whether they were right, never lecture. For move_on: a short acknowledgement only (the next question is asked separately)."
    } else {
        ""
    };
    let message_field = if fast { ",\"message\":\"...\"" } else { "" };
    let system = format!(
        r#"{who}

The candidate is interviewing for a {level}-level role. The goal on each question is to find out whether they know EVERY key point completely, not just the headline.

For the candidate's latest message:
1. "intent": "answer" (any attempt to answer, even partial or wrong), "clarify" (asks what the question means or about assumptions), "repeat" (wants the question again), "thinking" (thinking aloud with nothing to judge yet), "skip" (wants to move on), "answer_request" (asks to be told the answer), "off_topic".
2. Only if the intent is "answer", update the key points that the message addresses:
   "verified" = clearly shown, correct, in their own words; "partial" = mentioned but vague, incomplete or partly wrong; "missing" = wrong, or they could not answer when asked. Leave others out.
3. "action" (for answers): "probe" = ask about ONE point that is not verified yet; "challenge" = the answer sounds right but might be memorized, so push back on it; "move_on" = every point is verified or clearly missing, or no turns are left.
4. "focus": the number of the point to probe or challenge. "angle": one of {angles} — never one already used on that point.
5. "note": one short sentence for the interviewer: what exactly to dig into.{message_rule}

Reply with a JSON object only, exactly in this shape:
{{"intent":"answer","points":[{{"i":1,"status":"verified"}}],"action":"probe","focus":2,"angle":"edge_case","note":"..."{message_field}}}"#,
        level = iv.setup.level,
    );
    let points = q
        .points
        .iter()
        .enumerate()
        .map(|(i, pt)| {
            let closed = if pt.status != "verified" && pt.probes >= MAX_POINT_PROBES { " (closed: do not ask again)" } else { "" };
            let used = if pt.angles.is_empty() { String::new() } else { format!(" (angles used: {})", pt.angles.join(", ")) };
            format!("{}. [{}] {}{}{}", i + 1, pt.status, pt.text, used, closed)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut user = format!(
        "Question ({}, {}): {}\n\nKey points to verify:\n{points}\n\nReference answer (secret): {}",
        q.kind,
        q.difficulty,
        question_text(q),
        q.reference
    );
    if !q.sources.is_empty() {
        user.push_str(&format!("\n\nBook excerpts (secret):\n{}", excerpts_block(&q.sources)));
    }
    user.push_str(&format!(
        "\n\nConversation on this question so far:\n{}\n\nCandidate's latest message:\n<<<\n{}\n>>>\n\nAnswer turns left on this question after this one: {turns_left}{}",
        question_history(iv),
        candidate_text.trim(),
        if turns_left == 0 { " — if it is an answer, choose \"move_on\"." } else { "." }
    ));
    vec![Message::system(system), Message::user(user)]
}

pub fn validate_judge(out: &JudgeOut, fast: bool) -> Result<(), String> {
    if !INTENTS.contains(&out.intent.as_str()) {
        return Err(format!("\"intent\" must be one of {}", INTENTS.join(", ")));
    }
    if out.intent == "answer" && !matches!(out.action.as_str(), "probe" | "challenge" | "move_on") {
        return Err("for an answer, \"action\" must be probe, challenge or move_on".into());
    }
    if fast && out.action != "move_on" && out.intent != "repeat" && out.message.trim().is_empty() {
        return Err("\"message\" must not be empty".into());
    }
    Ok(())
}

/// The move the interviewer makes next, decided by the rules from the judgement.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// "probe" | "challenge" | "hint" | "clarify" | "repeat" | "wait" | "redirect" | "move_on"
    pub action: String,
    pub focus: Option<usize>,
    pub angle: String,
    pub note: String,
}

impl Decision {
    fn new(action: &str) -> Self {
        Decision { action: action.into(), focus: None, angle: String::new(), note: String::new() }
    }
    /// Moves that need words from the interviewer's voice (the rest are fixed lines).
    pub fn needs_voice(&self) -> bool {
        matches!(self.action.as_str(), "probe" | "challenge" | "hint" | "clarify" | "redirect")
    }
}

fn as_index(v: &Value) -> Option<usize> {
    match v {
        Value::Number(n) => n.as_u64().map(|n| n as usize),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn question_score(q: &IQuestion) -> u32 {
    if q.points.is_empty() {
        return 0;
    }
    let sum: f32 = q
        .points
        .iter()
        .map(|p| match p.status.as_str() {
            "verified" => 1.0,
            "partial" => 0.5,
            _ => 0.0,
        })
        .sum();
    ((sum / q.points.len() as f32 * 100.0).round() as u32).saturating_sub(q.hints * HINT_PENALTY)
}

/// Records what the candidate said.
pub fn add_candidate(iv: &mut Interview, kind: &str, text: &str) {
    let text = match kind {
        "dont_know" if text.trim().is_empty() => "I don't know.".to_string(),
        "hint_request" if text.trim().is_empty() => "Could I get a hint?".to_string(),
        _ => text.trim().to_string(),
    };
    let q = iv.current;
    iv.transcript.push(Turn { role: "candidate".into(), kind: kind.into(), text, question: q, at: now() });
}

/// Applies the judgement under the interview rules and returns the next move. Updates point
/// statuses and counters; the interviewer's words are added later by [`commit`].
pub fn decide(iv: &mut Interview, out: &JudgeOut) -> Decision {
    let cap = turn_cap(&iv.setup.level);
    let q = &mut iv.questions[iv.current];
    let intent = if INTENTS.contains(&out.intent.as_str()) { out.intent.as_str() } else { "answer" };

    // Not an answer: free (up to a limit), nothing is marked.
    let free = match intent {
        "clarify" => Some("clarify"),
        "repeat" => Some("repeat"),
        "thinking" => Some("wait"),
        "off_topic" => Some("redirect"),
        _ => None,
    };
    if let Some(action) = free {
        q.free_turns += 1;
        if q.free_turns > MAX_FREE_TURNS {
            q.turns += 1; // past the limit these count against the question
            if q.turns >= cap {
                return Decision::new("move_on");
            }
        }
        return Decision::new(action);
    }
    if intent == "skip" {
        return Decision::new("move_on");
    }

    q.turns += 1;
    let open_before = open_points(q);
    if intent == "answer_request" {
        q.hints += 1;
        let focus = q.last_focus.filter(|f| open_before.contains(f)).or(open_before.first().copied());
        return match focus {
            Some(f) if q.turns < cap => {
                q.points[f].probes += 1;
                q.last_focus = Some(f);
                Decision { focus: Some(f), ..Decision::new("hint") }
            }
            _ => Decision::new("move_on"),
        };
    }

    for p in &out.points {
        let (Some(i), Some(status)) = (p.get("i").and_then(as_index), p.get("status").and_then(Value::as_str)) else {
            continue;
        };
        if let Some(pt) = q.points.get_mut(i.wrapping_sub(1)) {
            if pt.status != "verified" && matches!(status, "verified" | "partial" | "missing") {
                pt.status = status.into();
            }
        }
    }
    let open = open_points(q);
    let wanted = out.focus.as_ref().and_then(as_index).and_then(|i| i.checked_sub(1)).filter(|i| *i < q.points.len());
    let action = match out.action.as_str() {
        "challenge" if q.turns < cap && wanted.is_some() => "challenge",
        "move_on" => "move_on",
        _ if open.is_empty() || q.turns >= cap => "move_on",
        _ => "probe",
    };
    if action == "move_on" || (action == "probe" && open.is_empty()) {
        return Decision::new("move_on");
    }
    // Probe an open point (the judge's choice if it is still open); a challenge may target any point.
    let focus = if action == "challenge" { wanted.expect("checked") } else { wanted.filter(|f| open.contains(f)).unwrap_or(open[0]) };
    let used = &q.points[focus].angles;
    let angle = if ANGLES.iter().any(|(a, _)| *a == out.angle) && !used.contains(&out.angle) {
        out.angle.clone()
    } else {
        ANGLES.iter().map(|(a, _)| a.to_string()).find(|a| !used.contains(a)).unwrap_or_else(|| "how".into())
    };
    let pt = &mut q.points[focus];
    pt.probes += 1;
    pt.angles.push(angle.clone());
    q.last_focus = Some(focus);
    Decision { action: action.into(), focus: Some(focus), angle, note: out.note.trim().to_string() }
}

pub fn voice_messages(iv: &Interview, d: &Decision, candidate_text: &str) -> Vec<Message> {
    let q = &iv.questions[iv.current];
    let p = persona(&iv.persona_id);
    let system = format!(
        r#"{persona}

You are speaking in a live technical interview. The evaluation is already done: say exactly the move described, in character.

Rules:
- 1-3 sentences of natural speech. Plain text only: no quotes around it, no name prefix, no lists.
- Never reveal the reference answer or the key points, never say whether the candidate was right, never lecture.
- Ask at most one question."#,
        persona = persona_block(p)
    );
    let point = d.focus.and_then(|f| q.points.get(f)).map(|p| p.text.as_str()).unwrap_or("");
    let angle = ANGLES.iter().find(|(a, _)| *a == d.angle).map(|(_, desc)| *desc).unwrap_or("ask how it works");
    let note = if d.note.is_empty() { String::new() } else { format!(" Evaluator's note: {}", d.note) };
    let mv = match d.action.as_str() {
        "probe" => format!("Ask ONE follow-up that makes the candidate show this, without stating it: \"{point}\". Approach: {angle}.{note}"),
        "challenge" => format!("Push back skeptically on what they said about \"{point}\" so they have to justify it.{note}"),
        "hint" => format!("They asked for a hint. Give one small nudge toward \"{point}\" without stating it (your hint style: {}), then ask them to try again.", p.hints),
        "clarify" => format!("They asked: \"{}\". Clarify what the question means (scope or assumptions) without giving away any part of the answer, then invite them to answer.", candidate_text.trim()),
        _ => format!("They went off-topic: \"{}\". Steer back to the question, politely and in character.", candidate_text.trim()),
    };
    let user = format!(
        "Question: {}\n\nReference answer (secret, do not reveal): {}\n\nConversation so far:\n{}\n\nYour move: {mv}",
        question_text(q),
        q.reference,
        question_history(iv)
    );
    vec![Message::system(system), Message::user(user)]
}

/// Tidies what the voice model said: no wrapping quotes or "Name:" prefix, not too long.
pub fn clean_speech(raw: &str, persona_name: &str) -> String {
    let mut s = raw.trim().to_string();
    let words: Vec<&str> = persona_name.split_whitespace().filter(|w| !w.ends_with('.')).collect();
    let first = words.first().copied().unwrap_or(persona_name);
    let last = words.last().copied().unwrap_or(persona_name);
    for prefix in [format!("{persona_name}:"), format!("{first}:"), format!("{last}:"), "Interviewer:".to_string()] {
        if let Some(rest) = s.strip_prefix(&prefix) {
            s = rest.trim().to_string();
        }
    }
    let s = s.trim_matches(|c| c == '"' || c == '“' || c == '”').trim().to_string();
    if s.chars().count() > 700 {
        let cut: String = s.chars().take(700).collect();
        match cut.rfind(['.', '?', '!']) {
            Some(i) => cut[..=i].to_string(),
            None => cut,
        }
    } else {
        s
    }
}

/// Fixed lines for moves that need no model, and fallbacks if the voice call fails.
pub fn fixed_line(iv: &Interview, d: &Decision) -> String {
    let q = &iv.questions[iv.current];
    match d.action.as_str() {
        "repeat" => format!("Sure. {}", question_text(q)),
        "wait" => "Take your time — go on.".into(),
        "probe" => "Can you go deeper on that? Walk me through exactly how it works.".into(),
        "challenge" => "Are you sure about that? Convince me.".into(),
        "hint" => "Here's a nudge: think about the details you haven't mentioned yet. Try again.".into(),
        "clarify" => "Answer it the way you understand it, and state any assumptions you make.".into(),
        "redirect" => "Let's stay with the question.".into(),
        _ => ["Okay.", "Alright, thanks.", "Got it.", "Fair enough.", "Okay, noted."][iv.transcript.len() % 5].into(),
    }
}

/// Adds the interviewer's line for the decided move; on "move_on", scores the question and asks
/// the next one (or closes the interview).
pub fn commit(iv: &mut Interview, d: &Decision, speech: &str) {
    let idx = iv.current;
    let line = if speech.trim().is_empty() { fixed_line(iv, d) } else { speech.trim().to_string() };
    if d.action != "move_on" {
        iv.transcript.push(interviewer(line, &d.action, idx));
        return;
    }
    let q = &mut iv.questions[idx];
    for pt in q.points.iter_mut().filter(|p| p.status == "untested") {
        pt.status = "missing".into();
    }
    q.done = true;
    q.score = Some(question_score(q));
    iv.transcript.push(interviewer(line, "transition", idx));
    ask_next(iv);
}

/// Ends the interview early: the question in progress is scored on what was shown so far.
pub fn stop(iv: &mut Interview) {
    let idx = iv.current;
    if let Some(q) = iv.questions.get_mut(idx) {
        if !q.done && q.turns > 0 {
            for pt in q.points.iter_mut().filter(|p| p.status == "untested") {
                pt.status = "missing".into();
            }
            q.done = true;
            q.score = Some(question_score(q));
        }
    }
    iv.status = "wrapping_up".into();
}

// ---------- report ----------

/// Finished questions in the order they were asked.
pub fn asked(iv: &Interview) -> impl Iterator<Item = (usize, &IQuestion)> {
    let mut qs: Vec<(usize, &IQuestion)> = iv.questions.iter().enumerate().filter(|(_, q)| q.done).collect();
    qs.sort_by_key(|(_, q)| q.order);
    qs.into_iter()
}

pub fn report_messages(iv: &Interview) -> Vec<Message> {
    let p = persona(&iv.persona_id);
    let system = format!(
        r#"{persona}

The interview is over. Write your honest notes for the candidate ({level} level), in your own voice but addressed to them ("you"). Be specific: refer to what they actually said.

Also write flashcards for key points they missed or only partly knew: 1-2 per question that had gaps, each testing ONE fact. "q" is the question number.

Reply with a JSON object only, exactly in this shape:
{{"summary":"3-5 sentences","strengths":["..."],"gaps":["specific things to study"],"best_moment":"...","weakest_moment":"...","depth":3,"clarity":3,"notes":[{{"q":1,"note":"one sentence"}}],"cards":[{{"q":1,"front":"...","back":"..."}}]}}
"depth" and "clarity" are 1-5."#,
        persona = persona_block(p),
        level = iv.setup.level,
    );
    let mut body = String::new();
    for (i, q) in asked(iv) {
        body.push_str(&format!("\nQuestion {} ({}, {}): {}\nKey points:\n", i + 1, q.kind, q.difficulty, q.prompt));
        for pt in &q.points {
            body.push_str(&format!("- [{}] {}\n", pt.status, pt.text));
        }
        let said: Vec<String> = iv
            .transcript
            .iter()
            .filter(|t| t.question == i && t.role == "candidate")
            .map(|t| {
                let s: String = t.text.chars().take(600).collect();
                format!("  > {s}")
            })
            .collect();
        body.push_str(&format!("Candidate said:\n{}\nHints used: {}\n", said.join("\n"), q.hints));
    }
    vec![Message::system(system), Message::user(format!("Subject: {}\n{body}", iv.title))]
}

#[derive(Debug, Default, Deserialize)]
pub struct ReportOut {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub strengths: Vec<Value>,
    #[serde(default)]
    pub gaps: Vec<Value>,
    #[serde(default)]
    pub best_moment: Option<Value>,
    #[serde(default)]
    pub weakest_moment: Option<Value>,
    #[serde(default)]
    pub depth: Option<Value>,
    #[serde(default)]
    pub clarity: Option<Value>,
    #[serde(default)]
    pub notes: Vec<Value>,
    #[serde(default)]
    pub cards: Vec<Value>,
}

pub fn validate_report(r: &ReportOut) -> Result<(), String> {
    if r.summary.trim().is_empty() {
        return Err("\"summary\" must not be empty".into());
    }
    Ok(())
}

pub fn build_report(iv: &Interview, out: Option<&ReportOut>, written_by: Value) -> Report {
    let done: Vec<&IQuestion> = asked(iv).map(|(_, q)| q).collect();
    let overall = if done.is_empty() {
        0
    } else {
        (done.iter().map(|q| q.score.unwrap_or(0)).sum::<u32>() as f32 / done.len() as f32).round() as u32
    };
    let (shown, total) = done.iter().flat_map(|q| &q.points).fold((0.0f32, 0usize), |(s, n), p| {
        (s + match p.status.as_str() {
            "verified" => 1.0,
            "partial" => 0.5,
            _ => 0.0,
        }, n + 1)
    });
    let coverage = if total == 0 { 0 } else { (shown / total as f32 * 100.0).round() as u32 };
    let verdict = match overall {
        80.. => "strong",
        60..=79 => "good",
        _ => "needs_work",
    };
    let mut reread: Vec<u32> = done
        .iter()
        .filter(|q| q.score.unwrap_or(0) < 80)
        .flat_map(|q| q.sources.iter().map(|s| s.page_start))
        .collect();
    reread.sort_unstable();
    reread.dedup();
    let list = |v: &[Value]| v.iter().map(text).filter(|s| !s.is_empty()).collect::<Vec<_>>();
    let rating = |v: &Option<Value>| v.as_ref().and_then(as_index).map(|n| (n as u32).clamp(1, 5));
    let missed_points = || -> Vec<String> {
        done.iter()
            .flat_map(|q| q.points.iter().filter(|p| p.status != "verified").map(|p| p.text.clone()))
            .take(6)
            .collect()
    };
    match out {
        Some(r) => Report {
            overall,
            verdict: verdict.into(),
            coverage,
            depth: rating(&r.depth),
            clarity: rating(&r.clarity),
            summary: r.summary.trim().to_string(),
            strengths: list(&r.strengths),
            gaps: {
                let g = list(&r.gaps);
                if g.is_empty() { missed_points() } else { g }
            },
            best_moment: r.best_moment.as_ref().map(text).filter(|s| !s.is_empty()),
            weakest_moment: r.weakest_moment.as_ref().map(text).filter(|s| !s.is_empty()),
            notes: r
                .notes
                .iter()
                .filter_map(|n| {
                    let q = n.get("q").and_then(as_index)?.checked_sub(1)?;
                    let note = n.get("note").map(text).filter(|s| !s.is_empty())?;
                    Some(QuestionNote { question: q, note })
                })
                .collect(),
            reread_pages: reread,
            written_by,
        },
        None => Report {
            overall,
            verdict: verdict.into(),
            coverage,
            depth: None,
            clarity: None,
            summary: format!("You showed {coverage}% of the key points across {} question(s).", done.len()),
            strengths: vec![],
            gaps: missed_points(),
            best_moment: None,
            weakest_moment: None,
            notes: vec![],
            reread_pages: reread,
            written_by,
        },
    }
}

/// Cards for points not shown: the interviewer's cards, else one plain card per question with gaps.
pub fn gap_cards(iv: &Interview, llm_cards: &[Value]) -> Vec<crate::cards::NewCard> {
    let mut out = Vec::new();
    for (i, q) in asked(iv) {
        let missed: Vec<&PointState> = q.points.iter().filter(|p| p.status != "verified").collect();
        if missed.is_empty() {
            continue;
        }
        let base = crate::cards::NewCard {
            topic_id: q.topic.clone(),
            attempt_id: Some(iv.id.clone()),
            question_id: Some(q.id.clone()),
            book_fingerprint: iv.setup.book_fingerprint.clone().filter(|_| !q.sources.is_empty()),
            page: q.sources.first().map(|s| s.page_start),
            ..Default::default()
        };
        let mine: Vec<crate::cards::NewCard> = llm_cards
            .iter()
            .filter(|c| c.get("q").and_then(as_index) == Some(i + 1))
            .filter_map(|c| {
                let front = c.get("front")?.as_str()?.trim();
                let back = c.get("back")?.as_str()?.trim();
                (front.len() >= 8 && !back.is_empty()).then(|| crate::cards::NewCard { front: front.into(), back: back.into(), ..base.clone() })
            })
            .take(2)
            .collect();
        if mine.is_empty() {
            let points = missed.iter().map(|p| format!("- {}", p.text)).collect::<Vec<_>>().join("\n");
            out.push(crate::cards::NewCard {
                front: q.prompt.clone(),
                back: format!("Key points:\n{points}\n\n{}", q.reference.trim()),
                ..base
            });
        } else {
            out.extend(mine);
        }
    }
    out
}

/// Skill-map evidence: each finished question with a topic. Junior-level questions can't prove full mastery.
pub fn evidence(iv: &Interview) -> Vec<crate::skills::Evidence> {
    if iv.status != "completed" {
        return vec![];
    }
    let cap = match iv.setup.level.as_str() {
        "junior" => 0.8,
        "mid" => 0.92,
        _ => 1.0,
    };
    asked(iv)
        .filter_map(|(_, q)| {
            Some(crate::skills::Evidence {
                topic: q.topic.clone()?,
                estimate: q.score? as f32 * cap,
                weight: 1.0,
                at: iv.updated_at.clone(),
            })
        })
        .collect()
}

// ---------- persistence ----------

pub fn save(conn: &Connection, iv: &Interview, device_id: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO interviews(id, title, persona_id, setup, questions, transcript, current_idx, status, report, topics,
                                model, created_at, updated_at, device_id, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 0)
         ON CONFLICT(id) DO UPDATE SET title = excluded.title, questions = excluded.questions,
             transcript = excluded.transcript, current_idx = excluded.current_idx, status = excluded.status,
             report = excluded.report, topics = excluded.topics, model = excluded.model,
             updated_at = excluded.updated_at, device_id = excluded.device_id",
        params![
            iv.id,
            iv.title,
            iv.persona_id,
            serde_json::to_string(&iv.setup)?,
            serde_json::to_string(&iv.questions)?,
            serde_json::to_string(&iv.transcript)?,
            iv.current as i64,
            iv.status,
            iv.report.as_ref().map(serde_json::to_string).transpose()?,
            serde_json::to_string(&iv.topics)?,
            iv.model.to_string(),
            iv.created_at,
            iv.updated_at,
            device_id
        ],
    )?;
    Ok(())
}

const COLS: &str =
    "id, title, persona_id, setup, questions, transcript, current_idx, status, report, topics, model, created_at, updated_at";

fn row_to_interview(r: &rusqlite::Row) -> rusqlite::Result<Interview> {
    let json = |i: usize| -> rusqlite::Result<String> { r.get(i) };
    let bad = |i: usize, e: serde_json::Error| rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e));
    Ok(Interview {
        id: r.get(0)?,
        title: r.get(1)?,
        persona_id: r.get(2)?,
        setup: serde_json::from_str(&json(3)?).map_err(|e| bad(3, e))?,
        questions: serde_json::from_str(&json(4)?).map_err(|e| bad(4, e))?,
        transcript: serde_json::from_str(&json(5)?).map_err(|e| bad(5, e))?,
        current: r.get::<_, i64>(6)? as usize,
        status: r.get(7)?,
        report: r.get::<_, Option<String>>(8)?.and_then(|s| serde_json::from_str(&s).ok()),
        topics: serde_json::from_str(&json(9)?).unwrap_or_default(),
        model: serde_json::from_str(&json(10)?).unwrap_or(Value::Null),
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

pub fn get(conn: &Connection, id: &str) -> AppResult<Interview> {
    conn.query_row(&format!("SELECT {COLS} FROM interviews WHERE id = ?1 AND deleted = 0"), [id], row_to_interview)
        .optional()?
        .ok_or_else(|| AppError::NotFound("Interview not found".into()))
}

pub fn all(conn: &Connection) -> AppResult<Vec<Interview>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM interviews WHERE deleted = 0 ORDER BY created_at"))?;
    let rows = stmt.query_map([], row_to_interview)?;
    // Skip rows that don't parse (e.g. written by a newer app version) rather than failing everything.
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn recent_personas(conn: &Connection, n: usize) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT persona_id FROM interviews WHERE deleted = 0 ORDER BY created_at DESC LIMIT ?1")?;
    let rows = stmt.query_map([n as i64], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn delete(conn: &Connection, id: &str, device_id: &str) -> AppResult<()> {
    conn.execute("UPDATE interviews SET deleted = 1, updated_at = ?2, device_id = ?3 WHERE id = ?1", params![id, now(), device_id])?;
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterviewSummary {
    pub id: String,
    pub title: String,
    pub persona_id: String,
    pub persona_name: String,
    pub level: String,
    pub status: String,
    pub overall: Option<u32>,
    pub verdict: Option<String>,
    pub questions: usize,
    pub asked: usize,
    pub planned: usize,
    pub created_at: String,
}

pub fn summaries(conn: &Connection) -> AppResult<Vec<InterviewSummary>> {
    let mut list: Vec<InterviewSummary> = all(conn)?
        .into_iter()
        .map(|iv| InterviewSummary {
            persona_name: persona(&iv.persona_id).name.clone(),
            overall: iv.report.as_ref().map(|r| r.overall),
            verdict: iv.report.as_ref().map(|r| r.verdict.clone()),
            questions: iv.questions.len(),
            asked: iv.questions.iter().filter(|q| q.done).count(),
            planned: question_count(iv.setup.minutes),
            level: iv.setup.level.clone(),
            id: iv.id,
            title: iv.title,
            persona_id: iv.persona_id,
            status: iv.status,
            created_at: iv.created_at,
        })
        .collect();
    list.reverse();
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn setup(level: &str, minutes: u32) -> Setup {
        Setup {
            source: "general".into(),
            book_fingerprint: None,
            chapter_idx: None,
            track: Some("fundamentals".into()),
            topic_ids: vec![],
            level: level.into(),
            minutes,
            persona_id: None,
            mode: "thorough".into(),
        }
    }

    fn planned() -> Vec<IQuestion> {
        let out: PlanOut = serde_json::from_str(
            r#"{"opening":"Hi, I'm Lena.","questions":[
                {"difficulty":"medium","kind":"concept","topic":"hashing","question":"How does a hash table find a key?","points":["hash function maps key to bucket","collisions are resolved by chaining or probing","average O(1) lookup"],"reference":"..."},
                {"difficulty":"easy","kind":"weird","topic":"nope","question":"Why resize a hash table?","points":["load factor grows","resizing keeps chains short"],"reference":"..."},
                {"difficulty":"hard","kind":"scenario","topic":"hashing","question":"An attacker sends keys that all collide. What happens and what do you do?","points":["lookups degrade to O(n)","use a randomized / keyed hash"],"reference":"..."},
                {"difficulty":"hard","kind":"code","question":"short","points":["a","b"]},
                {"difficulty":"easy","kind":"concept","question":"What is a hash collision?","points":["only one point"]},
                {"difficulty":"Easy","kind":"concept","topic":"hashing","question":"What is a load factor?","points":["entries divided by buckets","high load factor means more collisions"],"reference":"..."}
            ]}"#,
        )
        .unwrap();
        let topics = vec![("hashing".to_string(), "Hash tables".to_string())];
        normalize_plan(&out, &[], &topics, Some("trees"), 20)
    }

    fn judged(intent: &str, points: &str, action: &str, focus: Option<u32>, angle: &str) -> JudgeOut {
        serde_json::from_str(&format!(
            r#"{{"intent":"{intent}","points":{points},"action":"{action}","focus":{},"angle":"{angle}","note":"dig in"}}"#,
            focus.map(|f| f.to_string()).unwrap_or("null".into())
        ))
        .unwrap()
    }

    /// One round trip with fixed speech, as the command does.
    fn round(iv: &mut Interview, kind: &str, text: &str, out: Option<JudgeOut>) -> Decision {
        add_candidate(iv, kind, text);
        let out = quick_judgement(iv, kind, text).or(out).expect("needs a judgement");
        let d = decide(iv, &out);
        let speech = if d.needs_voice() { format!("({} line)", d.action) } else { String::new() };
        commit(iv, &d, &speech);
        d
    }

    #[test]
    fn plan_is_normalized() {
        let qs = planned();
        assert_eq!(qs.len(), 4, "too-short question and single-point question dropped");
        assert_eq!(qs[0].topic.as_deref(), Some("hashing"));
        assert_eq!((qs[1].kind.as_str(), qs[1].difficulty.as_str()), ("concept", "easy"));
        assert_eq!(qs[1].topic.as_deref(), Some("trees"), "unknown topic falls back to the default");
        assert_eq!(qs[3].difficulty, "easy");
        assert!(qs.iter().all(|q| q.order.is_none() && q.points.iter().all(|p| p.status == "untested")));
        assert_eq!(difficulty_mix("mid", 6), (2, 2, 2));
        assert_eq!(difficulty_mix("senior", 9), (2, 4, 3));
    }

    #[test]
    fn questions_adapt_to_the_candidate() {
        // mid starts between easy and medium → the easier one, with a topic not used yet.
        let iv = new_interview(setup("mid", 15), persona("lena"), "Hashing".into(), "Hello.", planned(), vec![], Value::Null);
        assert_eq!(iv.questions[iv.current].difficulty, "easy");
        assert_eq!(iv.questions[iv.current].order, Some(1));

        // A strong answer on the warm-up → a harder question next.
        let mut strong = iv.clone();
        let all = r#"[{"i":1,"status":"verified"},{"i":2,"status":"verified"}]"#;
        round(&mut strong, "answer", "Load factor grows; resizing keeps chains short.", Some(judged("answer", all, "move_on", None, "")));
        assert_eq!(strong.questions[strong.current].difficulty, "medium");
        assert!(target_difficulty(&strong) > 2.0);

        // A weak answer → stays easy.
        let mut weak = iv.clone();
        round(&mut weak, "dont_know", "", None);
        assert_eq!(weak.questions[weak.current].difficulty, "easy");
        assert_eq!(weak.questions[weak.current].order, Some(2));
    }

    #[test]
    fn intents_and_probing_rules() {
        let mut iv = new_interview(setup("mid", 15), persona("lena"), "Hashing".into(), "", planned(), vec![], Value::Null);
        let q0 = iv.current;
        let n_points = iv.questions[q0].points.len();

        // Clarifying questions, "repeat" and thinking aloud are free and mark nothing.
        let d = round(&mut iv, "answer", "Do you mean a chained table?", Some(judged("clarify", "[]", "", None, "")));
        assert_eq!(d.action, "clarify");
        let d = round(&mut iv, "answer", "Can you repeat the question?", None);
        assert_eq!(d.action, "repeat");
        assert!(iv.transcript.last().unwrap().text.starts_with("Sure. "));
        assert_eq!(iv.questions[q0].turns, 0);

        // An answer that shows point 1 → probe the next open point from an unused angle.
        let d = round(&mut iv, "answer", "Entries over buckets.", Some(judged("answer", r#"[{"i":1,"status":"verified"}]"#, "probe", Some(1), "why")));
        assert_eq!((d.action.as_str(), d.focus), ("probe", Some(1)), "point 1 is verified, so the judge's focus is redirected");
        assert_eq!(iv.questions[q0].points[1].angles, ["why"]);
        // The judge repeats an angle → the rules pick a fresh one.
        let d = round(&mut iv, "answer", "More collisions?", Some(judged("answer", r#"[{"i":1,"status":"partial"},{"i":2,"status":"partial"}]"#, "probe", Some(2), "why")));
        assert_eq!(iv.questions[q0].points[0].status, "verified", "verified points never go back");
        assert_ne!(d.angle, "why");
        // Point 2 has had two probes → closed; nothing open → move on even though the judge wants more.
        let d = round(&mut iv, "answer", "Hmm.", Some(judged("answer", "[]", "probe", Some(2), "example")));
        assert_eq!(d.action, "move_on");
        assert!(iv.questions[q0].done);
        assert_eq!(iv.questions[q0].score, Some(75), "verified + partial = 1.5 / 2");
        assert_eq!(n_points, 2);

        // "skip" moves on at once; untested points count as missing.
        let q1 = iv.current;
        let d = round(&mut iv, "answer", "Skip this one.", None);
        assert_eq!(d.action, "move_on");
        assert_eq!(iv.questions[q1].score, Some(0));

        // Hint button: costs points, nudges toward an open point.
        let q2 = iv.current;
        let d = round(&mut iv, "hint_request", "", None);
        assert_eq!(d.action, "hint");
        assert_eq!(iv.questions[q2].hints, 1);
        let all: String = format!(
            "[{}]",
            (1..=iv.questions[q2].points.len()).map(|i| format!(r#"{{"i":{i},"status":"verified"}}"#)).collect::<Vec<_>>().join(",")
        );
        round(&mut iv, "answer", "Full answer.", Some(judged("answer", &all, "move_on", None, "")));
        assert_eq!(iv.questions[q2].score, Some(100 - HINT_PENALTY));
    }

    #[test]
    fn free_turns_are_limited_and_turn_cap_ends_a_question() {
        let mut iv = new_interview(setup("junior", 15), persona("sam"), "x".into(), "", planned(), vec![], Value::Null);
        let q = iv.current;
        for _ in 0..MAX_FREE_TURNS {
            round(&mut iv, "answer", "What do you mean?", Some(judged("clarify", "[]", "", None, "")));
        }
        assert_eq!(iv.questions[q].turns, 0);
        round(&mut iv, "answer", "What do you mean?", Some(judged("clarify", "[]", "", None, "")));
        assert_eq!(iv.questions[q].turns, 1, "past the limit, clarifying counts as a turn");
        for _ in 0..turn_cap("junior") {
            if iv.current != q {
                break;
            }
            round(&mut iv, "answer", "hmm", Some(judged("answer", "[]", "challenge", Some(1), "why")));
        }
        assert_ne!(iv.current, q, "turn cap reached");
        assert!(iv.questions[q].points.iter().all(|p| p.status == "missing"));
    }

    #[test]
    fn whole_interview_report_and_cards() {
        let mut iv = new_interview(setup("mid", 15), persona("kenji"), "Hashing".into(), "", planned(), vec![], Value::Null);
        while iv.status == "in_progress" {
            let n = iv.questions[iv.current].points.len();
            let first_only = if n > 0 { r#"[{"i":1,"status":"verified"}]"# } else { "[]" };
            round(&mut iv, "answer", "Partial answer.", Some(judged("answer", first_only, "move_on", None, "")));
        }
        assert_eq!(asked_count(&iv), question_count(15));
        assert_eq!(iv.transcript.last().unwrap().kind, "closing");
        let orders: Vec<u32> = asked(&iv).map(|(_, q)| q.order.unwrap()).collect();
        assert_eq!(orders, [1, 2, 3, 4]);
        let r = build_report(&iv, None, Value::Null);
        assert!(r.overall > 0 && r.overall < 80);
        assert_eq!(gap_cards(&iv, &[]).len(), 4, "every question had gaps");
        assert!(evidence(&iv).is_empty(), "only completed interviews count as evidence");
    }

    #[test]
    fn speech_is_cleaned() {
        assert_eq!(clean_speech("Kenji: \"Are you sure?\"", "Kenji Watanabe"), "Are you sure?");
        assert_eq!(clean_speech("Watanabe: Hmm.", "Kenji Watanabe"), "Hmm.");
        let long = "Word. ".repeat(200);
        assert!(clean_speech(&long, "X").ends_with('.'));
    }

    #[test]
    fn personas_rotate_and_interviews_persist() {
        let recent: Vec<String> = vec!["priya".into(), "marcus".into(), "lena".into()];
        for seed in 0..20 {
            assert!(!recent.contains(&pick_persona(&recent, seed).id));
        }
        let conn = Connection::open_in_memory().unwrap();
        db::migrate(&conn).unwrap();
        let mut iv = new_interview(setup("senior", 15), persona("kenji"), "Hashing".into(), "", planned(), vec!["hashing".into()], Value::Null);
        save(&conn, &iv, "dev").unwrap();
        stop(&mut iv);
        iv.status = "completed".into();
        save(&conn, &iv, "dev").unwrap();
        let back = get(&conn, &iv.id).unwrap();
        assert_eq!(back.transcript.len(), 2);
        assert_eq!(back.setup.mode, "thorough");
        assert_eq!(recent_personas(&conn, 5).unwrap(), ["kenji"]);
        assert_eq!(summaries(&conn).unwrap()[0].persona_name, "Kenji Watanabe");
    }
}
