//! Tools the model can call: project files (sandboxed to the project folder) and the web.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

use crate::db::Project;
use crate::state::{new_id, AppState};

pub struct ToolCtx<'a> {
    pub app: &'a AppHandle,
    pub project: Option<&'a Project>,
}

pub struct ToolOutput {
    /// what the model sees
    pub content: String,
    /// what the UI renders (diff, hits, …)
    pub display: Value,
    pub ok: bool,
}

fn func(name: &str, desc: &str, params: Value) -> Value {
    json!({"type": "function", "function": {"name": name, "description": desc, "parameters": params}})
}

pub fn definitions(project: bool, web: bool) -> Vec<Value> {
    let mut v = Vec::new();
    if project {
        v.push(func(
            "search_project",
            "Hybrid semantic + keyword search across all files in the project (code, PDFs, documents). Returns the most relevant excerpts with file paths and line numbers.",
            json!({"type": "object", "properties": {
                "query": {"type": "string", "description": "What to look for (natural language or identifiers)"},
                "k": {"type": "integer", "description": "Number of results, default 8"}
            }, "required": ["query"]}),
        ));
        v.push(func(
            "list_files",
            "List files and folders of the project (relative paths).",
            json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "Sub-folder relative to the project root, default root"},
                "depth": {"type": "integer", "description": "Max depth, default 3"}
            }}),
        ));
        v.push(func(
            "read_file",
            "Read a project file with line numbers. PDFs and DOCX are converted to text.",
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "start_line": {"type": "integer"},
                "end_line": {"type": "integer"}
            }, "required": ["path"]}),
        ));
        v.push(func(
            "edit_file",
            "Replace an exact snippet in a file. old_text must appear exactly once (include enough surrounding lines). Use for targeted edits.",
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "old_text": {"type": "string"},
                "new_text": {"type": "string"}
            }, "required": ["path", "old_text", "new_text"]}),
        ));
        v.push(func(
            "write_file",
            "Create a new file or overwrite an existing file with the full content.",
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            }, "required": ["path", "content"]}),
        ));
    }
    if web {
        v.push(func(
            "web_search",
            "Search the web. Returns titles, URLs and snippets.",
            json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
        ));
        v.push(func(
            "fetch_url",
            "Fetch a web page (or PDF) and return its readable text.",
            json!({"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}),
        ));
    }
    v
}

/// Resolve a model-provided path inside the project folder, rejecting escapes.
pub fn resolve(folder: &Path, rel: &str) -> Result<(PathBuf, String)> {
    let root = dunce(folder.canonicalize()?);
    let rel = rel.trim().trim_start_matches("./").replace('\\', "/");
    let candidate = if Path::new(&rel).is_absolute() { PathBuf::from(&rel) } else { root.join(&rel) };
    let mut norm = PathBuf::new();
    for c in candidate.components() {
        match c {
            Component::ParentDir => {
                norm.pop();
            }
            Component::CurDir => {}
            other => norm.push(other.as_os_str()),
        }
    }
    if !norm.starts_with(&root) {
        return Err(anyhow!("path is outside the project folder"));
    }
    let rel_clean = norm.strip_prefix(&root).unwrap_or(&norm).to_string_lossy().replace('\\', "/");
    Ok((norm, rel_clean))
}

fn dunce(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        if !stripped.starts_with("UNC") {
            return PathBuf::from(stripped);
        }
    }
    p
}

pub fn tree(folder: &Path, sub: &str, depth: usize, max_entries: usize) -> String {
    let base = folder.join(sub);
    let mut lines = Vec::new();
    let walker = ignore::WalkBuilder::new(&base)
        .hidden(true)
        .git_ignore(true)
        .max_depth(Some(depth))
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !matches!(n.as_ref(), "node_modules" | ".git" | "target" | "__pycache__" | ".venv" | "venv" | "dist" | "build" | "LumenData")
        })
        .build();
    for e in walker.flatten().skip(1) {
        let d = e.depth();
        let name = e.file_name().to_string_lossy();
        let is_dir = e.file_type().map_or(false, |t| t.is_dir());
        lines.push(format!("{}{}{}", "  ".repeat(d.saturating_sub(1)), name, if is_dir { "/" } else { "" }));
        if lines.len() >= max_entries {
            lines.push("… (truncated)".into());
            break;
        }
    }
    if lines.is_empty() {
        "(empty)".into()
    } else {
        lines.join("\n")
    }
}

/// Saves the pre-change content (or "did not exist") so the diff card's Undo can restore it.
fn backup(state: &AppState, full: &Path, old: Option<&str>) -> String {
    let id = new_id();
    let rec = json!({"path": full.to_string_lossy(), "existed": old.is_some(), "content": old.unwrap_or("")});
    let _ = std::fs::write(state.paths.backups.join(format!("{id}.json")), rec.to_string());
    id
}

pub fn undo(state: &AppState, backup_id: &str) -> Result<String> {
    if !backup_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(anyhow!("bad id"));
    }
    let p = state.paths.backups.join(format!("{backup_id}.json"));
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(&p)?)?;
    let path = PathBuf::from(rec["path"].as_str().unwrap_or_default());
    if rec["existed"].as_bool().unwrap_or(false) {
        std::fs::write(&path, rec["content"].as_str().unwrap_or(""))?;
    } else {
        let _ = std::fs::remove_file(&path);
    }
    let _ = std::fs::remove_file(p);
    Ok(path.to_string_lossy().into())
}

fn diff_display(rel: &str, old: &str, new: &str, created: bool, backup_id: &str) -> (Value, String) {
    let d = similar::TextDiff::from_lines(old, new);
    let (mut add, mut del) = (0, 0);
    for ch in d.iter_all_changes() {
        match ch.tag() {
            similar::ChangeTag::Insert => add += 1,
            similar::ChangeTag::Delete => del += 1,
            _ => {}
        }
    }
    let unified = d.unified_diff().context_radius(3).header(&format!("a/{rel}"), &format!("b/{rel}")).to_string();
    let unified: String = if unified.len() > 120_000 { format!("{}\n… (diff truncated)", &unified[..unified.floor_char_boundary(120_000)]) } else { unified };
    let summary = format!("{} `{}` (+{} −{})", if created { "Created" } else { "Edited" }, rel, add, del);
    (
        json!({"kind": "diff", "path": rel, "created": created, "added": add, "removed": del, "diff": unified, "backup": backup_id}),
        summary,
    )
}

/// Executes one tool call. Errors are returned to the model as text (so it can retry) instead of
/// aborting the turn.
pub async fn run(ctx: &ToolCtx<'_>, name: &str, args_raw: &str) -> ToolOutput {
    let args: Value = serde_json::from_str(args_raw).unwrap_or_else(|_| json!({}));
    match run_inner(ctx, name, &args).await {
        Ok(o) => o,
        Err(e) => ToolOutput { content: format!("Error: {e}"), display: json!({"kind": "error", "error": e.to_string()}), ok: false },
    }
}

fn s<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().ok_or_else(|| anyhow!("missing argument `{k}`"))
}

async fn run_inner(ctx: &ToolCtx<'_>, name: &str, a: &Value) -> Result<ToolOutput> {
    let state = ctx.app.state::<Arc<AppState>>().inner().clone();
    let need_project = || ctx.project.ok_or_else(|| anyhow!("this tool is only available inside a project"));
    match name {
        "search_project" => {
            let p = need_project()?;
            let q = s(a, "query")?;
            let k = a["k"].as_u64().unwrap_or(8).clamp(1, 20) as usize;
            let hits = crate::rag::search(ctx.app, &p.id, q, k).await?;
            if hits.is_empty() {
                return Ok(ToolOutput { content: "No matches. The project may still be indexing, or try other words.".into(), display: json!({"kind": "search", "hits": []}), ok: true });
            }
            let mut out = String::new();
            for (i, h) in hits.iter().enumerate() {
                out.push_str(&format!("[{}] {}:{}-{}\n{}\n\n", i + 1, h.path, h.line_start, h.line_end, h.text));
            }
            let disp: Vec<Value> = hits.iter().map(|h| json!({"path": h.path, "lines": format!("{}-{}", h.line_start, h.line_end)})).collect();
            Ok(ToolOutput { content: out, display: json!({"kind": "search", "hits": disp}), ok: true })
        }
        "list_files" => {
            let p = need_project()?;
            let sub = a["path"].as_str().unwrap_or("");
            let (full, _) = resolve(Path::new(&p.folder), sub)?;
            let depth = a["depth"].as_u64().unwrap_or(3).clamp(1, 8) as usize;
            let root = PathBuf::from(&p.folder);
            let rel = full.strip_prefix(dunce(root.canonicalize()?)).map(|r| r.to_path_buf()).unwrap_or_default();
            let t = tree(&root, &rel.to_string_lossy(), depth, 400);
            Ok(ToolOutput { content: t, display: json!({"kind": "text"}), ok: true })
        }
        "read_file" => {
            let p = need_project()?;
            let (full, rel) = resolve(Path::new(&p.folder), s(a, "path")?)?;
            let f2 = full.clone();
            let text = tokio::task::spawn_blocking(move || crate::extract::extract(&f2)).await??;
            let lines: Vec<&str> = text.lines().collect();
            let total = lines.len();
            let start = a["start_line"].as_u64().unwrap_or(1).max(1) as usize;
            let mut end = a["end_line"].as_u64().map(|e| e as usize).unwrap_or(total).min(total);
            if end < start {
                end = start;
            }
            let mut out = String::new();
            let mut chars = 0;
            let mut last = start;
            for (i, l) in lines.iter().enumerate().take(end).skip(start - 1) {
                let row = format!("{:>5}│ {}\n", i + 1, l);
                chars += row.len();
                if chars > 60_000 {
                    out.push_str(&format!("… truncated at line {} of {total}; request a range with start_line/end_line\n", i));
                    break;
                }
                out.push_str(&row);
                last = i + 1;
            }
            Ok(ToolOutput {
                content: format!("{rel} (lines {start}-{last} of {total})\n{out}"),
                display: json!({"kind": "read", "path": rel, "lines": format!("{start}-{last}"), "total": total}),
                ok: true,
            })
        }
        "write_file" => {
            let p = need_project()?;
            let (full, rel) = resolve(Path::new(&p.folder), s(a, "path")?)?;
            let content = s(a, "content")?;
            let old = std::fs::read_to_string(&full).ok();
            if let Some(dir) = full.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let bid = backup(&state, &full, old.as_deref());
            std::fs::write(&full, content)?;
            crate::rag::refresh_file(ctx.app, &p.id, Path::new(&p.folder), &rel).await;
            let (disp, summary) = diff_display(&rel, old.as_deref().unwrap_or(""), content, old.is_none(), &bid);
            Ok(ToolOutput { content: format!("OK. {summary}"), display: disp, ok: true })
        }
        "edit_file" => {
            let p = need_project()?;
            let (full, rel) = resolve(Path::new(&p.folder), s(a, "path")?)?;
            let old_text = s(a, "old_text")?;
            let new_text = s(a, "new_text")?;
            let original = std::fs::read_to_string(&full).map_err(|e| anyhow!("cannot read {rel}: {e}"))?;
            // tolerate CRLF files
            let crlf = original.contains("\r\n");
            let norm = original.replace("\r\n", "\n");
            let old_n = old_text.replace("\r\n", "\n");
            let count = norm.matches(&old_n).count();
            if old_n.is_empty() || count == 0 {
                return Err(anyhow!("old_text not found in {rel}. Read the file again and copy the exact text."));
            }
            if count > 1 {
                return Err(anyhow!("old_text appears {count} times in {rel}; include more surrounding lines so it is unique"));
            }
            let updated_n = norm.replacen(&old_n, &new_text.replace("\r\n", "\n"), 1);
            let updated = if crlf { updated_n.replace('\n', "\r\n") } else { updated_n.clone() };
            let bid = backup(&state, &full, Some(&original));
            std::fs::write(&full, &updated)?;
            crate::rag::refresh_file(ctx.app, &p.id, Path::new(&p.folder), &rel).await;
            let (disp, summary) = diff_display(&rel, &norm, &updated_n, false, &bid);
            Ok(ToolOutput { content: format!("OK. {summary}"), display: disp, ok: true })
        }
        "web_search" => {
            let q = s(a, "query")?;
            let hits = crate::web::search(&state, q, 8).await?;
            let mut out = String::new();
            for (i, h) in hits.iter().enumerate() {
                out.push_str(&format!("[{}] {}\n{}\n{}\n\n", i + 1, h.title, h.url, h.snippet));
            }
            Ok(ToolOutput { content: out, display: json!({"kind": "web", "query": q, "hits": hits}), ok: true })
        }
        "fetch_url" => {
            let url = s(a, "url")?;
            let page = crate::web::fetch(&state, url).await?;
            let text: String = page.text.chars().take(24_000).collect();
            Ok(ToolOutput {
                content: format!("# {}\n{}\n\n{}", page.title, page.url, text),
                display: json!({"kind": "page", "url": page.url, "title": page.title, "chars": page.text.len()}),
                ok: true,
            })
        }
        other => Err(anyhow!("unknown tool `{other}`")),
    }
}
