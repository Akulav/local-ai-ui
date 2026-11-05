# Changelog

All notable changes to Lumen. The format follows [Keep a Changelog](https://keepachangelog.com/),
versions follow [Semantic Versioning](https://semver.org/). Note changes under **[Unreleased]** as you go;
`npm run version:bump [patch|minor|major|x.y.z]` turns that section into the new version; once that commit is
pushed to `main`, GitHub Actions tags, builds and publishes the release.

## [Unreleased]

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
