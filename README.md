# Bookshelf Tutor

A desktop app that turns your own folder of PDF books into a tutor: read a chapter, get quizzed on what the book
actually says, and get a detailed review of every answer with the pages to re-read. Works with a local model
(Ollama — free and offline) or any cloud provider with your own API key.

## Setup (Linux / macOS)
```bash
# prerequisites: Rust, Node 20+, and on Linux the Tauri deps (webkit2gtk-4.1, librsvg2, libssl)
npm install
scripts/fetch-pdfium.sh      # PDF engine for this OS
npm run tauri dev            # or: npm run tauri build
```

First launch asks for your books folder. Then add an AI provider in **Settings → AI providers**, tick the models
you want, and pick a model for each job under **What each model does**.

For local AI, install [Ollama](https://ollama.com) and pull a model, e.g. `ollama pull qwen2.5:7b`
(the app limits Ollama to half your CPU cores by default).

## Moving to another computer
- Code: this git repo. Books: copy or sync the books folder (Syncthing / Drive / iCloud is fine for PDFs).
- Progress: **Settings → Export progress** on one machine, **Import progress → Merge** on the other.
  API keys are never exported — paste them again on each machine.
- Never put the app's data folder (the SQLite database) in a synced folder.

See `docs/PLAN.md` for the full design and `docs/PROGRESS.md` for status.
