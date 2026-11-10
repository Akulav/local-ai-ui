//! Tools the model can call: project files (sandboxed to the project folder) and the web.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

use crate::codetools;
use crate::db::Project;
use crate::state::{new_id, AppState};

pub struct ToolCtx<'a> {
    pub app: &'a AppHandle,
    pub project: Option<&'a Project>,
    pub chat_id: &'a str,
    pub cancel: &'a AtomicBool,
    /// the Web toggle: off, `fetch_url` only opens links that appear in the conversation
    pub web: bool,
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
    let mut v = vec![
        func(
            "calculate",
            "Evaluate an arithmetic expression exactly instead of computing it in your head. Supports + - * / % ^ (or **), parentheses, !, pi, e and functions sqrt, cbrt, abs, sin, cos, tan, asin, acos, atan, atan2, ln, log (log10, or log(x, base)), log2, exp, floor, ceil, round (round(x, digits)), min, max, sum, avg, pow, hypot, deg, rad. Trigonometry uses radians.",
            json!({"type": "object", "properties": {
                "expression": {"type": "string", "description": "e.g. (1250 * 1.19) / 12 or sqrt(2)^3"}
            }, "required": ["expression"]}),
        ),
        func(
            "current_datetime",
            "Get the user's current local date, time, weekday and time zone.",
            json!({"type": "object", "properties": {}}),
        ),
        func(
            "report_findings",
            "Present code-review findings to the user as a structured review card (sorted by severity, with the code line shown). Call it once at the end of a review with all findings; then write only a short overview.",
            json!({"type": "object", "properties": {
                "summary": {"type": "string", "description": "One or two sentences: overall assessment of the reviewed code"},
                "findings": {"type": "array", "items": {"type": "object", "properties": {
                    "path": {"type": "string", "description": "File path relative to the project root"},
                    "line": {"type": "integer", "description": "1-based line number the issue is on"},
                    "severity": {"type": "string", "enum": ["critical", "high", "medium", "low", "info"]},
                    "title": {"type": "string", "description": "Short statement of the problem"},
                    "detail": {"type": "string", "description": "Why it is a problem / when it breaks"},
                    "suggestion": {"type": "string", "description": "Concrete fix"}
                }, "required": ["title", "severity"]}}
            }, "required": ["findings"]}),
        ),
    ];
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
        v.push(func(
            "grep",
            "Exact text or regex search through the project's files, returning every matching line with line numbers and surrounding context. Use it for identifiers, error messages, TODOs and usages (search_project is better for concepts).",
            json!({"type": "object", "properties": {
                "pattern": {"type": "string", "description": "Text to find (literal unless regex is true)"},
                "regex": {"type": "boolean", "description": "Treat pattern as a regular expression (Rust regex syntax), default false"},
                "case_sensitive": {"type": "boolean", "description": "Default false"},
                "whole_word": {"type": "boolean", "description": "Match whole words only, default false"},
                "path": {"type": "string", "description": "Sub-folder or file to search, default the whole project"},
                "glob": {"type": "string", "description": "File filter such as *.rs or src/**/*.ts (comma-separated for several)"},
                "context": {"type": "integer", "description": "Context lines before and after each match, default 2"},
                "max_results": {"type": "integer", "description": "Max matching lines, default 60"}
            }, "required": ["pattern"]}),
        ));
        v.push(func(
            "file_outline",
            "List the definitions in a file (functions, classes, structs, methods, headings…) with line numbers, to get its structure before reading the parts that matter.",
            json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
        ));
        v.push(func(
            "find_symbol",
            "Find where an identifier (function, type, variable, constant) is defined and every place it is used across the project. Use it to check the callers before changing a signature, and to follow code during a review.",
            json!({"type": "object", "properties": {
                "name": {"type": "string", "description": "Exact identifier, case-sensitive"},
                "path": {"type": "string", "description": "Limit to a sub-folder, default the whole project"},
                "glob": {"type": "string", "description": "File filter such as *.py"}
            }, "required": ["name"]}),
        ));
        v.push(func(
            "compare_files",
            "Show the differences between two project files as a unified diff.",
            json!({"type": "object", "properties": {
                "path_a": {"type": "string"},
                "path_b": {"type": "string"}
            }, "required": ["path_a", "path_b"]}),
        ));
        v.push(func(
            "git_status",
            "Show the git branch, the changed / staged / untracked files and the latest commits of the project. The user is asked to allow it first.",
            json!({"type": "object", "properties": {}}),
        ));
        v.push(func(
            "git_diff",
            "Show the git diff of the project: uncommitted changes by default, staged changes, or the changes since a revision (branch, tag, commit, HEAD~3). The user is asked to allow it first.",
            json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "Limit to a file or folder"},
                "staged": {"type": "boolean", "description": "Only staged changes"},
                "base": {"type": "string", "description": "Compare against this revision, e.g. main or HEAD~1 (use A...B for a branch range)"}
            }}),
        ));
        v.push(func(
            "audit_code",
            "Audit a whole project or folder for bugs and security issues, however large: the files are reviewed in batches, each in a fresh context, and all findings appear to the user as one review card. Use it for full-codebase or folder-wide audits instead of reading every file yourself (that would overflow your context). It can take several minutes on big projects.",
            json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "Folder or file to audit, default the whole project"},
                "glob": {"type": "string", "description": "File filter such as *.rs or src/**/*.ts"},
                "focus": {"type": "string", "description": "Optional emphasis, e.g. security, error handling, concurrency"},
                "language": {"type": "string", "enum": ["en", "ro"], "description": "Language of the user's messages; findings are written in it"},
                "max_files": {"type": "integer", "description": "Default 400"}
            }}),
        ));
        v.push(func(
            "run_command",
            "Run one of the project's own check commands in the project folder (tests, linter, type-checker, build), e.g. `cargo test`, `npm test`, `pytest -q`, `ruff check .`, `tsc --noEmit`. Runs through cmd.exe on Windows. The user sees the exact command and must allow it; never use it to modify, delete, install or download anything.",
            json!({"type": "object", "properties": {
                "command": {"type": "string", "description": "Single command line"},
                "cwd": {"type": "string", "description": "Sub-folder to run in, default the project root"},
                "timeout_sec": {"type": "integer", "description": "Default 180, max 900"}
            }, "required": ["command"]}),
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
    } else {
        // (always listed, so the prompt stays the same whether or not a link has appeared yet)
        v.push(func(
            "fetch_url",
            "Read a web page (or PDF) whose link appears in this conversation - in the user's messages or in a project file you read - or a page under such a link (for a GitHub repository also its raw files). Returns the readable text. Web search is off: other addresses are refused.",
            json!({"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}),
        ));
    }
    v
}

/// http(s) links in a text (Markdown and quotes around them are not part of the link).
pub fn extract_links(text: &str) -> Vec<String> {
    static LINK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = LINK.get_or_init(|| regex::Regex::new(r#"(?i)\bhttps?://[^\s<>"'`()\[\]{}|\\^]+"#).expect("link regex"));
    re.find_iter(text)
        .map(|m| m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?', '*', '_']).to_string())
        .filter(|l| l.len() > "https://x".len())
        .collect()
}

/// Links the user gave in this chat: in their messages, and in project files the model read. Links
/// on fetched pages and search results don't count, so with Web off the model can't crawl outward.
fn conversation_links(state: &AppState, chat_id: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for m in state.db.messages(chat_id)? {
        let from_web = matches!(m.meta["name"].as_str(), Some("fetch_url" | "web_search"));
        if m.role == "user" || (m.role == "tool" && !from_web) {
            out.extend(extract_links(&m.content));
            // (a clipped or pruned tool result keeps its full text in meta)
            if let Some(full) = m.meta["full"].as_str() {
                out.extend(extract_links(full));
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// "https://www.GitHub.com/a/b/#x" -> "github.com/a/b": scheme, www., fragment and trailing / don't matter.
fn norm_link(url: &str) -> String {
    let s = url.trim();
    // parsed like the request will be: `..` segments resolved, user@ and default ports dropped
    if let Ok(u) = reqwest::Url::parse(s) {
        if let Some(host) = u.host_str() {
            let host = host.to_ascii_lowercase();
            let host = host.strip_prefix("www.").unwrap_or(&host);
            let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
            let query = u.query().map(|q| format!("?{q}")).unwrap_or_default();
            return format!("{host}{port}{}{query}", u.path().trim_end_matches('/'));
        }
    }
    let s = s.split('#').next().unwrap_or(s);
    let s = s.split_once("://").map(|(_, rest)| rest).unwrap_or(s);
    let (host, path) = s.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((s, String::new()));
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    format!("{host}{}", path.trim_end_matches('/'))
}

/// May `url` be opened with Web off? It must be one of `links` or lie under one (same host, the path
/// continues after a `/`, `?` or the end). A GitHub repository link also covers its raw files.
pub fn link_allowed(url: &str, links: &[String]) -> bool {
    let u = norm_link(url);
    let under = |base: &str| u == base || u.strip_prefix(base).is_some_and(|rest| rest.starts_with('/') || rest.starts_with('?'));
    links.iter().any(|l| {
        let base = norm_link(l);
        if under(&base) {
            return true;
        }
        // github.com/<owner>/<repo>/… -> raw.githubusercontent.com/<owner>/<repo>/…
        let mut parts = base.splitn(4, '/');
        match (parts.next(), parts.next(), parts.next()) {
            (Some("github.com"), Some(owner), Some(repo)) if !owner.is_empty() && !repo.is_empty() => {
                under(&format!("raw.githubusercontent.com/{owner}/{repo}"))
            }
            _ => false,
        }
    })
}

/// Does `url` point at this computer or the local network (localhost, 192.168.x.x, *.local…)? With
/// Web on, the model may still open such an address only when the user gave the link.
pub fn is_local_address(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url.trim()) else { return false };
    let Some(host) = u.host_str() else { return false };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    let v4 = |ip: std::net::Ipv4Addr| {
        let o = ip.octets();
        ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || (o[0] == 100 && (64..128).contains(&o[1]))
    };
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => v4(ip),
        Ok(std::net::IpAddr::V6(ip)) => {
            let first = ip.segments()[0];
            ip.is_loopback() || ip.is_unspecified() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 || ip.to_ipv4_mapped().is_some_and(v4)
        }
        Err(_) => {
            host == "localhost" || !host.contains('.') || [".localhost", ".local", ".lan", ".internal", ".home.arpa"].iter().any(|s| host.ends_with(s))
        }
    }
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
    // a symlink or junction inside the project must not lead outside it: the deepest part of the
    // path that exists (a dangling link counts) has to resolve to a place under the root
    let mut existing = norm.as_path();
    while existing.symlink_metadata().is_err() {
        match existing.parent() {
            Some(p) => existing = p,
            None => break,
        }
    }
    if !existing.canonicalize().is_ok_and(|real| dunce(real).starts_with(&root)) {
        return Err(anyhow!("path is outside the project folder (it goes through a link)"));
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
        .filter_entry(|e| !codetools::SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
        .build();
    for e in walker.flatten().skip(1) {
        let d = e.depth();
        let name = e.file_name().to_string_lossy();
        let is_dir = e.file_type().is_some_and(|t| t.is_dir());
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
pub async fn run(ctx: &ToolCtx<'_>, call_id: &str, name: &str, args_raw: &str) -> ToolOutput {
    let args: Value = serde_json::from_str(args_raw).unwrap_or_else(|_| json!({}));
    match run_inner(ctx, call_id, name, &args).await {
        Ok(o) => o,
        Err(e) => ToolOutput { content: format!("Error: {e}"), display: json!({"kind": "error", "error": e.to_string()}), ok: false },
    }
}

fn s<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().ok_or_else(|| anyhow!("missing argument `{k}`"))
}

fn out((content, display): (String, Value)) -> ToolOutput {
    ToolOutput { content, display, ok: true }
}

/// Runs blocking file work (walks, regex over many files) off the async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await?
}

async fn run_inner(ctx: &ToolCtx<'_>, call_id: &str, name: &str, a: &Value) -> Result<ToolOutput> {
    let state = ctx.app.state::<Arc<AppState>>().inner().clone();
    let need_project = || ctx.project.ok_or_else(|| anyhow!("this tool is only available inside a project"));
    match name {
        "calculate" => {
            let e = s(a, "expression")?;
            let v = crate::calc::eval(e)?;
            let r = crate::calc::format_number(v);
            Ok(ToolOutput { content: format!("{e} = {r}"), display: json!({"kind": "calc", "expression": e, "result": r}), ok: true })
        }
        "current_datetime" => {
            let (dt, weekday, offset, tz) = crate::calc::local_now();
            Ok(ToolOutput {
                content: format!("Local time: {dt} ({weekday}), UTC{offset}, time zone: {tz}"),
                display: json!({"kind": "datetime", "text": format!("{dt} · {weekday} · UTC{offset}")}),
                ok: true,
            })
        }
        "report_findings" => {
            let folder = ctx.project.map(|p| p.folder.clone());
            let a = a.clone();
            blocking(move || codetools::findings(folder.as_deref(), &a)).await.map(out)
        }
        "grep" => {
            let p = need_project()?;
            let args = codetools::GrepArgs {
                pattern: s(a, "pattern")?.to_string(),
                regex: a["regex"].as_bool().unwrap_or(false),
                case_sensitive: a["case_sensitive"].as_bool().unwrap_or(false),
                whole_word: a["whole_word"].as_bool().unwrap_or(false),
                path: a["path"].as_str().unwrap_or("").to_string(),
                glob: a["glob"].as_str().map(String::from),
                context: a["context"].as_u64().unwrap_or(2).min(10) as usize,
                max_results: a["max_results"].as_u64().unwrap_or(60).clamp(1, 300) as usize,
            };
            if args.pattern.is_empty() {
                return Err(anyhow!("pattern is empty"));
            }
            let folder = p.folder.clone();
            blocking(move || codetools::grep(&folder, &args)).await.map(out)
        }
        "file_outline" => {
            let p = need_project()?;
            let (folder, path) = (p.folder.clone(), s(a, "path")?.to_string());
            blocking(move || codetools::outline(&folder, &path)).await.map(out)
        }
        "find_symbol" => {
            let p = need_project()?;
            let (folder, sym) = (p.folder.clone(), s(a, "name")?.to_string());
            let path = a["path"].as_str().unwrap_or("").to_string();
            let glob = a["glob"].as_str().map(String::from);
            blocking(move || codetools::find_symbol(&folder, &sym, &path, glob.as_deref())).await.map(out)
        }
        "compare_files" => {
            let p = need_project()?;
            let (folder, x, y) = (p.folder.clone(), s(a, "path_a")?.to_string(), s(a, "path_b")?.to_string());
            blocking(move || codetools::compare(&folder, &x, &y)).await.map(out)
        }
        "audit_code" => {
            let p = need_project()?;
            let c = crate::audit::AuditCtx { app: ctx.app, chat_id: ctx.chat_id, call_id, folder: &p.folder, cancel: ctx.cancel };
            crate::audit::run(&c, a).await.map(out)
        }
        "git_status" | "git_diff" | "run_command" => {
            let p = need_project()?;
            let g = codetools::GitCtx { app: ctx.app, chat_id: ctx.chat_id, call_id, folder: &p.folder, cancel: ctx.cancel };
            let r = match name {
                "git_status" => codetools::git_status(&g).await?,
                "git_diff" => codetools::git_diff(&g, a).await?,
                _ => codetools::run_command(&g, a).await?,
            };
            // a failing check is still a successful tool call, but mark it red in the UI
            let ok = r.1["ok"].as_bool().unwrap_or(true);
            Ok(ToolOutput { content: r.0, display: r.1, ok })
        }
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
            let local = is_local_address(url);
            if (!ctx.web || local) && !link_allowed(url, &conversation_links(&state, ctx.chat_id)?) {
                return Err(if local {
                    anyhow!("`{url}` is on this computer or the local network; such addresses open only when the user gives the link in a message.")
                } else {
                    anyhow!(
                        "`{url}` does not appear in this conversation. With Web off, only links from the user's messages or the project files you read (and pages under them) can be opened. Ask the user to turn on Web if you need more."
                    )
                });
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The field case: "Develop what is written inside the TODO.txt", whose lines link the reference repo.
    #[test]
    fn links_in_a_file_are_found() {
        let todo = "Make setup, update work just like https://github.com/Akulav/ai-girlfriend with inno setup installer.\n\
                    Structure the readme in similar way as https://github.com/Akulav/ai-girlfriend\n\
                    See [the docs](https://example.org/a/b?x=1). Or <https://example.net/>, \"http://lan.local:8080/p\".";
        let links = extract_links(todo);
        assert_eq!(links, [
            "https://github.com/Akulav/ai-girlfriend",
            "https://github.com/Akulav/ai-girlfriend",
            "https://example.org/a/b?x=1",
            "https://example.net/",
            "http://lan.local:8080/p",
        ]);
        assert!(extract_links("no links, just example.com and ftp://x").is_empty());
    }

    #[test]
    fn only_given_links_and_pages_under_them_open() {
        let given = vec!["https://github.com/Akulav/ai-girlfriend".to_string(), "https://example.org/docs/".to_string()];
        let ok = |u: &str| link_allowed(u, &given);
        assert!(ok("https://github.com/Akulav/ai-girlfriend"));
        assert!(ok("http://www.github.com/Akulav/ai-girlfriend/"), "scheme, www. and a trailing / don't matter");
        assert!(ok("https://github.com/Akulav/ai-girlfriend/blob/main/README.md"));
        assert!(ok("https://raw.githubusercontent.com/Akulav/ai-girlfriend/main/.github/workflows/release.yml"), "a repo's raw files");
        assert!(ok("https://example.org/docs/install?lang=en"));
        assert!(ok("https://example.org/docs#setup"));
        assert!(!ok("https://github.com/Akulav/ai-girlfriend-evil"), "a longer name is another repo");
        assert!(!ok("https://github.com/Akulav"), "the parent is not under the link");
        assert!(!ok("https://raw.githubusercontent.com/Akulav/other/main/x"));
        assert!(!ok("https://example.org/doc"));
        assert!(!ok("https://evil.com/github.com/Akulav/ai-girlfriend"));
        assert!(!link_allowed("https://github.com/Akulav/ai-girlfriend", &[]));
        assert!(!ok("https://github.com/Akulav/ai-girlfriend/../../evil/x"), "`..` is resolved like the request resolves it");
        assert!(!ok("https://github.com@evil.com/Akulav/ai-girlfriend"), "user@ is not the host");
        assert!(ok("https://github.com:443/Akulav/ai-girlfriend/issues"), "the default port doesn't matter");
    }

    /// A junction inside the project that points outside it must not let the model read or write there.
    #[cfg(windows)]
    #[test]
    fn links_out_of_the_project_are_refused() {
        let base = std::env::temp_dir().join(format!("lumen-resolve-{}", new_id()));
        let (proj, outside) = (base.join("proj"), base.join("outside"));
        std::fs::create_dir_all(proj.join("src")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let st = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(proj.join("escape")).arg(&outside).output().unwrap();
        assert!(st.status.success(), "mklink /J failed");
        assert!(resolve(&proj, "src/new.txt").is_ok(), "a new file in a real folder");
        assert!(resolve(&proj, "escape").is_err());
        assert!(resolve(&proj, "escape/secret.txt").is_err(), "a file that doesn't exist yet behind the link");
        assert!(resolve(&proj, "src/../escape/x/y.txt").is_err());
        let _ = std::fs::remove_dir(proj.join("escape"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn local_addresses_are_recognised() {
        for u in ["http://localhost:8080/x", "http://127.0.0.1/", "http://192.168.1.1/admin", "http://10.0.0.5", "http://[::1]:3000/", "http://169.254.169.254/latest", "http://router/", "http://nas.local/", "http://[::ffff:192.168.0.1]/"] {
            assert!(is_local_address(u), "{u}");
        }
        for u in ["https://github.com/a/b", "https://8.8.8.8/", "https://en.wikipedia.org/wiki/X", "not a url"] {
            assert!(!is_local_address(u), "{u}");
        }
    }

    /// A public repo link and a raw file under it, read the way fetch_url reads them (the field case's repo is private now). Network:
    /// `cargo test live_link_fetch -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_link_fetch() {
        let state = AppState::new().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let given = extract_links("Make it work like https://github.com/Akulav/local-ai-ui please.");
        for url in ["https://github.com/Akulav/local-ai-ui", "https://raw.githubusercontent.com/Akulav/local-ai-ui/main/.github/workflows/release.yml"] {
            assert!(link_allowed(url, &given), "{url}");
            let page = rt.block_on(crate::web::fetch(&state, url)).unwrap();
            println!("--- {url}: title {:?}, {} chars\n{}\n", page.title, page.text.len(), page.text.chars().take(400).collect::<String>());
            assert!(page.text.len() > 1000, "readable text came back");
        }
    }

    #[test]
    fn web_off_still_lists_fetch_url() {
        let names = |web: bool| -> Vec<String> {
            definitions(false, web).iter().map(|d| d["function"]["name"].as_str().unwrap().to_string()).collect()
        };
        assert!(names(false).contains(&"fetch_url".to_string()) && !names(false).contains(&"web_search".to_string()));
        assert!(names(true).contains(&"fetch_url".to_string()) && names(true).contains(&"web_search".to_string()));
    }
}
