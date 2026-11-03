# Changelog

All notable changes to Lumen. The format follows [Keep a Changelog](https://keepachangelog.com/),
versions follow [Semantic Versioning](https://semver.org/). Bump with `npm run version:bump [patch|minor|major|x.y.z]`.

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
