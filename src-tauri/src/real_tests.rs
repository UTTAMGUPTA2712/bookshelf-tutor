//! Slow checks against real data. Run with:
//!   BOOKS_DIR=../../books cargo test real_ -- --ignored --nocapture
//!   OLLAMA_MODEL=qwen2.5:7b BOOKS_DIR=../../books cargo test real_quiz -- --ignored --nocapture

use std::path::PathBuf;
use std::time::Instant;

use crate::{db, index, library, llm, quiz};

fn books_dir() -> PathBuf {
    PathBuf::from(std::env::var("BOOKS_DIR").expect("set BOOKS_DIR"))
}

#[test]
#[ignore]
fn real_scan_and_index_library() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    db::migrate(&conn).unwrap();
    let t = Instant::now();
    library::scan(&conn, &books_dir(), "test").unwrap();
    let books = library::list(&conn).unwrap();
    println!("scanned {} books in {:?}", books.len(), t.elapsed());
    let t = Instant::now();
    library::scan(&conn, &books_dir(), "test").unwrap();
    println!("rescan (cached) in {:?}", t.elapsed());

    let dir = tempfile::tempdir().unwrap();
    let mut idx = index::open(&dir.path().join("i.sqlite")).unwrap();
    let mut failures = vec![];
    for b in &books {
        let path = library::path_of(&conn, &b.fingerprint).unwrap();
        let t = Instant::now();
        match index::prepare(&path, |_, _| {}).and_then(|p| index::store(&mut idx, &b.fingerprint, &p)) {
            Ok(st) => {
                let ch = index::chapters(&idx, &b.fingerprint).unwrap();
                println!(
                    "{:>5} pages {:>4} text {:>3} chapters {:>6.1}s  {} | first: {:?}",
                    st.page_count,
                    st.text_pages,
                    st.chapters,
                    t.elapsed().as_secs_f32(),
                    b.title,
                    ch.iter().skip(1).take(3).map(|c| c.title.as_str()).collect::<Vec<_>>()
                );
            }
            Err(e) => failures.push(format!("{}: {e}", b.title)),
        }
    }
    println!("failures: {failures:#?}");
}

#[tokio::test]
#[ignore]
async fn real_quiz_with_ollama() {
    let model = std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| "qwen2.5-coder:1.5b".into());
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    db::migrate(&conn).unwrap();
    let book = books_dir().join("Head-First-Design-Patterns.pdf");
    let added = library::add(&conn, &books_dir(), &[book.clone()], library::AddMode::Link, "test").unwrap();
    let fp = added.added.first().cloned().unwrap_or_else(|| library::fingerprint(&book).unwrap());
    let dir = tempfile::tempdir().unwrap();
    let mut idx = index::open(&dir.path().join("i.sqlite")).unwrap();
    let p = index::prepare(&book, |_, _| {}).unwrap();
    index::store(&mut idx, &fp, &p).unwrap();
    let chapters = index::chapters(&idx, &fp).unwrap();
    let ch = chapters.iter().find(|c| c.title.to_lowercase().contains("observer")).unwrap_or(&chapters[3]);
    println!("chapter: {} (pages {}-{})", ch.title, ch.start_page, ch.end_page);
    let chunks = index::chapter_chunks(&idx, &fp, ch.idx).unwrap();

    let provider = llm::ProviderConfig {
        id: "ollama".into(),
        provider_type: "ollama".into(),
        label: "Ollama".into(),
        base_url: "http://localhost:11434".into(),
        enabled_models: vec![],
        options: Default::default(),
        adapter: "ollama".into(),
        needs_key: false,
        has_key: false,
    };
    let http = llm::http_client();
    let models = llm::list_models(&http, &provider, None).await.unwrap();
    println!("ollama models: {:?}", models.iter().map(|m| &m.id).collect::<Vec<_>>());
    let t = llm::Target { provider, key: None, model };

    let sources = quiz::label_sources(&index::select_context(&chunks, 18_000));
    let opts = quiz::QuizOptions { count: 3, difficulty: "medium".into(), types: vec![] };
    let msgs = quiz::generation_messages("Head First Design Patterns", &ch.title, &opts, &sources);
    let start = Instant::now();
    let validate = |g: &quiz::GenOut| {
        if quiz::normalize_generated(g, &sources, 3).is_empty() { Err("no usable questions".to_string()) } else { Ok(()) }
    };
    let reply = llm::chat_json::<quiz::GenOut>(&http, &t, msgs, &validate, 1800, &mut |tok| eprint!("{tok}")).await;
    println!("generation took {:?}", start.elapsed());
    let reply = reply.unwrap();
    let qs = quiz::normalize_generated(&reply.value, &sources, 3);
    for q in &qs {
        println!("\n[{}] {}\n  choices: {:?} correct: {:?}\n  sources: {:?}", q.qtype, q.prompt, q.choices, q.correct_choice,
            q.sources.iter().map(|s| (&s.label, s.page_start)).collect::<Vec<_>>());
    }
    let q = qs.iter().find(|q| q.qtype != "mcq").unwrap_or(&qs[0]);
    let start = Instant::now();
    let answer = "It lets objects get notified automatically when another object's state changes, without tight coupling.";
    let g = llm::chat_json::<quiz::GradeOut>(&http, &t, quiz::grade_messages(q, answer), &quiz::validate_grade, 1200, &mut |tok| eprint!("{tok}"))
        .await
        .unwrap();
    let grade = quiz::to_grade(q, &g.value, serde_json::Value::Null);
    println!("\ngrading took {:?}\n{grade:#?}", start.elapsed());
}

#[test]
#[ignore]
fn real_catalog_for_this_machine() {
    let sys = crate::models::system_info();
    println!("{sys:#?}");
    for m in crate::models::catalog_for(&sys, &Default::default()).models {
        println!(
            "{:<16} {:>5.1} GB need {:>5.1}  {:<8} q{}  ~{:>5.1} tok/s  ~{:>4}s/grade {}",
            m.name, m.size_gb, m.memory_needed_gb, m.fit, m.quality, m.est_tokens_per_sec, m.est_grade_seconds,
            if m.recommended { "★ recommended" } else { "" }
        );
    }
}

/// Prints which chapters each topic maps to, to tune taxonomy keywords and thresholds.
///   BOOKS_DIR=../../books cargo test --release real_topic_mapping -- --ignored --nocapture
#[test]
#[ignore]
fn real_topic_mapping() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    db::migrate(&conn).unwrap();
    library::scan(&conn, &books_dir(), "test").unwrap();
    let books = library::list(&conn).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut idx = index::open(&dir.path().join("i.sqlite")).unwrap();
    let only: Vec<String> = std::env::var("TRACKS").map(|t| t.split(',').map(String::from).collect()).unwrap_or_default();
    for track in crate::skills::tracks().iter().filter(|t| only.is_empty() || only.contains(&t.id)) {
        let candidates = crate::skills::track_books(track, &books);
        for b in &candidates {
            if !index::status(&idx, &b.fingerprint).unwrap().indexed {
                let p = index::prepare(&library::path_of(&conn, &b.fingerprint).unwrap(), |_, _| {}).unwrap();
                index::store(&mut idx, &b.fingerprint, &p).unwrap();
            }
        }
        println!("\n===== {} ({} books)", track.label, candidates.len());
        let (chapters, _) = crate::skills::chapter_index(&idx, track, &books).unwrap();
        for t in &track.topics {
            println!("\n## {}", t.label);
            for b in crate::skills::candidate_books(t, &books) {
                if let Some(ms) = chapters.get(&(t.id.clone(), b.fingerprint.clone())) {
                    if ms.is_empty() {
                        continue;
                    }
                    let list: Vec<String> = ms
                        .iter()
                        .take(5)
                        .map(|m| format!("{}{:.1} {}", if m.title_match { "*" } else { "" }, m.score, m.chapter.title))
                        .collect();
                    println!("  [{}] {} ({}): {}", b.status, b.title, ms.len(), list.join(" | "));
                }
            }
        }
    }
}
