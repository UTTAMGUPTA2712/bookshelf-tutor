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
use crate::{db, AppState};

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
    let path = library::path_of(&state.db(), &fingerprint)?;
    let s = state.inner().clone();
    blocking(move || {
        let prepared = index::prepare(&path, |page, total| {
            let _ = on_progress.send(IndexProgress { page, total });
        })?;
        let status = index::store(&mut s.index(), &fingerprint, &prepared)?;
        // Remember the page count for the library view.
        s.db().execute(
            "UPDATE books SET page_count = ?2 WHERE fingerprint = ?1 AND page_count IS NULL",
            rusqlite::params![fingerprint, status.page_count],
        )?;
        Ok(status)
    })
    .await
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
    Ok(TestResult { latency_ms: start.elapsed().as_millis(), json_ok, reply: text, usage })
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
    let (chapter, chunks) = {
        let idx = s.index();
        let chapter = index::chapters(&idx, &fingerprint)?
            .into_iter()
            .find(|c| c.idx == chapter_idx)
            .ok_or_else(|| AppError::NotFound("Chapter not found — open the book to index it first".into()))?;
        let chunks = index::chapter_chunks(&idx, &fingerprint, chapter_idx)?;
        (chapter, chunks)
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
                    llm::chat_json::<quiz::SummaryOut>(&http, &t, messages, &quiz::validate_summary, 800, &mut sink).await
                }
            })
            .await
            .ok()
            .map(|(reply, _)| reply.value)
        };
        let overall = quiz::to_overall(&a.questions, summary.as_ref());
        let db = s.db();
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
