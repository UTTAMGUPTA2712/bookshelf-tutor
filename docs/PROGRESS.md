# Progress

Updated: 2026-10-09

## Done — day 1 (Phase 1 + core of Phases 2–3)
- [x] Repo, Tauri v2 + React 19 + Vite + Tailwind 4 scaffold, pdfium fetch script
- [x] SQLite store (`bookshelf.sqlite`): migrations, device id, daily backups (14 kept), separate dev data dir
- [x] Library: folder scan, BLAKE3 fingerprints (cached by size+mtime), curated `books.json` (49 books: track/status/level)
- [x] Add books (copy into library or link in place, duplicate check by fingerprint), edit metadata, remove
- [x] Drag-and-drop PDFs onto the window
- [x] Reader: pdf.js viewer, remembers page, chapter sidebar, zoom, full-text search (FTS5)
- [x] Indexing: pdfium text + outline → chapters; fallback to "Chapter N"/"N.M" running headers; fixed sections last
      (all 49 real PDFs index in ~40 s total; 2 scanned books detected)
- [x] Providers: Ollama + OpenAI-compatible (OpenAI, Anthropic*, Gemini*, OpenRouter, Groq, Mistral, DeepSeek,
      Together, xAI, LM Studio, llama.cpp, custom) — live model lists, keys in OS keychain, "Try this model"
- [x] Roles (writing questions / grading) with up to 3-model fallback chains
- [x] Chapter quiz: grounded generation (questions must cite excerpts), MCQ/short/explain, streamed progress,
      background grading queue, detailed per-answer review, overall review, re-read page links, chapter marked done ≥70
- [x] Quiz history
- [x] Ollama model manager: hardware detection (RAM/cores/Apple Silicon/NVIDIA), curated catalog of 21 models
      (sizes verified against registry.ollama.ai on 2026-10-07) with fit badges, speed/grade-time estimates and
      recommendations; download with progress/cancel/resume, delete, any model by name (checked against the registry);
      thinking mode auto-disabled for thinking models
- [x] Export/import: versioned + checksummed JSON, preview, merge (newest wins) or replace, auto-backup before import
- [x] Safety: max-token limits, runaway-whitespace guard, Ollama uses half the cores by default, cancel requests
- [x] 18 Rust unit tests + 2 ignored real-data tests

\* via their OpenAI-compatible endpoints; native adapters still planned.

## Done — Phase 4 (skill test, skill map, roadmap) — 2026-10-09
- [x] Taxonomy `resources/taxonomy.json`: 4 tracks (Fundamentals 18 topics, Web/Node 9, C++ 11, Python/ML 13) with
      prerequisites, keywords and book tags
- [x] Chapter→topic mapping without an LLM (`skills.rs`): title keywords + FTS5 hit density vs. the whole book;
      covers/back matter/index letters ignored; tuned on the real library (`real_topic_mapping` test)
- [x] Chapter quizzes are tagged with up to 3 topics (`attempts.topics`, migration 2)
- [x] Adaptive skill test (`assessment.rs`): per topic medium → hard (if right) or easy (if wrong); topics whose
      prerequisites were failed (or skipped) are skipped; one model call writes easy/medium/hard per topic and the next
      topic is prefetched while you answer; optional written answers; "End test now"; resumable
- [x] Skill map page: mastery + confidence per topic, columns by prerequisite depth, last test summary
- [x] Roadmap page: weakest-first topological order, chapters from the best book on this PC (start-here/core and
      in-progress books first, never outdated), weekly plan at a chosen pace, "Find chapters" indexes candidate books
- [x] Synced `preferences` table (goal track, weekly pace); export format v2 (v1 files still import)
- [x] 27 Rust unit tests (+ `real_topic_mapping` ignored test)

## Not verified yet
- [ ] Clicking through the whole UI (app launched; needs a manual pass by the user)
- [ ] Quality with a 7B+ local model or a cloud model (only tested with qwen2.5-coder:1.5b — too weak to grade well)
- [ ] macOS (M5) build
- [ ] Keychain on the Mac / on Linux without GNOME Keyring

## Deliberate differences from PLAN.md (don't "fix" these without asking)
- Repo is `~/Desktop/Practice/bookshelf-tutor` (not `books/tutor/`); books stay in `~/Desktop/Practice/books`, not in git.
- No localStorage at all — every bit of learner data is in `bookshelf.sqlite` via Rust (user's decision).
- IPC types are hand-written in `src/ipc.ts` (no tauri-specta); keep them in sync with `commands.rs` by hand.
- One `index.sqlite` for all books (not one DB per book); it is rebuildable and never exported.
- Plain `Mutex<Connection>` (no r2d2 pool) and a small hand-written migration runner in `db.rs`.
- Anthropic and Gemini go through their OpenAI-compatible endpoints for now.
- No Zustand/Zod yet; TanStack Query + component state is enough so far.
- Skill map is **computed from attempts** (no `skills` table): merge can't conflict, nothing to recompute after import.
- One `taxonomy.json` (not a `taxonomy/` folder). Chapter→topic links come from keywords + FTS, not `books.json`/LLM.
- Skill-test questions come from the model's general knowledge (no book excerpts), graded against reference answer +
  key points. Chapter quizzes stay grounded in the book text.
- Roadmap never indexes books by itself; the "Find chapters" button does it on request (CPU).
- `pdfium-render` 0.8 with `sync` feature, `reqwest` 0.12, `rusqlite` 0.37, `keyring` 3, `sysinfo` 0.37 —
  pinned because newer majors changed APIs or need a newer Rust than this machine's 1.92.

## The user
- Machines: Linux desktop (15 GB RAM, 20 cores, no GPU) and a MacBook M5. Repo is private.
- CPU spikes upset them: never run local models without asking; build with `nice -n 19`.
- Prefers being shown a plan before big features, then "move ahead".

## Known issues
- Chinese PDFs may render missing glyphs: pdf.js CMaps/standard fonts aren't bundled yet
- Some outlines are noisy (e.g. Qt 4 book shows `toc.html` entries); front-matter entries appear as chapters
- Chapter start from running headers can be one page late (first page of a chapter often has no header)

## Not verified yet (Phase 4)
- [ ] A full skill test with a real model (needs a 7B+ local model or a cloud key; not run here — CPU rule)
- [ ] Skills/Roadmap pages clicked through in the running app

## Next
1. User test pass (incl. Phase 4) → fix what feels wrong
2. Spaced-repetition cards from mistakes + dashboard (Phase 5)
4. Native Anthropic/Gemini adapters, usage/cost view, embeddings + hybrid search
5. Packaging: Mac build, GitHub Actions for all OSes, sync folder (Phase 6)
