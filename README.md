# Lumen – hassle-free local AI

A single Windows exe (Rust + Tauri) for chatting with local LLMs. On first run it downloads
the official **llama.cpp** build (Vulkan by default, which is fast on AMD RDNA4 / RX 9070 XT) into its own
folder and runs `llama-server` for you. Quit Lumen and the server quits too. It is attached to a Windows
job object, so it dies even if Lumen crashes.

## Features
- **Chats** saved in SQLite, auto-titled, pinned, editable, regenerable.
- **Thinking** toggle, with a live, expandable reasoning panel while the model thinks.
- **Web** toggle (the model can search and read pages) and **Deep research** (plan → search → read →
  reflect for up to 3 rounds → cited report with sources). Works in English and Romanian.
- **Projects**: link a folder. Lumen indexes code, text, PDF and DOCX (incremental, `.gitignore`-aware) with its own
  hybrid RAG: BM25 that ignores diacritics and splits camelCase, plus Qwen3-Embedding vectors on CPU, fused with RRF.
  The model can search, read, edit and write files. Every change is shown as a coloured diff card with **Undo**.
- **Context management**: a live context meter. Chats compress older turns into a summary automatically
  (shown as a progress card), and the stable prefix keeps llama.cpp's prompt cache hot. Each chat can have its own window size.
- **Model manager**: curated picks for 16 GB cards with Hugging Face links, any GGUF repo from Hugging Face search,
  resumable downloads, load/unload/delete, switching on the fly.
- **Metrics**: live and average tokens/s, plus the VRAM and RAM of the llama-server process.
- **Tray**: closing the window keeps Lumen running in the tray. Tray menu: Open / Unload model / Quit.
- UI in English or Romanian.

## Using it
Run `Lumen.exe`. Everything is stored next to it in `LumenData\` (runtime, models, chats).
Default model for the RX 9070 XT: **Qwen3.8 27B UD-IQ4_XS** (13.3 GB, fits fully in VRAM).

Web search uses official APIs. Wikipedia works without a key. For full web results, add a free
[Brave Search API](https://brave.com/search/api/) key (or Tavily / your own SearXNG) in **Settings → Web search**.

## Building
Double-click **`run.bat`**. It checks for Node.js, Rust and the Visual Studio C++ Build Tools, installs
any that are missing with `winget` (the Build Tools ask for admin rights once), builds the release exe,
copies it to `dist\Lumen\Lumen.exe` and launches it. Other modes:

| Command | What it does |
|---|---|
| `run.bat` | install tools if needed → build → copy to `dist\Lumen` → launch |
| `run.bat build` | same, without launching |
| `run.bat dev` | dev mode (`tauri dev`) |
| `run.bat clean` | delete `target/` and `node_modules/` (keeps your models and chats) |

## Download
Get the latest build from [Releases](https://github.com/Akulav/local-ai-ui/releases): the **portable zip**
(unzip and run `Lumen.exe`) or the **installer** (`-setup.exe`, no admin needed).

## Versioning & releases
`package.json` holds the version (Tauri reads it, and it is synced into `Cargo.toml`). Note changes under
`[Unreleased]` in [CHANGELOG.md](CHANGELOG.md) as you go, then:
```
npm run version:bump            # patch, or: minor | major | 1.2.3 (turns [Unreleased] into the new version)
git commit -am "Release vX.Y.Z"
npm run release                 # checks everything, tags vX.Y.Z and pushes
```
Pushing the tag triggers the **Release** workflow. It builds on GitHub's Windows runners and publishes the
release with the portable zip, the installer, SHA256 checksums and the changelog entry as notes.
The **CI** workflow checks every push and pull request.

---
Made by **Akulav**.
