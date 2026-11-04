//! Streaming client for llama-server's OpenAI-compatible endpoint.

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::state::AppState;

#[derive(Debug, Clone, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Default)]
pub struct StreamResult {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub cached_tokens: u32,
    pub tps: f64,
    /// prompt processing speed and the prompt tokens actually evaluated (cache hits excluded)
    pub prompt_tps: f64,
    pub prompt_evaluated: u32,
    pub finish_reason: String,
    pub cancelled: bool,
}

pub enum Delta<'a> {
    Content(&'a str),
    Reasoning(&'a str),
    Tool { index: usize, name: &'a str },
    /// live tokens/s and tokens generated so far
    Progress { tps: f64, tokens: u32 },
}

/// POST `/v1/chat/completions` with `stream: true` and parse the SSE stream. `on` receives content,
/// reasoning and tool-call deltas plus ~5 Hz speed updates. Dropping the stream on `cancel`
/// closes the connection, which makes llama-server stop generating.
pub async fn stream_chat(
    state: &AppState,
    body: Value,
    cancel: &AtomicBool,
    mut on: impl FnMut(Delta),
) -> Result<StreamResult> {
    let base = state.llm_url().ok_or_else(|| anyhow!("No model is loaded. Pick one in Models."))?;
    state.touch();
    let r = stream_inner(state, &base, body, cancel, &mut on).await;
    state.touch(); // the idle timer counts from the end of the last generation
    r
}

async fn stream_inner(
    state: &AppState,
    base: &str,
    mut body: Value,
    cancel: &AtomicBool,
    on: &mut impl FnMut(Delta),
) -> Result<StreamResult> {
    body["stream"] = json!(true);
    body["stream_options"] = json!({"include_usage": true});
    body["cache_prompt"] = json!(true);
    if body.get("model").is_none() {
        body["model"] = json!("local");
    }
    let resp = state.http.post(format!("{base}/v1/chat/completions")).json(&body).send().await?;
    if !resp.status().is_success() {
        let code = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(String::from))
            .unwrap_or(text);
        return Err(anyhow!("llama-server {code}: {msg}"));
    }

    let mut res = StreamResult::default();
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::<u8>::new();
    let mut in_think = false;
    let mut first_tok: Option<Instant> = None;
    let mut n_tok: u32 = 0;
    let mut last_progress = Instant::now();

    'outer: while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            res.cancelled = true;
            break;
        }
        buf.extend_from_slice(&chunk?);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                break 'outer;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            if let Some(err) = v.get("error") {
                return Err(anyhow!("{}", err["message"].as_str().unwrap_or("generation error")));
            }
            if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                res.prompt_tokens = u["prompt_tokens"].as_u64().unwrap_or(0) as u32;
                res.completion_tokens = u["completion_tokens"].as_u64().unwrap_or(0) as u32;
            }
            if let Some(t) = v.get("timings").filter(|t| !t.is_null()) {
                if let Some(tps) = t["predicted_per_second"].as_f64() {
                    res.tps = tps;
                }
                res.cached_tokens = t["cache_n"].as_u64().unwrap_or(0) as u32;
                res.prompt_tps = t["prompt_per_second"].as_f64().unwrap_or(res.prompt_tps);
                res.prompt_evaluated = t["prompt_n"].as_u64().unwrap_or(0) as u32;
                if res.prompt_tokens == 0 {
                    res.prompt_tokens = (t["prompt_n"].as_u64().unwrap_or(0) + t["cache_n"].as_u64().unwrap_or(0)) as u32;
                }
            }
            let Some(choice) = v["choices"].get(0) else { continue };
            if let Some(fr) = choice["finish_reason"].as_str() {
                res.finish_reason = fr.to_string();
            }
            let d = &choice["delta"];
            let mut produced = false;
            if let Some(r) = d["reasoning_content"].as_str().filter(|s| !s.is_empty()) {
                res.reasoning.push_str(r);
                on(Delta::Reasoning(r));
                produced = true;
            }
            if let Some(c) = d["content"].as_str().filter(|s| !s.is_empty()) {
                produced = true;
                // fallback for templates that leave <think> tags in the content
                let mut s = c;
                loop {
                    if in_think {
                        if let Some(i) = s.find("</think>") {
                            res.reasoning.push_str(&s[..i]);
                            on(Delta::Reasoning(&s[..i]));
                            s = &s[i + 8..];
                            in_think = false;
                        } else {
                            res.reasoning.push_str(s);
                            on(Delta::Reasoning(s));
                            break;
                        }
                    } else if let Some(i) = s.find("<think>") {
                        res.content.push_str(&s[..i]);
                        on(Delta::Content(&s[..i]));
                        s = &s[i + 7..];
                        in_think = true;
                    } else {
                        res.content.push_str(s);
                        on(Delta::Content(s));
                        break;
                    }
                }
            }
            if let Some(tcs) = d["tool_calls"].as_array() {
                produced = true;
                for tc in tcs {
                    let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                    while res.tool_calls.len() <= idx {
                        res.tool_calls.push(ToolCall::default());
                    }
                    let slot = &mut res.tool_calls[idx];
                    if let Some(id) = tc["id"].as_str() {
                        slot.id = id.to_string();
                    }
                    if let Some(n) = tc["function"]["name"].as_str().filter(|s| !s.is_empty()) {
                        slot.name.push_str(n);
                        on(Delta::Tool { index: idx, name: n });
                    }
                    if let Some(a) = tc["function"]["arguments"].as_str() {
                        slot.arguments.push_str(a);
                    }
                }
            }
            if produced {
                n_tok += 1;
                let t0 = *first_tok.get_or_insert_with(Instant::now);
                if last_progress.elapsed().as_millis() > 200 {
                    let el = t0.elapsed().as_secs_f64();
                    let tps = if el > 0.25 { (n_tok.saturating_sub(1)) as f64 / el } else { 0.0 };
                    *state.live_tps.write() = tps;
                    on(Delta::Progress { tps, tokens: n_tok });
                    last_progress = Instant::now();
                }
            }
        }
    }
    *state.live_tps.write() = 0.0;
    if res.tps == 0.0 {
        if let Some(t0) = first_tok {
            let el = t0.elapsed().as_secs_f64();
            if el > 0.0 {
                res.tps = n_tok as f64 / el;
            }
        }
    }
    if res.completion_tokens == 0 {
        res.completion_tokens = n_tok;
    }
    for (i, tc) in res.tool_calls.iter_mut().enumerate() {
        if tc.id.is_empty() {
            tc.id = format!("call_{i}_{}", crate::state::new_id());
        }
    }
    res.tool_calls.retain(|t| !t.name.is_empty());
    Ok(res)
}

/// Non-interactive helper (titles, plans, JSON steps). Thinking off, optional JSON schema.
pub async fn complete(state: &AppState, messages: Value, max_tokens: u32, schema: Option<Value>) -> Result<String> {
    let mut body = json!({
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": 0.3,
        "chat_template_kwargs": {"enable_thinking": false},
    });
    if let Some(s) = schema {
        body["response_format"] = json!({"type": "json_schema", "json_schema": {"name": "out", "schema": s}});
    }
    let cancel = AtomicBool::new(false);
    let r = stream_chat(state, body, &cancel, |_| {}).await?;
    Ok(r.content.trim().to_string())
}

/// Rough token estimate when the server hasn't told us yet (~3.6 chars/token across EN/RO/code).
pub fn estimate_tokens(s: &str) -> u32 {
    (s.chars().count() as f32 / 3.6).ceil() as u32
}
