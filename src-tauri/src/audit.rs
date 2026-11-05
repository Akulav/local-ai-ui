//! `audit_code`: reviews a whole project (or a folder) that is far larger than the context window.
//! Map-reduce: the source files are packed into batches that fit about half of the window, each batch
//! is reviewed in its own fresh request (JSON-schema constrained findings), and the findings are
//! merged, validated against the files and shown as one review card. The chat itself only receives a
//! compact list, so the conversation stays small no matter how big the codebase is.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

use crate::codetools::{canonical_root, read_text, rel_of, walk_files};
use crate::llm;
use crate::state::AppState;
use crate::tools::resolve;

const SOURCE_EXT: &[&str] = &[
    "rs", "js", "mjs", "cjs", "jsx", "ts", "tsx", "mts", "cts", "vue", "svelte", "py", "pyi", "go", "java", "kt", "kts", "cs", "scala", "swift", "dart", "c", "h", "cc", "cpp",
    "cxx", "hpp", "hh", "m", "mm", "php", "rb", "lua", "pl", "sh", "bash", "ps1", "psm1", "bat", "cmd", "sql", "html", "htm", "css", "scss", "less", "toml", "yaml", "yml", "json",
    "xml", "gradle", "cmake", "zig", "nim", "ex", "exs", "erl", "hs", "ml", "fs", "r", "jl", "sol", "tf",
];
const SKIP_NAMES: &[&str] = &["package-lock.json", "yarn.lock", "pnpm-lock.yaml", "Cargo.lock", "poetry.lock", "composer.lock", "Gemfile.lock", "go.sum"];
const MAX_FILE_BYTES: u64 = 400_000;

/// One piece of a file sent to the reviewer (whole file, or a line window of a big one).
struct Unit {
    rel: String,
    start: usize,
    end: usize,
    total: usize,
    text: String,
}

fn est(chars: usize) -> usize {
    chars / 3 + 1
}

fn wanted(rel: &str) -> bool {
    let lower = rel.to_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    if SKIP_NAMES.iter().any(|n| n.eq_ignore_ascii_case(name)) || name.ends_with(".min.js") || name.ends_with(".min.css") || name.ends_with(".map") {
        return false;
    }
    if ["vendor/", "third_party/", "third-party/", "generated/", "__snapshots__/", ".next/", "coverage/"].iter().any(|d| lower.contains(d)) {
        return false;
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    SOURCE_EXT.contains(&ext)
}

/// Splits a file into numbered units of at most `max_tokens`, with a little overlap between windows.
fn units_of(rel: &str, text: &str, max_tokens: usize) -> Vec<Unit> {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let mut out = Vec::new();
    let mut start = 0;
    while start < total.max(1) {
        let mut body = String::new();
        let mut i = start;
        while i < total {
            let row = format!("{:>5}│ {}\n", i + 1, lines[i].chars().take(500).collect::<String>());
            if est(body.len() + row.len()) > max_tokens && i > start {
                break;
            }
            body.push_str(&row);
            i += 1;
        }
        out.push(Unit { rel: rel.to_string(), start: start + 1, end: i.max(start + 1), total, text: body });
        if i >= total {
            break;
        }
        start = i.saturating_sub(15).max(start + 1);
    }
    out
}

fn batch_prompt(focus: &str, lang: &str) -> String {
    let focus = if focus.trim().is_empty() { String::new() } else { format!("\nThe user asked to focus on: {}. Prioritise that, but still report other serious problems.\n", focus.trim()) };
    let lang = if lang == "ro" { "Romanian (with correct diacritics: ă â î ș ț)" } else { "English" };
    format!(
        r#"You are a meticulous senior code reviewer auditing a large project in parts. You receive some of its files, each line prefixed with its line number.
Report only real, specific problems you can see in THIS code:
- bugs and wrong logic, crashes/panics, unhandled errors or errors that are silently lost, wrong edge cases (empty, null, overflow, off-by-one, encoding);
- security issues: injection (SQL, shell, HTML/XSS), path traversal, secrets in code, unsafe deserialisation, missing validation of external input;
- concurrency problems, resource leaks, very slow code on hot paths;
- serious maintainability problems (duplicated logic that will diverge, dead code that hides bugs).
Do NOT report style preferences, formatting, missing comments, or guesses about code you cannot see. Be precise: the path exactly as shown, the line number from the prefix, a short title, why it is a problem and a concrete fix. If the code looks fine, return an empty list; an empty list is a good answer.
Severity: critical = security hole, data loss or crash in normal use; high = wrong behaviour; medium = edge-case bug or risky code; low = maintainability; info = minor.{focus}
Write title, detail and suggestion in {lang}."#
    )
}

fn schema() -> Value {
    json!({"type": "object", "properties": {"findings": {"type": "array", "maxItems": 15, "items": {"type": "object", "properties": {
        "path": {"type": "string"},
        "line": {"type": "integer"},
        "severity": {"type": "string", "enum": ["critical", "high", "medium", "low", "info"]},
        "title": {"type": "string"},
        "detail": {"type": "string"},
        "suggestion": {"type": "string"}
    }, "required": ["path", "line", "severity", "title", "detail", "suggestion"]}}}, "required": ["findings"]})
}

fn progress(app: &AppHandle, chat_id: &str, call_id: &str, done: usize, total: usize, text: &str) {
    crate::chat::emit(app, chat_id, "tool_progress", json!({"call_id": call_id, "done": done, "total": total, "text": text}));
}

pub struct AuditCtx<'a> {
    pub app: &'a AppHandle,
    pub chat_id: &'a str,
    pub call_id: &'a str,
    pub folder: &'a str,
    pub cancel: &'a AtomicBool,
}

pub async fn run(c: &AuditCtx<'_>, a: &Value) -> Result<(String, Value)> {
    let state = c.app.state::<Arc<AppState>>().inner().clone();
    let report = |done: usize, total: usize, text: &str| progress(c.app, c.chat_id, c.call_id, done, total, text);
    review(&state, Some(c.app), c.folder, a, c.cancel, &report).await
}

/// The pipeline itself, independent of the window (progress goes to `report`).
/// `app` lets a batch restart llama-server and retry once when the connection drops (None in tests).
pub async fn review(state: &AppState, app: Option<&AppHandle>, folder: &str, a: &Value, cancel: &AtomicBool, report: &(dyn Fn(usize, usize, &str) + Sync)) -> Result<(String, Value)> {
    let sub = a["path"].as_str().unwrap_or("").to_string();
    let glob = a["glob"].as_str().map(String::from);
    let focus = a["focus"].as_str().unwrap_or("").to_string();
    let lang = match a["language"].as_str() {
        Some("ro") => "ro".to_string(),
        Some(_) => "en".to_string(),
        None => if state.settings.read().reply_lang == "ro" { "ro".into() } else { "en".into() },
    };
    let max_files = a["max_files"].as_u64().unwrap_or(400).clamp(1, 3000) as usize;
    let n_ctx = state.info.read().n_ctx.max(4096) as usize;
    let sys = batch_prompt(&focus, &lang);
    // half the window for code, the rest for the instructions, the answer and the estimate's error
    let batch_tokens = (n_ctx / 2).saturating_sub(est(sys.len())).max(1500);
    let out_tokens = (n_ctx / 6).clamp(1024, 3072);

    // 1. collect the source files
    report(0, 0, "listing files");
    let root_folder = folder.to_string();
    let (units, files, skipped) = tokio::task::spawn_blocking(move || -> Result<(Vec<Unit>, usize, Vec<String>)> {
        let root = canonical_root(&root_folder)?;
        let (base, _) = resolve(Path::new(&root_folder), &sub)?;
        if !base.exists() {
            return Err(anyhow!("`{sub}` does not exist in the project"));
        }
        let mut units = Vec::new();
        let mut files = 0;
        let mut skipped = Vec::new();
        for f in walk_files(&base, glob.as_deref())? {
            let rel = rel_of(&root, &f);
            if !wanted(&rel) {
                continue;
            }
            if f.metadata().map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(true) {
                skipped.push(format!("{rel} (too large)"));
                continue;
            }
            if files >= max_files {
                skipped.push(rel);
                continue;
            }
            let Some(text) = read_text(&f) else { continue };
            if text.trim().is_empty() {
                continue;
            }
            files += 1;
            units.extend(units_of(&rel, &text, batch_tokens));
        }
        Ok((units, files, skipped))
    })
    .await??;
    if units.is_empty() {
        return Err(anyhow!("no source files found to audit (path `{}`)", a["path"].as_str().unwrap_or("")));
    }

    // 2. pack units into batches
    let mut batches: Vec<Vec<Unit>> = vec![Vec::new()];
    let mut size = 0;
    for u in units {
        let t = est(u.text.len()) + 20;
        if size + t > batch_tokens && !batches.last().unwrap().is_empty() {
            batches.push(Vec::new());
            size = 0;
        }
        size += t;
        batches.last_mut().unwrap().push(u);
    }
    let n = batches.len();

    // 3. review each batch in a fresh context
    let mut all: Vec<Value> = Vec::new();
    let mut failed = 0;
    let mut done = 0;
    let mut server_lost: Option<String> = None;
    for (bi, batch) in batches.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let names: Vec<&str> = batch.iter().map(|u| u.rel.as_str()).collect::<HashSet<_>>().into_iter().collect();
        let label = if names.len() == 1 { names[0].to_string() } else { format!("{} … (+{})", names[0], names.len() - 1) };
        report(bi, n, &label);
        let mut input = String::new();
        for u in batch {
            let range = if u.start == 1 && u.end >= u.total { String::new() } else { format!(" (lines {}-{} of {})", u.start, u.end, u.total) };
            input.push_str(&format!("=== {}{} ===\n{}\n", u.rel, range, u.text));
        }
        let body = json!({
            "messages": [{"role": "system", "content": sys}, {"role": "user", "content": input}],
            "max_tokens": out_tokens,
            "temperature": 0.2,
            "chat_template_kwargs": {"enable_thinking": false},
            "response_format": {"type": "json_schema", "json_schema": {"name": "findings", "schema": schema()}},
        });
        let mut res = llm::stream_chat(state, body.clone(), cancel, |_| {}).await;
        if let (Err(e), Some(app)) = (&res, app) {
            if llm::is_connection_error(e) && !cancel.load(Ordering::Relaxed) {
                state.log(format!("audit batch {}/{n}: lost the connection to llama-server ({e}); restarting it", bi + 1));
                report(bi, n, "restarting the model server");
                match crate::server::recover(app).await {
                    Ok(_) => res = llm::stream_chat(state, body, cancel, |_| {}).await,
                    Err(re) => {
                        server_lost = Some(re.to_string());
                        break;
                    }
                }
                if res.as_ref().err().is_some_and(llm::is_connection_error) {
                    // it died again on this batch: stop here and keep what was found so far
                    server_lost = Some(res.as_ref().err().map(|e| e.to_string()).unwrap_or_default());
                    failed += 1;
                    break;
                }
            }
        }
        match res {
            Ok(r) if r.cancelled => break,
            Ok(r) => match serde_json::from_str::<Value>(r.content.trim()) {
                Ok(v) => {
                    all.extend(v["findings"].as_array().cloned().unwrap_or_default());
                    done += 1;
                }
                Err(_) => failed += 1,
            },
            Err(e) => {
                state.log(format!("audit batch {}/{n} failed: {e}", bi + 1));
                failed += 1;
            }
        }
    }
    let stopped = cancel.load(Ordering::Relaxed);
    report(n, n, "merging findings");

    // 4. merge: drop duplicates (overlapping windows) and validate against the files
    let mut seen = HashSet::new();
    all.retain(|f| {
        let key = (f["path"].as_str().unwrap_or("").to_string(), f["line"].as_u64().unwrap_or(0) / 3, f["title"].as_str().unwrap_or("").to_lowercase());
        seen.insert(key)
    });
    let folder = folder.to_string();
    let args = json!({"summary": "", "findings": all});
    let (_, mut disp) = tokio::task::spawn_blocking(move || crate::codetools::findings(Some(&folder), &args)).await??;
    disp["audit"] = json!({"files": files, "batches": n, "reviewed": done, "failed": failed, "skipped": skipped.len(), "stopped": stopped || server_lost.is_some()});

    // 5. compact text for the chat model
    let items = disp["findings"].as_array().cloned().unwrap_or_default();
    let mut out = format!(
        "Audit {}: {files} files in {n} batches ({done} reviewed{}). {} findings. The user sees all of them in the review card: do NOT call report_findings for these.\n",
        if stopped { "stopped by the user".to_string() } else if let Some(e) = &server_lost { format!("stopped early because the model server crashed twice ({e}); the findings below cover the batches done before that") } else { "finished".to_string() },
        if failed > 0 { format!(", {failed} failed") } else { String::new() },
        items.len()
    );
    if let Some(counts) = disp["counts"].as_object() {
        out.push_str(&format!("By severity: {}\n", counts.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")));
    }
    out.push_str("\nFindings (most severe first):\n");
    for f in &items {
        let line = format!(
            "- [{}] {}:{} {} — {}\n",
            f["severity"].as_str().unwrap_or(""),
            f["path"].as_str().unwrap_or(""),
            f["line"],
            f["title"].as_str().unwrap_or(""),
            f["detail"].as_str().unwrap_or("").chars().take(180).collect::<String>()
        );
        if out.len() + line.len() > 9000 {
            out.push_str("- … more in the review card\n");
            break;
        }
        out.push_str(&line);
    }
    if !skipped.is_empty() {
        out.push_str(&format!("\nNot audited ({}): {}\n", skipped.len(), skipped.iter().take(20).cloned().collect::<Vec<_>>().join(", ")));
    }
    out.push_str("\nNow write a short overview for the user: the overall state, the most important problems grouped by theme, and what to fix first. Verify any finding you are unsure about with read_file before you repeat it.");
    Ok((out, disp))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whole pipeline on this repo's `src-tauri/src` against a stand-in llama-server that answers every
    /// batch with one finding on the first file it was sent, with an 8k window so it has to batch.
    /// Set LUMEN_TEST_SERVER=http://127.0.0.1:PORT to use a real llama-server instead.
    /// `cargo test -- --ignored live_audit --nocapture`
    #[test]
    #[ignore]
    fn live_audit() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let port = match std::env::var("LUMEN_TEST_SERVER") {
                Ok(u) => u.rsplit(':').next().unwrap().trim_end_matches('/').parse::<u16>().unwrap(),
                Err(_) => {
                    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let port = l.local_addr().unwrap().port();
                    let requests = requests.clone();
                    tokio::spawn(async move {
                        loop {
                            let (mut s, _) = l.accept().await.unwrap();
                            let requests = requests.clone();
                            tokio::spawn(async move {
                                let mut buf = Vec::new();
                                let mut tmp = [0u8; 65536];
                                loop {
                                    let n = s.read(&mut tmp).await.unwrap();
                                    buf.extend_from_slice(&tmp[..n]);
                                    let txt = String::from_utf8_lossy(&buf);
                                    if let Some(h) = txt.find("\r\n\r\n") {
                                        let len: usize = txt[..h].lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                                        if buf.len() >= h + 4 + len || n == 0 {
                                            break;
                                        }
                                    }
                                }
                                requests.fetch_add(1, Ordering::Relaxed);
                                let txt = String::from_utf8_lossy(&buf).to_string();
                                let body: Value = serde_json::from_str(&txt[txt.find("\r\n\r\n").unwrap() + 4..]).unwrap();
                                let user = body["messages"][1]["content"].as_str().unwrap().to_string();
                                assert!(body["response_format"]["json_schema"].is_object());
                                let path = user.lines().find_map(|l| l.strip_prefix("=== ")).unwrap().split(" ===").next().unwrap().split(" (lines").next().unwrap().to_string();
                                let content = json!({"findings": [{"path": path, "line": 1, "severity": "medium", "title": format!("check {path}"), "detail": "d", "suggestion": "s"}]}).to_string();
                                let chunk = json!({"choices": [{"delta": {"content": content}, "finish_reason": "stop"}]});
                                let sse = format!("data: {chunk}\n\ndata: [DONE]\n\n");
                                let resp = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}", sse.len());
                                let _ = s.write_all(resp.as_bytes()).await;
                            });
                        }
                    });
                    port
                }
            };
            let state = AppState::new().unwrap();
            {
                let mut i = state.info.write();
                i.state = "ready".into();
                i.port = port;
                i.n_ctx = 8192;
            }
            let repo = std::env::current_dir().unwrap().parent().unwrap().to_string_lossy().to_string();
            let cancel = AtomicBool::new(false);
            let seen = std::sync::Mutex::new(Vec::new());
            let report = |d: usize, t: usize, s: &str| seen.lock().unwrap().push(format!("{d}/{t} {s}"));
            let (text, disp) = review(&state, None, &repo, &json!({"path": "src-tauri/src"}), &cancel, &report).await.unwrap();
            println!("--- progress\n{}", seen.lock().unwrap().join("\n"));
            println!("--- to the model\n{text}");
            println!("--- card: {}", disp["audit"]);
            let batches = disp["audit"]["batches"].as_u64().unwrap() as usize;
            assert!(batches > 3, "an 8k window must need several batches");
            assert_eq!(disp["audit"]["reviewed"].as_u64().unwrap() as usize, batches);
            if std::env::var("LUMEN_TEST_SERVER").is_err() {
                assert_eq!(requests.load(Ordering::Relaxed), batches);
                assert!(!disp["findings"].as_array().unwrap().is_empty());
                assert!(disp["findings"][0]["code"].as_str().is_some());
            }
            assert!(text.len() < 12_000);
        });
    }

    #[test]
    fn filters_and_splits() {
        assert!(wanted("src/main.rs"));
        assert!(!wanted("ui/vendor/marked.umd.js"));
        assert!(!wanted("package-lock.json"));
        assert!(!wanted("dist/app.min.js"));
        assert!(!wanted("logo.png"));
        let text: String = (1..=300).map(|i| format!("let value_{i} = compute_something({i});\n")).collect();
        let u = units_of("a.rs", &text, 800);
        assert!(u.len() > 3);
        assert_eq!(u[0].start, 1);
        assert_eq!(u.last().unwrap().end, 300);
        assert!(u.windows(2).all(|w| w[1].start <= w[0].end)); // overlap
        assert!(u.iter().all(|x| est(x.text.len()) <= 900));
    }
}
