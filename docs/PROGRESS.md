# Progress

Updated: 2026-10-07

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

## Not verified yet
- [ ] Clicking through the whole UI (app launched; needs a manual pass by the user)
- [ ] Quality with a 7B+ local model or a cloud model (only tested with qwen2.5-coder:1.5b — too weak to grade well)
- [ ] macOS (M5) build
- [ ] Keychain on the Mac / on Linux without GNOME Keyring

## Known issues
- Chinese PDFs may render missing glyphs: pdf.js CMaps/standard fonts aren't bundled yet
- Some outlines are noisy (e.g. Qt 4 book shows `toc.html` entries); front-matter entries appear as chapters
- Chapter start from running headers can be one page late (first page of a chapter often has no header)

## Next
1. User test pass → fix what feels wrong
2. Skill test → skill map → roadmap (Phase 4)
3. Spaced-repetition cards from mistakes + dashboard (Phase 5)
4. Native Anthropic/Gemini adapters, usage/cost view, embeddings + hybrid search
5. Packaging: Mac build, GitHub Actions for all OSes, sync folder (Phase 6)
