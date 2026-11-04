//! Code-review tools: grep with context, file outlines, symbol definitions/references, git status and
//! diff, running the project's own checks (after the user confirms), structured review findings and
//! comparing two files. Every path goes through `tools::resolve`, so nothing leaves the project folder.

use anyhow::{anyhow, Result};
use regex::{Regex, RegexBuilder};
use serde_json::{json, Value};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio::io::AsyncReadExt;

use crate::state::{new_id, AppState};
use crate::tools::resolve;

/// Folders never worth searching (build output, dependencies, VCS data).
pub const SKIP_DIRS: [&str; 9] = ["node_modules", ".git", "target", "__pycache__", ".venv", "venv", "dist", "build", "LumenData"];
const MAX_FILE_BYTES: u64 = 2_000_000;

/// Walks the text files under `base` (gitignore-aware, skips hidden + build folders), optionally
/// filtered by a glob such as `*.rs` or `src/**/*.ts`.
fn walk_files(base: &Path, glob: Option<&str>) -> Result<Vec<PathBuf>> {
    if base.is_file() {
        return Ok(vec![base.to_path_buf()]);
    }
    let mut wb = ignore::WalkBuilder::new(base);
    wb.hidden(true).git_ignore(true).sort_by_file_name(|a, b| a.cmp(b)).filter_entry(|e| {
        let n = e.file_name().to_string_lossy();
        !SKIP_DIRS.contains(&n.as_ref())
    });
    if let Some(g) = glob.map(str::trim).filter(|g| !g.is_empty()) {
        let mut ob = ignore::overrides::OverrideBuilder::new(base);
        for part in g.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
            ob.add(part).map_err(|e| anyhow!("bad glob `{part}`: {e}"))?;
        }
        wb.overrides(ob.build()?);
    }
    Ok(wb
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter(|e| e.metadata().map(|m| m.len() <= MAX_FILE_BYTES).unwrap_or(false))
        .map(|e| e.into_path())
        .collect())
}

/// Reads a file as text; None for binary files (NUL byte in the first 8 KB).
fn read_text(p: &Path) -> Option<String> {
    let bytes = std::fs::read(p).ok()?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

fn canonical_root(folder: &str) -> Result<PathBuf> {
    let (root, _) = resolve(Path::new(folder), "")?;
    Ok(root)
}

// ------------------------------------------------------------------ grep

pub struct GrepArgs {
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub path: String,
    pub glob: Option<String>,
    pub context: usize,
    pub max_results: usize,
}

/// Line-based search with context lines, like `grep -n -C`. Returns (model text, UI display).
pub fn grep(folder: &str, a: &GrepArgs) -> Result<(String, Value)> {
    let root = canonical_root(folder)?;
    let (base, _) = resolve(Path::new(folder), &a.path)?;
    if !base.exists() {
        return Err(anyhow!("`{}` does not exist in the project", a.path));
    }
    let mut pat = if a.regex { a.pattern.clone() } else { regex::escape(&a.pattern) };
    if a.whole_word {
        pat = format!(r"\b(?:{pat})\b");
    }
    let re = RegexBuilder::new(&pat).case_insensitive(!a.case_sensitive).size_limit(1 << 22).build().map_err(|e| anyhow!("invalid regex: {e}"))?;
    let files = walk_files(&base, a.glob.as_deref())?;
    let mut out = String::new();
    let (mut matches, mut files_hit, mut truncated) = (0usize, 0usize, false);
    let mut hits = Vec::new();
    'files: for f in &files {
        let Some(text) = read_text(f) else { continue };
        let lines: Vec<&str> = text.lines().collect();
        let idx: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| re.is_match(l)).map(|(i, _)| i).collect();
        if idx.is_empty() {
            continue;
        }
        files_hit += 1;
        let rel = rel_of(&root, f);
        // merge overlapping context windows into blocks
        let mut blocks: Vec<(usize, usize)> = Vec::new();
        for &i in &idx {
            let (s, e) = (i.saturating_sub(a.context), (i + a.context).min(lines.len() - 1));
            match blocks.last_mut() {
                Some(b) if s <= b.1 + 1 => b.1 = e,
                _ => blocks.push((s, e)),
            }
        }
        out.push_str(&format!("## {rel}\n"));
        for (bi, (s, e)) in blocks.iter().enumerate() {
            if bi > 0 {
                out.push_str("--\n");
            }
            for (i, line) in lines.iter().enumerate().take(e + 1).skip(*s) {
                let is_hit = idx.binary_search(&i).is_ok();
                let line: String = line.chars().take(400).collect();
                out.push_str(&format!("{:>5}{} {}\n", i + 1, if is_hit { ':' } else { '-' }, line));
                if is_hit {
                    matches += 1;
                    if hits.len() < 40 {
                        hits.push(json!({"path": rel, "lines": (i + 1).to_string()}));
                    }
                    if matches >= a.max_results {
                        truncated = true;
                        break 'files;
                    }
                }
            }
        }
        out.push('\n');
        if out.len() > 60_000 {
            truncated = true;
            break;
        }
    }
    if matches == 0 {
        return Ok((format!("No matches for `{}` in {} files.", a.pattern, files.len()), json!({"kind": "grep", "matches": 0, "files": 0, "hits": []})));
    }
    let head = format!(
        "{matches} matching lines in {files_hit} files ({} files searched){}. Format: `line:` is a match, `line-` is context.\n\n",
        files.len(),
        if truncated { ", stopped early: narrow the pattern, path or glob to see more" } else { "" }
    );
    Ok((head + &out, json!({"kind": "grep", "matches": matches, "files": files_hit, "truncated": truncated, "hits": hits})))
}

// ------------------------------------------------------------------ outline / symbols

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Definition-line regex per language family (heuristic, no parser).
fn def_regex(ext: &str) -> &'static Regex {
    static RS: OnceLock<Regex> = OnceLock::new();
    static JS: OnceLock<Regex> = OnceLock::new();
    static PY: OnceLock<Regex> = OnceLock::new();
    static GO: OnceLock<Regex> = OnceLock::new();
    static JAVA: OnceLock<Regex> = OnceLock::new();
    static C: OnceLock<Regex> = OnceLock::new();
    static MD: OnceLock<Regex> = OnceLock::new();
    static PS: OnceLock<Regex> = OnceLock::new();
    static CSS: OnceLock<Regex> = OnceLock::new();
    static ANY: OnceLock<Regex> = OnceLock::new();
    let mk = |cell: &'static OnceLock<Regex>, p: &str| -> &'static Regex { cell.get_or_init(|| Regex::new(p).expect("outline regex")) };
    match ext {
        "rs" => mk(&RS, r"^\s*(?:#\[[^\]]*\]\s*)?(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?(?:extern\s+\S+\s+)?(?:fn|struct|enum|union|trait|impl|mod|type|const|static|macro_rules!)\b"),
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "mts" | "cts" | "vue" | "svelte" => mk(
            &JS,
            r"^(?:export\s+(?:default\s+)?)?(?:declare\s+)?(?:abstract\s+)?(?:async\s+)?(?:function\*?\s*[\w$]+|class\s+[\w$]+|interface\s+[\w$]+|type\s+[\w$]+\s*[=<]|enum\s+[\w$]+|namespace\s+[\w$]+|(?:const|let|var)\s+[\w$]+\s*=)|^\s+(?:(?:public|private|protected|static|readonly|override|async|get|set)\s+)*(?:[\w$]+)\s*\([^)]*\)\s*(?::\s*[^{]+)?\{\s*$",
        ),
        "py" | "pyi" => mk(&PY, r"^\s*(?:async\s+)?(?:def|class)\s+\w+"),
        "go" => mk(&GO, r"^(?:func|type)\s+|^(?:var|const)\s+\w+"),
        "java" | "kt" | "kts" | "cs" | "scala" | "swift" | "dart" => mk(
            &JAVA,
            r"^\s*(?:@\w+\s+)*(?:(?:public|private|protected|internal|static|final|abstract|sealed|override|virtual|async|open|data|inline|suspend|partial|readonly)\s+)*(?:class|interface|enum|record|struct|object|fun|func|namespace|extension)\s+\w+|^\s*(?:(?:public|private|protected|internal|static|final|abstract|override|virtual|async|synchronized)\s+)+[\w<>\[\],.?\s]*?\b\w+\s*\([^;]*$",
        ),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "m" | "mm" => mk(
            &C,
            r"^(?:template\s*<[^>]*>\s*)?(?:typedef\s+)?(?:struct|class|enum|union|namespace)\s+\w+|^[A-Za-z_][\w\s\*&:<>,]*[\s\*&]+[\w:~]+\s*\([^;]*$|^#define\s+\w+",
        ),
        "md" | "markdown" | "mdx" => mk(&MD, r"^#{1,6}\s+\S"),
        "ps1" | "psm1" => mk(&PS, r"(?i)^\s*(?:function|filter|class|enum)\s+[\w-]+"),
        "css" | "scss" | "less" => mk(&CSS, r"^[^\s{}][^{}]*\{\s*$|^@media"),
        _ => mk(&ANY, r"^\s*(?:export\s+)?(?:pub\s+)?(?:async\s+)?(?:def|class|function|fn|func|struct|enum|interface|trait|impl|module|type|sub|proc)\s+[\w$]+"),
    }
}

/// A file's top-level structure: definition lines with their line numbers and indentation.
pub fn outline(folder: &str, rel: &str) -> Result<(String, Value)> {
    let (full, rel) = resolve(Path::new(folder), rel)?;
    if !full.is_file() {
        return Err(anyhow!("`{rel}` is not a file"));
    }
    let text = read_text(&full).ok_or_else(|| anyhow!("`{rel}` is a binary file"))?;
    let re = def_regex(&ext_of(&full));
    let total = text.lines().count();
    let mut out = String::new();
    let mut n = 0;
    for (i, line) in text.lines().enumerate() {
        if !re.is_match(line) {
            continue;
        }
        let t = line.trim_end();
        let t = t.trim_end_matches('{').trim_end();
        let t: String = t.chars().take(160).collect();
        out.push_str(&format!("{:>5}  {}\n", i + 1, t));
        n += 1;
        if n >= 500 {
            out.push_str("… (outline truncated)\n");
            break;
        }
    }
    if n == 0 {
        out = "(no definitions recognised; use read_file instead)\n".into();
    }
    Ok((format!("Outline of {rel} ({total} lines, {n} entries)\n{out}"), json!({"kind": "outline", "path": rel, "count": n, "total": total})))
}

/// Where an identifier is defined and where it is used (word-boundary match, case-sensitive).
pub fn find_symbol(folder: &str, name: &str, path: &str, glob: Option<&str>) -> Result<(String, Value)> {
    let name = name.trim();
    if name.is_empty() || name.len() > 200 {
        return Err(anyhow!("give a symbol name"));
    }
    let root = canonical_root(folder)?;
    let (base, _) = resolve(Path::new(folder), path)?;
    let word = Regex::new(&format!(r"(?:^|[^\w$]){}(?:[^\w$]|$)", regex::escape(name)))?;
    let mut defs = Vec::new();
    let mut refs = Vec::new();
    let mut ref_count = 0usize;
    for f in walk_files(&base, glob)? {
        let Some(text) = read_text(&f) else { continue };
        if !text.contains(name) {
            continue;
        }
        let re = def_regex(&ext_of(&f));
        let rel = rel_of(&root, &f);
        for (i, line) in text.lines().enumerate() {
            if !word.is_match(line) {
                continue;
            }
            let snippet: String = line.trim().chars().take(200).collect();
            if re.is_match(line) && defines(line, name) {
                defs.push((rel.clone(), i + 1, snippet));
            } else {
                ref_count += 1;
                if refs.len() < 120 {
                    refs.push((rel.clone(), i + 1, snippet));
                }
            }
        }
    }
    let mut out = format!("Symbol `{name}`: {} definition(s), {ref_count} reference(s)\n\n## Definitions\n", defs.len());
    if defs.is_empty() {
        out.push_str("(none found; it may come from a dependency, or be defined dynamically)\n");
    }
    for (p, l, s) in &defs {
        out.push_str(&format!("{p}:{l}  {s}\n"));
    }
    out.push_str("\n## References\n");
    if refs.is_empty() {
        out.push_str("(none)\n");
    }
    for (p, l, s) in &refs {
        out.push_str(&format!("{p}:{l}  {s}\n"));
    }
    if ref_count > refs.len() {
        out.push_str(&format!("… {} more; use `grep` with a path or glob to narrow\n", ref_count - refs.len()));
    }
    let hits: Vec<Value> = defs.iter().take(20).map(|(p, l, _)| json!({"path": p, "lines": l.to_string(), "def": true})).collect();
    Ok((out, json!({"kind": "symbol", "name": name, "defs": defs.len(), "refs": ref_count, "hits": hits})))
}

/// True when the identifier right after a definition keyword (or the assigned name) is `name`.
fn defines(line: &str, name: &str) -> bool {
    static AFTER_KW: OnceLock<Regex> = OnceLock::new();
    let re = AFTER_KW.get_or_init(|| {
        Regex::new(r"(?:fn|struct|enum|union|trait|mod|type|const|static|let|var|function\*?|class|interface|namespace|def|func|fun|record|object|macro_rules!|define|impl(?:<[^>]*>)?)\s+([A-Za-z_$][\w$]*)").expect("defines regex")
    });
    if re.captures_iter(line).any(|c| &c[1] == name) {
        return true;
    }
    // methods / C-style functions: `name(` is the first call-like token on the line
    let t = line.trim_start();
    if let Some(pos) = t.find('(') {
        let before = t[..pos].trim_end();
        let last = before.rsplit(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$' || c == ':' || c == '~')).next().unwrap_or("");
        return last == name || last.ends_with(&format!("::{name}"));
    }
    false
}

// ------------------------------------------------------------------ findings

const SEVERITIES: [&str; 5] = ["critical", "high", "medium", "low", "info"];

/// Validates and normalises the model's structured review findings (paths inside the project, line
/// numbers that exist, known severities) and attaches the referenced source line for the UI card.
pub fn findings(folder: Option<&str>, a: &Value) -> Result<(String, Value)> {
    let list = a["findings"].as_array().ok_or_else(|| anyhow!("missing argument `findings` (an array)"))?;
    let summary = a["summary"].as_str().unwrap_or("").trim().to_string();
    let mut items = Vec::new();
    let mut problems = Vec::new();
    for (i, f) in list.iter().enumerate().take(100) {
        let title = f["title"].as_str().or(f["issue"].as_str()).unwrap_or("").trim().to_string();
        if title.is_empty() {
            problems.push(format!("#{}: missing title", i + 1));
            continue;
        }
        let mut sev = f["severity"].as_str().unwrap_or("medium").trim().to_lowercase();
        sev = match sev.as_str() {
            "blocker" | "critical" => "critical",
            "major" | "high" | "error" => "high",
            "minor" | "low" | "warning" => "low",
            "info" | "nit" | "note" | "suggestion" => "info",
            _ => "medium",
        }
        .to_string();
        let mut path = f["path"].as_str().or(f["file"].as_str()).unwrap_or("").trim().to_string();
        let line = f["line"].as_u64().or_else(|| f["line"].as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(0) as usize;
        let mut code = String::new();
        if let (Some(folder), false) = (folder, path.is_empty()) {
            match resolve(Path::new(folder), &path) {
                Ok((full, rel)) => {
                    path = rel.clone();
                    match read_text(&full) {
                        Some(text) => {
                            let n = text.lines().count();
                            if line > n {
                                problems.push(format!("#{} {rel}: line {line} does not exist ({n} lines)", i + 1));
                            } else if line > 0 {
                                code = text.lines().nth(line - 1).unwrap_or("").trim().chars().take(240).collect();
                            }
                        }
                        None => problems.push(format!("#{} {rel}: file not found or not text", i + 1)),
                    }
                }
                Err(e) => {
                    problems.push(format!("#{} {path}: {e}", i + 1));
                    continue;
                }
            }
        }
        items.push(json!({
            "path": path, "line": line, "severity": sev, "title": title,
            "detail": f["detail"].as_str().or(f["description"].as_str()).unwrap_or("").trim(),
            "suggestion": f["suggestion"].as_str().or(f["fix"].as_str()).unwrap_or("").trim(),
            "code": code,
        }));
    }
    items.sort_by_key(|f| SEVERITIES.iter().position(|s| *s == f["severity"].as_str().unwrap_or("")).unwrap_or(9));
    let mut counts = serde_json::Map::new();
    for s in SEVERITIES {
        let n = items.iter().filter(|f| f["severity"] == s).count();
        if n > 0 {
            counts.insert(s.into(), json!(n));
        }
    }
    let mut msg = format!("Recorded {} finding(s); the user sees them as a review card, so do not repeat each one in full.", items.len());
    if !problems.is_empty() {
        msg.push_str(&format!(" Check these and correct them if needed: {}.", problems.join("; ")));
    }
    Ok((msg, json!({"kind": "findings", "summary": summary, "findings": items, "counts": counts})))
}

// ------------------------------------------------------------------ compare

pub fn compare(folder: &str, a_rel: &str, b_rel: &str) -> Result<(String, Value)> {
    let (fa, ra) = resolve(Path::new(folder), a_rel)?;
    let (fb, rb) = resolve(Path::new(folder), b_rel)?;
    let ta = read_text(&fa).ok_or_else(|| anyhow!("cannot read `{ra}` as text"))?.replace("\r\n", "\n");
    let tb = read_text(&fb).ok_or_else(|| anyhow!("cannot read `{rb}` as text"))?.replace("\r\n", "\n");
    let d = similar::TextDiff::from_lines(&ta, &tb);
    let (mut add, mut del) = (0, 0);
    for ch in d.iter_all_changes() {
        match ch.tag() {
            similar::ChangeTag::Insert => add += 1,
            similar::ChangeTag::Delete => del += 1,
            _ => {}
        }
    }
    let mut unified = d.unified_diff().context_radius(3).header(&format!("a/{ra}"), &format!("b/{rb}")).to_string();
    if unified.len() > 60_000 {
        unified.truncate(unified.floor_char_boundary(60_000));
        unified.push_str("\n… (diff truncated)");
    }
    let content = if add + del == 0 { format!("`{ra}` and `{rb}` are identical.") } else { format!("`{ra}` → `{rb}`: +{add} −{del}\n{unified}") };
    Ok((content, json!({"kind": "diff", "compare": true, "path": format!("{ra} ↔ {rb}"), "added": add, "removed": del, "diff": unified})))
}

// ------------------------------------------------------------------ processes (git, checks)

/// Asks the user in the chat to allow a process. "always" remembers the scope (`git` or `run`) for
/// the rest of this chat while the app runs. Stopping the generation counts as a refusal.
pub async fn confirm(app: &AppHandle, chat_id: &str, call_id: &str, scope: &str, command: &str, cwd: &str, cancel: &AtomicBool) -> Result<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if state.allowed_tools.lock().get(chat_id).is_some_and(|s| s.contains(scope)) {
        return Ok(());
    }
    let id = new_id();
    let (tx, mut rx) = tokio::sync::oneshot::channel::<String>();
    state.approvals.lock().insert(id.clone(), tx);
    crate::chat::emit(app, chat_id, "tool_confirm", json!({"id": id, "call_id": call_id, "scope": scope, "command": command, "cwd": cwd}));
    let decision = loop {
        tokio::select! {
            r = &mut rx => break r.unwrap_or_else(|_| "deny".into()),
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if cancel.load(Ordering::Relaxed) {
                    break "deny".into();
                }
            }
        }
    };
    state.approvals.lock().remove(&id);
    crate::chat::emit(app, chat_id, "tool_confirmed", json!({"id": id, "call_id": call_id, "decision": decision}));
    match decision.as_str() {
        "always" => {
            state.allowed_tools.lock().entry(chat_id.to_string()).or_default().insert(scope.to_string());
            Ok(())
        }
        "allow" => Ok(()),
        _ => Err(anyhow!("the user declined to run `{command}`. Do not retry it; continue without it or ask the user")),
    }
}

/// Finds git: PATH, Git for Windows, then the copy bundled with GitHub Desktop.
pub fn git_exe() -> Option<PathBuf> {
    static GIT: OnceLock<Option<PathBuf>> = OnceLock::new();
    GIT.get_or_init(|| {
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                let p = dir.join("git.exe");
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        for base in [std::env::var_os("ProgramFiles"), std::env::var_os("ProgramW6432"), std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("Programs").into_os_string())]
            .into_iter()
            .flatten()
        {
            let p = PathBuf::from(base).join("Git").join("cmd").join("git.exe");
            if p.is_file() {
                return Some(p);
            }
        }
        let gd = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("GitHubDesktop");
        let mut apps: Vec<PathBuf> = std::fs::read_dir(gd).ok()?.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("app-"))).collect();
        apps.sort();
        apps.into_iter().rev().map(|a| a.join("resources/app/git/cmd/git.exe")).find(|p| p.is_file())
    })
    .clone()
}

pub struct ProcOut {
    pub code: Option<i32>,
    pub output: String,
    pub timed_out: bool,
    pub cancelled: bool,
}

/// Runs a process without a console window inside the app's job object, with a timeout; stdout and
/// stderr are merged (stderr after stdout). The whole process tree is killed on timeout / stop.
pub async fn run_process(job: &crate::win::Job, exe: &Path, args: &[String], raw: Option<&str>, cwd: &Path, timeout: Duration, cancel: &AtomicBool) -> Result<ProcOut> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args);
    if let Some(r) = raw {
        cmd.raw_arg(r);
    }
    cmd.current_dir(cwd)
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("NO_COLOR", "1")
        .env("CI", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| anyhow!("cannot start {}: {e}", exe.display()))?;
    let pid = child.id().unwrap_or(0);
    job.adopt(pid);
    let read_all = |pipe: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf).await;
            }
            buf
        })
    };
    let out_task = read_all(child.stdout.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>));
    let err_task = read_all(child.stderr.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>));
    let cancelled = async {
        while !cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };
    let (status, timed_out, was_cancelled) = tokio::select! {
        r = child.wait() => (Some(r?), false, false),
        _ = tokio::time::sleep(timeout) => (None, true, false),
        _ = cancelled => (None, false, true),
    };
    if status.is_none() {
        // kill the tree while the parent still exists, so grandchildren (cargo, node…) die too
        kill_tree(pid);
        let _ = child.kill().await;
    }
    let grace = Duration::from_secs(3);
    let stdout = tokio::time::timeout(grace, out_task).await.ok().and_then(|r| r.ok()).unwrap_or_default();
    let stderr = tokio::time::timeout(grace, err_task).await.ok().and_then(|r| r.ok()).unwrap_or_default();
    let mut text = decode(&stdout);
    let err = decode(&stderr);
    if !err.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&err);
    }
    Ok(ProcOut { code: status.and_then(|s| s.code()), output: text, timed_out, cancelled: was_cancelled })
}

fn kill_tree(pid: u32) {
    if pid == 0 {
        return;
    }
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x08000000)
        .status();
}

/// UTF-8 when valid, else Latin-1-ish lossy (console tools on Windows may print in the OEM code page).
fn decode(b: &[u8]) -> String {
    let s = match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => String::from_utf8_lossy(b).into_owned(),
    };
    // strip ANSI colour codes
    static ANSI: OnceLock<Regex> = OnceLock::new();
    ANSI.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").expect("ansi regex")).replace_all(&s, "").replace("\r\n", "\n")
}

/// Keeps the start and (mostly) the end of long output: errors and test summaries come last.
pub fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let head = s.floor_char_boundary(max / 4);
    let tail_start = s.ceil_char_boundary(s.len() - (max - max / 4));
    format!("{}\n… ({} characters omitted) …\n{}", &s[..head], tail_start - head, &s[tail_start..])
}

/// A git ref the model may pass (branch, tag, HEAD~2, a..b); rejects anything option-like.
fn safe_ref(r: &str) -> Result<String> {
    let r = r.trim();
    let ok = !r.is_empty() && r.len() < 200 && !r.starts_with('-') && r.chars().all(|c| c.is_ascii_alphanumeric() || "._/~^@{}-:".contains(c));
    if ok { Ok(r.to_string()) } else { Err(anyhow!("`{r}` is not a valid git revision")) }
}

pub struct GitCtx<'a> {
    pub app: &'a AppHandle,
    pub chat_id: &'a str,
    pub call_id: &'a str,
    pub folder: &'a str,
    pub cancel: &'a AtomicBool,
}

async fn git(g: &GitCtx<'_>, args: &[&str]) -> Result<String> {
    let state = g.app.state::<Arc<AppState>>().inner().clone();
    let exe = git_exe().ok_or_else(|| anyhow!("git is not installed (looked in PATH, Git for Windows and GitHub Desktop)"))?;
    let mut full: Vec<String> = ["--no-pager", "-c", "core.quotepath=off", "-c", "color.ui=never", "-c", "core.pager=cat"].iter().map(|s| s.to_string()).collect();
    full.extend(args.iter().map(|s| s.to_string()));
    let o = run_process(&state.job, &exe, &full, None, Path::new(g.folder), Duration::from_secs(60), g.cancel).await?;
    if o.timed_out {
        return Err(anyhow!("git {} timed out", args.join(" ")));
    }
    if o.cancelled {
        return Err(anyhow!("stopped"));
    }
    if o.code != Some(0) {
        return Err(anyhow!("git {} failed: {}", args.join(" "), o.output.trim()));
    }
    Ok(o.output)
}

async fn ensure_repo(g: &GitCtx<'_>) -> Result<()> {
    if git_exe().is_none() {
        return Err(anyhow!("git is not installed (looked in PATH, Git for Windows and GitHub Desktop)"));
    }
    match git(g, &["rev-parse", "--is-inside-work-tree"]).await {
        Ok(s) if s.trim() == "true" => Ok(()),
        _ => Err(anyhow!("the project folder is not a git repository")),
    }
}

/// `git status` (branch + changed files) and the latest commits, for the project folder only.
pub async fn git_status(g: &GitCtx<'_>) -> Result<(String, Value)> {
    confirm(g.app, g.chat_id, g.call_id, "git", "git status --porcelain -b -- .  &&  git log --oneline -n 10 -- .", g.folder, g.cancel).await?;
    ensure_repo(g).await?;
    let status = git(g, &["status", "--porcelain=v1", "-b", "--untracked-files=all", "--", "."]).await?;
    let log = git(g, &["log", "--oneline", "--decorate=short", "-n", "10", "--", "."]).await.unwrap_or_default();
    let mut branch = String::new();
    let mut files = Vec::new();
    for l in status.lines() {
        if let Some(b) = l.strip_prefix("## ") {
            branch = b.to_string();
        } else if l.len() > 3 {
            let (code, path) = l.split_at(3);
            files.push(json!({"status": code.trim(), "path": path.trim().trim_matches('"')}));
        }
    }
    let mut out = format!("Branch: {branch}\n\n## Changed files ({})\n", files.len());
    if files.is_empty() {
        out.push_str("(working tree clean)\n");
    }
    for f in files.iter().take(300) {
        out.push_str(&format!("{:>2} {}\n", f["status"].as_str().unwrap_or(""), f["path"].as_str().unwrap_or("")));
    }
    out.push_str("\nStatus codes: M modified, A added, D deleted, R renamed, ?? untracked (first column = staged, second = unstaged).\n\n## Recent commits\n");
    out.push_str(if log.trim().is_empty() { "(none)\n" } else { &log });
    let shown: Vec<Value> = files.iter().take(60).cloned().collect();
    Ok((out, json!({"kind": "git_status", "branch": branch, "files": shown, "count": files.len()})))
}

/// `git diff` of the working tree (or staged changes, or against a revision), split per file.
pub async fn git_diff(g: &GitCtx<'_>, a: &Value) -> Result<(String, Value)> {
    let staged = a["staged"].as_bool().unwrap_or(false);
    let base = a["base"].as_str().filter(|s| !s.trim().is_empty()).map(safe_ref).transpose()?;
    let rel = match a["path"].as_str().filter(|s| !s.trim().is_empty()) {
        Some(p) => {
            let (_, rel) = resolve(Path::new(g.folder), p)?;
            if rel.is_empty() { ".".to_string() } else { rel }
        }
        None => ".".to_string(),
    };
    let mut args: Vec<String> = vec!["diff".into(), "--no-ext-diff".into(), "--unified=3".into(), "-M".into()];
    if staged {
        args.push("--staged".into());
    }
    if let Some(b) = &base {
        args.push(b.clone());
    }
    args.push("--".into());
    args.push(rel.clone());
    let shown = format!("git {}", args.join(" "));
    confirm(g.app, g.chat_id, g.call_id, "git", &shown, g.folder, g.cancel).await?;
    ensure_repo(g).await?;
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let diff = git(g, &argv).await?;
    // untracked files never show in `git diff`; list them so the model can read them
    let untracked = if base.is_none() && !staged { git(g, &["ls-files", "--others", "--exclude-standard", "--", &rel]).await.unwrap_or_default() } else { String::new() };
    let files = split_diff(&diff);
    let (add, del): (usize, usize) = files.iter().fold((0, 0), |(a, d), f| (a + f["added"].as_u64().unwrap_or(0) as usize, d + f["removed"].as_u64().unwrap_or(0) as usize));
    let mut out = if files.is_empty() { format!("No differences for `{shown}`.\n") } else { format!("{} file(s) changed, +{add} −{del}\n\n{}", files.len(), clip(&diff, 60_000)) };
    let untracked: Vec<&str> = untracked.lines().filter(|l| !l.trim().is_empty()).take(100).collect();
    if !untracked.is_empty() {
        out.push_str(&format!("\nUntracked (new) files, not in the diff; read them with read_file:\n{}\n", untracked.join("\n")));
    }
    // the UI card gets every file but the bodies are capped
    let mut budget = 150_000usize;
    let disp: Vec<Value> = files
        .into_iter()
        .map(|mut f| {
            let len = f["diff"].as_str().map(str::len).unwrap_or(0);
            if len > budget {
                f["diff"] = json!("");
                f["clipped"] = json!(true);
            } else {
                budget -= len;
            }
            f
        })
        .collect();
    Ok((out, json!({"kind": "gitdiff", "command": shown, "files": disp, "added": add, "removed": del, "untracked": untracked})))
}

/// Splits `git diff` output into per-file unified diffs with +/− counts.
fn split_diff(diff: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut cur: Option<(String, String, usize, usize, bool)> = None;
    let flush = |c: Option<(String, String, usize, usize, bool)>, out: &mut Vec<Value>| {
        if let Some((path, body, a, d, created)) = c {
            out.push(json!({"path": path, "diff": body, "added": a, "removed": d, "created": created}));
        }
    };
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            flush(cur.take(), &mut out);
            let path = rest.rsplit_once(" b/").map(|(_, b)| b.to_string()).unwrap_or_else(|| rest.to_string());
            cur = Some((path, String::new(), 0, 0, false));
            continue;
        }
        let Some(c) = cur.as_mut() else { continue };
        if line.starts_with("new file mode") {
            c.4 = true;
        }
        if line.starts_with("@@") || !c.1.is_empty() {
            if line.starts_with('+') && !line.starts_with("+++") {
                c.2 += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                c.3 += 1;
            }
            c.1.push_str(line);
            c.1.push('\n');
        } else if line.starts_with("Binary files") {
            c.1.push_str("@@ binary @@\n");
        }
    }
    flush(cur, &mut out);
    out
}

/// Runs a check command (tests, linter, type-checker, build) in the project folder through `cmd /C`
/// after the user approves the exact command line.
pub async fn run_command(g: &GitCtx<'_>, a: &Value) -> Result<(String, Value)> {
    let command = a["command"].as_str().map(str::trim).filter(|c| !c.is_empty()).ok_or_else(|| anyhow!("missing argument `command`"))?;
    if command.len() > 2000 || command.contains('\n') {
        return Err(anyhow!("give a single command line"));
    }
    let cwd = match a["cwd"].as_str().filter(|s| !s.trim().is_empty()) {
        Some(c) => resolve(Path::new(g.folder), c)?.0,
        None => canonical_root(g.folder)?,
    };
    if !cwd.is_dir() {
        return Err(anyhow!("`{}` is not a folder in the project", cwd.display()));
    }
    let timeout = a["timeout_sec"].as_u64().unwrap_or(180).clamp(5, 900);
    confirm(g.app, g.chat_id, g.call_id, "run", command, &cwd.to_string_lossy(), g.cancel).await?;
    let state = g.app.state::<Arc<AppState>>().inner().clone();
    let comspec = std::env::var_os("ComSpec").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cmd.exe"));
    let started = std::time::Instant::now();
    // keep the model from being unloaded during long test runs
    state.touch();
    let o = run_process(&state.job, &comspec, &["/D".into(), "/S".into(), "/C".into()], Some(&format!("\"{command}\"")), &cwd, Duration::from_secs(timeout), g.cancel).await?;
    state.touch();
    let secs = started.elapsed().as_secs_f32();
    let status = if o.timed_out {
        format!("timed out after {timeout} s (process killed)")
    } else if o.cancelled {
        "stopped by the user".into()
    } else {
        format!("exit code {}", o.code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()))
    };
    let body = clip(&o.output, 24_000);
    let content = format!("$ {command}\n({status}, {secs:.1} s)\n\n{}", if body.trim().is_empty() { "(no output)" } else { &body });
    let ok = o.code == Some(0);
    Ok((content, json!({"kind": "command", "command": command, "code": o.code, "ok": ok, "timed_out": o.timed_out, "secs": secs})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_are_recognised() {
        assert!(def_regex("rs").is_match("pub async fn run_inner(ctx: &ToolCtx) -> Result<()> {"));
        assert!(def_regex("rs").is_match("impl<'a> Parser<'a> {"));
        assert!(def_regex("js").is_match("async function renderChat() {"));
        assert!(def_regex("js").is_match("const TOOL_LABEL = { a: 1 };"));
        assert!(def_regex("js").is_match("  render(x) {"));
        assert!(def_regex("py").is_match("    def handle(self, x):"));
        assert!(!def_regex("rs").is_match("    let x = foo();"));
        assert!(defines("pub fn tree(folder: &Path) -> String {", "tree"));
        assert!(defines("function toolEl(m) {", "toolEl"));
        assert!(!defines("pub fn tree(folder: &Path) -> String {", "folder"));
    }

    #[test]
    fn diff_is_split_per_file() {
        let d = "diff --git a/x.rs b/x.rs\nindex 1..2 100644\n--- a/x.rs\n+++ b/x.rs\n@@ -1,2 +1,2 @@\n a\n-b\n+c\ndiff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1 @@\n+hi\n";
        let f = split_diff(d);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0]["path"], "x.rs");
        assert_eq!(f[0]["added"], 1);
        assert_eq!(f[0]["removed"], 1);
        assert_eq!(f[1]["created"], true);
    }

    /// Runs the file tools on this repository and real processes (git, cmd) through the job object.
    /// `cargo test -- --ignored live_code_tools --nocapture`
    #[test]
    #[ignore]
    fn live_code_tools() {
        let repo = std::env::current_dir().unwrap().parent().unwrap().to_string_lossy().to_string();
        let now = crate::calc::local_now();
        println!("--- current_datetime {now:?}");
        assert_eq!(now.0.len(), 19);
        let (g, d) = grep(&repo, &GrepArgs { pattern: "fn resolve".into(), regex: false, case_sensitive: true, whole_word: false, path: "src-tauri/src".into(), glob: Some("*.rs".into()), context: 1, max_results: 20 }).unwrap();
        println!("--- grep\n{g}");
        assert!(d["matches"].as_u64().unwrap() >= 1);
        let (o, _) = outline(&repo, "src-tauri/src/calc.rs").unwrap();
        println!("--- outline\n{o}");
        assert!(o.contains("pub fn eval"));
        let (o, _) = outline(&repo, "ui/app.js").unwrap();
        println!("--- outline app.js (first lines)\n{}", o.lines().take(12).collect::<Vec<_>>().join("\n"));
        let (f, d) = find_symbol(&repo, "format_number", "", None).unwrap();
        println!("--- find_symbol\n{f}");
        assert_eq!(d["defs"], 1);
        let (c, _) = compare(&repo, "scripts/release-notes.ps1", "scripts/release.ps1").unwrap();
        println!("--- compare (head)\n{}", c.lines().take(8).collect::<Vec<_>>().join("\n"));
        let (r, d) = findings(Some(&repo), &json!({"summary": "ok", "findings": [
            {"path": "src-tauri/src/calc.rs", "line": 10, "severity": "major", "title": "x"},
            {"path": "../outside.txt", "line": 1, "severity": "low", "title": "escape"},
            {"path": "src-tauri/src/calc.rs", "line": 999999, "severity": "nit", "title": "bad line"}
        ]})).unwrap();
        println!("--- findings\n{r}\n{d}");
        assert_eq!(d["findings"].as_array().unwrap().len(), 2);
        assert!(resolve(Path::new(&repo), "../outside.txt").is_err());

        let rt = tokio::runtime::Runtime::new().unwrap();
        let job = crate::win::Job::new();
        let no = AtomicBool::new(false);
        rt.block_on(async {
            let git = git_exe().expect("git");
            println!("--- git at {}", git.display());
            let o = run_process(&job, &git, &["--no-pager".into(), "status".into(), "--porcelain=v1".into(), "-b".into()], None, Path::new(&repo), Duration::from_secs(30), &no).await.unwrap();
            println!("{} (exit {:?})", o.output, o.code);
            assert_eq!(o.code, Some(0));
            let o = run_process(&job, &git, &["--no-pager".into(), "diff".into(), "--stat".into()], None, Path::new(&repo), Duration::from_secs(30), &no).await.unwrap();
            println!("--- git diff --stat\n{}", o.output);
            let files = split_diff(&run_process(&job, &git, &["--no-pager".into(), "diff".into()], None, Path::new(&repo), Duration::from_secs(30), &no).await.unwrap().output);
            println!("split into {} files: {:?}", files.len(), files.iter().map(|f| f["path"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>());
            let cmd = std::env::var_os("ComSpec").map(PathBuf::from).unwrap();
            let o = run_process(&job, &cmd, &["/D".into(), "/S".into(), "/C".into()], Some("\"echo hello && echo oops 1>&2 && exit 3\""), Path::new(&repo), Duration::from_secs(30), &no).await.unwrap();
            println!("--- cmd: {:?} exit {:?}", o.output, o.code);
            assert_eq!(o.code, Some(3));
            assert!(o.output.contains("hello") && o.output.contains("oops"));
            let t0 = std::time::Instant::now();
            let o = run_process(&job, &cmd, &["/D".into(), "/S".into(), "/C".into()], Some("\"ping -n 30 127.0.0.1\""), Path::new(&repo), Duration::from_secs(2), &no).await.unwrap();
            println!("--- timeout: timed_out={} after {:?}, partial output {} chars", o.timed_out, t0.elapsed(), o.output.len());
            assert!(o.timed_out && t0.elapsed() < Duration::from_secs(10));
        });
    }

    #[test]
    fn refs_are_checked() {
        assert!(safe_ref("HEAD~2").is_ok());
        assert!(safe_ref("main...feature/x").is_ok());
        assert!(safe_ref("--output=x").is_err());
        assert!(safe_ref("a b").is_err());
    }
}
