mod commands;
mod db;
mod error;
mod index;
mod library;
mod llm;
mod pdf;
mod quiz;
mod secrets;
mod transfer;

#[cfg(test)]
mod real_tests;

use std::collections::HashMap;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::Connection;
use tauri::Manager;

pub struct Inner {
    db: Mutex<Connection>,
    index: Mutex<Connection>,
    http: reqwest::Client,
    data_dir: PathBuf,
    device_id: String,
    tasks: Mutex<HashMap<String, tokio::task::AbortHandle>>,
}

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

impl Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl Inner {
    fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn index(&self) -> MutexGuard<'_, Connection> {
        self.index.lock().unwrap_or_else(|p| p.into_inner())
    }
}

fn data_dir(app: &tauri::App) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = app.path().app_data_dir()?;
    // Debug builds keep their own data so development can never damage real progress.
    let dir = if cfg!(debug_assertions) {
        let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        dir.with_file_name(format!("{name}.dev"))
    } else {
        dir
    };
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().level(log::LevelFilter::Info).build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = data_dir(app)?;
            if let Ok(res) = app.path().resource_dir() {
                pdf::set_library_dir(res.join("resources").join("pdfium"));
            }
            let db = db::open(&data_dir.join("bookshelf.sqlite"))?;
            let device_id = db::device_id(&db)?;
            if let Err(e) = db::daily_backup(&db, &data_dir.join("backups"), 14) {
                log::warn!("daily backup failed: {e}");
            }
            let index = index::open(&data_dir.join("index.sqlite"))?;
            log::info!("data dir: {}", data_dir.display());
            app.manage(AppState(Arc::new(Inner {
                db: Mutex::new(db),
                index: Mutex::new(index),
                http: llm::http_client(),
                data_dir,
                device_id,
                tasks: Mutex::new(HashMap::new()),
            })));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::set_library_root,
            commands::scan_library,
            commands::list_books,
            commands::add_books,
            commands::update_book,
            commands::remove_book,
            commands::read_book_file,
            commands::save_progress,
            commands::index_status,
            commands::index_book,
            commands::book_chapters,
            commands::search_book,
            commands::provider_presets,
            commands::providers_list,
            commands::provider_save,
            commands::provider_delete,
            commands::provider_models,
            commands::provider_test,
            commands::roles_get,
            commands::role_set,
            commands::ai_cancel,
            commands::quiz_generate,
            commands::quiz_grade,
            commands::quiz_finish,
            commands::attempts_list,
            commands::attempt_get,
            commands::attempt_delete,
            commands::export_progress,
            commands::import_preview,
            commands::import_apply,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
