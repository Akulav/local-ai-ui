# CLAUDE.md – Lumen

Lumen is a portable Windows desktop app (made by Akulav) for chatting with local LLMs.
Tauri 2 shell + Rust backend + plain HTML/CSS/JS UI (no bundler). Inference is done by a
`llama-server` child process (llama.cpp, Vulkan build by default) that Lumen downloads and manages.

## Build & run
- `run.bat` – installs missing tools (Node, Rust, MSVC Build Tools via winget), builds the release exe,
  copies it to `dist\Lumen\Lumen.exe`, launches it. `run.bat build` / `dev` / `clean` also exist.
- Manual: `npm ci` then `npx tauri build --no-bundle` → `src-tauri\target\release\lumen.exe`.
- Fast compile check: `cargo check` inside `src-tauri` (Rust lives in `%USERPROFILE%\.cargo\bin`).
- `ui\` is **embedded in the exe at build time**: UI edits need a rebuild (or `run.bat dev`).
- Syntax-check the UI with `node --check ui\app.js`.
- git is not on PATH on the dev machine. Use GitHub Desktop or its bundled
  `%LOCALAPPDATA%\GitHubDesktop\app-*\resources\app\git\cmd\git.exe`.

## Versioning
- `package.json` is the source of truth. `tauri.conf.json` points at it, so `Cargo.toml` must match.
- Bump with `npm run version:bump [patch|minor|major|x.y.z]` (updates both + CHANGELOG.md). Then commit and tag `vX.Y.Z`.
- Record user-visible changes in `CHANGELOG.md` (Keep a Changelog format).

## Layout
```
run.bat, scripts/build.ps1   one-click build (toolchain auto-install)
scripts/bump-version.ps1     version bump; scripts/vendor.js re-copies UI libs into ui/vendor
ui/                          index.html, styles.css, app.js (all UI logic), i18n.js (EN/RO strings), vendor/
src-tauri/src/
  main.rs       app setup, tray, close-to-tray, shutdown of child processes
  commands.rs   every #[tauri::command] the UI calls (thin wrappers)
  state.rs      AppState, Settings (serde-default JSON in LumenData/data/settings.json), portable Paths
  db.rs         SQLite schema + queries (projects, chats, messages, chunks, files, stats)
  server.rs     spawns llama-server (chat) and the CPU embedding server; health/props; --fit auto ctx
  runtime.rs    downloads llama.cpp release zip from GitHub (vulkan|rocm|cpu); GPU detection
  models.rs     curated catalog for 16 GB cards, local list/delete, HF download/search
  download.rs   resumable downloader with progress events
  llm.rs        SSE streaming client for /v1/chat/completions (content, reasoning, tool calls, tps)
  chat.rs       prompt assembly, tool loop, title, context budget, compression (summary + compressed flag)
  tools.rs      model tools: search_project/list/read/edit/write (sandboxed, backups for undo), web tools
  rag.rs        chunking, BM25 (diacritics-folded, camelCase split), embeddings, RRF fusion, incremental index
  research.rs   deep research pipeline: plan → search → read → reflect (≤3 rounds) → cited report
  web.rs        search via APIs (Brave/Tavily/SearXNG, Wikipedia fallback) + readable page extraction
  extract.rs    text from code/text/PDF/DOCX
  prompts.rs    every system prompt (EN/RO language rules live here)
  metrics.rs    1 s sampler: process RAM + per-process VRAM → `metrics` event
  win.rs        Windows job object (kill children on exit/crash), RAM, PDH GPU memory counters
```

## Backend → UI events
`server-status`, `metrics`, `download-progress`, `index-progress`, `chat-event`
(`chat-event.kind`: busy, message, message_update, assistant_start, delta, progress, tool_pending,
tool_start, assistant_done, assistant_failed, compress_progress, compressed, ctx, title, error).

## Conventions & gotchas
- Never hold a `parking_lot` guard across `.await` (guards are !Send; commands get spawned).
- Spawn child processes with `creation_flags(0x08000000)` (no console) and `state.job.adopt(pid)`.
- Prompt-cache friendliness: keep the system prompt and old messages byte-stable; per-message RAG
  context is stored in `meta.prompt_content` and re-sent identically.
- Tool file access must go through `tools::resolve` (keeps paths inside the project folder) and
  `backup()` before writing, so the diff card's Undo works.
- Web search uses official APIs only. Do not add HTML scraping of search engines or tricks to get
  around bot checks. Wikipedia requests must send the honest `Lumen/...` User-Agent.
- All user-facing strings go in `ui/i18n.js` in both `en` and `ro`.
- Portable data lives in `LumenData/` next to the exe (falls back to `%LOCALAPPDATA%\Lumen`).
- Target hardware: AMD RX 9070 XT (16 GB) + 32 GB RAM; default model Qwen3.8-27B UD-IQ4_XS.
