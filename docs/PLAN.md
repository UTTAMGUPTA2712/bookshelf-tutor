# Bookshelf Tutor — Project Plan

> Implementation notes and deliberate deviations from this plan are tracked in `docs/PROGRESS.md`.

A local-first desktop app that turns this folder of programming books into a guided learning system:
it figures out what you already know, builds a roadmap through the right books and chapters, quizzes
you on what you read using the book's own text, gives a thorough AI review of every answer, and
schedules what you got wrong for spaced review. Progress lives in a local SQLite database owned by
the Rust core and moves between PCs (Linux desktop ⇄ MacBook M5) with an export file. Any LLM
provider (local or cloud) can be plugged in with an API key.

---

## 1. Goals and non-goals

### Goals
1. **Know where you stand**: an adaptive skill test that produces a per-topic skill map.
2. **Know what to read next**: a personal roadmap that maps weak topics to specific books and chapters in this folder.
3. **Learn actively**: chapter quizzes generated *from the book text*, graded with a detailed review (what was right, what was missing, misconceptions, pages to re-read).
4. **Retain it**: spaced-repetition cards created automatically from mistakes.
5. **Provider freedom**: any LLM provider, any model, picked from a live model list; local Ollama works fully offline.
6. **Portable progress**: export/import (with merge) so the same learner can study on several PCs.
7. **Fast**: heavy work (PDF parsing, indexing, search, network, crypto) in Rust.

### Non-goals (v1)
- Running user code (code answers are reviewed by the LLM, not executed).
- OCR for scanned PDFs without a text layer (detected and flagged instead).
- Cloud accounts, a hosted sync server, or multi-user support.
- Mobile apps.
- Editing or annotating the PDFs themselves.

---

## 2. Environment (verified on this PC)

| Item | Status |
|---|---|
| Rust | 1.92.0 |
| Node | v20.19.1 (npm) |
| Tauri v2 Linux deps | `libwebkit2gtk-4.1-dev` 2.52.6, `librsvg2-dev`, `libssl-dev`, `build-essential` installed |
| Ollama | Running on `localhost:11434`; has `qwen2.5-coder:1.5b`, `nomic-embed-text` |
| Hardware | 15 GB RAM, 20 cores, no NVIDIA GPU (CPU inference: 7–8B models workable, 14B tight) |
| Library | ~50 PDFs, 442 MB, in the parent folder (`../`) |

**Second machine: MacBook M5** (Apple Silicon, `aarch64-apple-darwin`). Ollama runs on the GPU via
Metal with unified memory, so it will handle bigger/faster local models than this PC; the model
manager's fit badges are computed per machine. Needs Xcode Command Line Tools, Rust and Node to build.

---

## 3. Architecture

```
┌──────────────────────────── Tauri window (WebView) ────────────────────────────┐
│ React + TypeScript UI                                                          │
│  Library · Reader (pdf.js) · Skill test · Quiz/Review · Cards · Dashboard ·    │
│  Settings (providers, models, export/import)                                   │
│                                                                                │
│  Server state via TanStack Query (cache only; no data stored in the WebView)   │
└───────────────▲───────────────────────────────────────────────┬────────────────┘
                │ events / Channels (streaming tokens, progress) │ invoke()
┌───────────────┴───────────────────────────────────────────────▼────────────────┐
│ Rust core (src-tauri)                                                          │
│  library   – scan folder, fingerprint (BLAKE3), metadata, watch for changes    │
│  pdf       – text + outline extraction (pdfium), page labels, text-layer check │
│  index     – chunking, SQLite+FTS5 store, embeddings, hybrid search            │
│  llm       – Provider trait, adapters, registry, model lists, fallback chains, │
│              structured-output validation & repair, usage/cost tracking        │
│  models    – Ollama pull/delete/progress, curated catalog, hardware fit        │
│  secrets   – API keys in OS keychain; passphrase-encrypted key export (age)    │
│  sysinfo   – RAM/CPU/GPU detection                                             │
│  store     – bookshelf.sqlite: progress, attempts, skills, cards, roadmap,     │
│              migrations, backups; FSRS scheduling; roadmap computation         │
│  transfer  – export/import + merge, sync-folder watcher                        │
└────────────────────────────────────────────────────────────────────────────────┘
        │                         │                               │
   PDFs on disk     App data dir (bookshelf.sqlite,        LLM providers (HTTP)
                    index DBs, backups, logs)
```

### Responsibility split
- **Rust owns**: all data (SQLite), files, PDFs, indexing, search, every network call to an LLM,
  secrets, hardware detection, spaced-repetition scheduling, roadmap computation, export/import/merge.
  API keys never enter the WebView.
- **Frontend owns**: UI only. It reads and writes through typed commands and keeps a short-lived
  cache (TanStack Query); closing the window loses nothing.

---

## 4. Tech stack

### Frontend
| Concern | Choice |
|---|---|
| Framework | React 19 + TypeScript + Vite |
| Routing | React Router |
| Data access | TanStack Query over typed IPC (`tauri-specta`-generated bindings); Zustand for UI-only state (open panels, current quiz screen) |
| UI | Tailwind CSS + shadcn/ui (Radix), light/dark theme |
| PDF viewer | `pdfjs-dist`, file served through Tauri's asset protocol |
| Markdown in reviews | `react-markdown` + `remark-gfm` + syntax highlighting |
| Code answers | CodeMirror 6 |
| Charts | Recharts |
| Tests | Vitest (components, IPC wrappers) |

### Rust (src-tauri)
| Concern | Crate |
|---|---|
| App shell | `tauri` 2, plugins: `dialog`, `fs`, `opener`, `updater` (later) |
| Async + HTTP | `tokio`, `reqwest` (rustls, streaming), `futures`, `eventsource-stream` |
| Serialization | `serde`, `serde_json`, `schemars` (JSON Schemas for structured output), `jsonschema` (validation) |
| PDF | `pdfium-render` (bundled pdfium binary per OS) |
| Storage/search | `rusqlite` (bundled, FTS5, WAL), `r2d2_sqlite` (pool), `rusqlite_migration` |
| Spaced repetition | `rs-fsrs` (FSRS algorithm) |
| IPC types | `specta` + `tauri-specta` (generates the TypeScript bindings) |
| IDs/time | `uuid` (v7), `chrono` |
| Hashing | `blake3` |
| Secrets | `keyring` (Secret Service / Credential Manager / Keychain), `age` (passphrase encryption) |
| System info | `sysinfo` |
| Files | `walkdir`, `notify` (folder watcher) |
| Errors/logging | `thiserror`, `tracing`, `tracing-subscriber` (rolling log file) |
| Tests | built-in tests, `wiremock` (fake provider servers), `insta` (snapshots) |

---

## 5. Repository layout

```
books/
├── *.pdf …                         # the library (unchanged)
└── tutor/
    ├── PLAN.md                     # this file
    ├── package.json, vite.config.ts, tsconfig.json, tailwind.config.ts
    ├── src/
    │   ├── app/                    # routes, layout, providers
    │   ├── features/
    │   │   ├── library/            # book list, tracks, filters
    │   │   ├── reader/             # pdf.js viewer, chapter sidebar, "Quiz me"
    │   │   ├── assessment/         # skill test flow
    │   │   ├── quiz/               # question UI, answers, review screen
    │   │   ├── cards/              # spaced repetition
    │   │   ├── roadmap/
    │   │   ├── dashboard/
    │   │   └── settings/           # providers, models, roles, transfer, backups
    │   ├── ipc/                    # generated bindings (tauri-specta) + query hooks
    │   └── lib/
    ├── src-tauri/
    │   ├── Cargo.toml, tauri.conf.json, capabilities/
    │   ├── src/
    │   │   ├── main.rs, lib.rs
    │   │   ├── commands/           # thin #[tauri::command] layer
    │   │   ├── library/            # scan, fingerprint, metadata
    │   │   ├── pdf/                # extract text, outline, page labels
    │   │   ├── index/              # chunker, sqlite store, search, embed jobs
    │   │   ├── llm/
    │   │   │   ├── provider.rs     # Provider trait + common types
    │   │   │   ├── openai_compat.rs
    │   │   │   ├── anthropic.rs
    │   │   │   ├── gemini.rs
    │   │   │   ├── ollama.rs
    │   │   │   ├── registry.rs     # provider definitions, instances
    │   │   │   ├── router.rs       # role → model, fallback chain
    │   │   │   ├── structured.rs   # JSON schema, validation, repair loop
    │   │   │   └── prompts/        # versioned prompt templates
    │   │   ├── models/             # Ollama catalog, pull/delete, hardware fit
    │   │   ├── store/              # bookshelf.sqlite: schema, migrations, repos, backups, FSRS, roadmap
    │   │   ├── secrets.rs
    │   │   ├── sysinfo.rs
    │   │   └── transfer.rs
    │   └── resources/
    │       ├── providers.json      # built-in provider definitions
    │       ├── ollama-catalog.json # curated downloadable models
    │       ├── books.json          # curated metadata for known books
    │       ├── taxonomy/           # topic graphs per track
    │       └── pdfium/             # platform binaries
    └── .github/workflows/release.yml
```

---

## 6. Data model

### 6.1 Where data lives

| Data | Location | Exported? |
|---|---|---|
All learner data lives in **one SQLite database owned by Rust**; the frontend never stores data itself
(no `localStorage`). `<appData>` is the OS app-data dir: `~/.local/share/com.bookshelf.tutor/` on
Linux, `~/Library/Application Support/com.bookshelf.tutor/` on macOS.

| Data | Location | Exported? |
|---|---|---|
| Learner progress, reading positions, quiz attempts (full reviews + raw model output), skill map, roadmap, cards, provider *configs* (no keys), role assignments, preferences, AI usage log | `<appData>/bookshelf.sqlite` | **Yes** |
| Device ID, library folder path, per-device role overrides, window state | `device` table in the same DB | No (per PC) |
| API keys | OS keychain | No (optional separate encrypted export) |
| Extracted text, chunks, embeddings | `<appData>/index/<fingerprint>.sqlite` | No (rebuildable) |
| Automatic backups | `<appData>/backups/` | — |
| Logs | `<appData>/logs/` | No |

### 6.2 Database rules
- `rusqlite` (bundled SQLite) in **WAL mode**, one connection pool owned by Rust; all reads and writes go
  through typed Tauri commands.
- **Schema migrations** with `rusqlite_migration`, run on startup; the schema version is stored in the DB.
- **No size limit worth worrying about**: full quiz reviews and raw model output are kept forever.
- **Automatic backups**: a SQLite online backup on startup (at most once a day); the last 14 are kept;
  "Restore backup" in Settings.
- **Dev and installed builds use different data dirs** (debug builds use `com.bookshelf.tutor.dev`) so
  development can never damage real progress.
- **Never put the database in a synced folder** (Dropbox/Drive can corrupt a live SQLite file); moving
  data between PCs goes through export files (§7).
- The app identifier `com.bookshelf.tutor` must never change, because it decides where the data lives.

### 6.3 Record shapes
Stored as SQLite tables (one per record type, JSON columns for nested parts); the same shapes are
used over IPC and in the export file. Shown in TypeScript for readability; Rust has matching `serde`
structs, and TypeScript types are generated from them with `specta`/`tauri-specta` so the two never drift.

```ts
type ID = string;           // uuid v7 (time-ordered)
type ISO = string;          // timestamp

interface Meta { id: ID; createdAt: ISO; updatedAt: ISO; deviceId: string; deleted?: boolean }

interface BookProgress extends Meta {
  fingerprint: string;      // BLAKE3 of file contents
  title: string;
  status: 'not-started' | 'reading' | 'finished' | 'skipped';
  lastPage: number;
  chaptersDone: string[];   // chapter ids (stable: outline path + start page)
  minutesRead: number;
}

interface QuizAttempt extends Meta {           // append-only
  kind: 'chapter' | 'assessment' | 'free';
  bookFingerprint?: string; chapterId?: string; topics: string[];
  model: { provider: string; model: string; promptVersion: string };
  questions: Array<{
    id: ID; type: 'mcq' | 'short' | 'explain' | 'code' | 'true-false';
    prompt: string; choices?: string[]; sourcePages: number[];
    answer: string;
    grade: { score: number; verdict: 'correct' | 'partial' | 'incorrect';
             correct: string[]; missing: string[]; misconceptions: string[];
             feedback: string; rereadPages: number[] };
  }>;
  overall: { score: number; summary: string; strengths: string[]; focusNext: string[] };
  raw?: string;             // full model output; pruned for old attempts
}

interface TopicSkill extends Meta {
  topicId: string;          // from taxonomy, e.g. "fund.hashing"
  mastery: number;          // 0–100
  confidence: number;       // 0–1, grows with evidence
  evidence: ID[];           // attempt ids
}

interface Card extends Meta {
  front: string; back: string; topicId?: string;
  source: { attemptId?: ID; bookFingerprint?: string; page?: number };
  fsrs: FsrsState;          // due, stability, difficulty, reps, lapses, state
}

interface ProviderConfig extends Meta {        // no secrets here
  providerType: string;     // "openai" | "anthropic" | "gemini" | "ollama" | "openrouter" | "custom" …
  label: string; baseUrl: string;
  enabledModels: string[];  // chosen from the live list
  hasKey: boolean;          // whether this PC's keychain has a key
}

interface RoleAssignment extends Meta {
  role: 'question' | 'grade' | 'chat' | 'embed';
  chain: Array<{ providerId: ID; model: string }>;   // first = primary, rest = fallbacks
}
```

Streaks, totals and charts are **computed** from attempts and progress, never stored, so merging never
creates conflicting counters.

---

## 7. Moving progress between PCs

### 7.1 Export file
`bookshelf-progress-<device>-<yyyy-mm-dd>.json`

```json
{
  "format": "bookshelf-tutor-export",
  "schemaVersion": 1,
  "appVersion": "0.1.0",
  "exportedAt": "2026-10-07T10:00:00Z",
  "deviceId": "…",
  "data": { "books": [], "attempts": [], "skills": [], "cards": [], "roadmap": {},
            "providers": [], "roles": [], "preferences": {} },
  "checksum": "blake3:…"
}
```
- Written by Rust (from the database) through a native Save dialog; validated against `serde` types on import.
- `schemaVersion` + an ordered **migration chain** means older exports always import into newer apps.
- The checksum catches truncated or corrupted files.

### 7.2 Import modes
- **Merge (default):** union by `id`. For the same id, the newer `updatedAt` wins (ties broken by
  `deviceId`). Tombstones (`deleted: true`) win over older live records. Append-only attempts are
  simply combined. Card FSRS state takes the record with the most recent review. Skills are
  **recomputed** from the merged evidence rather than picked from one side.
- **Replace:** wipe local learner data and load the file (an automatic backup export is written first).
- A **preview** shows what changes before applying: "+34 attempts, 12 cards updated, 2 books newer".

### 7.3 Books on another PC
Progress points to books by **fingerprint**, not path. On import, fingerprints are matched against the
local library. Unmatched books fall back to normalized filename + size. Anything still missing is
listed as "book not found on this PC" and keeps its progress.

### 7.4 Sync folder (optional, Phase 6)
Pick a folder (Google Drive, Dropbox, Syncthing, USB). The app exports there when it closes and, when it
opens, offers to merge if it finds a newer export from another device. It's only a folder of JSON
files, so there's no server.

### 7.5 Model choices across PCs with different hardware/OS
Role chains are exported, but a model chosen on one PC may not exist on another (e.g. a 14B Ollama
model on a 32 GB Mac, nothing installed on a laptop). So:
- Role chains are **shared by default**, with an optional **per-device override** stored in
  the `device` table (never exported).
- After import and on every startup, each link in each chain is checked against this PC (provider has
  a key? Ollama model installed? fits RAM?). Unavailable links are skipped and shown with a fix button
  ("Pull model", "Paste key", "Use a device override").

### 7.6 API keys across PCs
Provider configs travel in the export; keys do not. On a new PC each provider shows **"Paste key"**.
An optional **Export keys** writes a separate file encrypted with a passphrase (`age`/scrypt).

---

## 8. LLM provider system

### 8.1 Built-in providers (`resources/providers.json`)

| Provider | Adapter | Default base URL | List models | Embeddings |
|---|---|---|---|---|
| Ollama | ollama | `http://localhost:11434` | `GET /api/tags` | yes |
| LM Studio | openai_compat | `http://localhost:1234/v1` | `GET /models` | yes |
| llama.cpp server | openai_compat | `http://localhost:8080/v1` | `GET /models` | yes |
| OpenAI | openai_compat | `https://api.openai.com/v1` | `GET /models` | yes |
| Anthropic | anthropic | `https://api.anthropic.com/v1` | `GET /models` | no |
| Google Gemini | gemini | `https://generativelanguage.googleapis.com/v1beta` | `GET /models` | yes |
| OpenRouter | openai_compat | `https://openrouter.ai/api/v1` | `GET /models` (has pricing + context) | no |
| Groq | openai_compat | `https://api.groq.com/openai/v1` | `GET /models` | no |
| Mistral | openai_compat | `https://api.mistral.ai/v1` | `GET /models` | yes |
| DeepSeek | openai_compat | `https://api.deepseek.com` | `GET /models` | no |
| Together | openai_compat | `https://api.together.xyz/v1` | `GET /models` | yes |
| xAI | openai_compat | `https://api.x.ai/v1` | `GET /models` | no |
| Custom | openai_compat | user-entered | `GET /models` | user-set |

Each definition also records auth style (bearer / header / query), whether it supports native JSON
schema output, a model-ID filter (to hide non-chat models such as TTS or image models), and a
docs/pricing link. Adding a provider usually means adding one JSON entry.

### 8.2 Provider trait (Rust)

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;          // id, context, pricing?, caps
    async fn chat(&self, req: ChatRequest, sink: TokenSink) -> Result<ChatResponse>; // streaming
    async fn chat_json(&self, req: ChatRequest, schema: &Schema) -> Result<serde_json::Value>;
    async fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>>;
    fn capabilities(&self) -> Capabilities;                          // json_schema, embeddings, streaming
}
```

### 8.3 Adding a provider (UI flow)
1. **Add provider** → pick from the list (or Custom), paste the API key (local providers need none).
2. **Test & fetch models**: Rust saves the key to the keychain, calls the list endpoint, and returns
   models with whatever metadata the provider gives (context length, price per token, capabilities).
3. Search/filter the list and tick the models you want to use.
4. **Try this model**: runs one sample question generation and one grading against a fixed passage,
   then shows latency, tokens, estimated cost, and whether the JSON came back valid.

### 8.4 Roles and fallback chains
Roles: **question** (writing quizzes), **grade** (reviews), **chat** (follow-up questions),
**embed** (search index). Each role has an ordered chain of `provider + model`. On a timeout,
429 rate limit, 5xx error, auth failure, or invalid JSON after repair, the router moves to the next
link and records which model actually answered.

### 8.5 Structured output, built to work with any model
- Every AI task has a JSON Schema generated from Rust types (`schemars`).
- Where the provider supports it, the schema is sent natively (OpenAI `response_format`, Ollama
  `format`, Gemini `responseSchema`, Anthropic tool use).
- Otherwise the schema goes in the prompt, and the reply is parsed by stripping code fences, extracting
  the first JSON object, and validating it.
- **Repair loop:** on a validation error, the model is sent the error and asked to fix its output
  (max 2 tries), then the router falls back to the next model.
- Prompts are **versioned** (`question.v1`, `grade.v1`), and the version is stored with each attempt so
  results stay comparable as prompts improve.

### 8.6 Usage and cost
Every call records provider, model, role, input/output tokens and latency. The dashboard shows usage
per provider, plus estimated cost where pricing is known. An optional **daily spend cap** per provider
blocks cloud calls past the limit and falls back to local models.

### 8.7 Privacy notice
When a cloud provider is used, **book excerpts and your answers are sent to that provider**. The
provider screen says this plainly, and a per-role "local only" switch is available.

---

## 9. Local model manager (Ollama)

- **Installed** tab: from `/api/tags` + `/api/show`, showing size, parameter count, quantization,
  context length, and a delete button (`/api/delete`).
- **Download** tab: a curated `ollama-catalog.json` (Ollama has no public catalog API). Each entry has
  name, tags/sizes, download size, RAM needed, what it's good for (questions / grading / code / embedding),
  and a quality note.
  - **Fit badge** from `sysinfo`: ✅ fits (needs < 50% of free RAM), ⚠️ tight, ❌ too big.
  - **Speed estimate** from parameter count and core count (calibrated by "Try this model").
  - **Any model**: a text field accepting any `name:tag` from ollama.com/library.
- **Pull** streams progress events (`/api/pull`) into a progress bar; downloads can be cancelled and resumed.
- **Ollama missing or stopped:** a setup screen with install instructions per OS and a "Retry" button.
- **Starting catalog** (to verify at build time): qwen2.5 3B/7B/14B, qwen2.5-coder 7B, llama3.1 8B,
  llama3.2 3B, gemma2/gemma3 small sizes, mistral 7B, phi-family small models, deepseek-r1 distills 7B/8B;
  embedding models: nomic-embed-text, mxbai-embed-large, bge-m3.

---

## 10. Library and PDF pipeline

### 10.1 Scan
- On first run, choose the library folder (dev default: `..`).
- Rust walks the folder for `*.pdf`, and computes a BLAKE3 fingerprint for each file. The fingerprint is
  cached by path + size + mtime, so only new or changed files are rehashed.
- Each book is matched against `books.json` (curated metadata for the books already here):
  clean title, author, edition, year, **track** (Fundamentals / Web-Node / C++-Systems / ML / Specialist),
  **level** (beginner → advanced), **status** (`start-here`, `core`, `reference`, `later`, `outdated`),
  and topic tags. Unknown books get their title from PDF metadata or the filename.
- The `notify` watcher picks up books that are added or removed while the app is open.

### 10.1a Adding books (upload)
- **Add book** button, drag-and-drop onto the window, or File → Add books (multi-select).
- Two modes, chosen per upload (default remembered):
  - **Copy into library (default):** the PDF is copied into the library folder, so the folder always
    holds the complete collection and can be synced to the other PC as plain files.
  - **Link in place:** the app remembers the file where it is, without copying.
- **Duplicate check** by fingerprint: "You already have this book" (offers to link the new location).
- **Metadata**: from `books.json` if known; otherwise from PDF metadata + filename, then the LLM
  suggests title, author, edition, track, level, status and topics from the title and table of contents.
  Shown as an editable form before saving.
- The new book is indexed in the background straight away (extract → chunk → FTS5 → embeddings).
- **Remove book**: removes it from the library (and optionally deletes the copied file). Its progress
  is kept unless "also delete progress" is ticked.
- Formats: PDF in v1; EPUB later (text extraction is easier than PDF, the reader needs an EPUB viewer).
- **On the other PC**: exported progress includes the book's metadata but not the file. Books missing on
  that PC show "not on this PC — locate file / add". Keeping the library folder in Syncthing / Drive /
  iCloud Drive is safe (unlike the database) and makes uploads appear on both machines.

### 10.2 Extract (background job per book, with resumable progress events)
- `pdfium-render` gets per-page text, the **outline/bookmarks**, and **page labels** (so "page 213"
  matches the printed page number).
- **Chapters**: from the outline. If there's no outline, look for "Chapter N" headings, or as a last
  resort use 20-page sections. Chapter ranges can be edited by hand in the reader.
- **Text-layer check**: if most pages have no text, the book is flagged "scanned — quizzes unavailable".
- Mixed-language books (the Chinese titles) are detected and tagged; quizzes follow the book's language
  or the chosen UI language.

### 10.3 Index (`<appData>/index/<fingerprint>.sqlite`)
- Chunks of about 800 tokens with 15% overlap that never cross chapter boundaries. Each chunk stores
  `chapter_id`, `page_start`, `page_end` and the text.
- **FTS5** full-text index (BM25), which works with no embedding model at all.
- **Embeddings** (when an embed role is set) are stored as `f32` blobs keyed by the embedding model id.
  Changing the model triggers re-embedding in the background.
- **Hybrid search**: BM25 + cosine similarity, merged with reciprocal rank fusion, limited to a chapter
  or book. Brute-force cosine over one book (a few thousand chunks) takes under 10 ms in Rust.
- Books are indexed **on demand**, when first opened or quizzed, plus an optional "index whole library" button.

---

## 11. Learning features

### 11.1 Onboarding
1. Choose the library folder.
2. Choose a goal track (all four are supported; this only sets the starting roadmap).
3. Set up AI: detect Ollama, suggest a model that fits this PC, or add a cloud provider.
4. Take the skill test (it can be skipped and taken later).

### 11.2 Topic taxonomy (`resources/taxonomy/*.json`)
For each track, a graph of about 30–60 topics with prerequisites, e.g. Fundamentals:
`complexity → arrays → hashing → recursion → sorting → trees → graphs → dynamic-programming`;
`memory-model → processes → concurrency → networking`; `oop → design-patterns`. Each topic links to
book chapters. Chapter→topic links come from `books.json` for known books and are suggested by the LLM
for unknown ones (cached in the index DB).

### 11.3 Skill test (adaptive)
- 2–3 questions per topic in the chosen track, starting at medium difficulty, going **up after a correct
  answer and down after a wrong one**. Topics whose prerequisites were failed are skipped.
- Mixed question types: multiple choice, short answer, "explain in your own words", code reading/writing.
- Multiple choice is graded locally; free-text answers are graded by the **grade** role.
- Result: a **skill map** (mastery 0–100 + confidence per topic), shown as a topic graph colored
  red → green, plus a written summary of strengths and gaps.

### 11.4 Roadmap
- A topological walk over the topic graph, picking the **weakest topics whose prerequisites are met**.
- Each step points to a specific `book → chapters`, preferring `start-here`/`core` books and books
  already in progress; `outdated` books are never chosen.
- Shown as a weekly plan with a target pace (pages or chapters per week, chosen by you), and
  recalculated after every quiz.

### 11.5 Chapter quiz and thorough review
1. In the reader, **Quiz me** on the current chapter: choose the number of questions (5/10/15),
   difficulty, and types.
2. **Generate** (question role): Rust retrieves the chapter's most important chunks (hybrid search on
   the chapter's own headings + key terms; long chapters are covered section by section). The model
   returns questions with a hidden reference answer, a grading rubric, and **source chunk ids**.
   - **Grounding check:** each question's cited chunks must exist and belong to the chapter.
     Questions that fail are regenerated or dropped.
3. **Answer**: one question per screen, with a CodeMirror editor for code, an "I don't know" button,
   and optional confidence (sure / unsure).
4. **Grade** (grade role, streamed): each answer is graded against the rubric + source passage and
   returns score, verdict, what was correct, what was missing, misconceptions, a short explanation,
   and **pages to re-read** (clickable, they open the reader at that page).
5. **Chapter review**: overall score, strengths, focus areas, and suggested next step (re-read pages X–Y /
   move on / review cards).
6. **Follow-up chat** (chat role) about any question, grounded in the same passages.
7. **Dispute grade**: re-grade one answer with the next model in the chain, or mark it as "I was right".
8. Results update the skill map, roadmap, and book progress; mistakes become cards.

### 11.6 Spaced-repetition cards
- Created automatically from missed points and misconceptions (one atomic fact per card), plus
  manual cards from any selected text in the reader.
- Scheduled with **FSRS** (`rs-fsrs`, in Rust) and rated Again / Hard / Good / Easy.
- A daily review queue with a limit, shown on the dashboard.
- Optional "AI check": type the answer instead of flipping the card, and the grade role judges it.

### 11.7 Dashboard
Today's queue (cards due + next roadmap step), streak, skill map, per-book progress, score trend,
weakest topics, reading time, and AI usage/cost per provider.

### 11.8 Reader
pdf.js viewer with a chapter sidebar, remembered position per book, page-label navigation, text
search (FTS5 through Rust), "make card from selection", "ask about this page" (chat role), and
**Quiz me**.

---

## 12. IPC surface (Rust commands)

| Area | Commands / events |
|---|---|
| Library | `library_set_root`, `library_scan` → `Book[]`, event `library://changed` |
| PDF/Index | `book_outline`, `book_chapters_update`, `index_book` (Channel: progress), `index_status`, `search(book, chapter?, query, k)` |
| Providers | `providers_builtin`, `provider_save_key`, `provider_delete_key`, `provider_list_models`, `provider_try_model` |
| AI tasks | `ai_generate_questions`, `ai_grade_answer` (Channel: tokens), `ai_chapter_summary`, `ai_chat` (Channel), `ai_cancel(request_id)`, `ai_map_chapters_to_topics` |
| Ollama | `ollama_status`, `ollama_installed`, `ollama_catalog`, `ollama_pull` (Channel: progress), `ollama_cancel_pull`, `ollama_delete` |
| System | `system_info` (RAM total/free, cores, GPU if detectable) |
| Transfer | `export_write(json)`, `import_read()` (open dialog), `keys_export(passphrase)`, `keys_import(passphrase)`, `sync_folder_set`, `sync_folder_check` |
| Usage | `usage_summary(range)` |

All long-running commands take a `request_id` and can be cancelled. Errors cross IPC as a typed enum
(`Network`, `Auth`, `RateLimited`, `InvalidOutput`, `NotFound`, `Cancelled`, …) so the UI can say what
actually went wrong.

---

## 13. Security

- API keys: only in the OS keychain. If no keychain is available (some minimal Linux setups),
  fall back to an `age`-encrypted file unlocked with a passphrase per session.
- The WebView never sees keys; Tauri capabilities allow only the needed commands; CSP blocks remote scripts.
- File access is limited to the chosen library folder and the app data dir (asset protocol scope).
- No telemetry. Logs never contain keys or full prompts unless debug logging is switched on.

---

## 14. Performance targets

| Operation | Target |
|---|---|
| Cold start to usable UI | < 1.5 s |
| Rescan of 50-book library (cached) | < 300 ms; first full hash < 3 s |
| Text extraction of a 1,000-page book | < 15 s, in the background |
| Hybrid search within a book | < 30 ms |
| Embedding a whole book on CPU (nomic-embed-text) | minutes, background, resumable, chapter-first priority |
| First streamed grading token (local 7B) | ~1–3 s; full grade 10–30 s |
| Installer size | ~15–25 MB including pdfium |

---

## 15. Testing

- **Rust**: unit tests for chunking, chapter detection, fingerprinting, hybrid search, and JSON
  extract/repair; `wiremock` fakes for each provider adapter (model listing, streaming, errors, 429s);
  snapshot tests for prompt rendering.
- **Rust store**: merge (property tests with `proptest`: merge is idempotent and order-independent),
  DB migrations (every old schema upgrades cleanly), export-file migrations, FSRS integration,
  roadmap computation, backup/restore.
- **Recorded model outputs**: real responses from several models (including messy small-model outputs)
  replayed through the parser to make sure they are handled.
- **TypeScript**: Vitest for UI components and IPC hooks.
- **Manual checklist per release**: fresh install, onboarding, quiz on 3 different books, export on PC A →
  import/merge on PC B, Ollama offline, bad API key, scanned PDF.

---

## 16. Packaging and other PCs

- `npm run tauri build` → Linux `.AppImage` + `.deb` (built here).
- **MacBook M5: build the app on the Mac itself** (`npm run tauri build` → `.app` + `.dmg` for
  `aarch64-apple-darwin`). Apps you build yourself aren't quarantined, so there's no Gatekeeper warning
  and no Apple Developer account needed.
- GitHub Actions matrix (`ubuntu`, `windows`, `macos`) using `tauri-action` → `.msi`/`.exe`,
  `.dmg`, AppImage/deb, with the right pdfium binary bundled per OS.
- Optional later: Tauri updater plugin with signed releases.

### Cross-platform notes
| | Linux | Windows 10/11 | macOS |
|---|---|---|---|
| Web engine | WebKitGTK | WebView2 (Chromium; installer bootstraps it on Win10 if missing) | WKWebView (Safari) |
| Build where | here or CI (`ubuntu-22.04` for older-glibc compatibility) | CI `windows-latest` | on the Mac itself, or CI `macos-latest` (`aarch64-apple-darwin` for the M5) |
| Keys stored in | Secret Service (GNOME Keyring/KWallet) | Credential Manager | Keychain |
| pdfium binary | `libpdfium.so` | `pdfium.dll` | `libpdfium.dylib` |
| Ollama acceleration | CPU / NVIDIA / AMD | CPU / NVIDIA / AMD | Metal (Apple Silicon is fast) |
| Unsigned-app warning | none | SmartScreen "More info → Run anyway" | Gatekeeper: right-click → Open (or sign with an Apple Developer ID) |

- Export files are UTF-8 JSON with no file paths in them, and book fingerprints are computed from file
  contents, so the same PDF has the same fingerprint on every OS.
- Paths are handled with `std::path` / Tauri path APIs only (no hard-coded `/` or `\`); Windows long
  paths (> 260 chars) are supported via Rust's verbatim paths.
- The release checklist includes running the export → import round trip across **all three** OSes.

### Platform dependencies
Rule: **the installed app needs nothing extra** except (optionally) Ollama. Everything OS-specific is
either bundled by the installer, compiled in, or chosen automatically at build time.

**1. Runtime (what a user's PC needs)**
| Dependency | Linux | Windows | macOS | How it's handled |
|---|---|---|---|---|
| Web engine | `libwebkit2gtk-4.1` | WebView2 | WKWebView | `.deb` declares it; AppImage bundles it; Windows installer bootstraps WebView2; built into macOS |
| pdfium | `.so` | `.dll` | `.dylib` | Bundled as a Tauri resource per OS by CI |
| SQLite | — | — | — | Compiled into the binary (`rusqlite` `bundled`) — no system SQLite |
| TLS | — | — | — | `rustls` compiled in — no OpenSSL needed on any OS |
| Keychain | Secret Service | Credential Manager | Keychain | Built into the OS; encrypted-file fallback if missing |
| Ollama | optional | optional | optional | Detected at runtime; setup screen with per-OS install steps |

**2. Rust crates with per-OS backends** (selected in `Cargo.toml`, no code changes)
```toml
[dependencies]
rusqlite = { version = "*", features = ["bundled"] }
reqwest  = { version = "*", default-features = false, features = ["rustls-tls", "stream", "json"] }

[target.'cfg(target_os = "linux")'.dependencies]
keyring = { version = "3", features = ["linux-native-sync-persistent", "crypto-rust"] }

[target.'cfg(target_os = "windows")'.dependencies]
keyring = { version = "3", features = ["windows-native"] }

[target.'cfg(target_os = "macos")'.dependencies]
keyring = { version = "3", features = ["apple-native"] }
```
`notify` (inotify / ReadDirectoryChangesW / FSEvents), `sysinfo` and `pdfium-render` pick their
backend automatically. Any OS-specific Rust code lives behind `#[cfg(target_os = …)]` in one module
(`src-tauri/src/platform/`), never scattered.

**3. Build machine (only needed to build locally — CI already has these)**
| Linux (Ubuntu) | Windows | macOS |
|---|---|---|
| `build-essential libwebkit2gtk-4.1-dev librsvg2-dev libssl-dev`, Rust, Node | MSVC Build Tools (C++ workload), WebView2, Rust, Node | Xcode Command Line Tools, Rust, Node |

**Rules to keep it portable**
- **No shelling out to OS tools** (e.g. `pdftotext` exists here but not on Windows) — that's why PDF work uses pdfium.
- **Frontend uses pure-JS npm packages only** (no native Node addons), so `package-lock.json` is identical on every OS.
- `Cargo.lock` and `package-lock.json` are committed; Rust toolchain pinned in `rust-toolchain.toml`; Node version in `.nvmrc`.
- **CI builds and runs tests on all three OSes on every push**, so an OS-specific break shows up immediately, not on the day you switch PCs.
- `scripts/doctor` (Rust or Node script) checks a dev machine for the build prerequisites above and prints the exact install command for that OS.
- New-PC checklist (shown in the app): install the app → choose the books folder → import progress →
  paste keys or import the encrypted key file → (optional) install Ollama and pull a model.

---

## 17. Phases and acceptance criteria

### Phase 1 — Foundation
Tauri + React scaffold, app identifier fixed, layout/theme, library scan + fingerprints + `books.json`
tracks/status, **add/remove books (copy or link, duplicate check, editable metadata)**, pdf.js reader with position memory, SQLite store + migrations + daily backups, **export / import
(merge + replace + preview)**.
**Done when:** all PDFs are listed by track; a book opens at the last page after restart; an export from
one profile merges into another with the correct preview and no duplicates.

### Phase 2 — Providers and models
Provider trait + 4 adapters, `providers.json`, keychain, add/test provider, live model lists, roles +
fallback chains, "Try this model", structured output + repair, usage tracking; Ollama manager (installed,
catalog with fit badges, pull with progress, delete); encrypted key export.
**Done when:** Ollama plus at least one cloud provider are added from the UI; each lists its models live;
a role with a broken first link falls back to the second; a model is pulled and deleted from the app.

### Phase 3 — PDF pipeline and chapter quizzes
Extraction, chapters, page labels, text-layer check, chunking, SQLite/FTS5, embeddings, hybrid search,
question generation with grounding check, answering UI, streamed grading, chapter review, follow-up chat,
dispute grade.
**Done when:** a 10-question quiz on a chapter of *Head First Design Patterns* cites real pages and every
answer gets a structured review; it works with local Ollama only, offline.

### Phase 4 — Assessment, skill map, roadmap
Taxonomies for the 4 tracks, chapter→topic mapping, adaptive skill test, skill map view, roadmap
generation + weekly plan, updates after quizzes.
**Done when:** a skill test finishes in ~15–20 minutes and produces a skill map and a roadmap that
points only to non-outdated books in this folder.

### Phase 5 — Cards and dashboard
Auto cards from mistakes, manual cards from the reader, FSRS review queue, dashboard with streak,
trends, weak topics, reading time, AI usage/cost.
**Done when:** wrong answers from a quiz appear as cards due the next day, and the dashboard numbers
match the attempt history after an export/merge round trip.

### Phase 6 — Packaging and sync
Linux installers, GitHub Actions for Windows/macOS, sync folder, new-PC onboarding, error/empty states,
accessibility pass, keyboard shortcuts.
**Done when:** the app installs on a second PC, picks up progress through the sync folder, and runs a quiz there.

---

## 18. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Database corruption or a bad migration | WAL mode, daily automatic backups (14 kept), separate dev data dir, migration tests, "Restore backup" in Settings |
| Small local models return broken JSON or shallow grading | Native schema output where possible, repair loop, fallback chain, "Try this model" before choosing |
| CPU-only inference is slow | Streaming, smaller model for question writing, grade answers while the user reads the next question |
| Books without an outline or with bad text extraction | Heading detection, fixed-size sections, manual chapter editing, scanned-PDF flag |
| AI invents questions not in the book | Chunk-id citations + grounding check, page refs shown to the user, dispute button |
| Provider APIs change | Adapter per API style, definitions in JSON, `wiremock` contract tests, custom provider as an escape hatch |
| No keychain on a Linux setup | Encrypted file fallback |
| pdfium bundling per OS | Pinned binaries in CI; fallback to `lopdf` text extraction if pdfium fails to load |
| Copyrighted PDFs in a public repo | `.gitignore` PDFs if the repo is pushed; the app never uploads files, only excerpts to the provider you choose |

---

## 19. Open decisions (defaults in bold)

1. First goal track for the curated taxonomy and `books.json` depth — all four tracks get a basic
   taxonomy; **Fundamentals** gets the most detail first unless you pick another.
2. Sync folder — **Phase 6** (manual export/import from Phase 1).
3. UI language for quizzes on Chinese books — **follow the UI language (English)**.
