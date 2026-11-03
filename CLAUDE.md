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

## Versioning & releases
- `package.json` is the source of truth. `tauri.conf.json` points at it, so `Cargo.toml` must match (CI fails otherwise).
- Every user-visible change gets a line under `## [Unreleased]` in `CHANGELOG.md` **in the same commit**
  (Keep a Changelog: Added / Changed / Fixed / Removed).
- Release flow (repo: github.com/Akulav/local-ai-ui, branch `main`):
  1. `npm run version:bump [patch|minor|major|x.y.z]` bumps package.json + Cargo.toml and turns `[Unreleased]` into `[X.Y.Z] - date`.
  2. Commit: `Release vX.Y.Z`.
  3. `npm run release` (scripts/release.ps1) checks the versions match, the notes are non-empty and the tree is clean,
     then creates the annotated tag `vX.Y.Z` and pushes branch + tag. `-NoPush` only tags.
  4. `.github/workflows/release.yml` builds on windows-latest and publishes the release with
     `Lumen-vX.Y.Z-windows-x64-portable.zip`, `Lumen_X.Y.Z_x64-setup.exe` (NSIS, per-user) and `SHA256SUMS.txt`.
     The body is the CHANGELOG section (`scripts/release-notes.ps1 X.Y.Z` prints it locally).
- **Pushing, tagging and publishing are outward-facing: only do them when the user asks.** Bumping and committing locally is fine when asked to prepare a release.
- Pre-release: use a version like `1.0.0-beta.1`. The workflow marks it as a pre-release.
- A manual run of `Release` (Actions tab, workflow_dispatch) builds artifacts without releasing; tick `publish` for a draft release.

## CI
- `.github/workflows/ci.yml` runs on every push to `main` and on PRs: `node --check` on the UI scripts, the version
  match check, and `cargo check --locked`. Keep `Cargo.lock` and `package-lock.json` committed, since `--locked`/`npm ci` need them.
- If you add a dependency, commit the updated lockfile in the same change, or CI breaks.
- The workflows use only maintained third-party actions: actions/checkout, actions/setup-node, dtolnay/rust-toolchain,
  Swatinem/rust-cache, actions/upload-artifact, softprops/action-gh-release.
- Before pushing, mirror CI locally: `node --check ui\app.js`, `cargo check --locked` in `src-tauri`.

## Working notes for Claude
- This is Windows + PowerShell 5.1: no `&&`; read and write UTF-8 files with `[IO.File]::ReadAllText/WriteAllText`
  (`Get-Content -Raw` decodes as ANSI and garbles `→`, `ă`, `ș`…). Keep `.ps1` files ASCII-only.
- `cmd /c` does not inherit PowerShell's `Set-Location`. Call `run.bat` by full path or `cd /d` inside the cmd string.
- In PS 5.1, `[regex]::Replace(s, p, r, 1)` treats `1` as RegexOptions, not a count. Use `([regex]'p').Replace(s, r, 1)`.
- A running `dist\Lumen\Lumen.exe` locks the exe; `run.bat` closes it. Killing it also kills llama-server (job object).
- Model downloads are 5–20 GB. For UI/flow testing, use a small GGUF (e.g. unsloth/Qwen3.5-2B-GGUF Q4_K_M, 1.2 GB) via curl.
- When testing the app through computer use, the native folder picker only accepts clicks (typing into it is blocked).

## Layout
```
run.bat, scripts/build.ps1   one-click build (toolchain auto-install)
scripts/bump-version.ps1     version bump; scripts/vendor.js re-copies UI libs into ui/vendor
scripts/release.ps1          tag + push a release; scripts/release-notes.ps1 extracts CHANGELOG section
.github/workflows/           ci.yml (checks), release.yml (build + publish on v* tags)
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
