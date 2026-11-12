# Changelog

All notable changes to Lumen. The format follows [Keep a Changelog](https://keepachangelog.com/),
versions follow [Semantic Versioning](https://semver.org/). Note changes under **[Unreleased]** as you go;
`npm run version:bump [patch|minor|major|x.y.z]` turns that section into the new version; once that commit is
pushed to `main`, GitHub Actions tags, builds and publishes the release.

## [Unreleased]

## [0.15.0] - 2026-11-12

### Changed
- **Everything lives in the folder you install Lumen to**: models, chats, settings, the engine and now also the
  window's own browser data (`LumenData/webview`) go to `LumenData` next to the program, never to `%LOCALAPPDATA%` or
  `~/.local/share`. Install on `D:\` and your models go to `D:\` too.
- Linux and macOS: the first start asks where to install with the system's folder picker (default
  `~/.local/share/lumen` or `~/Applications/Lumen`; `--install-dir <folder>` skips the question). The folder is
  remembered, so a newer download updates that copy. On macOS the data now sits next to `Lumen.app` instead of in
  `~/Library/Application Support/Lumen`; an older `Lumen.app` and its data move into the new folder.
- New README in the same style as the other Akulav apps.

## [0.14.1] - 2026-11-10

### Security
- Git tools no longer run programs named in a project's own `.git/config`: `core.fsmonitor`, clean/smudge filters
  (Git LFS's standard commands stay on) and textconv drivers are switched off for `git_status` / `git_diff`.
- Project file tools refuse paths that leave the project through a symlink or junction.
- `fetch_url` opens addresses on this computer or the local network (localhost, 192.168.x.x, *.local…) only when
  the user gave the link, also with Web on; allowed links are compared after resolving `..` and `user@` like the
  request does.
- The window has a Content-Security-Policy: no remote images in replies (a page could otherwise make the model leak
  text through an image URL), scripts only from the app.
- Opening a path from the UI is limited to folders; model downloads accept only `owner/name` repositories.

### Fixed
- A second reply in the same chat (double send, regenerate or edit while answering) is refused instead of taking
  over the first one's Stop button; regenerate no longer deletes the answer first and then fails.
- Errors before a reply starts ("No model is loaded yet", chat busy) are shown in the chat instead of vanishing;
  regenerate errors are no longer shown twice.
- Downloading the same model twice at once is refused (both wrote into one `.part` file).
- `fetch_url` stops reading at 25 MB instead of loading the whole response into memory first.

## [0.14.0] - 2026-11-08

### Changed
- Exactly three release files, one per system, and the same file installs and updates: `Lumen-setup.exe`,
  `lumen-linux-x64` and `lumen-macos-arm64` (plus `SHA256SUMS.txt`). The macOS disk image is gone: started from
  Downloads, `lumen-macos-arm64` builds `~/Applications/Lumen.app` around itself (icon, ad-hoc signature) and opens it;
  an existing `/Applications/Lumen.app` from the disk image is updated in place instead.
- macOS: `--uninstall` now removes Lumen.app (`--delete-data` also the data); `--here` runs without installing.
- Linux / macOS: a copy started with `--here` stays portable after an update instead of installing itself.

## [0.13.0] - 2026-11-08

### Added
- CUDA engine for NVIDIA cards (Windows and Linux): Settings → GPU backend → CUDA. Lumen checks the NVIDIA driver,
  picks llama.cpp's CUDA 12 or CUDA 13 build to match the driver and card, and installs NVIDIA's runtime libraries
  with it (first download ≈ 600 MB). Usually faster than Vulkan on NVIDIA, especially at reading long prompts. On an
  NVIDIA card it is marked as recommended; without an NVIDIA driver it says so before downloading anything.

## [0.12.1] - 2026-11-07

### Fixed
- Context meter: with no model loaded it measured against a 2,048-token window ("40k / 2k"). It now uses the
  window of the model loaded last (remembered across restarts), shows only the tokens used if no model was ever
  loaded, and switches to the real window as soon as a model loads.

## [0.12.0] - 2026-11-07

### Added
- Error messages can be copied (Copy button on error toasts), and clicking the status bar's cut-off message shows it in
  full.

### Fixed
- Error messages could run off the screen or vanish before you read them, with no way to close them. Toasts now have
  a close button, stay while you hover them, wrap long text (paths, crash logs) and scroll inside instead of growing
  past the window; at most four show at once and a repeated message doesn't stack. Esc closes them. The error box in a
  chat wraps, has a height limit and can be dismissed; long dialogs scroll.

### Removed
- Two unused interface strings; a formatting nit.

## [0.11.0] - 2026-11-07

### Added
- A new chat gets a name the moment you send its first message (from your words); the model's own title replaces it
  when the reply is done. Long project tasks no longer sit as "New chat", and a stopped turn keeps its name.
  Renaming the chat yourself always wins.

### Fixed
- Going to another chat or page while Lumen is writing no longer looks like it stopped: the reply keeps running, and
  when you come back the chat shows what it has written so far (or the tool it is running) and keeps streaming.
- Esc stopped the reply even when you were on Settings or Models, renaming a chat or editing a message. It now stops
  it only in the chat view, when you're not typing elsewhere.
- Loading another model or unloading while a reply is running now asks first (it restarts the engine).

## [0.10.0] - 2026-11-07

### Added
- Lumen can read links you give it even with Web off: a link in your message, or in a project file it read (e.g. a
  TODO that points to a GitHub repository), and pages under that link (for a GitHub repository also its raw files).
  Web search and other addresses still need Web on. Before, the model said it couldn't open the link.

### Fixed
- Unloading or switching the model while a reply was being written made Lumen think llama-server had crashed: it
  restarted the model you had just unloaded and the reply was lost without a word. Unload, switch and quit now stop
  the reply first.
- A page that needs a login (a private GitHub repository answers 404) now says so to the model, instead of a bare
  "HTTP 404".

## [0.9.0] - 2026-11-07

### Added
- Linux (x86-64) and macOS (Apple Silicon) versions, built automatically with Windows on every release. Linux: one
  program that installs itself (app menu entry); macOS: Lumen.app in a disk image. llama.cpp runs with Vulkan/ROCm/CPU
  on Linux and Metal on macOS.
- New Windows setup (Inno Setup, English/Romanian): `Lumen-setup.exe` is the only Windows file and also the update.
  Existing installs (0.8 and earlier) are taken over in place; LumenData is kept.
- Check for updates button in the sidebar and Settings, next to the Update button.
- "Made by Akulav" on the welcome screen, in the setup, the exe and the macOS app.
- Delete a chat from a project's page (trash button on each chat in the list).
- Delete a single exchange: the trash button on your message removes it and Lumen's reply (with its tool steps),
  without deleting the whole chat.
- When a reply finishes while Lumen is in the background, the taskbar button flashes (Dock icon bounces on macOS);
  when Lumen is hidden in the tray, the tray tooltip says a reply is ready.

### Changed
- Releases build and publish all three systems with one checksum list; the release notes list the file per system.
- README and CLAUDE.md rewritten for the new layout.

### Fixed
- A long request ("check the project and see what features may be missing") could end with reasoning and tool calls
  but no answer and no error: it used all 40 tool rounds, and the turn then just stopped. Now the model is told when
  5 rounds are left, and if the rounds still run out it is asked once more, with tools off, to answer from what it
  found (a reply with reasoning but no text gets the same). If even that gives no text, the chat says why.
- "bad allocation" in long chats: llama-server copied whole conversations into its RAM prompt cache (up to 8 GB) and
  ran out of memory (RAM + page file). The cache is now sized from the free memory, and the error is explained.

## [0.8.1] - 2026-11-05

### Fixed
- When llama-server crashed or dropped the connection mid-reply (seen at the end of a long code audit), the turn
  failed with a connection error, and Lumen kept showing the model as ready, so every later message failed too.
  Now:
  - a lost connection restarts the model server and retries the reply once (a toast says so);
  - `audit_code` does the same per batch, and if the server dies again it stops early and keeps the findings so far;
  - a server that exits on its own is noticed within a second: the status bar says it stopped (with the exit code,
    e.g. access violation or failed assertion), and the next message reloads the model.
- The server log is now also written to `LumenData\logs\lumen.log` (rotated at 8 MB), so the reason for a crash
  survives closing the app.

## [0.8.0] - 2026-11-05

### Added
- A proper installer: English/Romanian language choice, Lumen icon, branded welcome/finish and header images,
  publisher "Akulav", Start menu shortcut, optional desktop shortcut and "run Lumen" at the end. Installs per user
  without administrator rights.
- `run.bat` now builds the installer next to the portable exe (`dist\Lumen_X.Y.Z_x64-setup.exe`);
  `run.bat quick` builds only the portable exe.

### Fixed
- The taskbar, Alt+Tab and Task Manager showed a generic icon for Lumen: the window only had a small icon. It now
  also gets a big one, picked from the app icon for the screen's DPI.
- Uninstalling with "Delete the application data" ticked now also removes `LumenData` (models, chats, settings)
  of an installed copy; before, it stayed behind in `%LOCALAPPDATA%\Lumen`. Updates keep everything.

## [0.7.1] - 2026-11-05

### Fixed
- "request exceeds the available context size" in long conversations: when llama-server rejects a prompt, Lumen now
  also summarises the earlier turns (not only old tool results) before it retries, and it compresses a little
  earlier before sending (at 80 % of the window instead of 90 %), leaving room for the estimate's error and the reply.

## [0.7.0] - 2026-11-05

### Added
- `audit_code`: audits a whole project or folder of any size. The source files are reviewed in batches that fit
  the model's window, each in a fresh context, and all findings land in one review card (with how many files and
  batches were covered). The tool card shows live progress, and Stop works between batches. Lockfiles, minified,
  vendored and generated files are skipped.

### Fixed
- Long tool sessions in a single message (for example "audit this project") no longer run out of context and fail.
  Compression only summarised earlier messages, so one long turn kept growing until llama-server rejected it. Now:
  - each tool result is cut to fit the room left in the window (start and end kept, with a hint to ask for less);
  - when the prompt gets near the limit, the oldest tool results are replaced by a one-line note (still shown in
    full in the chat), so the model keeps working;
  - if llama-server still reports a context overflow, Lumen prunes harder and retries once, then explains what to do
    instead of failing with a raw error.
- The system prompt tells the model that its context is limited: use `audit_code` for big reviews, read files in
  ranges, and prefer `grep`/`find_symbol` over reading everything.

## [0.6.1] - 2026-11-05

### Fixed
- Chats started from the sidebar could not see a project's files even after the folder was indexed, because only
  chats created on the project page were linked to it. New chat (and Ctrl+N) now creates the chat in the project
  you are in (its page or one of its chats, the button says "New chat in …").
- Any chat can be linked to a project, moved to another one or taken out of it: the "Link to project" button in
  the top bar of a standalone chat, or "Move to project" in the chat menu. The files and tools work from the next
  message.

## [0.6.0] - 2026-11-05

### Added
- Code review tools the model uses on its own in project chats: `grep` (exact/regex search with context lines),
  `file_outline` (functions, classes and other definitions with line numbers), `find_symbol` (definitions and every
  usage of an identifier), `compare_files`, `git_status` and `git_diff` (uncommitted, staged or against a
  branch/commit; works with Git for Windows or the git bundled with GitHub Desktop).
- `run_command`: the model can run the project's own tests, linter, type-checker or build. Every command waits for
  your OK in the chat (Run / Always in this chat / Deny); git asks the same way. Runs without a console window, is
  killed with its child processes on timeout or Stop, and its output comes back to the model.
- Code review card: the model reports findings with file, line, severity, problem and fix, sorted by severity
  with the offending line shown; copy the whole review as Markdown.
- General tools in every chat: `calculate` (exact arithmetic and math functions) and `current_datetime`.
- The system prompt now tells the model when to review code by itself (scope with git, read callers, run checks,
  then report findings).

### Changed
- Diff cards without an Undo (git diffs, file comparisons) no longer show the button, and the stray empty last
  line in diff cards is gone.

## [0.5.0] - 2026-11-04

### Added
- Update check: Lumen looks for new Lumen and llama.cpp releases on GitHub at start-up and every 6 hours
  (Settings → About, can be turned off). When one exists, an Update entry appears in the sidebar and in the
  tray's right-click menu, and Settings shows what's new.
- One-click Lumen update: downloads the release, verifies it against the release's SHA256SUMS.txt, installs it
  (portable copies swap the exe, installed copies run the installer) and restarts.
- llama.cpp engine updates are offered the same way; the model is reloaded afterwards.
- Engine telemetry panel (chip icon in the status bar): GPU and CPU load of llama-server, VRAM/shared/RAM,
  uptime, live/last/average generation speed, prompt-processing speed, and the server's token and request
  counters since the model loaded. Everything stays on the PC.
- GPU and CPU load in the status bar, and prompt-processing speed under each reply.

## [0.4.0] - 2026-11-04

### Added
- Auto-unload: the model is unloaded after a set idle time to free VRAM (Settings → Engine, default 30 min,
  or Never). The next message reloads it automatically, and the status bar says why it was unloaded.
- DuckDuckGo search provider (no API key), now the default for new installs. It uses DuckDuckGo's official
  Instant Answer API (topic summaries and official sites) and fills the rest with Wikipedia results.
  Existing installs keep their saved provider; switch in Settings → Web search.

### Fixed
- Changing a setting after loading a different model no longer resets "Load last model on start" to the old model.

### Changed
- Releases publish automatically: pushing a new `package.json` version to `main` makes the Release workflow
  create the `vX.Y.Z` tag, build and publish. No local tag push (or GitHub sign-in from the terminal) needed.

## [0.3.0] - 2026-11-04

### Added
- LICENSE: source-available terms (run releases and modify privately; no redistribution or commercial use).
- Export a chat as Markdown (chat menu or Ctrl+Shift+E), including reasoning and tool calls.
- Sidebar search also finds chats by what was said in them, not only by title.
- Custom instructions (Settings → Chat): always-on guidance added to every chat's system prompt.
- Unsent text is kept per chat when switching between chats.
- "Jump to latest" button when you scroll up in a long chat.
- Keyboard shortcuts: Ctrl+K (search), Ctrl+, (settings), Esc closes menus and dialogs before it stops a
  generation, Enter confirms dialogs. All shortcuts are listed in Settings.
- Message timestamps on hover.

### Changed
- Remaining hard-coded English UI strings (settings sections, status bar tooltips, model list) are translated.
- The Wikipedia User-Agent now reports the real app version.

### Removed
- Internal cleanup: unused `once_cell` dependency, unused catalog/state fields, duplicated server-spawn,
  log, row-mapping and UI helpers; project deletion now runs in a single transaction. Clippy is warning-free.

## [0.2.1] - 2026-11-03
### Added
- GitHub Actions: `CI` checks every push/PR (cargo check, UI syntax, version match); `Release` builds the
  exe + NSIS installer on `v*` tags and publishes a GitHub release with a portable zip, the installer
  and SHA256 checksums. The release notes come from this changelog.
- `npm run release` (scripts/release.ps1): checks the version and notes, then tags and pushes.

### Fixed
- `version:bump` no longer garbles non-ASCII characters in CHANGELOG.md on Windows PowerShell 5.
- `version:bump` also updates the app's version in `Cargo.lock`, so `cargo check --locked` keeps passing.

## [0.2.0] - 2026-11-03
### Added
- `run.bat`: one-click build that installs Node.js, Rust and the MSVC build tools when missing.
- Single-source versioning (package.json → Tauri, synced into Cargo.toml) and this changelog.
- Version and "Made by Akulav" shown in the sidebar and Settings → About.
- `CLAUDE.md` with architecture notes, a proper `.gitignore`, and code comments.

### Changed
- Web search uses official APIs (Brave, Tavily, SearXNG) with Wikipedia as the keyless fallback.
- Reasoning budget is capped at 8k tokens so small models cannot think forever.

## [0.1.0] - 2026-11-03
### Added
- First release: chats, projects with hybrid RAG and file editing (diff + undo), deep research,
  auto-compression, live context meter, model manager, tok/s + VRAM/RAM metrics, tray, EN/RO UI.
