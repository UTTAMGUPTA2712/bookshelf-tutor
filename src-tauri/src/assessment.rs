//! Adaptive skill test: per topic, a medium question first, then a harder one after a right answer or
//! an easier one after a wrong answer. Topics whose prerequisites were clearly failed are skipped.
//!
//! Questions for a topic are written in one model call (easy + medium + hard) and kept in a pool in
//! the attempt's `model.pool`, so the next topic can be prepared while the learner answers.

use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::llm::Message;
use crate::quiz::{self, Attempt, GenOut, Question};
use crate::skills::{self, Topic, TrackTopics};

pub const PROMPT_VERSION: &str = "assessment.v1";
/// Below this estimated mastery a topic counts as failed and the topics that need it are skipped.
const FAILED: f32 = 30.0;

pub fn generation_messages(track_label: &str, topic: &Topic, free_text: bool) -> Vec<Message> {
    let hard_type = if free_text { "short" } else { "mcq" };
    let system = r#"You are an experienced computer-science tutor writing a placement test. Each question checks real understanding of one topic, not trivia.

Rules:
- Write exactly 3 questions about the given topic: one "easy", one "medium", one "hard".
- easy: core definitions and basic facts. medium: applying the idea to a small concrete case. hard: trade-offs, edge cases, or reasoning about consequences.
- Questions must be self-contained and have one clearly correct answer. No questions about books, authors or history.
- "mcq": exactly 4 "choices", exactly one correct, plausible distractors; "correct_choice" is the 0-based index of the correct choice.
- "short": answerable in 1-3 sentences.
- "reference_answer": a complete model answer. "key_points": 2-4 short facts a good answer must contain.
- Write in English.

Reply with a JSON object only, exactly in this shape:
{"questions":[{"difficulty":"easy","type":"mcq","question":"...","choices":["...","...","...","..."],"correct_choice":0,"reference_answer":"...","key_points":["..."]}]}"#;
    let user = format!(
        "Track: {track_label}\nTopic: {}\nCovers: {}\n\nWrite the easy question as \"mcq\", the medium question as \"mcq\", and the hard question as \"{hard_type}\".",
        topic.label, topic.description
    );
    vec![Message::system(system), Message::user(user)]
}

/// Usable questions for a topic, each tagged with topic + difficulty (guessed from order if missing).
pub fn normalize(out: &GenOut, topic_id: &str) -> Vec<Question> {
    let mut qs = quiz::normalize_generated(out, &[], 3);
    let order = ["easy", "medium", "hard"];
    for (i, q) in qs.iter_mut().enumerate() {
        q.topic = Some(topic_id.to_string());
        if q.difficulty.is_none() {
            q.difficulty = Some(order[i.min(2)].to_string());
        }
    }
    qs
}

pub fn validate(out: &GenOut, topic_id: &str) -> Result<(), String> {
    let qs = normalize(out, topic_id);
    if qs.len() < 2 || !qs.iter().any(|q| q.qtype == "mcq") {
        return Err("need 3 usable questions (\"difficulty\", \"type\", \"question\"; mcq needs 4 \"choices\" and \"correct_choice\")".into());
    }
    Ok(())
}

// ---------- adaptive flow ----------

#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    Ask { topic: String, difficulty: &'static str },
    Done,
}

fn asked<'a>(a: &'a Attempt, topic: &str) -> Vec<&'a Question> {
    a.questions.iter().filter(|q| q.topic.as_deref() == Some(topic)).collect()
}

/// True once a topic's answers are all graded and clearly wrong.
fn failed(a: &Attempt, topic: &str) -> bool {
    let qs = asked(a, topic);
    if qs.is_empty() || qs.iter().any(|q| q.grade.is_none()) {
        return false;
    }
    let est: f32 = qs
        .iter()
        .map(|q| skills::question_estimate(q.difficulty.as_deref(), q.grade.as_ref().map_or(0, |g| g.score)))
        .sum::<f32>()
        / qs.len() as f32;
    est < FAILED
}

/// A prerequisite was failed, or was itself skipped for that reason.
fn blocked(track: &TrackTopics, a: &Attempt, t: &Topic) -> bool {
    t.prereqs.iter().any(|p| {
        failed(a, p)
            || track
                .topics
                .iter()
                .find(|x| x.id == *p)
                .is_some_and(|x| asked(a, &x.id).is_empty() && blocked(track, a, x))
    })
}

/// What to ask next, given the answers so far.
pub fn next(track: &TrackTopics, a: &Attempt) -> Next {
    for t in &track.topics {
        let qs = asked(a, &t.id);
        match qs.len() {
            0 if blocked(track, a, t) => continue,
            0 => return Next::Ask { topic: t.id.clone(), difficulty: "medium" },
            1 => {
                // Free-text answers may still be grading; assume "half right" rather than wait.
                let score = qs[0].grade.as_ref().map_or(50, |g| g.score);
                return Next::Ask { topic: t.id.clone(), difficulty: if score >= 70 { "hard" } else { "easy" } };
            }
            _ => continue,
        }
    }
    Next::Done
}

/// Topics skipped because a prerequisite was failed.
pub fn skipped(track: &TrackTopics, a: &Attempt) -> Vec<String> {
    track.topics.iter().filter(|t| asked(a, &t.id).is_empty() && blocked(track, a, t)).map(|t| t.id.clone()).collect()
}

/// (topics started or skipped, total topics) for the progress bar.
pub fn progress(track: &TrackTopics, a: &Attempt) -> (usize, usize) {
    let started = track.topics.iter().filter(|t| !asked(a, &t.id).is_empty() || blocked(track, a, t)).count();
    (started, track.topics.len())
}

/// The next topic whose questions aren't written yet (to prepare in the background).
pub fn prefetch_topic(track: &TrackTopics, a: &Attempt) -> Option<String> {
    track
        .topics
        .iter()
        .find(|t| asked(a, &t.id).is_empty() && !blocked(track, a, t) && !has_pool(a, &t.id))
        .map(|t| t.id.clone())
}

// ---------- question pool (in `model.pool`) ----------

pub fn has_pool(a: &Attempt, topic: &str) -> bool {
    a.model.get("pool").and_then(|p| p.get(topic)).is_some()
}

pub fn put_pool(a: &mut Attempt, topic: &str, qs: &[Question]) -> AppResult<()> {
    let model = a.model.as_object_mut().ok_or_else(|| AppError::Invalid("Broken skill test record".into()))?;
    let pool = model.entry("pool").or_insert_with(|| json!({}));
    pool[topic] = serde_json::to_value(qs)?;
    Ok(())
}

/// Takes the question of the wanted difficulty (or the closest one) out of the topic's pool.
pub fn take(a: &mut Attempt, topic: &str, difficulty: &str) -> AppResult<Option<Question>> {
    let Some(list) = a.model.get_mut("pool").and_then(|p| p.get_mut(topic)) else { return Ok(None) };
    let mut qs: Vec<Question> = serde_json::from_value(list.take()).unwrap_or_default();
    let preference: &[&str] = match difficulty {
        "easy" => &["easy", "medium", "hard"],
        "hard" => &["hard", "medium", "easy"],
        _ => &["medium", "easy", "hard"],
    };
    let pos = preference
        .iter()
        .find_map(|d| qs.iter().position(|q| q.difficulty.as_deref() == Some(*d)));
    let picked = pos.map(|i| qs.remove(i));
    *list = serde_json::to_value(&qs)?;
    Ok(picked)
}

pub fn new_attempt(track: &TrackTopics, free_text: bool) -> Attempt {
    let ts = crate::db::now();
    Attempt {
        id: uuid::Uuid::now_v7().to_string(),
        kind: "assessment".into(),
        book_fingerprint: None,
        chapter_id: None,
        chapter_title: Some(format!("Skill test: {}", track.label)),
        status: "in_progress".into(),
        model: json!({ "track": track.id, "freeText": free_text, "promptVersion": PROMPT_VERSION, "pool": {} }),
        questions: vec![],
        overall: None,
        topics: track.topics.iter().map(|t| t.id.clone()).collect(),
        created_at: ts.clone(),
        updated_at: ts,
    }
}

pub fn free_text(a: &Attempt) -> bool {
    a.model.get("freeText").and_then(Value::as_bool).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(topic: &str, difficulty: &str) -> Question {
        Question {
            id: uuid::Uuid::now_v7().to_string(),
            qtype: "mcq".into(),
            prompt: format!("{topic} {difficulty}"),
            choices: Some(vec!["a".into(), "b".into(), "c".into(), "d".into()]),
            correct_choice: Some(0),
            reference_answer: "a".into(),
            key_points: vec![],
            sources: vec![],
            answer: None,
            grade: None,
            topic: Some(topic.into()),
            difficulty: Some(difficulty.into()),
        }
    }

    /// Simulates a learner: asks `next`, answers right or wrong, repeats.
    fn run(track: &TrackTopics, right: impl Fn(&str, &str) -> bool) -> Attempt {
        let mut a = new_attempt(track, false);
        for _ in 0..200 {
            match next(track, &a) {
                Next::Done => return a,
                Next::Ask { topic, difficulty } => {
                    if !has_pool(&a, &topic) {
                        put_pool(&mut a, &topic, &[q(&topic, "easy"), q(&topic, "medium"), q(&topic, "hard")]).unwrap();
                    }
                    let mut question = take(&mut a, &topic, difficulty).unwrap().unwrap();
                    assert_eq!(question.difficulty.as_deref(), Some(difficulty));
                    let ok = right(&topic, difficulty);
                    question.answer = Some(if ok { "0" } else { "1" }.into());
                    question.grade = quiz::local_grade(&question, question.answer.as_deref().unwrap());
                    a.questions.push(question);
                }
            }
        }
        panic!("the test never ended");
    }

    #[test]
    fn adapts_and_skips_failed_prerequisites() {
        let track = skills::track("fundamentals").unwrap();
        // Knows everything except recursion (and fails it completely).
        let a = run(track, |topic, _| topic != "recursion");
        let rec = asked(&a, "recursion");
        assert_eq!(rec.iter().map(|q| q.difficulty.as_deref().unwrap()).collect::<Vec<_>>(), ["medium", "easy"]);
        let hashing = asked(&a, "hashing");
        assert_eq!(hashing.iter().map(|q| q.difficulty.as_deref().unwrap()).collect::<Vec<_>>(), ["medium", "hard"]);
        let skipped = skipped(track, &a);
        for t in ["sorting", "trees", "dynamic-programming"] {
            assert!(skipped.contains(&t.to_string()), "{t} needs recursion");
        }
        assert!(skipped.contains(&"graphs".to_string()), "graphs needs trees, which was skipped");
        assert_eq!(progress(track, &a), (track.topics.len(), track.topics.len()));
        assert!(a.questions.len() <= track.topics.len() * 2);

        let m = skills::mastery_map(&[Attempt { status: "completed".into(), ..a }]);
        assert!(m["hashing"].mastery >= 80);
        assert!(m["recursion"].mastery < 30);
        assert!(!m.contains_key("trees"));
    }

    #[test]
    fn pool_falls_back_to_nearest_difficulty() {
        let track = skills::track("web-node").unwrap();
        let mut a = new_attempt(track, true);
        put_pool(&mut a, "js-core", &[q("js-core", "easy"), q("js-core", "hard")]).unwrap();
        assert_eq!(take(&mut a, "js-core", "medium").unwrap().unwrap().difficulty.as_deref(), Some("easy"));
        assert_eq!(take(&mut a, "js-core", "easy").unwrap().unwrap().difficulty.as_deref(), Some("hard"));
        assert!(take(&mut a, "js-core", "easy").unwrap().is_none());
        assert_eq!(prefetch_topic(track, &a).as_deref(), Some("event-loop"));
        assert!(free_text(&a));
    }

    #[test]
    fn normalizes_generated_questions() {
        let out: GenOut = serde_json::from_str(
            r#"{"questions":[
                {"difficulty":"Easy","type":"mcq","question":"What is a hash function?","choices":["a","b","c","d"],"correct_choice":1},
                {"type":"mcq","question":"Which load factor triggers resizing?","choices":["a","b","c","d"],"correct_choice":"C"},
                {"difficulty":"hard","type":"short","question":"Why does linear probing cluster?","reference_answer":"..."}
            ]}"#,
        )
        .unwrap();
        let qs = normalize(&out, "hashing");
        assert_eq!(qs.len(), 3);
        assert_eq!(qs[0].difficulty.as_deref(), Some("easy"));
        assert_eq!(qs[1].difficulty.as_deref(), Some("medium"), "guessed from order");
        assert_eq!(qs[2].qtype, "short");
        assert!(qs.iter().all(|q| q.topic.as_deref() == Some("hashing")));
        assert!(validate(&out, "hashing").is_ok());
    }
}
