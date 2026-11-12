<div align="center">

<img src="ui/logo.png" alt="Lumen" width="128" height="128">

# Lumen

**Chat with local AI models on your own computer, made by Akulav.**

No cloud. No telemetry. No setup to babysit. One file per system that installs and updates itself.

[![Latest release](https://img.shields.io/github/v/release/Akulav/local-ai-ui?style=for-the-badge&label=release&color=e5383b)](https://github.com/Akulav/local-ai-ui/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/Akulav/local-ai-ui/total?style=for-the-badge&color=f4b740)](https://github.com/Akulav/local-ai-ui/releases)
[![Build](https://img.shields.io/github/actions/workflow/status/Akulav/local-ai-ui/release.yml?branch=main&style=for-the-badge&label=build)](https://github.com/Akulav/local-ai-ui/actions/workflows/release.yml)

![Windows](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D6?style=flat-square&logo=windows&logoColor=white)
![Linux](https://img.shields.io/badge/Linux-x64-FCC624?style=flat-square&logo=linux&logoColor=black)
![macOS](https://img.shields.io/badge/macOS-Apple%20Silicon-000000?style=flat-square&logo=apple&logoColor=white)
![Rust](https://img.shields.io/badge/made%20with-Rust%20%2B%20Tauri-B7410E?style=flat-square&logo=rust&logoColor=white)
![License](https://img.shields.io/badge/license-source--available-5c6270?style=flat-square)

### [⬇ Download the latest release](https://github.com/Akulav/local-ai-ui/releases/latest)

</div>

---

## ⬇️ Download

One file per system. The same file installs Lumen the first time and updates it later.

| Platform | File | Notes |
| :-- | :-- | :-- |
| 🪟 **Windows 10 / 11** (x64) | [`Lumen-setup.exe`](https://github.com/Akulav/local-ai-ui/releases/latest/download/Lumen-setup.exe) | Asks where to install (English / Romanian). No admin rights needed. |
| 🐧 **Linux** (x64) | [`lumen-linux-x64`](https://github.com/Akulav/local-ai-ui/releases/latest/download/lumen-linux-x64) | WebKitGTK 4.1 desktop |
| 🍎 **macOS** (Apple Silicon) | [`lumen-macos-arm64`](https://github.com/Akulav/local-ai-ui/releases/latest/download/lumen-macos-arm64) | macOS 11+ |

Every release also ships `SHA256SUMS.txt` so you can verify your download.

## ✨ Features

<table>
<tr>
<td width="50%" valign="top">

### 🏠 Local only
- The chat model, project-search embeddings and every chat run on your computer.
- Lumen downloads the official **llama.cpp** engine itself (Vulkan, CUDA, ROCm, CPU, Metal).
- Web search is optional, off by default, and uses official APIs only.

</td>
<td width="50%" valign="top">

### 🛠️ Works on your code
- Link a folder: hybrid search (BM25 + embeddings), grep, outline, find symbols, git status / diff.
- Edits are diff cards with **Undo**; whole-codebase audits; runs your tests after you allow the command.
- **Deep research** plans, searches, reads and writes a cited report.

</td>
</tr>
<tr>
<td width="50%" valign="top">

### 💬 Chats & models
- Chats saved, titled, pinned, searched and exported; a **Think** toggle shows the reasoning live.
- Curated picks for 16 GB cards, any GGUF from Hugging Face, resumable downloads, automatic context size.
- Live GPU / CPU / VRAM / tokens-per-second telemetry. English and Romanian.

</td>
<td width="50%" valign="top">

### 🔄 Always up to date
- **You choose the install folder** once; Lumen, your models and chats live there.
- **One-click updates** of Lumen and llama.cpp from GitHub, verified with SHA-256, into the same folder.
- The engine stops with Lumen, even if Lumen crashes. Close to tray, idle unload.

</td>
</tr>
</table>

## 🚀 Getting started

### Windows
1. Download **`Lumen-setup.exe`** and run it.
2. Pick the install folder (default `%LOCALAPPDATA%\Lumen`, any folder you can write to works, e.g. `D:\AI\Lumen`;
   models of 5–20 GB go there too). It adds Start-menu and (optional) Desktop shortcuts and an entry in
   *Settings → Apps*.
3. Updates install into the same folder without asking: in-app with one click, or by running a newer setup.

Uninstall from *Settings → Apps*. The uninstaller asks whether to delete `LumenData` too (default: keep).

### Linux
```bash
chmod +x lumen-linux-x64
./lumen-linux-x64
```
It asks for an install folder with your system's folder picker (default `~/.local/share/lumen`; or pass
`--install-dir <folder>`), copies itself there and adds an app-menu entry. Cancelling runs it from where it is.
Running a newer download later updates the installed copy in its folder. Uninstall with `<folder>/lumen --uninstall`.

### macOS (Apple Silicon)
```bash
chmod +x lumen-macos-arm64
xattr -d com.apple.quarantine lumen-macos-arm64
./lumen-macos-arm64
```
It asks for an install folder (default `~/Applications/Lumen`) and creates `Lumen.app` there. An older
`~/Applications/Lumen.app` and its data move into the folder.

### First chat
1. **Start Lumen.** It downloads the llama.cpp engine (about 100 MB) on its own.
2. **Pick a model** in **Models**. On a 16 GB card the recommended **Qwen3.8 27B UD-IQ4_XS** (13.3 GB) fits in VRAM.
3. **Chat**, or link a folder as a **Project** to talk about your own files.

> [!TIP]
> Web search works without a key (DuckDuckGo instant answers + Wikipedia). For full web results add a free
> [Brave Search API](https://brave.com/search/api/) key (or Tavily, or your own SearXNG) in **Settings → Web search**.

## 📁 Where your data lives

**In the install folder you chose**, in `LumenData` next to the program (on macOS next to `Lumen.app`). Install on
`D:\AI\Lumen` and every model, chat and the engine go to `D:\AI\Lumen\LumenData`. The folder is remembered (Windows: by
the setup; Linux/macOS: in `~/.config/lumen/install-dir` or `~/Library/Application Support/lumen/install-dir`), so
updates keep everything in place. Only if that folder can't be written to does Lumen fall back to the OS's app-data
folder.

| Path | Contents |
| :-- | :-- |
| `LumenData/models/` | downloaded models (and the embedding model) |
| `LumenData/runtime/` | the llama.cpp engine |
| `LumenData/data/` | chats (SQLite), settings, logs, backups of edited project files |
| `LumenData/webview/` | the window's own browser data (Windows/Linux; macOS keeps it in `~/Library/WebKit`) |

## 🖥️ Command line

| Flag | Meaning |
| :-- | :-- |
| `--version` | Print the version and exit |
| `--here` | Linux / macOS: run where the program is instead of installing it |
| `--install-dir <folder>` | Linux / macOS: install into this folder without asking |
| `--uninstall [--delete-data]` | Linux / macOS: remove the program and its menu entry / app (and the data) |

Closing the window keeps Lumen and the loaded model running in the tray (menu bar on macOS); Settings → App →
*Close to tray* turns that off.

## 🔒 Privacy and safety

- Chats, projects, models and telemetry never leave the computer. The network is used for downloads (models, the
  engine, updates) and, only when you turn it on, web search through official APIs.
- File tools stay inside the project folder and back a file up before changing it; commands run only after you allow
  them in the window.
- Updates install only files whose SHA-256 matches the release's `SHA256SUMS.txt`.

<details>
<summary><b>🛠️ For maintainers: updates, releases and building</b></summary>

### Publishing a release

1. Note changes under `## [Unreleased]` in [CHANGELOG.md](CHANGELOG.md), then `npm run version:bump`
   (`patch` | `minor` | `major`; keeps `package.json`, `src-tauri/Cargo.toml` and `Cargo.lock` in sync).
2. Push to `main` (`npm run release` checks everything first).
3. If no release with that version exists, the **Release** workflow tests and builds all three systems, tags the
   commit `vX.Y.Z` and publishes a GitHub release with exactly three programs, `Lumen-setup.exe`, `lumen-linux-x64`
   and `lumen-macos-arm64`, plus `SHA256SUMS.txt`. The **CI** workflow checks every push and pull request.

### How updates are applied

An update downloads this system's file, checks it against `SHA256SUMS.txt`, then

- **Windows:** runs `Lumen-setup.exe` silently on your install folder; it replaces the program and starts it.
- **Linux / macOS:** swaps the program in place (macOS: inside Lumen.app, which is re-signed) and starts it again.

The llama.cpp engine is offered the same way. A copy built from source never replaces itself with a download.

### Building from source

- **Windows:** `run.bat` installs missing tools with winget (Node, Rust, MSVC Build Tools, Inno Setup), builds
  `lumen.exe` and the setup into `dist\` and starts Lumen (`run.bat build` / `quick` / `dev` / `clean`).
- **Any system:** `npm ci`, then `npx tauri build --no-bundle` → `src-tauri/target/release/lumen(.exe)`.
  Linux needs `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev`.
- Tests: `cargo test --locked` in `src-tauri`; `node --check ui/app.js`.

</details>

## 🙏 Credits

Inference by [llama.cpp](https://github.com/ggml-org/llama.cpp). Built with [Tauri](https://tauri.app). Markdown by
marked, sanitising by DOMPurify, highlighting by highlight.js (see `ui/vendor`).

---

<div align="center">

**Made with ❤️ by [Akulav](https://github.com/Akulav)**

<sub>© 2026 Akulav. Source-available under the <a href="LICENSE">Akulav Source-Available License</a>.</sub>

</div>
