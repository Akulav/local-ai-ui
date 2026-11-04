# Changelog

All notable changes to Lumen. The format follows [Keep a Changelog](https://keepachangelog.com/),
versions follow [Semantic Versioning](https://semver.org/). Note changes under **[Unreleased]** as you go;
`npm run version:bump [patch|minor|major|x.y.z]` turns that section into the new version; once that commit is
pushed to `main`, GitHub Actions tags, builds and publishes the release.

## [Unreleased]

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
