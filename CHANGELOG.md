# Changelog

All notable changes to Lumen. The format follows [Keep a Changelog](https://keepachangelog.com/),
versions follow [Semantic Versioning](https://semver.org/). Note changes under **[Unreleased]** as you go;
`npm run version:bump [patch|minor|major|x.y.z]` turns that section into the new version, and `npm run release`
tags it so GitHub Actions builds and publishes the release.

## [Unreleased]

### Added
- LICENSE: source-available terms (run releases and modify privately; no redistribution or commercial use).

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
