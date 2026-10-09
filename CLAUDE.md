# Bookshelf Tutor

Local-first Tauri v2 desktop app (Rust + React/TS) that quizzes the user on chapters of their own PDF books.
Full design: `docs/PLAN.md`. Current status and next steps: `docs/PROGRESS.md` — read both before starting work.

## Commands
- `scripts/fetch-pdfium.sh` — download the pdfium library for this OS (required once per machine)
- `npm run tauri dev` — run the app (debug data dir: `<appData>/com.bookshelf.tutor.dev`)
- `npm run tauri build` — installers for the current OS
- `cd src-tauri && cargo test` — unit tests
- `cd src-tauri && BOOKS_DIR=../../books cargo test --release real_scan -- --ignored --nocapture` — index every real PDF
- `npx tsc --noEmit` — type-check the frontend

## This machine's limits (important)
- Run builds with `nice -n 19`. Cargo is capped at 6 jobs in `src-tauri/.cargo/config.toml`; don't remove it.
- **Never run a local LLM (Ollama) without asking the user first** — it pins the CPU. `real_quiz_with_ollama` is such a test.
- Every LLM call must pass a max-token limit; Ollama calls default to half the CPU cores (`num_thread`).

## Layout
- `src-tauri/src/` — `db` (SQLite + migrations), `library` (scan, fingerprints, add/remove), `pdf` (pdfium, chapter
  detection), `index` (chunks + FTS5), `llm` (providers, roles, streaming, JSON repair), `quiz` (prompts, grounding,
  grading), `assessment` (adaptive skill test), `skills` (taxonomy, chapter→topic mapping, skill map, roadmap),
  `transfer` (export/import/merge), `secrets` (OS keychain), `commands` (thin IPC layer)
- `src-tauri/resources/` — `books.json` (curated metadata for known books), `providers.json` (provider presets),
  `taxonomy.json` (topic graphs per track; prereqs must be listed before the topics that need them)
- `src/ipc.ts` — typed wrappers for every Rust command; keep in sync with `commands.rs`
- `src/pages/` — Library, Reader, Quiz, History, Skills, Assessment, Roadmap, Settings

## Rules
- All learner data lives in `bookshelf.sqlite` (Rust). The frontend stores nothing (no localStorage).
- Device-local data (file paths, `device` table, API keys) is never exported.
- Never edit a shipped migration in `db.rs`; append a new one. Bump `INDEX_VERSION` when extraction/chunking changes.
- Synced records carry `updated_at` + `device_id` + `deleted`; merge is newest-`updated_at`-wins.
- Never hold a DB mutex across `.await`.
- No shelling out to OS tools — everything must work on Linux, macOS (M5) and Windows.
- API keys only via `secrets.rs`; they must never reach the WebView, logs or exports.
