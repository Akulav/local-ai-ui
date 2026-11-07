# CLAUDE.md – Lumen

Lumen (made by Akulav) is a desktop app for chatting with local LLMs on **Windows, Linux (x86-64) and macOS (Apple
Silicon)**. Tauri 2 shell + Rust backend + plain HTML/CSS/JS UI (no bundler). Inference runs in a `llama-server`
child process (official llama.cpp build: Vulkan / ROCm / CPU on Windows and Linux, Metal on macOS) that Lumen
downloads and manages. Dev machine: Windows (RX 9070 XT 16 GB, 32 GB RAM, 4 GB page file).

## Always: bump the version and prove it works
The owner wants every change to ship as a release. For **every** change set (feature, fix, cleanup):
1. Note it under `## [Unreleased]` in `CHANGELOG.md`, then `npm run version:bump` with `patch` (fixes/cleanup only),
   `minor` (anything user-visible added) or `major` (only when asked). Not optional, don't ask.
2. Verify before saying it is done, and report each result honestly (paste the failure if one fails):
   - `node --check ui\app.js` and `node --check ui\i18n.js`
   - every `t("…")` key used in `app.js` exists in both `en` and `ro` in `ui/i18n.js` (`t("sev_" + x)` is a prefix)
   - `cargo clippy --locked --all-targets` in `src-tauri` with zero warnings, and `cargo test --locked`
   - Linux/macOS-only code can't be built here: type-check `unix.rs` / `install.rs` with a scratch crate that includes
     them by `#[path]` (+ stubs for crate fns), `cargo clippy --target x86_64-unknown-linux-gnu` and
     `--target aarch64-apple-darwin -- -D warnings` (both std targets are installed); CI then builds all three
   - `scripts\*.ps1` parse and stay ASCII-only; workflow YAML parses if touched; the setup compiles if
     `src-tauri/installer` changed (`ISCC` is in `%LOCALAPPDATA%\Programs\Inno Setup 6`)
   - UI changes: exercise them (browser pane, `window.__TAURI__` mocked, served over http – `file://` pages are static)
   - backend behaviour reachable without a GPU model (web search, parsing, updater pieces…): call it for real once
3. Commit as `Release vX.Y.Z: <short summary>`. Pushing (which publishes the release) stays the user's call; Claude's
   shell can't authenticate to GitHub anyway – the user pushes from GitHub Desktop.

## Build & run
- `run.bat` (Windows) installs missing tools with winget (Node, Rust, MSVC Build Tools, Inno Setup per user), builds
  `lumen.exe` and the setup, copies them to `dist\Lumen\Lumen.exe` and `dist\Lumen-setup.exe`, launches the exe.
  `run.bat build` (no launch) / `quick` (exe only) / `dev` / `clean`.
- Any system: `npm ci` then `npx tauri build --no-bundle` → `src-tauri/target/release/lumen(.exe)`. Tauri bundling is
  off (`bundle.active: false`): the release workflow makes the setup / .app / .dmg itself.
- Linux build deps: `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev`.
- `ui\` is **embedded in the exe at build time**: UI edits need a rebuild (or `run.bat dev`).
- Rust lives in `%USERPROFILE%\.cargo\bin`. git is not on PATH: use GitHub Desktop's
  `%LOCALAPPDATA%\GitHubDesktop\app-*\resources\app\git\cmd\git.exe`.

## Releases (like github.com/Akulav/ai-girlfriend)
- `package.json` is the source of truth; `src-tauri/Cargo.toml` must match (`npm run version:bump` keeps both and
  `Cargo.lock` in sync, and turns `[Unreleased]` into `[X.Y.Z] - date`).
- Flow: bump → commit `Release vX.Y.Z: …` → push `main` (`npm run release` checks versions, notes, clean tree, tag not
  yet on GitHub, then pushes; `-NoPush` only checks).
- `.github/workflows/release.yml` (push to main touching `package.json` or the workflow, or "Run workflow"):
  `check` (versions agree, tag `vX.Y.Z` not on GitHub yet) → `tests` (Windows + Ubuntu 22.04) → `build` (windows-latest,
  ubuntu-22.04, macos-15) → `release` (SHA256SUMS.txt, notes from CHANGELOG via `scripts/release-notes.ps1`, creates
  the tag, publishes; `-beta` versions are pre-releases). **Never create or push `v*` tags by hand.**
- One file per system – the same file installs and updates (updater.rs `asset_name`):
  `Lumen-setup.exe` (Windows, Inno Setup), `lumen-linux-x64`, `lumen-macos-arm64` (+ `Lumen-macos-arm64.dmg` with
  Lumen.app for the first install). Keep these names and `SHA256SUMS.txt`: installed copies look for them.
- `.github/workflows/ci.yml`: every push/PR – versions agree, UI scripts parse, `cargo clippy -D warnings` + `cargo
  test` on Windows / Ubuntu 22.04 / macOS 15, and the setup compiles. Keep `Cargo.lock` and `package-lock.json`
  committed (`--locked`, `npm ci`); a new dependency's lockfile change goes in the same commit.
- Workflows use only maintained actions: checkout, setup-node, dtolnay/rust-toolchain, Swatinem/rust-cache,
  upload-artifact, download-artifact, softprops/action-gh-release.

## Install & update
- **Windows**: `src-tauri/installer/lumen.iss` (Inno Setup 6, per user, EN + RO via `Romanian.isl` which overrides
  only the visible texts on top of `Default.isl`; wizard pictures from `scripts\installer-images.ps1`). Default folder
  `%LOCALAPPDATA%\Lumen`; `PrepareToInstall` silently uninstalls the previous version and takes over an old NSIS
  install (Lumen ≤ 0.8, registry `HKCU\…\Uninstall\Lumen`) with `uninstall.exe /S _?=<dir>` – `_?=` makes NSIS run in
  place and wait. Uninstall asks before deleting `LumenData` (never when silent). Never change the AppId GUID.
  `.iss`/`.isl` are UTF-8 **with BOM** (Inno needs it for ă/ș/ț). Test builds use `/DName=… /DGuid=… /DNsisKeyName=…`
  so a test install never touches the real one; the uninstaller may be `unins001.exe` after an update – read
  `UninstallString` from the registry.
- **Linux** (`install.rs`): the downloaded program copies itself to `~/.local/share/lumen/lumen`, writes
  `lumen.desktop` (app menu + Desktop) and starts that copy; `--here`, `--uninstall [--delete-data]`, `--version`.
- **macOS**: Lumen.app from the dmg (`src-tauri/installer/Info.plist`, ad-hoc signed in CI); data always in
  `~/Library/Application Support/Lumen` (never write into the bundle).
- **Updater** (`updater.rs`): checks 20 s after start + every 6 h (setting `check_updates`) and on demand; also offers
  newer llama.cpp builds. Install = download → SHA-256 against the release's `SHA256SUMS.txt` → Windows: start the setup
  detached with `/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /LAUNCH=1 /DIR=<app dir> /LOG=…` and exit;
  Linux/macOS: swap the program (`.old` kept until next start), macOS re-signs the bundle, relaunch via `sh -c 'sleep 2;
  …'` (the single-instance plugin would swallow an earlier start). A copy built here (`…/target/…` or `<repo>/dist/…`)
  never updates itself (`self_built`). Nothing installs without the user confirming in the window.

## Platform code
- `mod platform` = `win.rs` on Windows, `unix.rs` elsewhere (same functions): `Job` (Windows job object; Linux uses
  `PR_SET_PDEATHSIG` in `quiet`), `quiet(cmd)` (no console / parent-death signal – call it on every child process),
  `own_group` + `kill_tree`, `shell(command)`, `process_ram`, `process_cpu_time`, `available_memory_mb`,
  `GpuMemCounter` (PDH on Windows; 0 elsewhere), `set_window_icons` (Windows only: Tauri sets only ICON_SMALL).
- Spawn children from async code (tokio worker threads): on Linux the death signal follows the spawning thread.
- `runtime.rs`: `effective_backend` (macOS → `metal`), `asset_matches` per OS (`.zip` on Windows, `.tar.gz` elsewhere),
  `llama-server` without `.exe` off Windows; Linux children get `LD_LIBRARY_PATH` = the runtime folder.
- No tray (Linux without app indicator) sets `state.no_tray`: closing the window then quits.

## Working notes for Claude
- Windows + PowerShell 5.1: no `&&`; read/write UTF-8 with `[IO.File]::ReadAllText/WriteAllText` (`Get-Content -Raw`
  decodes as ANSI and garbles `→`, `ă`, `ș`…). Keep `.ps1` files ASCII-only.
- `cmd /c` does not inherit PowerShell's `Set-Location`: `cd /d` inside the cmd string.
- PS 5.1: `[regex]::Replace(s, p, r, 1)` treats `1` as RegexOptions; use `([regex]'p').Replace(s, r, 1)`.
- A running `dist\Lumen\Lumen.exe` locks the exe; `run.bat` closes it. Killing it also kills llama-server.
- The user's real install is NSIS 0.8 at `%LOCALAPPDATA%\Lumen`: never run setups/uninstallers against it in tests.
- Model downloads are 5–20 GB. For tests use a small GGUF (e.g. `dist\Lumen\LumenData\models\embedding\Qwen3-Embedding-0.6B`).
- Computer use: the native folder picker only accepts clicks.

## Layout
```
run.bat, scripts/build.ps1   one-click Windows build (toolchain auto-install, Inno Setup)
scripts/bump-version.ps1     version bump; release.ps1 pre-release checks + push; release-notes.ps1 CHANGELOG section
scripts/installer-images.ps1 setup wizard pictures; scripts/vendor.js re-copies UI libs into ui/vendor
.github/workflows/           ci.yml (3-OS checks), release.yml (tests → 3 builds → tag + release)
ui/                          index.html, styles.css, app.js (all UI logic), i18n.js (EN/RO strings), vendor/
src-tauri/installer/         lumen.iss, Romanian.isl, wizard-*.bmp (Windows setup); Info.plist (macOS app)
src-tauri/src/
  main.rs       app setup, tray (+ update entries), close-to-tray, macOS reopen, shutdown of child processes
  commands.rs   every #[tauri::command] the UI calls (thin wrappers)
  state.rs      AppState, Settings (serde-default JSON in LumenData/data/settings.json), Paths, logs
  db.rs         SQLite schema + queries (projects, chats, messages, chunks, files, stats)
  server.rs     spawns llama-server (chat) and the CPU embedding server; health/props; --fit; prompt-cache cap
  runtime.rs    downloads the llama.cpp build for this OS/backend from GitHub; GPU detection
  models.rs     curated catalog for 16 GB cards, local list/delete, HF download/search
  download.rs   resumable downloader with progress events
  llm.rs        SSE streaming client for /v1/chat/completions; readable server errors (bad allocation)
  chat.rs       prompt assembly, tool loop, title, context budget, compression, export
  tools.rs      model tools: project files (sandboxed, backups for undo), web tools
  codetools.rs  grep/outline/find_symbol/compare, git, run_command (confirmed by the user), process runner
  audit.rs      audit_code (whole codebase in batches); calc.rs calculate + current_datetime
  rag.rs        chunking, BM25 (diacritics-folded, camelCase split), embeddings, RRF fusion, incremental index
  research.rs   deep research: plan → search → read → reflect (≤3 rounds) → cited report
  web.rs        search via APIs (DuckDuckGo instant answers + Wikipedia, Brave, Tavily, SearXNG), page extraction
  extract.rs    text from code/text/PDF/DOCX; prompts.rs every system prompt (EN/RO language rules)
  metrics.rs    1 s sampler: RAM, VRAM, GPU/CPU load, llama-server /metrics → `metrics`; idle unload; crash check
  updater.rs    update check (Lumen + llama.cpp), SHA256-verified install per OS, relaunch, cleanup
  install.rs    Linux self-install / --uninstall (not on Windows)
  win.rs        platform on Windows: job object, console-free spawns, PDH GPU counters, commit memory, window icons
  unix.rs       platform on Linux/macOS: parent-death signal, process groups, sysinfo RAM/CPU, free memory
```

## Backend → UI events
`server-status`, `metrics`, `download-progress`, `index-progress`, `update-status` (UpdateInfo),
`update-prompt` (tray asked to update/check; the window confirms), `chat-event`
(`chat-event.kind`: busy, message, message_update, assistant_start, delta, progress, tool_pending,
tool_start, tool_confirm / tool_confirmed (a process tool waits for the user; answered with `confirm_tool`),
tool_progress (audit_code batches), assistant_done, assistant_failed, compress_progress, compressed, ctx, title, error).

## Conventions & gotchas
- Never hold a `parking_lot` guard across `.await` (guards are !Send; commands get spawned).
- Every child process: `crate::platform::quiet(&mut cmd)` and `state.job.adopt(pid)`.
- **llama-server's RAM prompt cache**: it copies a whole conversation's state to RAM whenever a different prompt
  arrives (titles, summaries, research, other chats) – several GB for a 27B model. Past the commit limit that throws
  `std::bad_alloc` ("bad allocation"). `server::prompt_cache_mib` caps `--cache-ram` at ¼ of the free memory (≤ 4 GiB);
  llama-server skips a bigger state instead of allocating it. Don't remove the cap.
- Prompt-cache friendliness: keep the system prompt and old messages byte-stable; per-message RAG context is stored in
  `meta.prompt_content` and re-sent identically.
- Tool file access goes through `tools::resolve` (keeps paths inside the project folder) and `backup()` before writing.
- Web search uses official APIs only – no HTML scraping of search engines, no bot-check tricks. Wikipedia and
  DuckDuckGo get the honest `Lumen/<version>` User-Agent. Default provider: DuckDuckGo's keyless Instant Answer API
  topped up with Wikipedia (no full-web results; `html.duckduckgo.com` is off limits).
- Links with Web off: `fetch_url` is always in the tool list (stable prompt) but `tools::link_allowed` only opens links
  that appear in the user's messages or project-file tool results (not fetched pages / search results), and pages
  under them (a GitHub repo link also covers its raw.githubusercontent.com files). Web on: any page.
- Tool loop (`chat::run_turn`): at most `MAX_ROUNDS`; warned at `WRAP_UP_AT` rounds left; if they run out (or a reply has
  no text) a last round with `tool_choice: "none"` asks for the answer. A turn must never end silently.
- Unload / model switch / quit call `chat::stop_all` first: a reply whose server vanished after a stop is a stop, not a
  crash (`recover` would otherwise restart the model the user just unloaded).
- Idle auto-unload (`settings.idle_unload_min`, metrics.rs `check_idle`): anything using the chat model registers in
  `state.generations` or calls `state.touch()`.
- All user-facing strings go in `ui/i18n.js` in both `en` and `ro`; the tray's few labels are in main.rs `tray_text`.
- Branding: "Made by Akulav" in the sidebar, welcome screen, About, tray tooltip, exe/setup version info, macOS plist.
- Opt-in tests (`cargo test -- --ignored`): `install_pipeline…`-style network tests, `live_llama_server_telemetry`
  (`LUMEN_TEST_SERVER`, `LUMEN_TEST_MODEL`), `live_windows_update` (`LUMEN_TEST_SETUP`, `LUMEN_TEST_DIR`), `live_code_tools`.
