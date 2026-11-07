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
use crate::state::{AppState, DEFAULT_TITLES};
use crate::tools::{self, ToolCtx};

pub fn emit(app: &AppHandle, chat_id: &str, kind: &str, mut payload: Value) {
    payload["chat_id"] = json!(chat_id);
    payload["kind"] = json!(kind);
    let _ = app.emit("chat-event", payload);
}

/// Today's date (UTC) as YYYY-MM-DD.
pub fn today() -> String {
    ymd(crate::state::now_ms() / 1000)
}

/// Unix seconds → YYYY-MM-DD (UTC), using the civil-from-days algorithm.
pub fn ymd(secs: i64) -> String {
    let z = secs.div_euclid(86400) + 719468;
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

/// Re-estimates what the chat occupies after its history changed outside a turn (a deleted
/// exchange), stores it for the context meter and returns it.
pub fn refresh_ctx_estimate(state: &AppState, chat_id: &str) -> Result<u32> {
    let chat = state.db.chat(chat_id)?.ok_or_else(|| anyhow!("chat not found"))?;
    let project = state.db.chat_project(&chat)?;
    let msgs = build_messages(state, &chat, project.as_ref(), false)?;
    let used = llm::estimate_tokens(&serde_json::to_string(&msgs)?);
    state.db.set_chat_ctx(chat_id, used)?;
    Ok(used)
}

pub fn system_prompt(state: &AppState, chat: &Chat, project: Option<&Project>, web: bool) -> String {
    let (lang, custom) = {
        let s = state.settings.read();
        (s.reply_lang.clone(), s.custom_instructions.clone())
    };
    let mut sys = prompts::base(&lang, &today());
    sys.push_str(&prompts::custom_instructions(&custom));
    sys.push_str(prompts::GENERAL_TOOLS);
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
                    .filter(|t| t["id"].as_str().is_some_and(|id| answered.contains(id)))
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
    let project = state.db.chat_project(&chat)?;
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
        // 0.8, not 0.9: the estimate can be ~10% low, and the reply needs room too
        if fresh.ctx_used + incoming > (budget(&state, &fresh) as f32 * 0.8) as u32 {
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
            if DEFAULT_TITLES.contains(&c.title.as_str()) {
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
    if !cancel.load(Ordering::Relaxed) {
        crate::reply_finished(&app);
    }
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
        "prompt_tps": r.prompt_tps,
        "prompt_evaluated": r.prompt_evaluated,
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
        "temperature": if think { s.temperature.clamp(0.5, 0.7) } else { s.temperature },
        "top_p": s.top_p,
        "top_k": 20,
        "min_p": 0.0,
        "chat_template_kwargs": {"enable_thinking": think},
    })
}

/// Conservative token estimate for context fitting (code tokenises denser than prose).
fn est_tokens(chars: usize) -> u32 {
    (chars as f32 / 3.0).ceil() as u32
}

/// Estimated prompt size of the next request (messages + tool definitions).
fn prompt_estimate(state: &AppState, chat: &Chat, project: Option<&Project>, web: bool, defs: &[Value]) -> Result<u32> {
    let msgs = build_messages(state, chat, project, web)?;
    Ok(est_tokens(serde_json::to_string(&msgs)?.len() + serde_json::to_string(defs)?.len()))
}

/// Keeps long tool sessions inside the window. Compression only summarises *earlier* turns, so a
/// single long turn (an audit reading dozens of files) would otherwise grow until llama-server
/// rejects it. When the prompt passes `high` × budget, the oldest tool results (all but the last
/// `keep`) are replaced by a one-line stub until it is under `low` × budget. The original stays in
/// `meta.full` for the UI; the stub is stored, so later prompts stay byte-stable for the cache.
fn fit_context(state: &AppState, chat_id: &str, project: Option<&Project>, web: bool, defs: &[Value], (high, low, keep): (f32, f32, usize)) -> Result<u32> {
    let chat = state.db.chat(chat_id)?.ok_or_else(|| anyhow!("chat deleted"))?;
    let b = budget(state, &chat) as f32;
    let mut est =prompt_estimate(state, &chat, project, web, defs)?;
    if (est as f32) <= b * high {
        return Ok(est);
    }
    let msgs = state.db.messages(chat_id)?;
    let tools_live: Vec<&Message> = msgs.iter().filter(|m| !m.compressed && m.role == "tool").collect();
    // oldest first, sparing the last `keep` results; if that is not enough (a few huge reads in a
    // small window), continue into them too, always sparing the newest one
    let n_tools = tools_live.len();
    let order = (0..n_tools.saturating_sub(keep)).chain(n_tools.saturating_sub(keep)..n_tools.saturating_sub(1));
    let mut pruned = 0;
    for m in order.map(|i| tools_live[i]) {
        if (est as f32) <= b * low {
            break;
        }
        if m.meta["pruned"].as_bool().unwrap_or(false) || m.content.len() < 400 {
            continue;
        }
        let name = m.meta["name"].as_str().unwrap_or("tool");
        let args: Value = serde_json::from_str(m.meta["arguments"].as_str().unwrap_or("{}")).unwrap_or_default();
        let what = ["path", "pattern", "name", "query", "command", "url"].iter().find_map(|k| args[*k].as_str()).unwrap_or("");
        let stub = format!("[{name}({what}) output removed to save context ({} chars). You already used it above; call the tool again if you need it.]", m.content.len());
        let mut meta = m.meta.clone();
        meta["pruned"] = json!(true);
        meta["full"] = json!(m.content);
        state.db.update_message(&m.id, &stub, &m.reasoning, &meta)?;
        est = est.saturating_sub(est_tokens(m.content.len())) + est_tokens(stub.len());
        pruned += 1;
    }
    if pruned > 0 {
        state.log(format!("context: pruned {pruned} old tool results, ~{est} tokens of {b}"));
        let chat = state.db.chat(chat_id)?.ok_or_else(|| anyhow!("chat deleted"))?;
        est = prompt_estimate(state, &chat, project, web, defs)?;
    }
    Ok(est)
}

/// Cuts a tool result down to `max_tokens`, keeping the start and the end, and tells the model how
/// to get the rest instead of overflowing the window.
fn clip_tool_output(content: &str, max_tokens: u32) -> String {
    let max_chars = (max_tokens as usize) * 3;
    if content.len() <= max_chars {
        return content.to_string();
    }
    let head = content.floor_char_boundary(max_chars * 4 / 5);
    let tail = content.ceil_char_boundary(content.len() - max_chars / 5);
    format!(
        "{}\n\n[… {} characters cut to fit the context window. Ask for less at once: a line range with read_file, a narrower path/glob, or audit_code for large reviews …]\n\n{}",
        &content[..head],
        tail - head,
        &content[tail..]
    )
}

fn is_context_overflow(e: &anyhow::Error) -> bool {
    let s = e.to_string().to_lowercase();
    s.contains("context") && (s.contains("exceed") || s.contains("too long") || s.contains("size") || s.contains("n_ctx"))
}

/// Model rounds one user message may take (each round: a reply that may call tools).
const MAX_ROUNDS: usize = 40;
/// Rounds before the limit at which the model is told to wrap up.
const WRAP_UP_AT: usize = 5;
const _: () = assert!(WRAP_UP_AT < MAX_ROUNDS);

/// Told to the model with the last tool results when few rounds are left.
fn rounds_left_note(left: usize) -> String {
    format!("\n\n[Lumen: {left} tool rounds left for this request. Stop exploring soon and give the user your answer.]")
}

/// The one-off instruction for the final round once all rounds are used (not stored in the chat).
const ANSWER_NOW: &str = "[Lumen: you have used all the tool rounds for this request, so tools are switched off now. \
Answer the user's request now from what you have found so far. Say briefly what you could not check yet, \
so the user can ask you to continue.]";

/// Assistant loop: stream a reply; if it requested tools, execute them, store the results and ask
/// again (at most `MAX_ROUNDS`). Mid-loop it keeps long tool sessions inside the window. A turn always
/// ends with an answer: when the rounds run out, a last round without tools makes the model reply
/// (before, the loop just stopped after the last tool result - the user saw reasoning and no answer).
async fn run_turn(app: &AppHandle, chat_id: &str, project: Option<&Project>, think: bool, web: bool, cancel: &AtomicBool) -> Result<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let defs = tools::definitions(project.is_some(), web);
    let ctx = ToolCtx { app, project, chat_id, cancel };
    let mut answered = false;
    for round in 0..MAX_ROUNDS {
        let _ = fit_context(&state, chat_id, project, web, &defs, (0.72, 0.5, 4))?;
        let chat = state.db.chat(chat_id)?.ok_or_else(|| anyhow!("chat deleted"))?;
        let messages = build_messages(&state, &chat, project, web)?;
        let mut body = gen_params(&state, think);
        body["messages"] = json!(messages);
        if !defs.is_empty() {
            body["tools"] = json!(defs);
        }
        let (_msg, r) = match stream_assistant(app, chat_id, body.clone(), cancel, json!({})).await {
            Ok(x) => x,
            Err(e) if is_context_overflow(&e) => {
                // the estimate was off: drop everything but the latest tool result, summarise the earlier
                // turns too (long conversations overflow without any tool output), and try once more
                state.log(format!("context overflow, shrinking the prompt and retrying: {e}"));
                fit_context(&state, chat_id, project, web, &defs, (0.0, 0.3, 1))?;
                compress(app, chat_id, cancel).await?;
                let chat = state.db.chat(chat_id)?.ok_or_else(|| anyhow!("chat deleted"))?;
                body["messages"] = json!(build_messages(&state, &chat, project, web)?);
                stream_assistant(app, chat_id, body, cancel, json!({})).await.map_err(|e| {
                    if is_context_overflow(&e) {
                        anyhow!("This turn no longer fits in the model's context window. Start a new chat, raise the context size in Settings, or ask for a smaller part (a folder) to be reviewed with audit_code.")
                    } else {
                        e
                    }
                })?
            }
            Err(e) if llm::is_connection_error(&e) && !cancel.load(Ordering::Relaxed) => {
                // llama-server died or dropped the connection (e.g. after a long audit): bring it back and retry
                state.log(format!("lost the connection to llama-server: {e}"));
                emit(app, chat_id, "notice", json!({"key": "serverRestarting"}));
                crate::server::recover(app).await.map_err(|re| anyhow!("The model server stopped and could not be restarted: {re}"))?;
                stream_assistant(app, chat_id, body, cancel, json!({}))
                    .await
                    .map_err(|e2| if llm::is_connection_error(&e2) { anyhow!("The model server stopped again while answering. Its last output is in LumenData\\logs\\lumen.log (Settings → Server logs). Try a smaller request, a smaller context size, or another model.") } else { e2 })?
            }
            Err(e) => return Err(e),
        };
        if r.cancelled || r.tool_calls.is_empty() {
            // (a reply with reasoning but no text and no tool calls is no answer either)
            answered = r.cancelled || !r.content.trim().is_empty();
            break;
        }
        let left = MAX_ROUNDS - round - 1;
        // room left for this round's results, shared between the calls
        let b = budget(&state, &state.db.chat(chat_id)?.ok_or_else(|| anyhow!("chat deleted"))?);
        let mut used = (r.prompt_tokens + r.completion_tokens).max(1);
        let n_calls = r.tool_calls.len() as u32;
        for (i, tc) in r.tool_calls.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            emit(app, chat_id, "tool_start", json!({"id": tc.id, "name": tc.name, "arguments": tc.arguments}));
            let out = tools::run(&ctx, &tc.id, &tc.name, &tc.arguments).await;
            let free = (b as f32 * 0.8) as u32;
            let share = free.saturating_sub(used) / (n_calls - i as u32).max(1);
            let mut content = clip_tool_output(&out.content, share.clamp(1500, 16_000));
            if left == WRAP_UP_AT && i + 1 == r.tool_calls.len() {
                content.push_str(&rounds_left_note(left));
            }
            used += est_tokens(content.len());
            let mut meta = json!({"tool_call_id": tc.id, "name": tc.name, "arguments": tc.arguments, "display": out.display, "ok": out.ok});
            if content.len() < out.content.len() {
                meta["full"] = json!(out.content);
                meta["clipped"] = json!(true);
            }
            let m = state.db.add_message(chat_id, "tool", &content, "", &meta)?;
            emit(app, chat_id, "message", json!({"message": m}));
        }
        // compress mid-loop if long tool sessions approach the limit
        let c = state.db.chat(chat_id)?.ok_or_else(|| anyhow!("chat deleted"))?;
        if c.ctx_used as f32 > budget(&state, &c) as f32 * 0.88 {
            compress(app, chat_id, cancel).await?;
        }
    }
    if !answered && !cancel.load(Ordering::Relaxed) {
        state.log(format!("[lumen] the turn ended without an answer (all {MAX_ROUNDS} rounds used, or no text): asking for it without tools"));
        emit(app, chat_id, "notice", json!({"key": "answerNow"}));
        let _ = fit_context(&state, chat_id, project, web, &defs, (0.72, 0.5, 4))?;
        let chat = state.db.chat(chat_id)?.ok_or_else(|| anyhow!("chat deleted"))?;
        let mut body = gen_params(&state, think);
        body["messages"] = json!(final_round_messages(build_messages(&state, &chat, project, web)?));
        // the same tool list keeps the prompt prefix (and llama-server's cache) of the earlier rounds;
        // tool_choice "none" plus the note ask for text. (llama-server still parses a tool call the model
        // writes anyway, so the result is checked below.)
        if !defs.is_empty() {
            body["tools"] = json!(defs);
            body["tool_choice"] = json!("none");
        }
        let (_msg, r) = stream_assistant(app, chat_id, body, cancel, json!({"forced_answer": true})).await?;
        if r.content.trim().is_empty() && !r.cancelled {
            return Err(anyhow!(
                "The model kept using tools instead of answering ({MAX_ROUNDS} rounds). Ask it to answer with what it has found so far, or to continue with a narrower part."
            ));
        }
    }
    Ok(())
}

/// The request for the last round: the conversation plus the one-off "answer now" note (not stored,
/// so later prompts stay byte-stable).
fn final_round_messages(mut messages: Vec<Value>) -> Vec<Value> {
    messages.push(json!({"role": "user", "content": ANSWER_NOW}));
    messages
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
    let project = state.db.chat_project(&chat2)?;
    let msgs = build_messages(&state, &chat2, project.as_ref(), false)?;
    let after = llm::estimate_tokens(&serde_json::to_string(&msgs)?);
    state.db.set_chat_ctx(chat_id, after)?;
    let meta = json!({"kind": "compress", "state": "done", "before": before, "after": after, "count": old.len(), "summary": summary});
    state.db.update_message(&ev.id, "", "", &meta)?;
    emit(app, chat_id, "message_update", json!({"message": {"id": ev.id, "meta": meta}}));
    emit(app, chat_id, "compressed", json!({"upto_seq": keep_from, "ctx_used": after, "ctx_budget": budget(&state, &chat2)}));
    Ok(())
}

/// The whole conversation (compressed messages included) as a Markdown document.
pub fn export_markdown(chat: &Chat, messages: &[Message]) -> String {
    let mut out = format!("# {}\n\n_Exported from Lumen · {}_\n", chat.title, ymd(chat.created / 1000));
    for m in messages {
        match m.role.as_str() {
            "user" => {
                out.push_str("\n## You\n\n");
                out.push_str(m.content.trim());
                out.push('\n');
            }
            "assistant" => {
                if m.content.trim().is_empty() {
                    continue;
                }
                out.push_str("\n## Lumen\n\n");
                if !m.reasoning.trim().is_empty() {
                    out.push_str("<details><summary>Reasoning</summary>\n\n");
                    out.push_str(m.reasoning.trim());
                    out.push_str("\n\n</details>\n\n");
                }
                out.push_str(m.content.trim());
                out.push('\n');
            }
            "tool" => {
                let name = m.meta["name"].as_str().unwrap_or("tool");
                let args: Value = serde_json::from_str(m.meta["arguments"].as_str().unwrap_or("{}")).unwrap_or_default();
                let arg = ["path", "query", "url", "pattern", "name", "command", "expression"].iter().find_map(|k| args[*k].as_str()).unwrap_or("");
                out.push_str(&format!("\n> Tool `{name}` {arg}\n"));
            }
            _ => {}
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_keeps_start_and_end() {
        let s: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        let c = clip_tool_output(&s, 1500);
        assert!(c.len() < 5200 && c.starts_with("line 0\n") && c.trim_end().ends_with("line 4999"));
        assert!(c.contains("cut to fit the context window"));
        assert_eq!(clip_tool_output("short", 1500), "short");
    }

    /// The field case: "check the project and see what features may be missing" used all 40 rounds
    /// (85 tool calls) and the turn ended on a tool result with no answer. The last round now asks for
    /// the answer with tools off, with a one-off note after the stored history.
    #[test]
    fn rounds_running_out_end_with_an_answer() {
        let history = vec![
            json!({"role": "system", "content": "sys"}),
            json!({"role": "user", "content": "Hey, check the project and see what features may be missing"}),
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "read_file", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "…"}),
        ];
        let req = final_round_messages(history.clone());
        assert_eq!(req.len(), history.len() + 1);
        assert_eq!(&req[..history.len()], &history[..], "the stored history is sent unchanged (prompt cache)");
        assert_eq!(req.last().unwrap()["role"], "user", "a trailing system message breaks some chat templates");
        assert!(req.last().unwrap()["content"].as_str().unwrap().contains("Answer the user's request now"));
        assert!(rounds_left_note(WRAP_UP_AT).contains("5 tool rounds left"));
    }

    #[test]
    fn overflow_errors_are_recognised() {
        assert!(is_context_overflow(&anyhow!("llama-server 400 Bad Request: the request exceeds the available context size, try increasing it")));
        // the exact message reported from the field
        assert!(is_context_overflow(&anyhow!("llama-server 400 Bad Request: request (40678 tokens) exceeds the available context size (39680 tokens), try increasing it")));
        assert!(!is_context_overflow(&anyhow!("connection refused")));
    }

    /// A single-turn audit that read many files: old tool results get stubbed until the prompt fits.
    #[test]
    fn long_turn_is_pruned_to_fit() {
        let state = AppState::new().unwrap();
        state.info.write().n_ctx = 8192;
        let chat = state.db.create_chat(None, "New chat").unwrap();
        state.db.add_message(&chat.id, "user", "Audit the whole project", "", &json!({})).unwrap();
        let big: String = "fn x() { let y = 1; }\n".repeat(400); // ~8.8k chars each
        for i in 0..8 {
            let id = format!("call{i}");
            state.db.add_message(&chat.id, "assistant", "", "", &json!({"tool_calls": [{"id": id, "name": "read_file", "arguments": format!("{{\"path\":\"f{i}.rs\"}}")}]})).unwrap();
            state.db.add_message(&chat.id, "tool", &big, "", &json!({"tool_call_id": id, "name": "read_file", "arguments": format!("{{\"path\":\"f{i}.rs\"}}")})).unwrap();
        }
        let c = state.db.chat(&chat.id).unwrap().unwrap();
        let before = prompt_estimate(&state, &c, None, false, &[]).unwrap();
        assert!(before as f32 > 8192.0 * 0.72, "test setup must overflow: {before}");
        let after = fit_context(&state, &chat.id, None, false, &[], (0.72, 0.5, 4)).unwrap();
        println!("estimate {before} -> {after} tokens (window 8192)");
        assert!(after as f32 <= 8192.0 * 0.6);
        let msgs = state.db.messages(&chat.id).unwrap();
        let tools: Vec<&Message> = msgs.iter().filter(|m| m.role == "tool").collect();
        assert!(tools[0].meta["pruned"].as_bool().unwrap_or(false));
        assert_eq!(tools[0].meta["full"].as_str().unwrap(), big);
        assert!(tools[0].content.contains("read_file(f0.rs) output removed"));
        assert!(!tools.last().unwrap().meta["pruned"].as_bool().unwrap_or(false), "the latest results stay");
        // already fitting: nothing more changes, so the prompt stays byte-stable
        let again = fit_context(&state, &chat.id, None, false, &[], (0.72, 0.5, 4)).unwrap();
        assert_eq!(again, after);
        state.db.delete_chat(&chat.id).unwrap();
    }
}
