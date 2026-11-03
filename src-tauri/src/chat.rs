//! Chat turns: prompt assembly, streaming to the UI, tool loop, context accounting and compression.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};

use crate::db::{Chat, Message, Project};
use crate::llm::{self, Delta, StreamResult};
use crate::prompts;
use crate::state::AppState;
use crate::tools::{self, ToolCtx};

pub fn emit(app: &AppHandle, chat_id: &str, kind: &str, mut payload: Value) {
    payload["chat_id"] = json!(chat_id);
    payload["kind"] = json!(kind);
    let _ = app.emit("chat-event", payload);
}

pub fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let z = secs / 86400 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// Effective context window for a chat: its own cap (if set) bounded by the loaded model.
pub fn budget(state: &AppState, chat: &Chat) -> u32 {
    let n_ctx = state.info.read().n_ctx.max(2048);
    if chat.ctx_limit > 0 { chat.ctx_limit.min(n_ctx) } else { n_ctx }
}

pub fn system_prompt(state: &AppState, chat: &Chat, project: Option<&Project>, web: bool) -> String {
    let lang = state.settings.read().reply_lang.clone();
    let mut sys = prompts::base(&lang, &today());
    if web {
        sys.push_str(prompts::WEB_TOOLS);
    }
    if let Some(p) = project {
        let tree = tools::tree(Path::new(&p.folder), "", 3, 220);
        sys.push_str(&prompts::project(&p.name, &p.folder, &tree, &p.instructions));
    }
    if !chat.summary.trim().is_empty() {
        sys.push_str("\n\n# Summary of the earlier part of this conversation (it was compressed to save context)\n");
        sys.push_str(chat.summary.trim());
    }
    sys
}

/// OpenAI-style message list for the next request: system prompt (+ summary of compressed turns),
/// then every non-compressed message. Tool calls without a stored result (cancelled turns) are
/// dropped so the chat template never sees a dangling call.
pub fn build_messages(state: &AppState, chat: &Chat, project: Option<&Project>, web: bool) -> Result<Vec<Value>> {
    let all = state.db.messages(&chat.id)?;
    let live: Vec<&Message> = all.iter().filter(|m| !m.compressed && m.role != "event").collect();
    let answered: HashSet<String> = live
        .iter()
        .filter(|m| m.role == "tool")
        .filter_map(|m| m.meta["tool_call_id"].as_str().map(String::from))
        .collect();
    let mut out = vec![json!({"role": "system", "content": system_prompt(state, chat, project, web)})];
    for m in live {
        match m.role.as_str() {
            "user" => {
                let c = m.meta["prompt_content"].as_str().unwrap_or(&m.content);
                out.push(json!({"role": "user", "content": c}));
            }
            "assistant" => {
                let calls: Vec<Value> = m.meta["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|t| t["id"].as_str().map_or(false, |id| answered.contains(id)))
                    .map(|t| json!({"id": t["id"], "type": "function", "function": {"name": t["name"], "arguments": t["arguments"]}}))
                    .collect();
                if m.content.is_empty() && calls.is_empty() {
                    continue;
                }
                let mut v = json!({"role": "assistant", "content": m.content});
                if !calls.is_empty() {
                    v["tool_calls"] = json!(calls);
                }
                out.push(v);
            }
            "tool" => out.push(json!({"role": "tool", "tool_call_id": m.meta["tool_call_id"], "content": m.content})),
            _ => {}
        }
    }
    Ok(out)
}

pub struct SendOpts {
    pub chat_id: String,
    pub text: String,
    pub think: bool,
    pub web: bool,
    pub research: bool,
}

fn register_cancel(state: &AppState, chat_id: &str) -> Arc<AtomicBool> {
    let c = Arc::new(AtomicBool::new(false));
    state.generations.lock().insert(chat_id.to_string(), c.clone());
    c
}

pub fn stop(state: &AppState, chat_id: &str) {
    if let Some(c) = state.generations.lock().get(chat_id) {
        c.store(true, Ordering::Relaxed);
    }
}

/// Full user turn: auto-retrieval (projects), pre-emptive compression if the message would not fit,
/// store the user message, run the assistant (normal tool loop or deep research), then title +
/// post-turn compression. Progress reaches the UI only through `chat-event`s.
pub async fn send(app: AppHandle, opts: SendOpts) -> Result<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if state.llm_url().is_none() {
        return Err(anyhow!("No model is loaded yet. Open Models and load one."));
    }
    let chat = state.db.chat(&opts.chat_id)?.ok_or_else(|| anyhow!("chat not found"))?;
    let project = match &chat.project_id {
        Some(pid) => state.db.project(pid)?,
        None => None,
    };
    let cancel = register_cancel(&state, &chat.id);
    emit(&app, &chat.id, "busy", json!({"busy": true}));

    let result = async {
        // auto-retrieval for project chats
        let mut meta = json!({});
        if let Some(p) = &project {
            if let Ok(hits) = crate::rag::search(&app, &p.id, &opts.text, 6).await {
                if !hits.is_empty() {
                    let mut ctx = String::new();
                    for h in &hits {
                        let t: String = h.text.chars().take(1800).collect();
                        ctx.push_str(&format!("\n--- {}:{}-{} ---\n{}\n", h.path, h.line_start, h.line_end, t));
                    }
                    meta["prompt_content"] = json!(format!("{}\n\n<retrieved_context note=\"auto-retrieved excerpts from the project, may be partial\">{}</retrieved_context>", opts.text, ctx));
                    meta["retrieved"] = json!(hits.iter().map(|h| json!({"path": h.path, "lines": format!("{}-{}", h.line_start, h.line_end)})).collect::<Vec<_>>());
                }
            }
        }
        if opts.research {
            meta["research"] = json!(true);
        }
        // compress first if the new message would not fit comfortably
        let fresh = state.db.chat(&chat.id)?.unwrap_or(chat.clone());
        let incoming = llm::estimate_tokens(meta["prompt_content"].as_str().unwrap_or(&opts.text));
        if fresh.ctx_used + incoming > (budget(&state, &fresh) as f32 * 0.9) as u32 {
            let _ = compress(&app, &chat.id, &cancel).await;
        }
        let user = state.db.add_message(&chat.id, "user", &opts.text, "", &meta)?;
        emit(&app, &chat.id, "message", json!({"message": user}));

        if opts.research {
            crate::research::run(&app, &chat.id, &opts.text, opts.think, &cancel).await?;
        } else {
            run_turn(&app, &chat.id, project.as_ref(), opts.think, opts.web, &cancel).await?;
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    if let Err(e) = &result {
        emit(&app, &chat.id, "error", json!({"error": e.to_string()}));
    }
    // follow-ups: title + compression (only if not cancelled)
    if !cancel.load(Ordering::Relaxed) {
        if let Ok(Some(c)) = state.db.chat(&chat.id) {
            if c.title == "New chat" || c.title == "Chat nou" {
                let _ = make_title(&app, &c).await;
            }
            let c = state.db.chat(&chat.id)?.unwrap_or(c);
            let compress_at = state.settings.read().compress_at;
            if c.ctx_used as f32 > budget(&state, &c) as f32 * compress_at {
                let _ = compress(&app, &chat.id, &cancel).await;
            }
        }
    }
    state.generations.lock().remove(&chat.id);
    emit(&app, &chat.id, "busy", json!({"busy": false}));
    result
}

pub async fn regenerate(app: AppHandle, chat_id: String, think: bool, web: bool) -> Result<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let msgs = state.db.messages(&chat_id)?;
    let last_user = msgs.iter().rev().find(|m| m.role == "user").ok_or_else(|| anyhow!("nothing to regenerate"))?;
    let research = last_user.meta["research"].as_bool().unwrap_or(false);
    let text = last_user.content.clone();
    state.db.truncate_from(&chat_id, last_user.seq)?;
    send(app, SendOpts { chat_id, text, think, web, research }).await
}

/// Streams one assistant message: emits deltas to the UI and persists it.
pub async fn stream_assistant(
    app: &AppHandle,
    chat_id: &str,
    body: Value,
    cancel: &AtomicBool,
    extra_meta: Value,
) -> Result<(Message, StreamResult)> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let temp_id = crate::state::new_id();
    emit(app, chat_id, "assistant_start", json!({"temp_id": temp_id}));
    let started = Instant::now();
    let mut pend_c = String::new();
    let mut pend_r = String::new();
    let mut last_flush = Instant::now();
    let r = llm::stream_chat(&state, body, cancel, |d| {
        match d {
            Delta::Content(s) => pend_c.push_str(s),
            Delta::Reasoning(s) => pend_r.push_str(s),
            Delta::Tool { index, name } => emit(app, chat_id, "tool_pending", json!({"temp_id": temp_id, "index": index, "name": name})),
            Delta::Progress { tps, tokens } => emit(app, chat_id, "progress", json!({"temp_id": temp_id, "tps": tps, "tokens": tokens})),
        }
        if last_flush.elapsed().as_millis() >= 33 && (!pend_c.is_empty() || !pend_r.is_empty()) {
            emit(app, chat_id, "delta", json!({"temp_id": temp_id, "content": pend_c, "reasoning": pend_r}));
            pend_c.clear();
            pend_r.clear();
            last_flush = Instant::now();
        }
    })
    .await;
    if !pend_c.is_empty() || !pend_r.is_empty() {
        emit(app, chat_id, "delta", json!({"temp_id": temp_id, "content": pend_c, "reasoning": pend_r}));
    }
    let r = match r {
        Ok(r) => r,
        Err(e) => {
            emit(app, chat_id, "assistant_failed", json!({"temp_id": temp_id}));
            return Err(e);
        }
    };
    let model = state.info.read().model.clone().unwrap_or_default();
    let mut meta = json!({
        "tool_calls": r.tool_calls.iter().map(|t| json!({"id": t.id, "name": t.name, "arguments": t.arguments})).collect::<Vec<_>>(),
        "tps": r.tps,
        "tokens": r.completion_tokens,
        "prompt_tokens": r.prompt_tokens,
        "cached_tokens": r.cached_tokens,
        "ctx": r.prompt_tokens + r.completion_tokens,
        "model": model,
        "elapsed_ms": started.elapsed().as_millis() as u64,
        "cancelled": r.cancelled,
        "finish": r.finish_reason,
    });
    if let (Some(m), Some(extra)) = (meta.as_object_mut(), extra_meta.as_object()) {
        for (k, v) in extra {
            m.insert(k.clone(), v.clone());
        }
    }
    let msg = state.db.add_message(chat_id, "assistant", &r.content, &r.reasoning, &meta)?;
    if r.prompt_tokens > 0 {
        state.db.set_chat_ctx(chat_id, r.prompt_tokens + r.completion_tokens)?;
    }
    if r.completion_tokens > 8 && !model.is_empty() {
        let _ = state.db.add_stat(&model, r.tps, r.completion_tokens);
    }
    let chat = state.db.chat(chat_id)?;
    let b = chat.as_ref().map(|c| budget(&state, c)).unwrap_or(0);
    emit(app, chat_id, "assistant_done", json!({"temp_id": temp_id, "message": msg, "ctx_used": r.prompt_tokens + r.completion_tokens, "ctx_budget": b}));
    Ok((msg, r))
}

/// Sampling settings (Qwen-recommended top_k/min_p) and the per-request thinking switch.
pub fn gen_params(state: &AppState, think: bool) -> Value {
    let s = state.settings.read();
    json!({
        "temperature": if think { s.temperature.min(0.7).max(0.5) } else { s.temperature },
        "top_p": s.top_p,
        "top_k": 20,
        "min_p": 0.0,
        "chat_template_kwargs": {"enable_thinking": think},
    })
}

/// Assistant loop: stream a reply; if it requested tools, execute them, store the results and ask
/// again (max 24 rounds). Compresses mid-loop when long tool sessions approach the window.
async fn run_turn(app: &AppHandle, chat_id: &str, project: Option<&Project>, think: bool, web: bool, cancel: &AtomicBool) -> Result<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let defs = tools::definitions(project.is_some(), web);
    let ctx = ToolCtx { app, project };
    for _ in 0..24 {
        let chat = state.db.chat(chat_id)?.ok_or_else(|| anyhow!("chat deleted"))?;
        let messages = build_messages(&state, &chat, project, web)?;
        let mut body = gen_params(&state, think);
        body["messages"] = json!(messages);
        if !defs.is_empty() {
            body["tools"] = json!(defs);
        }
        let (_msg, r) = stream_assistant(app, chat_id, body, cancel, json!({})).await?;
        if r.cancelled || r.tool_calls.is_empty() {
            break;
        }
        for tc in &r.tool_calls {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            emit(app, chat_id, "tool_start", json!({"id": tc.id, "name": tc.name, "arguments": tc.arguments}));
            let out = tools::run(&ctx, &tc.name, &tc.arguments).await;
            let meta = json!({"tool_call_id": tc.id, "name": tc.name, "arguments": tc.arguments, "display": out.display, "ok": out.ok});
            let m = state.db.add_message(chat_id, "tool", &out.content, "", &meta)?;
            emit(app, chat_id, "message", json!({"message": m}));
        }
        // compress mid-loop if long tool sessions approach the limit
        let c = state.db.chat(chat_id)?.ok_or_else(|| anyhow!("chat deleted"))?;
        if c.ctx_used as f32 > budget(&state, &c) as f32 * 0.88 {
            compress(app, chat_id, cancel).await?;
        }
    }
    Ok(())
}

async fn make_title(app: &AppHandle, chat: &Chat) -> Result<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let msgs = state.db.messages(&chat.id)?;
    let first_user = msgs.iter().find(|m| m.role == "user").map(|m| m.content.clone()).unwrap_or_default();
    let first_ans = msgs.iter().find(|m| m.role == "assistant" && !m.content.is_empty()).map(|m| m.content.clone()).unwrap_or_default();
    if first_user.is_empty() {
        return Ok(());
    }
    let convo = format!(
        "User: {}\n\nAssistant: {}",
        first_user.chars().take(1500).collect::<String>(),
        first_ans.chars().take(800).collect::<String>()
    );
    let t = llm::complete(&state, json!([{"role": "system", "content": prompts::TITLE}, {"role": "user", "content": convo}]), 24, None).await?;
    let t = t.lines().next().unwrap_or("").trim().trim_matches(|c| c == '"' || c == '\'' || c == '*' || c == '#').trim().to_string();
    if !t.is_empty() {
        let t: String = t.chars().take(60).collect();
        state.db.rename_chat(&chat.id, &t)?;
        emit(app, &chat.id, "title", json!({"title": t}));
    }
    Ok(())
}

/// Plain-text transcript line for the compressor (tool output truncated, it is rarely needed verbatim).
fn render_for_summary(m: &Message) -> String {
    match m.role.as_str() {
        "user" => format!("USER: {}", m.content),
        "assistant" => {
            let mut s = format!("ASSISTANT: {}", m.content);
            if let Some(tc) = m.meta["tool_calls"].as_array().filter(|a| !a.is_empty()) {
                let names: Vec<String> = tc
                    .iter()
                    .map(|t| format!("{}({})", t["name"].as_str().unwrap_or(""), t["arguments"].as_str().unwrap_or("").chars().take(200).collect::<String>()))
                    .collect();
                s.push_str(&format!("\n[called tools: {}]", names.join(", ")));
            }
            s
        }
        "tool" => format!("TOOL RESULT ({}): {}", m.meta["name"].as_str().unwrap_or(""), m.content.chars().take(1500).collect::<String>()),
        _ => String::new(),
    }
}

/// Summarise everything except the latest turn(s) into the chat summary. Shown in the chat as a progress card.
pub async fn compress(app: &AppHandle, chat_id: &str, cancel: &AtomicBool) -> Result<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let chat = state.db.chat(chat_id)?.ok_or_else(|| anyhow!("chat not found"))?;
    let all = state.db.messages(chat_id)?;
    let live: Vec<&Message> = all.iter().filter(|m| !m.compressed && m.role != "event").collect();
    let user_seqs: Vec<i64> = live.iter().filter(|m| m.role == "user").map(|m| m.seq).collect();
    if user_seqs.len() < 2 {
        return Ok(()); // nothing older than the current turn
    }
    let b = budget(&state, &chat);
    // keep the last two turns when they are small, otherwise just the last one
    let last2 = user_seqs[user_seqs.len().saturating_sub(2)];
    let tail_tokens: u32 = live.iter().filter(|m| m.seq >= last2).map(|m| llm::estimate_tokens(&m.content)).sum();
    let keep_from = if user_seqs.len() >= 3 && tail_tokens < b / 3 { last2 } else { *user_seqs.last().unwrap() };
    let old: Vec<&&Message> = live.iter().filter(|m| m.seq < keep_from).collect();
    if old.is_empty() {
        return Ok(());
    }
    let before = chat.ctx_used.max(1);
    let ev = state.db.add_message(chat_id, "event", "", "", &json!({"kind": "compress", "state": "running", "before": before, "count": old.len()}))?;
    emit(app, chat_id, "message", json!({"message": ev}));

    // map-reduce over parts that fit into ~55% of the window
    let max_chars = ((b as f32 * 0.55) * 3.4) as usize;
    let mut parts: Vec<String> = vec![String::new()];
    for m in &old {
        let r = render_for_summary(m);
        let r: String = r.chars().take(max_chars / 2).collect();
        if parts.last().unwrap().len() + r.len() > max_chars {
            parts.push(String::new());
        }
        let p = parts.last_mut().unwrap();
        p.push_str(&r);
        p.push_str("\n\n");
    }
    let mut summary = chat.summary.clone();
    let target = 1400u32;
    let n_parts = parts.len();
    for (pi, part) in parts.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let mut input = String::new();
        if !summary.trim().is_empty() {
            input.push_str("<previous_summary>\n");
            input.push_str(&summary);
            input.push_str("\n</previous_summary>\n\n");
        }
        input.push_str("<conversation>\n");
        input.push_str(part);
        input.push_str("</conversation>");
        let body = json!({
            "messages": [{"role": "system", "content": prompts::COMPRESS}, {"role": "user", "content": input}],
            "max_tokens": 2600,
            "temperature": 0.2,
            "chat_template_kwargs": {"enable_thinking": false},
        });
        let ev_id = ev.id.clone();
        let mut last = Instant::now();
        let r = llm::stream_chat(&state, body, cancel, |d| {
            if let Delta::Progress { tokens, .. } = d {
                if last.elapsed().as_millis() > 120 {
                    let frac = ((pi as f32) + (tokens as f32 / target as f32).min(0.97)) / n_parts as f32;
                    emit(app, chat_id, "compress_progress", json!({"id": ev_id, "progress": frac}));
                    last = Instant::now();
                }
            }
        })
        .await;
        match r {
            Ok(r) if !r.cancelled && !r.content.trim().is_empty() => summary = r.content.trim().to_string(),
            Ok(_) => break,
            Err(e) => {
                let meta = json!({"kind": "compress", "state": "error", "error": e.to_string()});
                state.db.update_message(&ev.id, "", "", &meta)?;
                emit(app, chat_id, "message_update", json!({"message": {"id": ev.id, "meta": meta}}));
                return Err(e);
            }
        }
    }
    if cancel.load(Ordering::Relaxed) || summary == chat.summary {
        let meta = json!({"kind": "compress", "state": "cancelled"});
        state.db.update_message(&ev.id, "", "", &meta)?;
        emit(app, chat_id, "message_update", json!({"message": {"id": ev.id, "meta": meta}}));
        return Ok(());
    }
    state.db.set_summary(chat_id, &summary)?;
    state.db.mark_compressed(chat_id, keep_from)?;
    // estimate the new prompt size
    let chat2 = state.db.chat(chat_id)?.ok_or_else(|| anyhow!("chat not found"))?;
    let project = match &chat2.project_id {
        Some(p) => state.db.project(p)?,
        None => None,
    };
    let msgs = build_messages(&state, &chat2, project.as_ref(), false)?;
    let after = llm::estimate_tokens(&serde_json::to_string(&msgs)?);
    state.db.set_chat_ctx(chat_id, after)?;
    let meta = json!({"kind": "compress", "state": "done", "before": before, "after": after, "count": old.len(), "summary": summary});
    state.db.update_message(&ev.id, "", "", &meta)?;
    emit(app, chat_id, "message_update", json!({"message": {"id": ev.id, "meta": meta}}));
    emit(app, chat_id, "compressed", json!({"upto_seq": keep_from, "ctx_used": after, "ctx_budget": budget(&state, &chat2)}));
    Ok(())
}

/// Manual compression triggered from the UI.
pub async fn compress_now(app: AppHandle, chat_id: String) -> Result<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let cancel = register_cancel(&state, &chat_id);
    emit(&app, &chat_id, "busy", json!({"busy": true}));
    let r = compress(&app, &chat_id, &cancel).await;
    state.generations.lock().remove(&chat_id);
    emit(&app, &chat_id, "busy", json!({"busy": false}));
    r
}
