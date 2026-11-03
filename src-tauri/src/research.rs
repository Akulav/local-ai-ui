//! Deep research: plan → search → read → reflect (up to 3 rounds) → cited report.

use anyhow::{anyhow, Result};
use futures_util::stream::{self, StreamExt};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

use crate::chat::{emit, gen_params, stream_assistant, today};
use crate::db::Message;
use crate::llm;
use crate::prompts;
use crate::rag::{chunk, ProjectIndex, Chunk};
use crate::state::AppState;

struct Card {
    msg: Message,
    meta: Value,
}

impl Card {
    fn step(&mut self, app: &AppHandle, state: &AppState, icon: &str, text: &str, progress: f32) {
        self.meta["steps"].as_array_mut().unwrap().push(json!({"icon": icon, "text": text}));
        self.meta["progress"] = json!(progress);
        self.flush(app, state);
    }
    fn flush(&self, app: &AppHandle, state: &AppState) {
        let _ = state.db.update_message(&self.msg.id, "", "", &self.meta);
        emit(app, &self.msg.chat_id, "message_update", json!({"message": {"id": self.msg.id, "meta": self.meta}}));
    }
}

struct Source {
    title: String,
    url: String,
    note: String,
}

fn domain(url: &str) -> String {
    url.split("//").nth(1).unwrap_or(url).split('/').next().unwrap_or("").trim_start_matches("www.").to_string()
}

/// Runs the whole pipeline. Progress lives in an `event` message (the research card) that is updated
/// in place; the final report is a normal assistant message carrying `meta.sources` for citations.
/// Planning/reflection use JSON-schema constrained output so even small models return valid JSON.
pub async fn run(app: &AppHandle, chat_id: &str, question: &str, think: bool, cancel: &AtomicBool) -> Result<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let reply_lang = state.settings.read().reply_lang.clone();
    let date = today();
    let ev = state.db.add_message(chat_id, "event", "", "", &json!({"kind": "research", "state": "running", "steps": [], "sources": [], "progress": 0.02}))?;
    emit(app, chat_id, "message", json!({"message": ev}));
    let mut card = Card { meta: ev.meta.clone(), msg: ev };

    // context from the conversation so follow-up research requests make sense
    let history: String = state
        .db
        .messages(chat_id)?
        .iter()
        .filter(|m| !m.compressed && (m.role == "user" || m.role == "assistant") && !m.content.is_empty())
        .rev()
        .skip(1)
        .take(4)
        .map(|m| format!("{}: {}", m.role, m.content.chars().take(600).collect::<String>()))
        .collect::<Vec<_>>()
        .join("\n");

    // 1. plan
    card.step(app, &state, "plan", "Planning research", 0.05);
    let plan_schema = json!({
        "type": "object",
        "properties": {
            "objective": {"type": "string"},
            "language": {"type": "string", "enum": ["en", "ro"]},
            "subquestions": {"type": "array", "items": {"type": "string"}, "minItems": 2, "maxItems": 6},
            "queries": {"type": "array", "items": {"type": "string"}, "minItems": 3, "maxItems": 7}
        },
        "required": ["objective", "language", "subquestions", "queries"]
    });
    let user_in = if history.is_empty() { question.to_string() } else { format!("Recent conversation:\n{history}\n\nResearch request: {question}") };
    let plan_txt = llm::complete(
        &state,
        json!([{"role": "system", "content": prompts::research_plan(&reply_lang, &date)}, {"role": "user", "content": user_in}]),
        900,
        Some(plan_schema),
    )
    .await?;
    let plan: Value = serde_json::from_str(&plan_txt).map_err(|_| anyhow!("research planner returned invalid JSON"))?;
    let objective = plan["objective"].as_str().unwrap_or(question).to_string();
    let lang = plan["language"].as_str().unwrap_or("en").to_string();
    let subq: Vec<String> = plan["subquestions"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect();
    let mut queries: Vec<String> = plan["queries"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect();
    card.meta["objective"] = json!(objective);
    card.meta["subquestions"] = json!(subq);
    card.step(app, &state, "plan", &format!("Plan ready · {} sub-questions", subq.len()), 0.1);

    let focus = format!("{objective}\n{}", subq.join("\n"));
    let mut sources: Vec<Source> = Vec::new();
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut used_queries: Vec<String> = Vec::new();
    let max_rounds = 3;
    let page_budget_per_round = 7;

    for round in 0..max_rounds {
        if cancel.load(Ordering::Relaxed) || queries.is_empty() {
            break;
        }
        let base_p = 0.1 + round as f32 * 0.25;
        // 2. search
        let mut candidates: Vec<crate::web::SearchHit> = Vec::new();
        for (qi, q) in queries.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            card.step(app, &state, "search", &format!("Searching: {q}"), base_p + 0.08 * (qi as f32 / queries.len() as f32));
            match crate::web::search(&state, q, 6).await {
                Ok(hits) => candidates.extend(hits),
                Err(e) => card.step(app, &state, "warn", &format!("Search failed: {e}"), base_p),
            }
            used_queries.push(q.clone());
        }
        // pick new URLs, spread across domains
        let mut picked: Vec<crate::web::SearchHit> = Vec::new();
        let mut per_domain: std::collections::HashMap<String, usize> = Default::default();
        for h in candidates {
            if picked.len() >= page_budget_per_round {
                break;
            }
            if seen_urls.contains(&h.url) {
                continue;
            }
            let d = domain(&h.url);
            let n = per_domain.entry(d).or_default();
            if *n >= 2 {
                continue;
            }
            *n += 1;
            seen_urls.insert(h.url.clone());
            picked.push(h);
        }
        // 3. read pages concurrently
        card.step(app, &state, "read", &format!("Reading {} pages", picked.len()), base_p + 0.1);
        let st = state.clone();
        let pages: Vec<(crate::web::SearchHit, Option<crate::web::Page>)> = stream::iter(picked.into_iter())
            .map(|h| {
                let st = st.clone();
                async move {
                    let p = crate::web::fetch(&st, &h.url).await.ok();
                    (h, p)
                }
            })
            .buffer_unordered(4)
            .collect()
            .await;
        for (h, page) in pages {
            let (title, text) = match page {
                Some(p) if p.text.len() > 300 => (if p.title.is_empty() { h.title.clone() } else { p.title }, p.text),
                _ => {
                    if h.snippet.len() > 60 {
                        (h.title.clone(), h.snippet.clone())
                    } else {
                        continue;
                    }
                }
            };
            let note = best_passages(&text, &focus, 3, 4200);
            if note.trim().is_empty() {
                continue;
            }
            sources.push(Source { title: title.chars().take(160).collect(), url: h.url.clone(), note });
            card.meta["sources"].as_array_mut().unwrap().push(json!({"n": sources.len(), "title": sources.last().unwrap().title, "url": h.url}));
            card.flush(app, &state);
        }
        card.step(app, &state, "read", &format!("{} sources collected", sources.len()), base_p + 0.18);
        if round + 1 >= max_rounds || cancel.load(Ordering::Relaxed) {
            break;
        }
        // 4. reflect
        card.step(app, &state, "think", "Checking for gaps", base_p + 0.2);
        let notes = notes_block(&sources, 2200, 26_000);
        let reflect_schema = json!({
            "type": "object",
            "properties": {
                "sufficient": {"type": "boolean"},
                "gaps": {"type": "array", "items": {"type": "string"}, "maxItems": 4},
                "queries": {"type": "array", "items": {"type": "string"}, "maxItems": 4}
            },
            "required": ["sufficient", "gaps", "queries"]
        });
        let input = format!(
            "Objective: {objective}\nSub-questions:\n- {}\n\nQueries already used:\n- {}\n\nNotes so far:\n{notes}",
            subq.join("\n- "),
            used_queries.join("\n- ")
        );
        let r = llm::complete(
            &state,
            json!([{"role": "system", "content": prompts::research_reflect(&date)}, {"role": "user", "content": input}]),
            600,
            Some(reflect_schema),
        )
        .await
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or(json!({"sufficient": true}));
        let sufficient = r["sufficient"].as_bool().unwrap_or(true) && sources.len() >= 4;
        queries = r["queries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|q| q.as_str().map(String::from))
            .filter(|q| !used_queries.contains(q))
            .collect();
        if let Some(g) = r["gaps"].as_array().filter(|g| !g.is_empty()) {
            let gaps: Vec<String> = g.iter().filter_map(|x| x.as_str().map(String::from)).collect();
            card.step(app, &state, "think", &format!("Gaps: {}", gaps.join("; ")), base_p + 0.22);
        }
        if sufficient || queries.is_empty() {
            break;
        }
    }
    if sources.is_empty() {
        card.meta["state"] = json!("error");
        card.step(app, &state, "warn", "No usable sources found (offline?)", 1.0);
        return Err(anyhow!("deep research could not collect any sources"));
    }

    // 5. write the report
    card.step(app, &state, "write", &format!("Writing report from {} sources", sources.len()), 0.85);
    let n_ctx = state.info.read().n_ctx.max(8192) as usize;
    let note_budget = ((n_ctx as f32 * 0.5) * 3.4) as usize;
    let per = (note_budget / sources.len().max(1)).clamp(800, 4200);
    let notes = notes_block(&sources, per, note_budget);
    let input = format!(
        "Research request: {question}\n\nObjective: {objective}\nSub-questions:\n- {}\n\n# Source notes\n{notes}",
        subq.join("\n- ")
    );
    let mut body = gen_params(&state, think);
    body["messages"] = json!([
        {"role": "system", "content": prompts::research_write(&lang, &date)},
        {"role": "user", "content": input}
    ]);
    let src_meta: Vec<Value> = sources.iter().enumerate().map(|(i, s)| json!({"n": i + 1, "title": s.title, "url": s.url})).collect();
    let r = stream_assistant(app, chat_id, body, cancel, json!({"sources": src_meta, "research": true})).await;
    card.meta["state"] = json!(if r.is_ok() { "done" } else { "error" });
    card.step(app, &state, "done", "Research complete", 1.0);
    // the report was generated from a separate prompt; re-estimate what the chat itself now occupies
    if r.is_ok() {
        if let Some(c) = state.db.chat(chat_id)? {
            let msgs = crate::chat::build_messages(&state, &c, None, false)?;
            let used = llm::estimate_tokens(&serde_json::to_string(&msgs)?);
            state.db.set_chat_ctx(chat_id, used)?;
            emit(app, chat_id, "ctx", json!({"ctx_used": used}));
        }
    }
    r.map(|_| ())
}

fn notes_block(sources: &[Source], per_source: usize, total: usize) -> String {
    let mut out = String::new();
    for (i, s) in sources.iter().enumerate() {
        let note: String = s.note.chars().take(per_source).collect();
        let block = format!("[{}] {} — {}\n{}\n\n", i + 1, s.title, s.url, note);
        if out.len() + block.len() > total {
            break;
        }
        out.push_str(&block);
    }
    out
}

/// Pick the passages of a page most relevant to the research focus (BM25 over page chunks).
fn best_passages(text: &str, focus: &str, k: usize, max_chars: usize) -> String {
    let chunks: Vec<Chunk> = chunk(text, false)
        .into_iter()
        .enumerate()
        .map(|(i, (a, b, t))| Chunk { id: i as i64, path: String::new(), line_start: a, line_end: b, text: t })
        .collect();
    if chunks.is_empty() {
        return String::new();
    }
    if chunks.len() <= k {
        return chunks.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("\n…\n").chars().take(max_chars).collect();
    }
    let n = chunks.len();
    let idx = ProjectIndex::build(chunks, vec![None; n]);
    let mut top: Vec<usize> = idx.bm25(focus, k).into_iter().map(|(d, _)| d).collect();
    if top.is_empty() {
        top = (0..k.min(n)).collect();
    }
    top.sort();
    top.iter().map(|&d| idx.chunks[d].text.as_str()).collect::<Vec<_>>().join("\n…\n").chars().take(max_chars).collect()
}
