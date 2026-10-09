//! Thin `#[tauri::command]` layer: locking, threading and cancellation around the core modules.
//! Database locks are never held across an `.await`.

use std::path::PathBuf;
use std::time::Instant;

use serde::Serialize;
use serde_json::json;
use tauri::ipc::{Channel, Response};
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::index::{self, IndexStatus, SearchHit};
use crate::library::{self, AddMode, AddResult, BookPatch, BookProgress, BookView, ProgressPatch, Track};
use crate::llm::{self, ModelInfo, Preset, ProviderConfig, ProviderInput, RoleLink, Target};
use crate::pdf::{self, Chapter};
use crate::models::{self, CatalogView, LookupResult, PullProgress, SystemInfo};
use crate::quiz::{self, Attempt, AttemptSummary, QuizOptions};
use crate::transfer::{self, ImportMode, ImportPreview, TableChanges};
use crate::skills::{self, Roadmap, SkillMap};
use crate::{assessment, cards, db, interview, lesson, stats, AppState};

// ---------- app & library ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    device_id: String,
    library_root: Option<String>,
    data_dir: String,
    version: String,
    pdfium_available: bool,
    tracks: Vec<Track>,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppResult<AppInfo> {
    let db = state.db();
    Ok(AppInfo {
        device_id: state.device_id.clone(),
        library_root: db::get_setting(&db, "library_root")?,
        data_dir: state.data_dir.to_string_lossy().into(),
        version: env!("CARGO_PKG_VERSION").into(),
        pdfium_available: pdf::is_available(),
        tracks: library::tracks(),
    })
}

fn library_root(state: &AppState) -> AppResult<Option<PathBuf>> {
    Ok(db::get_setting(&state.db(), "library_root")?.map(PathBuf::from))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::Invalid(format!("Background task failed: {e}")))?
}

#[tauri::command]
pub async fn set_library_root(state: State<'_, AppState>, path: String) -> AppResult<Vec<BookView>> {
    let s = state.inner().clone();
    blocking(move || {
        let root = PathBuf::from(&path);
        if !root.is_dir() {
            return Err(AppError::NotFound(format!("Folder not found: {path}")));
        }
        let db = s.db();
        db::set_setting(&db, "library_root", &path)?;
        library::scan(&db, &root, &s.device_id)?;
        library::list(&db)
    })
    .await
}

#[tauri::command]
pub async fn scan_library(state: State<'_, AppState>) -> AppResult<Vec<BookView>> {
    let s = state.inner().clone();
    blocking(move || {
        let root = library_root(&s)?.ok_or_else(|| AppError::Invalid("Choose a library folder first".into()))?;
        let db = s.db();
        library::scan(&db, &root, &s.device_id)?;
        library::list(&db)
    })
    .await
}

#[tauri::command]
pub fn list_books(state: State<'_, AppState>) -> AppResult<Vec<BookView>> {
    library::list(&state.db())
}

#[tauri::command]
pub async fn add_books(state: State<'_, AppState>, paths: Vec<String>, mode: AddMode) -> AppResult<AddResult> {
    let s = state.inner().clone();
    blocking(move || {
        let root = library_root(&s)?.ok_or_else(|| AppError::Invalid("Choose a library folder first".into()))?;
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        library::add(&s.db(), &root, &paths, mode, &s.device_id)
    })
    .await
}

#[tauri::command]
pub fn update_book(state: State<'_, AppState>, fingerprint: String, patch: BookPatch) -> AppResult<BookView> {
    let db = state.db();
    library::update(&db, &fingerprint, &patch, &state.device_id)?;
    library::get(&db, &fingerprint)
}

#[tauri::command]
pub fn remove_book(state: State<'_, AppState>, fingerprint: String, delete_file: bool, delete_progress: bool) -> AppResult<()> {
    let root = library_root(&state)?;
    library::remove(&state.db(), root.as_deref(), &fingerprint, delete_file, delete_progress, &state.device_id)
}

#[tauri::command]
pub async fn read_book_file(state: State<'_, AppState>, fingerprint: String) -> AppResult<Response> {
    let path = library::path_of(&state.db(), &fingerprint)?;
    let bytes = tokio::fs::read(&path).await?;
    Ok(Response::new(bytes))
}

#[tauri::command]
pub fn save_progress(state: State<'_, AppState>, fingerprint: String, patch: ProgressPatch) -> AppResult<BookProgress> {
    library::save_progress(&state.db(), &fingerprint, &patch, &state.device_id)
}

// ---------- index ----------

#[tauri::command]
pub fn index_status(state: State<'_, AppState>, fingerprint: String) -> AppResult<IndexStatus> {
    index::status(&state.index(), &fingerprint)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexProgress {
    page: u32,
    total: u32,
}

#[tauri::command]
pub async fn index_book(
    state: State<'_, AppState>,
    fingerprint: String,
    force: bool,
    on_progress: Channel<IndexProgress>,
) -> AppResult<IndexStatus> {
    if !force {
        let st = index::status(&state.index(), &fingerprint)?;
        if st.indexed {
            return Ok(st);
        }
    }
    let s = state.inner().clone();
    blocking(move || {
        index_now(&s, &fingerprint, |page, total| {
            let _ = on_progress.send(IndexProgress { page, total });
        })
    })
    .await
}

/// Extracts and stores one book's index (blocking; call from a blocking thread).
fn index_now(s: &AppState, fingerprint: &str, on_page: impl FnMut(u32, u32)) -> AppResult<IndexStatus> {
    let path = library::path_of(&s.db(), fingerprint)?;
    let prepared = index::prepare(&path, on_page)?;
    let status = index::store(&mut s.index(), fingerprint, &prepared)?;
    // Remember the page count for the library view.
    s.db().execute(
        "UPDATE books SET page_count = ?2 WHERE fingerprint = ?1 AND page_count IS NULL",
        rusqlite::params![fingerprint, status.page_count],
    )?;
    Ok(status)
}

#[tauri::command]
pub fn book_chapters(state: State<'_, AppState>, fingerprint: String) -> AppResult<Vec<Chapter>> {
    index::chapters(&state.index(), &fingerprint)
}

#[tauri::command]
pub fn search_book(
    state: State<'_, AppState>,
    fingerprint: String,
    query: String,
    chapter_idx: Option<u32>,
) -> AppResult<Vec<SearchHit>> {
    index::search(&state.index(), &fingerprint, &query, chapter_idx, 30)
}

// ---------- providers & roles ----------

#[tauri::command]
pub fn provider_presets() -> Vec<Preset> {
    llm::presets().to_vec()
}

#[tauri::command]
pub fn providers_list(state: State<'_, AppState>) -> AppResult<Vec<ProviderConfig>> {
    llm::list_providers(&state.db())
}

#[tauri::command]
pub fn provider_save(state: State<'_, AppState>, input: ProviderInput) -> AppResult<ProviderConfig> {
    let db = state.db();
    let id = llm::save_provider(&db, &input, &state.device_id)?;
    llm::list_providers(&db)?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| AppError::NotFound("Provider not found".into()))
}

#[tauri::command]
pub fn provider_delete(state: State<'_, AppState>, id: String) -> AppResult<()> {
    llm::delete_provider(&state.db(), &id, &state.device_id)
}

fn target_for(state: &AppState, provider_id: &str, model: &str) -> AppResult<Target> {
    let provider = llm::get_provider(&state.db(), provider_id)?;
    let key = crate::secrets::get_key(&provider.id)?;
    if provider.needs_key && key.is_none() {
        return Err(AppError::Auth(format!("{} needs an API key on this computer", provider.label)));
    }
    Ok(Target { provider, key, model: model.into() })
}

#[tauri::command]
pub async fn provider_models(state: State<'_, AppState>, id: String) -> AppResult<Vec<ModelInfo>> {
    let provider = llm::get_provider(&state.db(), &id)?;
    let key = crate::secrets::get_key(&id)?;
    if provider.needs_key && key.is_none() {
        return Err(AppError::Auth("Paste an API key first".into()));
    }
    llm::list_models(&state.http, &provider, key.as_deref()).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    latency_ms: u128,
    json_ok: bool,
    reply: String,
    usage: llm::Usage,
}

/// "Try this model": a tiny JSON task that shows speed and whether structured output works.
#[tauri::command]
pub async fn provider_test(state: State<'_, AppState>, provider_id: String, model: String) -> AppResult<TestResult> {
    let t = target_for(&state, &provider_id, &model)?;
    let messages = vec![
        llm::Message::system("You are a terse assistant. Reply with a JSON object only."),
        llm::Message::user(
            "In one sentence, what is a hash table? Reply exactly as {\"answer\":\"...\",\"confidence\":0.0}",
        ),
    ];
    let start = Instant::now();
    let (text, usage) = llm::chat_stream(&state.http, &t, &messages, true, 300, &mut |_| {}).await?;
    let json_ok = llm::extract_json(&text)
        .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
        .is_some_and(|v| v.get("answer").is_some());
    let latency_ms = start.elapsed().as_millis();
    // Remembered on this machine for the interview's model check.
    let saved = json!({ "jsonOk": json_ok, "latencyMs": latency_ms as u64, "at": db::now() });
    let _ = db::set_setting(&state.db(), &format!("model_test:{provider_id}:{model}"), &saved.to_string());
    Ok(TestResult { latency_ms, json_ok, reply: text, usage })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleView {
    role: String,
    label: String,
    chain: Vec<RoleLink>,
}

#[tauri::command]
pub fn roles_get(state: State<'_, AppState>) -> AppResult<Vec<RoleView>> {
    let mut roles = llm::get_roles(&state.db())?;
    Ok(llm::ROLES
        .iter()
        .map(|(r, l)| RoleView { role: r.to_string(), label: l.to_string(), chain: roles.remove(*r).unwrap_or_default() })
        .collect())
}

#[tauri::command]
pub fn role_set(state: State<'_, AppState>, role: String, chain: Vec<RoleLink>) -> AppResult<()> {
    llm::set_role(&state.db(), &role, &chain, &state.device_id)
}

// ---------- local models (Ollama) ----------

#[tauri::command]
pub fn system_info() -> SystemInfo {
    models::system_info()
}

fn ollama_provider(state: &AppState, provider_id: &str) -> AppResult<ProviderConfig> {
    let p = llm::get_provider(&state.db(), provider_id)?;
    if p.adapter != "ollama" {
        return Err(AppError::Invalid("Model downloads are only available for Ollama".into()));
    }
    Ok(p)
}

#[tauri::command]
pub async fn ollama_catalog(state: State<'_, AppState>, provider_id: String) -> AppResult<CatalogView> {
    let p = ollama_provider(&state, &provider_id)?;
    let installed = llm::list_models(&state.http, &p, None).await?.into_iter().map(|m| m.id).collect();
    Ok(models::catalog_for(&models::system_info(), &installed))
}

#[tauri::command]
pub async fn ollama_lookup(state: State<'_, AppState>, name: String) -> AppResult<LookupResult> {
    models::lookup(&state.http, &name, &models::system_info()).await
}

/// Downloads a model and adds it to the provider's selected models.
#[tauri::command]
pub async fn ollama_pull(
    state: State<'_, AppState>,
    request_id: String,
    provider_id: String,
    name: String,
    on_progress: Channel<PullProgress>,
) -> AppResult<ProviderConfig> {
    let p = ollama_provider(&state, &provider_id)?;
    let s = state.inner().clone();
    cancellable(&s.clone(), request_id, async move {
        models::pull(&s.http, &p.base_url, &name, &mut |pr| {
            let _ = on_progress.send(pr);
        })
        .await?;
        let mut enabled = p.enabled_models.clone();
        if !enabled.contains(&name) {
            enabled.push(name.clone());
        }
        let input = ProviderInput {
            id: Some(p.id.clone()),
            provider_type: p.provider_type.clone(),
            label: p.label.clone(),
            base_url: p.base_url.clone(),
            enabled_models: enabled,
            options: p.options.clone(),
            api_key: None,
        };
        let db = s.db();
        llm::save_provider(&db, &input, &s.device_id)?;
        llm::get_provider(&db, &p.id)
    })
    .await
}

#[tauri::command]
pub async fn ollama_delete(state: State<'_, AppState>, provider_id: String, name: String) -> AppResult<()> {
    let p = ollama_provider(&state, &provider_id)?;
    models::delete(&state.http, &p.base_url, &name).await?;
    let input = ProviderInput {
        id: Some(p.id.clone()),
        provider_type: p.provider_type.clone(),
        label: p.label.clone(),
        base_url: p.base_url.clone(),
        enabled_models: p.enabled_models.iter().filter(|m| **m != name).cloned().collect(),
        options: p.options.clone(),
        api_key: None,
    };
    llm::save_provider(&state.db(), &input, &state.device_id)?;
    Ok(())
}

// ---------- quizzes ----------

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StreamEvent {
    Token { text: String },
    Status { message: String },
    /// Plain-text words of the interviewer, as they are written.
    Speech { text: String },
}

/// Runs an AI task as its own tokio task so `ai_cancel` can abort it mid-stream.
async fn cancellable<T: Send + 'static>(
    state: &AppState,
    request_id: String,
    fut: impl std::future::Future<Output = AppResult<T>> + Send + 'static,
) -> AppResult<T> {
    let handle = tokio::spawn(fut);
    state.tasks.lock().unwrap().insert(request_id.clone(), handle.abort_handle());
    let result = handle.await;
    state.tasks.lock().unwrap().remove(&request_id);
    match result {
        Ok(r) => r,
        Err(e) if e.is_cancelled() => Err(AppError::Cancelled),
        Err(e) => Err(AppError::Invalid(format!("AI task failed: {e}"))),
    }
}

#[tauri::command]
pub fn ai_cancel(state: State<'_, AppState>, request_id: String) {
    if let Some(h) = state.tasks.lock().unwrap().remove(&request_id) {
        h.abort();
    }
}

fn context_budget(t: &Target) -> usize {
    if t.provider.adapter == "ollama" {
        let ctx = t.provider.options.num_ctx.unwrap_or(8192) as usize;
        ctx.saturating_sub(2500).max(1500) * 3
    } else {
        40_000
    }
}

fn token_sink(ch: Channel<StreamEvent>) -> impl FnMut(&str) + Send {
    move |t: &str| {
        let _ = ch.send(StreamEvent::Token { text: t.to_string() });
    }
}

#[tauri::command]
pub async fn quiz_generate(
    state: State<'_, AppState>,
    request_id: String,
    fingerprint: String,
    chapter_idx: u32,
    options: QuizOptions,
    on_event: Channel<StreamEvent>,
) -> AppResult<Attempt> {
    let s = state.inner().clone();
    let (book, targets) = {
        let db = s.db();
        (library::get(&db, &fingerprint)?, llm::resolve_role(&db, "question")?)
    };
    let (chapter, chunks, topics) = {
        let idx = s.index();
        let chapter = index::chapters(&idx, &fingerprint)?
            .into_iter()
            .find(|c| c.idx == chapter_idx)
            .ok_or_else(|| AppError::NotFound("Chapter not found — open the book to index it first".into()))?;
        let chunks = index::chapter_chunks(&idx, &fingerprint, chapter_idx)?;
        // Which skill-map topics this quiz is evidence for.
        let topics = skills::chapter_topics(&idx, &book, chapter_idx).unwrap_or_else(|e| {
            log::warn!("topic mapping failed: {e}");
            vec![]
        });
        (chapter, chunks, topics)
    };
    if chunks.is_empty() {
        return Err(AppError::Invalid("This chapter has no extractable text (it may be scanned images).".into()));
    }
    let count = options.count.clamp(1, 20) as usize;
    let options = QuizOptions { count: count as u32, ..options };

    cancellable(&s.clone(), request_id, async move {
        let _ = on_event.send(StreamEvent::Status { message: format!("Reading “{}”…", chapter.title) });
        let ((questions, raw, usage), target) = llm::with_fallback(&targets, |t| {
            let sources = quiz::label_sources(&index::select_context(&chunks, context_budget(&t)));
            let messages = quiz::generation_messages(&book.title, &chapter.title, &options, &sources);
            let http = s.http.clone();
            let mut sink = token_sink(on_event.clone());
            let ev = on_event.clone();
            async move {
                let _ = ev.send(StreamEvent::Status { message: format!("Writing questions with {}…", t.model) });
                let validate = |g: &quiz::GenOut| {
                    if quiz::normalize_generated(g, &sources, count).is_empty() {
                        Err("no usable questions — each needs \"type\", \"question\", and \"sources\" citing the S-labels (mcq also needs 4 \"choices\" and \"correct_choice\")".to_string())
                    } else {
                        Ok(())
                    }
                };
                let max_tokens = 400 + 450 * count as u32;
                let reply = llm::chat_json::<quiz::GenOut>(&http, &t, messages, &validate, max_tokens, &mut sink).await?;
                Ok((quiz::normalize_generated(&reply.value, &sources, count), reply.raw, reply.usage))
            }
        })
        .await?;
        log::info!("generated {} questions ({} chars raw)", questions.len(), raw.len());
        let ts = db::now();
        let attempt = Attempt {
            id: uuid::Uuid::now_v7().to_string(),
            kind: "chapter".into(),
            book_fingerprint: Some(fingerprint),
            chapter_id: Some(chapter.id.clone()),
            chapter_title: Some(chapter.title.clone()),
            status: "in_progress".into(),
            model: json!({ "question": target.describe(), "promptVersion": quiz::PROMPT_VERSION, "usage": usage }),
            questions,
            overall: None,
            topics,
            created_at: ts.clone(),
            updated_at: ts,
        };
        quiz::save(&s.db(), &attempt, &s.device_id)?;
        Ok(attempt)
    })
    .await
}

/// Load → modify one question → save, atomically under the DB lock (grading runs concurrently).
fn update_question(s: &AppState, attempt_id: &str, question_id: &str, f: impl FnOnce(&mut quiz::Question)) -> AppResult<Attempt> {
    let db = s.db();
    let mut a = quiz::get(&db, attempt_id)?;
    let q = a
        .questions
        .iter_mut()
        .find(|q| q.id == question_id)
        .ok_or_else(|| AppError::NotFound("Question not found".into()))?;
    f(q);
    a.updated_at = db::now();
    quiz::save(&db, &a, &s.device_id)?;
    Ok(a)
}

/// Records an answer without grading it (grading follows in the background).
#[tauri::command]
pub fn quiz_save_answer(state: State<'_, AppState>, attempt_id: String, question_id: String, answer: String) -> AppResult<Attempt> {
    update_question(&state, &attempt_id, &question_id, |q| {
        q.answer = Some(answer);
        q.grade = None;
    })
}

#[tauri::command]
pub async fn quiz_grade(
    state: State<'_, AppState>,
    request_id: String,
    attempt_id: String,
    question_id: String,
    answer: String,
    on_event: Channel<StreamEvent>,
) -> AppResult<Attempt> {
    let s = state.inner().clone();
    let a = update_question(&s, &attempt_id, &question_id, |q| {
        q.answer = Some(answer.clone());
        q.grade = None;
    })?;
    let q = a.questions.iter().find(|q| q.id == question_id).cloned().expect("question exists");

    if let Some(grade) = quiz::local_grade(&q, &answer) {
        return update_question(&s, &attempt_id, &question_id, |q| q.grade = Some(grade));
    }
    let targets = llm::resolve_role(&s.db(), "grade")?;
    cancellable(&s.clone(), request_id, async move {
        let (grade, _) = llm::with_fallback(&targets, |t| {
            let messages = quiz::grade_messages(&q, &answer);
            let http = s.http.clone();
            let mut sink = token_sink(on_event.clone());
            let q = q.clone();
            async move {
                let reply =
                    llm::chat_json::<quiz::GradeOut>(&http, &t, messages, &quiz::validate_grade, 1200, &mut sink).await?;
                Ok(quiz::to_grade(
                    &q,
                    &reply.value,
                    json!({ "model": t.describe(), "promptVersion": quiz::PROMPT_VERSION, "usage": reply.usage }),
                ))
            }
        })
        .await?;
        update_question(&s, &attempt_id, &question_id, |q| q.grade = Some(grade))
    })
    .await
}

#[tauri::command]
pub async fn quiz_finish(
    state: State<'_, AppState>,
    request_id: String,
    attempt_id: String,
    on_event: Channel<StreamEvent>,
) -> AppResult<Attempt> {
    let s = state.inner().clone();
    let mut a = quiz::get(&s.db(), &attempt_id)?;
    if a.kind == "assessment" {
        // A skill test ended early: the question on screen was never answered and says nothing.
        a.questions.retain(|q| q.answer.is_some());
    }
    for q in a.questions.iter_mut().filter(|q| q.grade.is_none()) {
        let answer = q.answer.clone().unwrap_or_default();
        q.grade = quiz::local_grade(q, "").map(|mut g| {
            if !answer.is_empty() {
                g.feedback = format!("Not graded. {}", g.feedback);
            }
            g
        });
    }
    let targets = llm::resolve_role(&s.db(), "grade").unwrap_or_default();
    cancellable(&s.clone(), request_id, async move {
        let chapter_title = a.chapter_title.clone().unwrap_or_default();
        let summary = if targets.is_empty() {
            None
        } else {
            let _ = on_event.send(StreamEvent::Status { message: "Writing your review…".into() });
            let questions = a.questions.clone();
            llm::with_fallback(&targets, |t| {
                let messages = quiz::summary_messages(&chapter_title, &questions);
                let http = s.http.clone();
                let mut sink = token_sink(on_event.clone());
                async move {
                    llm::chat_json::<quiz::SummaryOut>(&http, &t, messages, &quiz::validate_summary, 1600, &mut sink).await
                }
            })
            .await
            .ok()
            .map(|(reply, _)| reply.value)
        };
        let overall = quiz::to_overall(&a.questions, summary.as_ref());
        let db = s.db();
        // Mistakes become review cards, due tomorrow (once per quiz; regrading doesn't add more).
        if a.kind == "chapter" && !cards::has_cards_for(&db, &a.id)? {
            let llm_cards = summary.as_ref().map(|s| s.cards.as_slice()).unwrap_or_default();
            let due = cards::tomorrow_start();
            for c in cards::cards_for_mistakes(&a, llm_cards) {
                if let Err(e) = cards::create(&db, &c, due, &s.device_id) {
                    log::warn!("card not created: {e}");
                }
            }
        }
        if overall.score >= 70 {
            if let (Some(fp), Some(ch)) = (&a.book_fingerprint, &a.chapter_id) {
                let patch = ProgressPatch { last_page: None, status: None, chapter_done: Some(ch.clone()), chapter_undone: None };
                library::save_progress(&db, fp, &patch, &s.device_id)?;
            }
        }
        a.overall = Some(overall);
        a.status = "completed".into();
        a.updated_at = db::now();
        quiz::save(&db, &a, &s.device_id)?;
        Ok(a)
    })
    .await
}

#[tauri::command]
pub fn attempts_list(state: State<'_, AppState>, fingerprint: Option<String>) -> AppResult<Vec<AttemptSummary>> {
    quiz::list(&state.db(), fingerprint.as_deref())
}

#[tauri::command]
pub fn attempt_get(state: State<'_, AppState>, id: String) -> AppResult<Attempt> {
    quiz::get(&state.db(), &id)
}

#[tauri::command]
pub fn attempt_delete(state: State<'_, AppState>, id: String) -> AppResult<()> {
    quiz::delete(&state.db(), &id, &state.device_id)
}

// ---------- skill test ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillTrack {
    id: String,
    label: String,
    topic_count: usize,
}

#[tauri::command]
pub fn skill_tracks() -> Vec<SkillTrack> {
    skills::tracks()
        .iter()
        .map(|t| SkillTrack { id: t.id.clone(), label: t.label.clone(), topic_count: t.topics.len() })
        .collect()
}

#[tauri::command]
pub fn assessment_start(state: State<'_, AppState>, track: String, free_text: bool) -> AppResult<Attempt> {
    let track = skills::track(&track)?;
    let a = assessment::new_attempt(track, free_text);
    quiz::save(&state.db(), &a, &state.device_id)?;
    Ok(a)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssessmentStep {
    attempt: Attempt,
    /// The question to show now; `None` when the test is over.
    question_id: Option<String>,
    topics_done: usize,
    topics_total: usize,
    skipped: Vec<String>,
}

fn step(track: &skills::TrackTopics, attempt: Attempt, question_id: Option<String>) -> AssessmentStep {
    let (topics_done, topics_total) = assessment::progress(track, &attempt);
    let skipped = assessment::skipped(track, &attempt);
    AssessmentStep { attempt, question_id, topics_done, topics_total, skipped }
}

fn load_assessment(s: &AppState, attempt_id: &str) -> AppResult<(Attempt, &'static skills::TrackTopics)> {
    let a = quiz::get(&s.db(), attempt_id)?;
    let track = skills::assessment_track(&a)
        .ok_or_else(|| AppError::Invalid("This quiz is not a skill test".into()))
        .and_then(skills::track)?;
    Ok((a, track))
}

/// Writes one topic's questions (one model call) and stores them in the attempt's pool.
async fn write_topic_questions(
    s: &AppState,
    attempt_id: &str,
    track: &'static skills::TrackTopics,
    topic_id: &str,
    free_text: bool,
    on_event: Channel<StreamEvent>,
) -> AppResult<()> {
    let topic = skills::topic(topic_id).ok_or_else(|| AppError::NotFound(format!("Unknown topic {topic_id}")))?;
    let targets = llm::resolve_role(&s.db(), "question")?;
    let (questions, target) = llm::with_fallback(&targets, |t| {
        let messages = assessment::generation_messages(&track.label, topic, free_text);
        let http = s.http.clone();
        let mut sink = token_sink(on_event.clone());
        let ev = on_event.clone();
        let topic_id = topic_id.to_string();
        let label = topic.label.clone();
        async move {
            let _ = ev.send(StreamEvent::Status { message: format!("Writing questions on {label} with {}…", t.model) });
            let validate = |g: &quiz::GenOut| assessment::validate(g, &topic_id);
            let reply = llm::chat_json::<quiz::GenOut>(&http, &t, messages, &validate, 1600, &mut sink).await?;
            Ok(assessment::normalize(&reply.value, &topic_id))
        }
    })
    .await?;
    let db = s.db();
    let mut a = quiz::get(&db, attempt_id)?;
    if !assessment::has_pool(&a, topic_id) {
        assessment::put_pool(&mut a, topic_id, &questions)?;
        if let Some(m) = a.model.as_object_mut() {
            m.entry("question").or_insert(target.describe());
        }
        a.updated_at = db::now();
        quiz::save(&db, &a, &s.device_id)?;
    }
    Ok(())
}

/// The question to answer now: an unanswered one, or the next one the adaptive rules pick
/// (writing that topic's questions first if needed).
#[tauri::command]
pub async fn assessment_next(
    state: State<'_, AppState>,
    request_id: String,
    attempt_id: String,
    on_event: Channel<StreamEvent>,
) -> AppResult<AssessmentStep> {
    let s = state.inner().clone();
    let (a, track) = load_assessment(&s, &attempt_id)?;
    if let Some(q) = a.questions.iter().find(|q| q.answer.is_none()) {
        let id = q.id.clone();
        return Ok(step(track, a, Some(id)));
    }
    cancellable(&s.clone(), request_id, async move {
        // Normally one or two rounds: write the topic's questions if needed, then take one.
        for _ in 0..3 {
            let (mut a, _) = load_assessment(&s, &attempt_id)?;
            let assessment::Next::Ask { topic, difficulty } = assessment::next(track, &a) else {
                return Ok(step(track, a, None));
            };
            if !assessment::has_pool(&a, &topic) {
                write_topic_questions(&s, &attempt_id, track, &topic, assessment::free_text(&a), on_event.clone()).await?;
                continue;
            }
            let db = s.db();
            a = quiz::get(&db, &attempt_id)?; // fresh copy under the lock
            if let Some(q) = a.questions.iter().find(|q| q.answer.is_none()) {
                let id = q.id.clone(); // a concurrent call already picked one
                return Ok(step(track, a, Some(id)));
            }
            match assessment::take(&mut a, &topic, difficulty)? {
                Some(q) => {
                    let id = q.id.clone();
                    a.questions.push(q);
                    a.updated_at = db::now();
                    quiz::save(&db, &a, &s.device_id)?;
                    return Ok(step(track, a, Some(id)));
                }
                None => {
                    // Pool used up (the model wrote fewer questions than asked): write fresh ones.
                    if let Some(pool) = a.model.get_mut("pool").and_then(|p| p.as_object_mut()) {
                        pool.remove(&topic);
                    }
                    quiz::save(&db, &a, &s.device_id)?;
                }
            }
        }
        Err(AppError::InvalidOutput("Couldn't prepare the next question".into()))
    })
    .await
}

/// Writes the next topic's questions in the background while the learner answers.
#[tauri::command]
pub async fn assessment_prefetch(state: State<'_, AppState>, request_id: String, attempt_id: String) -> AppResult<()> {
    let s = state.inner().clone();
    let (a, track) = load_assessment(&s, &attempt_id)?;
    let Some(topic) = assessment::prefetch_topic(track, &a) else { return Ok(()) };
    let free_text = assessment::free_text(&a);
    cancellable(&s.clone(), request_id, async move {
        write_topic_questions(&s, &attempt_id, track, &topic, free_text, Channel::new(|_| Ok(()))).await
    })
    .await
}

// ---------- skill map & roadmap ----------

#[tauri::command]
pub fn skills_get(state: State<'_, AppState>, track: String) -> AppResult<SkillMap> {
    let db = state.db();
    skills::skill_map(&track, &quiz::all(&db)?, &interview_evidence(&db)?)
}

#[tauri::command]
pub async fn roadmap_get(state: State<'_, AppState>, track: String) -> AppResult<Roadmap> {
    let s = state.inner().clone();
    blocking(move || {
        let (attempts, books, pace, extra) = {
            let db = s.db();
            let pace = db::get_pref(&db, "weekly_pace")?.and_then(|p| p.parse().ok()).unwrap_or(3);
            (quiz::all(&db)?, library::list(&db)?, pace, interview_evidence(&db)?)
        };
        let t = skills::track(&track)?;
        let (chapters, needs_index) = skills::chapter_index(&s.index(), t, &books)?;
        skills::roadmap(&track, &attempts, &books, &chapters, needs_index, pace, &extra)
    })
    .await
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareProgress {
    done: usize,
    total: usize,
    title: String,
}

/// Indexes every book the track's roadmap could point to, so chapters can be suggested.
#[tauri::command]
pub async fn roadmap_prepare(
    state: State<'_, AppState>,
    track: String,
    on_progress: Channel<PrepareProgress>,
) -> AppResult<Vec<String>> {
    let s = state.inner().clone();
    blocking(move || {
        let books = library::list(&s.db())?;
        let t = skills::track(&track)?;
        let todo: Vec<(String, String)> = {
            let idx = s.index();
            skills::track_books(t, &books)
                .into_iter()
                .filter(|b| !index::status(&idx, &b.fingerprint).map(|st| st.indexed).unwrap_or(false))
                .map(|b| (b.fingerprint.clone(), b.title.clone()))
                .collect()
        };
        let mut errors = Vec::new();
        for (i, (fp, title)) in todo.iter().enumerate() {
            let _ = on_progress.send(PrepareProgress { done: i, total: todo.len(), title: title.clone() });
            if let Err(e) = index_now(&s, fp, |_, _| {}) {
                errors.push(format!("{title}: {e}"));
            }
        }
        let _ = on_progress.send(PrepareProgress { done: todo.len(), total: todo.len(), title: String::new() });
        Ok(errors)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    goal_track: Option<String>,
    weekly_pace: u32,
    daily_cards: u32,
}

#[tauri::command]
pub fn prefs_get(state: State<'_, AppState>) -> AppResult<Prefs> {
    let db = state.db();
    Ok(Prefs {
        goal_track: db::get_pref(&db, "goal_track")?,
        weekly_pace: db::get_pref(&db, "weekly_pace")?.and_then(|p| p.parse().ok()).unwrap_or(3),
        daily_cards: daily_limit(&db)?,
    })
}

#[tauri::command]
pub fn prefs_set(
    state: State<'_, AppState>,
    goal_track: Option<String>,
    weekly_pace: Option<u32>,
    daily_cards: Option<u32>,
) -> AppResult<Prefs> {
    {
        let db = state.db();
        if let Some(t) = goal_track {
            skills::track(&t)?;
            db::set_pref(&db, "goal_track", &t, &state.device_id)?;
        }
        if let Some(p) = weekly_pace {
            db::set_pref(&db, "weekly_pace", &p.clamp(1, 20).to_string(), &state.device_id)?;
        }
        if let Some(n) = daily_cards {
            db::set_pref(&db, "daily_cards", &n.clamp(5, 500).to_string(), &state.device_id)?;
        }
    }
    prefs_get(state)
}

// ---------- cards ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardView {
    #[serde(flatten)]
    card: cards::Card,
    /// Wait after Again / Hard / Good / Easy, e.g. ["10m", "1d", "3d", "16d"].
    preview: [String; 4],
}

fn card_view(card: cards::Card) -> CardView {
    let preview = cards::preview(&card.schedule, chrono::Utc::now());
    CardView { card, preview }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DueQueue {
    cards: Vec<CardView>,
    /// All cards due now (the queue itself is capped by the daily limit).
    due_now: u32,
    daily_limit: u32,
}

fn daily_limit(conn: &rusqlite::Connection) -> AppResult<u32> {
    Ok(db::get_pref(conn, "daily_cards")?.and_then(|p| p.parse().ok()).unwrap_or(50))
}

#[tauri::command]
pub fn cards_due(state: State<'_, AppState>) -> AppResult<DueQueue> {
    let db = state.db();
    let now = chrono::Utc::now();
    let limit = daily_limit(&db)?;
    let reviewed_today: u32 = {
        let start = chrono::Local::now().date_naive().and_hms_opt(0, 0, 0).expect("valid time");
        let start = start.and_local_timezone(chrono::Local).earliest().map(|t| t.with_timezone(&chrono::Utc)).unwrap_or(now);
        db.query_row(
            "SELECT COUNT(DISTINCT card_id) FROM reviews WHERE reviewed_at >= ?1",
            [start.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)],
            |r| r.get(0),
        )?
    };
    let remaining = limit.saturating_sub(reviewed_today);
    Ok(DueQueue {
        cards: cards::due(&db, now, remaining)?.into_iter().map(card_view).collect(),
        due_now: cards::count_due(&db, now)?,
        daily_limit: limit,
    })
}

#[tauri::command]
pub fn card_review(state: State<'_, AppState>, id: String, rating: u8) -> AppResult<CardView> {
    cards::review(&state.db(), &id, rating, chrono::Utc::now(), &state.device_id).map(card_view)
}

#[tauri::command]
pub fn cards_list(state: State<'_, AppState>, query: Option<String>) -> AppResult<Vec<CardView>> {
    let q = query.filter(|q| !q.trim().is_empty());
    Ok(cards::list(&state.db(), q.as_deref())?.into_iter().map(card_view).collect())
}

/// Manual cards (e.g. from text selected in the reader) are due right away.
#[tauri::command]
pub fn card_create(state: State<'_, AppState>, card: cards::NewCard) -> AppResult<CardView> {
    let db = state.db();
    let id = cards::create(&db, &card, chrono::Utc::now(), &state.device_id)?;
    cards::get(&db, &id).map(card_view)
}

#[tauri::command]
pub fn card_update(state: State<'_, AppState>, id: String, front: String, back: String) -> AppResult<CardView> {
    cards::update_text(&state.db(), &id, &front, &back, &state.device_id).map(card_view)
}

#[tauri::command]
pub fn card_delete(state: State<'_, AppState>, id: String) -> AppResult<()> {
    cards::delete(&state.db(), &id, &state.device_id)
}

// ---------- reading time & dashboard ----------

#[tauri::command]
pub fn reading_tick(state: State<'_, AppState>, fingerprint: String, minutes: u32) -> AppResult<()> {
    stats::log_reading(&state.db(), &fingerprint, minutes, chrono::Local::now().fixed_offset(), &state.device_id)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeakTopic {
    id: String,
    label: String,
    mastery: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    #[serde(flatten)]
    stats: stats::Stats,
    goal_track: String,
    goal_label: String,
    weak_topics: Vec<WeakTopic>,
    untested_topics: usize,
    next_step: Option<skills::WeekItem>,
    daily_limit: u32,
}

#[tauri::command]
pub async fn dashboard_get(state: State<'_, AppState>) -> AppResult<Dashboard> {
    let s = state.inner().clone();
    blocking(move || {
        let (attempts, books, goal, pace, stats, limit, extra) = {
            let db = s.db();
            let attempts = quiz::all(&db)?;
            let stats = stats::compute(&db, &attempts, chrono::Local::now().fixed_offset())?;
            let goal = db::get_pref(&db, "goal_track")?.unwrap_or_else(|| "fundamentals".into());
            let pace = db::get_pref(&db, "weekly_pace")?.and_then(|p| p.parse().ok()).unwrap_or(3);
            (attempts, library::list(&db)?, goal, pace, stats, daily_limit(&db)?, interview_evidence(&db)?)
        };
        let track = skills::track(&goal).or_else(|_| skills::track("fundamentals"))?;
        let map = skills::skill_map(&track.id, &attempts, &extra)?;
        let mut weak: Vec<WeakTopic> = map
            .topics
            .iter()
            .filter_map(|t| Some(WeakTopic { id: t.id.clone(), label: t.label.clone(), mastery: t.mastery? }))
            .filter(|t| t.mastery < 75)
            .collect();
        weak.sort_by_key(|t| t.mastery);
        weak.truncate(5);
        let (chapters, needs_index) = skills::chapter_index(&s.index(), track, &books)?;
        let roadmap = skills::roadmap(&track.id, &attempts, &books, &chapters, needs_index, pace, &extra)?;
        Ok(Dashboard {
            stats,
            goal_track: track.id.clone(),
            goal_label: track.label.clone(),
            untested_topics: map.topics.iter().filter(|t| t.mastery.is_none()).count(),
            weak_topics: weak,
            next_step: roadmap.weeks.first().and_then(|w| w.items.first().cloned()),
            daily_limit: limit,
        })
    })
    .await
}

// ---------- mock interviews ----------

#[tauri::command]
pub fn interview_personas() -> Vec<interview::Persona> {
    interview::personas().to_vec()
}

fn interview_evidence(conn: &rusqlite::Connection) -> AppResult<Vec<skills::Evidence>> {
    Ok(interview::all(conn)?.iter().flat_map(interview::evidence).collect())
}

/// What an interview will be about: title, text chunks to ground it in, taggable topics, default topic.
struct InterviewContext {
    title: String,
    chunks: Vec<index::Chunk>,
    topics: Vec<(String, String)>,
    default_topic: Option<String>,
}

fn topic_pairs<'a>(ts: impl IntoIterator<Item = &'a skills::Topic>) -> Vec<(String, String)> {
    ts.into_iter().map(|t| (t.id.clone(), t.label.clone())).collect()
}

fn interview_context(s: &AppState, setup: &interview::Setup) -> AppResult<InterviewContext> {
    let need_book = || setup.book_fingerprint.clone().ok_or_else(|| AppError::Invalid("Choose a book".into()));
    let not_indexed = || AppError::Invalid("Open this book once so the app can read it, then start the interview.".into());
    match setup.source.as_str() {
        "chapter" => {
            let fp = need_book()?;
            let idx_no = setup.chapter_idx.ok_or_else(|| AppError::Invalid("Choose a chapter".into()))?;
            let book = library::get(&s.db(), &fp)?;
            let idx = s.index();
            let chapter = index::chapters(&idx, &fp)?.into_iter().find(|c| c.idx == idx_no).ok_or_else(not_indexed)?;
            let chunks = index::chapter_chunks(&idx, &fp, idx_no)?;
            let ids = skills::chapter_topics(&idx, &book, idx_no).unwrap_or_default();
            let topics = topic_pairs(ids.iter().filter_map(|id| skills::topic(id)));
            Ok(InterviewContext {
                title: format!("{} — {}", book.title, chapter.title),
                chunks,
                default_topic: ids.first().cloned(),
                topics,
            })
        }
        "book" => {
            let fp = need_book()?;
            let book = library::get(&s.db(), &fp)?;
            let idx = s.index();
            if !index::status(&idx, &fp)?.indexed {
                return Err(not_indexed());
            }
            // Prefer the chapters you've finished; otherwise the whole book.
            let done: Vec<u32> = {
                let ids = book.progress.as_ref().map(|p| p.chapters_done.clone()).unwrap_or_default();
                index::chapters(&idx, &fp)?.into_iter().filter(|c| ids.contains(&c.id)).map(|c| c.idx).collect()
            };
            let chunks = index::book_chunks(&idx, &fp, (!done.is_empty()).then_some(done.as_slice()))?;
            Ok(InterviewContext {
                title: book.title.clone(),
                chunks,
                topics: topic_pairs(skills::book_topics(&book)),
                default_topic: None,
            })
        }
        "topics" | "general" => {
            let track = skills::track(setup.track.as_deref().unwrap_or("fundamentals"))?;
            let (attempts, books, extra) = {
                let db = s.db();
                (quiz::all(&db)?, library::list(&db)?, interview_evidence(&db)?)
            };
            let mut ids = setup.topic_ids.clone();
            if ids.is_empty() {
                // Weakest measured topics first, then untested ones.
                let map = skills::skill_map(&track.id, &attempts, &extra)?;
                let mut ts: Vec<_> = map.topics.iter().filter(|t| t.mastery.is_none_or(|m| m < 75)).collect();
                ts.sort_by_key(|t| t.mastery.unwrap_or(45));
                ids = ts.iter().take(4).map(|t| t.id.clone()).collect();
                if ids.is_empty() {
                    ids = track.topics.iter().take(4).map(|t| t.id.clone()).collect();
                }
            }
            let topics: Vec<&skills::Topic> = ids.iter().filter_map(|id| skills::topic(id)).collect();
            let mut chunks = Vec::new();
            if setup.source == "topics" {
                // Ground each topic in the best matching chapter of an indexed book, when there is one.
                let idx = s.index();
                for t in &topics {
                    for b in skills::candidate_books(t, &books) {
                        if !index::status(&idx, &b.fingerprint)?.indexed {
                            continue;
                        }
                        let matches = skills::topic_chapters(&idx, &b.fingerprint, t)?;
                        if let Some(m) = skills::best_matches(&matches).first() {
                            chunks.extend(index::chapter_chunks(&idx, &b.fingerprint, m.chapter.idx)?);
                            break;
                        }
                    }
                }
            }
            Ok(InterviewContext {
                title: format!("{}: {}", track.label, topics.iter().map(|t| t.label.as_str()).collect::<Vec<_>>().join(", ")),
                chunks,
                default_topic: ids.first().cloned(),
                topics: topic_pairs(topics),
            })
        }
        other => Err(AppError::Invalid(format!("Unknown interview source “{other}”"))),
    }
}

#[tauri::command]
pub async fn interview_start(
    state: State<'_, AppState>,
    request_id: String,
    setup: interview::Setup,
    on_event: Channel<StreamEvent>,
) -> AppResult<interview::Interview> {
    let s = state.inner().clone();
    let mut setup = setup;
    if !matches!(setup.level.as_str(), "junior" | "mid" | "senior") {
        setup.level = "mid".into();
    }
    let persona = match &setup.persona_id {
        Some(id) => interview::persona(id),
        None => {
            let recent = interview::recent_personas(&s.db(), 5)?;
            let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(7);
            interview::pick_persona(&recent, seed)
        }
    };
    setup.persona_id = Some(persona.id.clone());
    if !matches!(setup.mode.as_str(), "fast" | "thorough") {
        setup.mode = recommended_mode(&s.db()).into();
    }
    let targets = llm::resolve_role(&s.db(), "question")?;
    let ctx = {
        let s = s.clone();
        let setup = setup.clone();
        blocking(move || interview_context(&s, &setup)).await?
    };
    let count = interview::question_count(setup.minutes);

    cancellable(&s.clone(), request_id, async move {
        let _ = on_event.send(StreamEvent::Status { message: format!("{} is preparing your interview…", persona.name) });
        let max_tokens = 600 + 450 * count as u32;
        let ((questions, opening, usage), target) = llm::with_fallback(&targets, |t| {
            // The plan itself is long; leave room for it in a local model's context window.
            let budget = if t.provider.adapter == "ollama" {
                let ctx = t.provider.options.num_ctx.unwrap_or(8192) as usize;
                ctx.saturating_sub(1500 + max_tokens as usize).max(1000) * 3
            } else {
                context_budget(&t)
            };
            let sources = quiz::label_sources(&index::select_context(&ctx.chunks, budget));
            let input = interview::PlanInput {
                persona,
                level: &setup.level,
                count,
                subject: &ctx.title,
                topics: &ctx.topics,
                sources: &sources,
            };
            let messages = interview::plan_messages(&input);
            let http = s.http.clone();
            let mut sink = token_sink(on_event.clone());
            let topics = ctx.topics.clone();
            let default_topic = ctx.default_topic.clone();
            async move {
                let validate = |p: &interview::PlanOut| interview::validate_plan(p, &sources, count);
                let reply = llm::chat_json::<interview::PlanOut>(&http, &t, messages, &validate, max_tokens, &mut sink).await?;
                let qs = interview::normalize_plan(&reply.value, &sources, &topics, default_topic.as_deref(), count);
                Ok((qs, reply.value.opening.clone(), reply.usage))
            }
        })
        .await?;
        let topics: Vec<String> = {
            let mut t: Vec<String> = questions.iter().filter_map(|q| q.topic.clone()).collect();
            t.sort();
            t.dedup();
            t
        };
        let model = json!({ "plannedBy": target.describe(), "promptVersion": interview::PROMPT_VERSION, "usage": usage });
        let iv = interview::new_interview(setup, persona, ctx.title.clone(), &opening, questions, topics, model);
        interview::save(&s.db(), &iv, &s.device_id)?;
        Ok(iv)
    })
    .await
}

/// Records the candidate's message (answer, hint request or "I don't know") before the interviewer replies.
#[tauri::command]
pub fn interview_say(state: State<'_, AppState>, id: String, kind: String, text: String) -> AppResult<interview::Interview> {
    let db = state.db();
    let mut iv = interview::get(&db, &id)?;
    if iv.status != "in_progress" {
        return Err(AppError::Invalid("This interview is over".into()));
    }
    if iv.transcript.last().is_some_and(|t| t.role == "candidate") {
        return Err(AppError::Invalid("Waiting for the interviewer's reply".into()));
    }
    let kind = match kind.as_str() {
        "hint_request" | "dont_know" => kind,
        _ if text.trim().is_empty() => return Err(AppError::Invalid("Write an answer first".into())),
        _ => "answer".to_string(),
    };
    interview::add_candidate(&mut iv, &kind, &text);
    iv.updated_at = db::now();
    interview::save(&db, &iv, &state.device_id)?;
    Ok(iv)
}

fn voice_targets(conn: &rusqlite::Connection) -> AppResult<Vec<Target>> {
    match llm::resolve_role(conn, "interviewer") {
        Ok(t) if !t.is_empty() => Ok(t),
        _ => llm::resolve_role(conn, "grade"),
    }
}

/// Fast (one call per answer) when the judge or the voice runs locally on a CPU; thorough otherwise.
fn recommended_mode(conn: &rusqlite::Connection) -> &'static str {
    let local = |role: &str| llm::resolve_role(conn, role).ok().and_then(|t| t.first().map(|t| llm::is_local(&t.provider)));
    let judge_local = local("grade").unwrap_or(false);
    let voice_local = voice_targets(conn).ok().and_then(|t| t.first().map(|t| llm::is_local(&t.provider))).unwrap_or(false);
    if (judge_local || voice_local) && models::system_info().accelerator == "cpu" {
        "fast"
    } else {
        "thorough"
    }
}

/// The interviewer reacts to the candidate's last message. Thorough mode: judge (intent + points)
/// → rules decide the move → the interviewer's voice says it. Fast mode: one call does both.
/// Obvious messages (hint, "I don't know", "repeat", "skip") skip the judge.
#[tauri::command]
pub async fn interview_respond(
    state: State<'_, AppState>,
    request_id: String,
    id: String,
    on_event: Channel<StreamEvent>,
) -> AppResult<interview::Interview> {
    let s = state.inner().clone();
    let iv = interview::get(&s.db(), &id)?;
    let Some(last) = iv.transcript.last().filter(|t| t.role == "candidate").cloned() else {
        return Ok(iv); // nothing to answer
    };
    let fast = iv.setup.mode == "fast";
    let quick = interview::quick_judgement(&iv, &last.kind, &last.text);
    let (judges, voices) = {
        let db = s.db();
        let judges = if quick.is_none() { llm::resolve_role(&db, "grade")? } else { vec![] };
        (judges, voice_targets(&db).unwrap_or_default())
    };
    let name = interview::persona(&iv.persona_id).name.clone();
    cancellable(&s.clone(), request_id, async move {
        let out = match quick {
            Some(o) => o,
            None => {
                let (out, _) = llm::with_fallback(&judges, |t| {
                    let messages = interview::judge_messages(&iv, &last.text, fast);
                    let http = s.http.clone();
                    let mut sink = token_sink(on_event.clone());
                    async move {
                        let validate = |o: &interview::JudgeOut| interview::validate_judge(o, fast);
                        let r = llm::chat_json::<interview::JudgeOut>(&http, &t, messages, &validate, if fast { 700 } else { 400 }, &mut sink)
                            .await?;
                        Ok(r.value)
                    }
                })
                .await?;
                out
            }
        };
        let before = iv.transcript.len();
        let mut next = iv.clone();
        let decision = interview::decide(&mut next, &out);
        let speech = if !decision.needs_voice() {
            String::new()
        } else if fast || voices.is_empty() {
            out.message.clone()
        } else {
            let _ = on_event.send(StreamEvent::Status { message: format!("{name} is answering…") });
            let said = llm::with_fallback(&voices, |t| {
                let messages = interview::voice_messages(&next, &decision, &last.text);
                let http = s.http.clone();
                let ev = on_event.clone();
                async move {
                    let mut sink = move |tok: &str| {
                        let _ = ev.send(StreamEvent::Speech { text: tok.to_string() });
                    };
                    let (text, _) = llm::chat_stream(&http, &t, &messages, false, 220, &mut sink).await?;
                    Ok(text)
                }
            })
            .await;
            match said {
                Ok((text, _)) => interview::clean_speech(&text, &name),
                Err(AppError::Cancelled) => return Err(AppError::Cancelled),
                Err(e) => {
                    log::warn!("interviewer voice failed, using a fixed line: {e}");
                    String::new()
                }
            }
        };
        interview::commit(&mut next, &decision, &speech);
        next.updated_at = db::now();
        let db = s.db();
        let current = interview::get(&db, &id)?;
        if current.transcript.len() != before {
            return Ok(current); // answered meanwhile (e.g. a second window)
        }
        interview::save(&db, &next, &s.device_id)?;
        Ok(next)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckItem {
    role: String,
    job: String,
    provider_id: Option<String>,
    model: Option<String>,
    /// "ok" | "warn" | "bad"
    level: String,
    messages: Vec<String>,
    /// From the last "Try this model" run on this machine.
    tested: Option<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCheck {
    items: Vec<CheckItem>,
    recommended_mode: String,
    /// Something is badly wrong (no model, model far too small, JSON failed).
    blocking: bool,
}

/// Checks the models an interview will use, before starting: set up, big enough, enough context,
/// returns valid JSON (from the last "Try this model" on this PC).
#[tauri::command]
pub async fn interview_check(state: State<'_, AppState>, minutes: u32) -> AppResult<ModelCheck> {
    let s = state.inner().clone();
    let jobs = [
        ("question", "Plans the questions", 3.0f32, 7.0f32, true),
        ("grade", "Judges your answers", 3.0, 7.0, true),
        ("interviewer", "Speaks as the interviewer", 1.0, 3.0, false),
    ];
    let count = interview::question_count(minutes) as u32;
    let plan_tokens = 600 + 450 * (count + interview::spare_count(count as usize) as u32);
    let mut items = Vec::new();
    for (role, job, bad_below, warn_below, needs_json) in jobs {
        let resolved = if role == "interviewer" { voice_targets(&s.db()) } else { llm::resolve_role(&s.db(), role) };
        let t = match resolved {
            Ok(t) if !t.is_empty() => t[0].clone(),
            Ok(_) | Err(_) => {
                let msg = match llm::resolve_role(&s.db(), role) {
                    Err(e) => e.to_string(),
                    Ok(_) => "No model chosen".into(),
                };
                items.push(CheckItem { role: role.into(), job: job.into(), provider_id: None, model: None, level: "bad".into(), messages: vec![msg], tested: None });
                continue;
            }
        };
        let mut level = "ok";
        let mut messages: Vec<String> = Vec::new();
        fn worse(l: &'static str, m: String, level: &mut &'static str, messages: &mut Vec<String>) {
            if l == "bad" || (l == "warn" && *level == "ok") {
                *level = l;
            }
            messages.push(m);
        }
        if role == "interviewer" && llm::resolve_role(&s.db(), "interviewer").map(|t| t.is_empty()).unwrap_or(true) {
            messages.push("Uses the grading model (set “Interviewer's voice” in Settings to use a smaller, faster one).".into());
        }
        if llm::is_local(&t.provider) {
            let size = if t.provider.adapter == "ollama" {
                llm::list_models(&s.http, &t.provider, None)
                    .await
                    .ok()
                    .and_then(|ms| ms.into_iter().find(|m| m.id == t.model))
                    .and_then(|m| m.parameter_size.as_deref().and_then(llm::param_billions))
                    .or_else(|| llm::param_billions(&t.model))
            } else {
                llm::param_billions(&t.model)
            };
            match size {
                Some(b) if b < bad_below => worse("bad", format!("{} is a {b}B model — too small for this job (needs {warn_below:.0}B or more).", t.model), &mut level, &mut messages),
                Some(b) if b < warn_below => worse("warn", format!("{} is a {b}B model — expect mistakes; {warn_below:.0}B or more works much better.", t.model), &mut level, &mut messages),
                Some(_) => {}
                None => worse("warn", format!("Couldn't tell how big {} is.", t.model), &mut level, &mut messages),
            }
            if t.provider.adapter == "ollama" && role == "question" {
                let ctx = t.provider.options.num_ctx.unwrap_or(8192);
                if (ctx as u32) < plan_tokens + 2500 {
                    worse("warn", format!("Context is {ctx} tokens; planning a {minutes}-minute interview needs about {} — book excerpts will be cut short. Raise “Context” for this provider in Settings or pick a shorter interview.", plan_tokens + 2500), &mut level, &mut messages);
                }
            }
        }
        let tested: Option<serde_json::Value> = db::get_setting(&s.db(), &format!("model_test:{}:{}", t.provider.id, t.model))?
            .and_then(|v| serde_json::from_str(&v).ok());
        if needs_json {
            match tested.as_ref().and_then(|v| v.get("jsonOk")).and_then(|v| v.as_bool()) {
                Some(false) => worse("bad", "Its last test didn't return valid JSON, which this job needs.".into(), &mut level, &mut messages),
                Some(true) => {}
                None => worse("warn", "Not tested on this computer yet.".into(), &mut level, &mut messages),
            }
        }
        items.push(CheckItem {
            role: role.into(),
            job: job.into(),
            provider_id: Some(t.provider.id.clone()),
            model: Some(t.model.clone()),
            level: level.into(),
            messages,
            tested,
        });
    }
    let blocking = items.iter().any(|i| i.level == "bad");
    let mode = recommended_mode(&s.db()).to_string();
    Ok(ModelCheck { items, recommended_mode: mode, blocking })
}

/// Ends the interview (early or after the last question) and writes the report.
#[tauri::command]
pub async fn interview_finish(
    state: State<'_, AppState>,
    request_id: String,
    id: String,
    on_event: Channel<StreamEvent>,
) -> AppResult<interview::Interview> {
    let s = state.inner().clone();
    let mut iv = interview::get(&s.db(), &id)?;
    if iv.status == "in_progress" {
        interview::stop(&mut iv);
    }
    let targets = llm::resolve_role(&s.db(), "grade").unwrap_or_default();
    cancellable(&s.clone(), request_id, async move {
        let persona = interview::persona(&iv.persona_id);
        let reply = if targets.is_empty() || interview::asked(&iv).next().is_none() {
            None
        } else {
            let _ = on_event.send(StreamEvent::Status { message: format!("{} is writing up notes…", persona.name) });
            let iv2 = iv.clone();
            llm::with_fallback(&targets, |t| {
                let messages = interview::report_messages(&iv2);
                let http = s.http.clone();
                let mut sink = token_sink(on_event.clone());
                async move {
                    let r = llm::chat_json::<interview::ReportOut>(&http, &t, messages, &interview::validate_report, 2000, &mut sink).await?;
                    Ok((r.value, t.describe()))
                }
            })
            .await
            .ok()
            .map(|(r, _)| r)
        };
        let (out, by) = match reply {
            Some((r, by)) => (Some(r), by),
            None => (None, json!({ "local": true })),
        };
        iv.report = Some(interview::build_report(&iv, out.as_ref(), by));
        iv.status = "completed".into();
        iv.updated_at = db::now();
        let db = s.db();
        if !cards::has_cards_for(&db, &iv.id)? {
            let llm_cards = out.as_ref().map(|r| r.cards.as_slice()).unwrap_or_default();
            let due = cards::tomorrow_start();
            for c in interview::gap_cards(&iv, llm_cards) {
                if let Err(e) = cards::create(&db, &c, due, &s.device_id) {
                    log::warn!("card not created: {e}");
                }
            }
        }
        interview::save(&db, &iv, &s.device_id)?;
        Ok(iv)
    })
    .await
}

#[tauri::command]
pub fn interviews_list(state: State<'_, AppState>) -> AppResult<Vec<interview::InterviewSummary>> {
    interview::summaries(&state.db())
}

#[tauri::command]
pub fn interview_get(state: State<'_, AppState>, id: String) -> AppResult<interview::Interview> {
    interview::get(&state.db(), &id)
}

#[tauri::command]
pub fn interview_delete(state: State<'_, AppState>, id: String) -> AppResult<()> {
    interview::delete(&state.db(), &id, &state.device_id)
}

// ---------- teacher (lessons) ----------

fn teacher_targets(conn: &rusqlite::Connection) -> AppResult<Vec<Target>> {
    match llm::resolve_role(conn, "teacher") {
        Ok(t) if !t.is_empty() => Ok(t),
        _ => llm::resolve_role(conn, "question"),
    }
}

/// Characters of book text per call, leaving room in a local model's context for a long reply.
fn teaching_budget(t: &Target, reply_tokens: usize) -> usize {
    if t.provider.adapter == "ollama" {
        let ctx = t.provider.options.num_ctx.unwrap_or(8192) as usize;
        ctx.saturating_sub(1200 + reply_tokens).max(1000) * 3
    } else {
        40_000
    }
}

fn chapter_and_chunks(s: &AppState, fp: &str, chapter_idx: u32) -> AppResult<(pdf::Chapter, Vec<index::Chunk>)> {
    let idx = s.index();
    let chapter = index::chapters(&idx, fp)?
        .into_iter()
        .find(|c| c.idx == chapter_idx)
        .ok_or_else(|| AppError::NotFound("Chapter not found — open the book once so the app can read it".into()))?;
    let chunks = index::chapter_chunks(&idx, fp, chapter_idx)?;
    if chunks.is_empty() {
        return Err(AppError::Invalid("This chapter has no extractable text (it may be scanned images).".into()));
    }
    Ok((chapter, chunks))
}

#[tauri::command]
pub fn lesson_get(state: State<'_, AppState>, fingerprint: String, chapter_idx: u32) -> AppResult<Option<lesson::Lesson>> {
    let chapter = index::chapters(&state.index(), &fingerprint)?.into_iter().find(|c| c.idx == chapter_idx);
    match chapter {
        Some(c) => lesson::get(&state.db(), &lesson::lesson_id(&fingerprint, &c.id)),
        None => Ok(None),
    }
}

/// Step 1: summary, keywords and topics for a chapter (read in parts if it is long).
#[tauri::command]
pub async fn lesson_summarize(
    state: State<'_, AppState>,
    request_id: String,
    fingerprint: String,
    chapter_idx: u32,
    on_event: Channel<StreamEvent>,
) -> AppResult<lesson::Lesson> {
    let s = state.inner().clone();
    let (book, targets) = {
        let db = s.db();
        (library::get(&db, &fingerprint)?, teacher_targets(&db)?)
    };
    let (chapter, chunks) = chapter_and_chunks(&s, &fingerprint, chapter_idx)?;
    const OUTLINE_TOKENS: u32 = 2200;
    cancellable(&s.clone(), request_id, async move {
        let (notes, target) = llm::with_fallback(&targets, |t| {
            let parts = lesson::split_parts(&chunks, teaching_budget(&t, OUTLINE_TOKENS as usize));
            let http = s.http.clone();
            let ev = on_event.clone();
            let (book_title, chapter_title) = (book.title.clone(), chapter.title.clone());
            async move {
                let mut notes = Vec::new();
                for (i, part) in parts.iter().enumerate() {
                    let _ = ev.send(StreamEvent::Status {
                        message: if parts.len() == 1 {
                            format!("Reading “{chapter_title}” with {}…", t.model)
                        } else {
                            format!("Reading part {} of {} with {}…", i + 1, parts.len(), t.model)
                        },
                    });
                    let sources = quiz::label_sources(part);
                    let messages = lesson::outline_messages(&book_title, &chapter_title, i, parts.len(), &sources);
                    let mut sink = token_sink(ev.clone());
                    let r = llm::chat_json::<lesson::OutlineOut>(&http, &t, messages, &lesson::validate_outline, OUTLINE_TOKENS, &mut sink).await?;
                    notes.push(lesson::part_notes(&r.value, &sources));
                }
                Ok(notes)
            }
        })
        .await?;
        let combined = if notes.len() > 1 {
            let _ = on_event.send(StreamEvent::Status { message: "Putting the summary together…".into() });
            let messages = lesson::combine_messages(&chapter.title, &notes);
            let mut sink = token_sink(on_event.clone());
            match llm::chat_json::<lesson::CombineOut>(&s.http, &target, messages, &lesson::validate_combine, 1500, &mut sink).await {
                Ok(r) => Some((
                    r.value.overview.trim().to_string(),
                    r.value.summary.iter().filter_map(|v| v.as_str()).map(lesson::bullet).filter(|v| !v.is_empty()).collect(),
                )),
                Err(AppError::Cancelled) => return Err(AppError::Cancelled),
                Err(e) => {
                    log::warn!("combining lesson parts failed, keeping part summaries: {e}");
                    None
                }
            }
        } else {
            None
        };
        let info = lesson::ChapterInfo { fingerprint: &fingerprint, book_title: &book.title, chapter: &chapter };
        let model = json!({ "writtenBy": target.describe(), "promptVersion": lesson::PROMPT_VERSION });
        let mut l = lesson::build_lesson(&info, notes, combined, model);
        let db = s.db();
        // Re-summarizing keeps the topic lessons already written for topics with the same title.
        if let Some(old) = lesson::get(&db, &l.id)? {
            l.created_at = old.created_at;
            for t in &mut l.topics {
                if let Some(prev) = old.topics.iter().find(|o| o.title.eq_ignore_ascii_case(&t.title)) {
                    t.lesson = prev.lesson.clone();
                }
            }
        }
        lesson::save(&db, &l, &s.device_id)?;
        Ok(l)
    })
    .await
}

/// Step 2: teach one topic — in plain words, in industry terms, in short.
#[tauri::command]
pub async fn lesson_teach(
    state: State<'_, AppState>,
    request_id: String,
    lesson_id: String,
    topic_idx: usize,
    on_event: Channel<StreamEvent>,
) -> AppResult<lesson::Lesson> {
    let s = state.inner().clone();
    let (l, targets) = {
        let db = s.db();
        (lesson::require(&db, &lesson_id)?, teacher_targets(&db)?)
    };
    let topic = l.topics.get(topic_idx).cloned().ok_or_else(|| AppError::NotFound("Topic not found".into()))?;
    let (_, chunks) = chapter_and_chunks(&s, &l.fingerprint, l.chapter_idx)?;
    let relevant = lesson::topic_chunks(&chunks, &topic);
    const TEACH_TOKENS: u32 = 2000;
    cancellable(&s.clone(), request_id, async move {
        let (taught, _) = llm::with_fallback(&targets, |t| {
            let sources = quiz::label_sources(&index::select_context(&relevant, teaching_budget(&t, TEACH_TOKENS as usize)));
            let messages = lesson::teach_messages(&l, &topic, &sources);
            let http = s.http.clone();
            let mut sink = token_sink(on_event.clone());
            let ev = on_event.clone();
            let title = topic.title.clone();
            async move {
                let _ = ev.send(StreamEvent::Status { message: format!("Teaching “{title}” with {}…", t.model) });
                let r = llm::chat_json::<lesson::TeachOut>(&http, &t, messages, &lesson::validate_teach, TEACH_TOKENS, &mut sink).await?;
                Ok(lesson::to_topic_lesson(&r.value, topic.page_start, json!({ "model": t.describe(), "promptVersion": lesson::PROMPT_VERSION })))
            }
        })
        .await?;
        let db = s.db();
        let mut fresh = lesson::require(&db, &lesson_id)?;
        if let Some(t) = fresh.topics.get_mut(topic_idx) {
            t.lesson = Some(taught);
            lesson::absorb_terms(&mut fresh, topic_idx);
        }
        fresh.updated_at = db::now();
        lesson::save(&db, &fresh, &s.device_id)?;
        Ok(fresh)
    })
    .await
}

#[tauri::command]
pub fn lessons_list(state: State<'_, AppState>) -> AppResult<Vec<lesson::LessonSummary>> {
    lesson::list(&state.db())
}

#[tauri::command]
pub fn lesson_delete(state: State<'_, AppState>, id: String) -> AppResult<()> {
    lesson::delete(&state.db(), &id, &state.device_id)
}

/// Turns glossary terms into review cards ("What is X?" → definition), skipping ones already made.
#[tauri::command]
pub fn lesson_terms_to_cards(state: State<'_, AppState>, lesson_id: String, terms: Vec<String>) -> AppResult<u32> {
    let db = state.db();
    let l = lesson::require(&db, &lesson_id)?;
    let mut made = 0;
    for name in terms {
        let Some(t) = l.terms.iter().find(|t| t.term.eq_ignore_ascii_case(&name)) else { continue };
        let key = format!("term:{}", t.term.to_lowercase());
        if cards::exists(&db, &l.id, &key)? {
            continue;
        }
        let card = cards::NewCard {
            front: format!("What is **{}**?", t.term),
            back: t.definition.clone(),
            topic_id: None,
            attempt_id: Some(l.id.clone()),
            question_id: Some(key),
            book_fingerprint: Some(l.fingerprint.clone()),
            page: t.page,
        };
        cards::create(&db, &card, chrono::Utc::now(), &state.device_id)?;
        made += 1;
    }
    Ok(made)
}

// ---------- export / import ----------

#[tauri::command]
pub fn export_progress(state: State<'_, AppState>, path: String) -> AppResult<()> {
    transfer::write_export(&state.db(), &state.device_id, &PathBuf::from(path))
}

#[tauri::command]
pub fn import_preview(state: State<'_, AppState>, path: String) -> AppResult<ImportPreview> {
    let file = transfer::read_export(&PathBuf::from(path))?;
    transfer::preview(&state.db(), &file, &state.device_id)
}

#[tauri::command]
pub fn import_apply(state: State<'_, AppState>, path: String, mode: ImportMode) -> AppResult<Vec<TableChanges>> {
    let file = transfer::read_export(&PathBuf::from(path))?;
    let mut db = state.db();
    // Safety net: snapshot current progress before changing anything.
    let backup_dir = state.data_dir.join("backups");
    std::fs::create_dir_all(&backup_dir)?;
    let stamp = chrono::Utc::now().format("%Y-%m-%d-%H%M%S");
    transfer::write_export(&db, &state.device_id, &backup_dir.join(format!("pre-import-{stamp}.json")))?;
    transfer::apply(&mut db, &file, mode)
}
