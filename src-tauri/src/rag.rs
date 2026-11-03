//! Project retrieval: incremental indexing, BM25 (diacritics-insensitive, code-aware) + dense
//! embeddings, fused with reciprocal rank fusion.

use anyhow::Result;
use rusqlite::params;
use serde::Serialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use unicode_normalization::UnicodeNormalization;

use crate::extract;
use crate::state::AppState;

const STOP: &[&str] = &[
    "the", "an", "and", "or", "of", "to", "in", "is", "are", "for", "on", "with", "as", "at", "by", "it", "this",
    "that", "be", "from", "was", "were", "what", "how", "si", "sau", "de", "la", "pe", "cu", "un", "este", "sunt",
    "care", "ce", "din", "pentru", "nu", "se", "sa", "mai", "ca", "al", "ale", "lui", "fi", "am", "ai", "unei", "unui",
];

pub fn fold(s: &str) -> String {
    s.nfd().filter(|c| !unicode_normalization::char::is_combining_mark(*c)).collect()
}

pub fn tokenize(s: &str) -> Vec<String> {
    let folded = fold(s);
    let mut out = Vec::new();
    for word in folded.split(|c: char| !c.is_alphanumeric()) {
        if word.is_empty() {
            continue;
        }
        let lower = word.to_lowercase();
        // camelCase / PascalCase parts
        let mut parts = Vec::new();
        let mut cur = String::new();
        let chars: Vec<char> = word.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            let boundary = i > 0 && c.is_uppercase() && (chars[i - 1].is_lowercase() || chars.get(i + 1).map_or(false, |n| n.is_lowercase()) && chars[i - 1].is_uppercase());
            if boundary && !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
            cur.extend(c.to_lowercase());
        }
        if !cur.is_empty() {
            parts.push(cur);
        }
        if lower.chars().count() >= 2 && !STOP.contains(&lower.as_str()) {
            out.push(lower.clone());
        }
        if parts.len() > 1 {
            for p in parts {
                if p.chars().count() >= 2 && p != lower && !STOP.contains(&p.as_str()) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Split text into overlapping chunks: (line_start, line_end, text). 1-based inclusive lines.
pub fn chunk(text: &str, code: bool) -> Vec<(usize, usize, String)> {
    let target = if code { 2400 } else { 1700 };
    let overlap = if code { 10 } else { 2 };
    // break pathological long lines (PDF paragraphs) into pieces
    let mut lines: Vec<(usize, String)> = Vec::new();
    for (ln, l) in text.lines().enumerate() {
        if l.len() > target {
            let chars: Vec<char> = l.chars().collect();
            for piece in chars.chunks(target * 3 / 4) {
                lines.push((ln + 1, piece.iter().collect()));
            }
        } else {
            lines.push((ln + 1, l.to_string()));
        }
    }
    let n = lines.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let mut j = i;
        let mut size = 0;
        while j < n && (size + lines[j].1.len() < target || j == i) {
            size += lines[j].1.len() + 1;
            j += 1;
        }
        if !code && j < n {
            // prefer ending on a paragraph break
            let mid = i + (j - i) / 2;
            if let Some(b) = (mid..j).rev().find(|&k| lines[k].1.trim().is_empty()) {
                j = b + 1;
            }
        }
        let body: Vec<&str> = lines[i..j].iter().map(|(_, s)| s.as_str()).collect();
        let body = body.join("\n");
        if !body.trim().is_empty() {
            out.push((lines[i].0, lines[j - 1].0, body));
        }
        if j >= n {
            break;
        }
        i = j.saturating_sub(overlap).max(i + 1);
    }
    out
}

#[derive(Clone, Serialize)]
pub struct Chunk {
    pub id: i64,
    pub path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub text: String,
}

pub struct ProjectIndex {
    pub chunks: Vec<Chunk>,
    postings: HashMap<String, Vec<(u32, u16)>>,
    doc_len: Vec<u32>,
    avgdl: f32,
    embs: Vec<Option<Vec<f32>>>,
}

impl ProjectIndex {
    pub fn build(chunks: Vec<Chunk>, embs: Vec<Option<Vec<f32>>>) -> Self {
        let mut postings: HashMap<String, Vec<(u32, u16)>> = HashMap::new();
        let mut doc_len = Vec::with_capacity(chunks.len());
        for (d, c) in chunks.iter().enumerate() {
            let toks = tokenize(&format!("{} {}", c.path, c.text));
            doc_len.push(toks.len() as u32);
            let mut tf: HashMap<String, u16> = HashMap::new();
            for t in toks {
                *tf.entry(t).or_default() += 1;
            }
            for (t, f) in tf {
                postings.entry(t).or_default().push((d as u32, f));
            }
        }
        let avgdl = if doc_len.is_empty() { 1.0 } else { doc_len.iter().sum::<u32>() as f32 / doc_len.len() as f32 };
        ProjectIndex { chunks, postings, doc_len, avgdl, embs }
    }

    pub fn has_embeddings(&self) -> bool {
        self.embs.iter().any(|e| e.is_some())
    }

    pub fn bm25(&self, query: &str, k: usize) -> Vec<(usize, f32)> {
        let n = self.chunks.len() as f32;
        let (k1, b) = (1.3f32, 0.72f32);
        let mut scores: HashMap<u32, f32> = HashMap::new();
        let qt: HashSet<String> = tokenize(query).into_iter().collect();
        for t in qt {
            let Some(list) = self.postings.get(&t) else { continue };
            let df = list.len() as f32;
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            for &(d, tf) in list {
                let tf = tf as f32;
                let dl = self.doc_len[d as usize] as f32;
                let s = idf * tf * (k1 + 1.0) / (tf + k1 * (1.0 - b + b * dl / self.avgdl));
                *scores.entry(d).or_default() += s;
            }
        }
        let mut v: Vec<(usize, f32)> = scores.into_iter().map(|(d, s)| (d as usize, s)).collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(k);
        v
    }

    pub fn dense(&self, q: &[f32], k: usize) -> Vec<(usize, f32)> {
        let mut v: Vec<(usize, f32)> = self
            .embs
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.as_ref().map(|e| (i, dot(e, q))))
            .collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(k);
        v
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

fn emb_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn blob_to_emb(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

pub fn load_index(state: &AppState, project_id: &str) -> Result<Arc<ProjectIndex>> {
    if let Some(i) = state.indexes.lock().get(project_id) {
        return Ok(i.clone());
    }
    let (chunks, embs) = {
        let c = state.db.0.lock();
        let mut st = c.prepare("SELECT id,path,line_start,line_end,text,emb FROM chunks WHERE project_id=?1 ORDER BY path, idx")?;
        let mut chunks = Vec::new();
        let mut embs = Vec::new();
        let rows = st.query_map([project_id], |r| {
            Ok((
                Chunk { id: r.get(0)?, path: r.get(1)?, line_start: r.get::<_, i64>(2)? as usize, line_end: r.get::<_, i64>(3)? as usize, text: r.get(4)? },
                r.get::<_, Option<Vec<u8>>>(5)?,
            ))
        })?;
        for row in rows.flatten() {
            chunks.push(row.0);
            embs.push(row.1.map(|b| blob_to_emb(&b)));
        }
        (chunks, embs)
    };
    let idx = Arc::new(ProjectIndex::build(chunks, embs));
    state.indexes.lock().insert(project_id.to_string(), idx.clone());
    Ok(idx)
}

const QUERY_INSTRUCT: &str = "Instruct: Given a question or search query, retrieve relevant passages from documents and source code\nQuery: ";

/// Hybrid search. Returns best chunks first.
pub async fn search(app: &AppHandle, project_id: &str, query: &str, k: usize) -> Result<Vec<Chunk>> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let idx = load_index(&state, project_id)?;
    if idx.chunks.is_empty() {
        return Ok(vec![]);
    }
    let lexical = idx.bm25(query, 60);
    let mut fused: HashMap<usize, f32> = HashMap::new();
    for (rank, (d, _)) in lexical.iter().enumerate() {
        *fused.entry(*d).or_default() += 1.0 / (60.0 + rank as f32);
    }
    if idx.has_embeddings() && state.settings.read().semantic_search {
        if let Ok(mut e) = crate::server::embed(app, &[format!("{QUERY_INSTRUCT}{query}")]).await {
            if let Some(q) = e.pop().filter(|v| !v.is_empty()) {
                let q = normalize(q);
                for (rank, (d, _)) in idx.dense(&q, 60).iter().enumerate() {
                    *fused.entry(*d).or_default() += 1.0 / (60.0 + rank as f32);
                }
            }
        }
    }
    let mut v: Vec<(usize, f32)> = fused.into_iter().collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    // diversify: at most 3 chunks per file
    let mut per_file: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::new();
    for (d, _) in v {
        let c = &idx.chunks[d];
        let n = per_file.entry(c.path.as_str()).or_default();
        if *n >= 3 {
            continue;
        }
        *n += 1;
        out.push(c.clone());
        if out.len() >= k {
            break;
        }
    }
    Ok(out)
}

fn emit_progress(app: &AppHandle, project_id: &str, phase: &str, done: usize, total: usize, extra: serde_json::Value) {
    let _ = app.emit("index-progress", json!({"project_id": project_id, "phase": phase, "done": done, "total": total, "extra": extra}));
}

const SKIP_DIRS: &[&str] = &["node_modules", ".git", "target", "dist", "build", ".venv", "venv", "__pycache__", ".next", ".idea", ".vs", "bin", "obj", "LumenData"];

pub fn walk(folder: &Path) -> Vec<(String, i64, u64)> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(folder)
        .hidden(true)
        .git_ignore(true)
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !(e.file_type().map_or(false, |t| t.is_dir()) && SKIP_DIRS.contains(&name.as_ref()))
        })
        .build();
    for e in walker.flatten() {
        if !e.file_type().map_or(false, |t| t.is_file()) {
            continue;
        }
        let p = e.path();
        if !extract::is_supported(p) {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
        let limit = if ext == "pdf" || ext == "docx" { 120 * 1024 * 1024 } else { 3 * 1024 * 1024 };
        if md.len() > limit {
            continue;
        }
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Ok(rel) = p.strip_prefix(folder) {
            out.push((rel.to_string_lossy().replace('\\', "/"), mtime, md.len()));
        }
        if out.len() >= 25000 {
            break;
        }
    }
    out
}

/// Incrementally (re)index a project folder.
pub async fn index_project(app: &AppHandle, project_id: &str) -> Result<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let project = state.db.project(project_id)?.ok_or_else(|| anyhow::anyhow!("project not found"))?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut m = state.indexing.lock();
        if m.contains_key(project_id) {
            return Ok(()); // already running
        }
        m.insert(project_id.to_string(), cancel.clone());
    }
    let r = index_inner(app, &state, &project.id, PathBuf::from(&project.folder), &cancel).await;
    state.indexing.lock().remove(project_id);
    state.indexes.lock().remove(project_id);
    let _ = load_index(&state, project_id);
    match &r {
        Ok(_) => emit_progress(app, project_id, "done", 1, 1, json!(stats(&state, project_id))),
        Err(e) => emit_progress(app, project_id, "error", 0, 0, json!({"error": e.to_string()})),
    }
    r
}

pub fn stats(state: &AppState, project_id: &str) -> serde_json::Value {
    let c = state.db.0.lock();
    let files: i64 = c.query_row("SELECT COUNT(*) FROM files WHERE project_id=?1", [project_id], |r| r.get(0)).unwrap_or(0);
    let chunks: i64 = c.query_row("SELECT COUNT(*) FROM chunks WHERE project_id=?1", [project_id], |r| r.get(0)).unwrap_or(0);
    let embedded: i64 = c
        .query_row("SELECT COUNT(*) FROM chunks WHERE project_id=?1 AND emb IS NOT NULL", [project_id], |r| r.get(0))
        .unwrap_or(0);
    json!({"files": files, "chunks": chunks, "embedded": embedded})
}

async fn index_inner(app: &AppHandle, state: &Arc<AppState>, pid: &str, folder: PathBuf, cancel: &AtomicBool) -> Result<()> {
    emit_progress(app, pid, "scan", 0, 0, json!({}));
    let f2 = folder.clone();
    let files = tokio::task::spawn_blocking(move || walk(&f2)).await?;
    let known: HashMap<String, (i64, u64)> = {
        let c = state.db.0.lock();
        let mut st = c.prepare("SELECT path, mtime, size FROM files WHERE project_id=?1")?;
        let rows = st.query_map([pid], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)? as u64))))?;
        rows.flatten().collect()
    };
    let present: HashSet<&str> = files.iter().map(|f| f.0.as_str()).collect();
    {
        let c = state.db.0.lock();
        for path in known.keys().filter(|p| !present.contains(p.as_str())) {
            c.execute("DELETE FROM chunks WHERE project_id=?1 AND path=?2", params![pid, path])?;
            c.execute("DELETE FROM files WHERE project_id=?1 AND path=?2", params![pid, path])?;
        }
    }
    let changed: Vec<&(String, i64, u64)> = files.iter().filter(|f| known.get(&f.0) != Some(&(f.1, f.2))).collect();
    let total = changed.len();
    for (n, (rel, mtime, size)) in changed.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if n % 3 == 0 {
            emit_progress(app, pid, "extract", n, total, json!({"file": rel}));
        }
        index_file(state, pid, &folder, rel, *mtime, *size).await;
    }
    if state.settings.read().semantic_search {
        embed_missing(app, state, pid, cancel).await?;
    }
    Ok(())
}

/// Extract + chunk one file into the DB (embeddings are filled later).
pub async fn index_file(state: &AppState, pid: &str, folder: &Path, rel: &str, mtime: i64, size: u64) {
    let full = folder.join(rel);
    let code = extract::is_code(&full);
    let text = tokio::task::spawn_blocking(move || extract::extract(&full)).await.ok().and_then(|r| r.ok());
    let c = state.db.0.lock();
    let _ = c.execute("DELETE FROM chunks WHERE project_id=?1 AND path=?2", params![pid, rel]);
    if let Some(text) = text {
        for (i, (a, b, body)) in chunk(&text, code).into_iter().enumerate() {
            let _ = c.execute(
                "INSERT INTO chunks(project_id,path,idx,line_start,line_end,text) VALUES(?1,?2,?3,?4,?5,?6)",
                params![pid, rel, i as i64, a as i64, b as i64, body],
            );
        }
    }
    let _ = c.execute(
        "INSERT OR REPLACE INTO files(project_id,path,mtime,size) VALUES(?1,?2,?3,?4)",
        params![pid, rel, mtime, size as i64],
    );
}

async fn embed_missing(app: &AppHandle, state: &Arc<AppState>, pid: &str, cancel: &AtomicBool) -> Result<()> {
    let todo: Vec<(i64, String, String)> = {
        let c = state.db.0.lock();
        let mut st = c.prepare("SELECT id, path, text FROM chunks WHERE project_id=?1 AND emb IS NULL")?;
        let rows = st.query_map([pid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.flatten().collect()
    };
    if todo.is_empty() {
        return Ok(());
    }
    let total = todo.len();
    emit_progress(app, pid, "embed", 0, total, json!({}));
    for (bi, batch) in todo.chunks(16).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let inputs: Vec<String> = batch
            .iter()
            .map(|(_, p, t)| {
                let s: String = t.chars().take(3000).collect();
                format!("{p}\n{s}")
            })
            .collect();
        let embs = crate::server::embed(app, &inputs).await?;
        {
            let c = state.db.0.lock();
            for ((id, _, _), e) in batch.iter().zip(embs) {
                if !e.is_empty() {
                    let _ = c.execute("UPDATE chunks SET emb=?2 WHERE id=?1", params![id, emb_to_blob(&normalize(e))]);
                }
            }
        }
        emit_progress(app, pid, "embed", ((bi + 1) * 16).min(total), total, json!({}));
    }
    Ok(())
}

/// Re-index a single file after the assistant wrote it.
pub async fn refresh_file(app: &AppHandle, pid: &str, folder: &Path, rel: &str) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let full = folder.join(rel);
    let Ok(md) = std::fs::metadata(&full) else { return };
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    index_file(&state, pid, folder, rel, mtime, md.len()).await;
    state.indexes.lock().remove(pid);
    let cancel = AtomicBool::new(false);
    if state.settings.read().semantic_search {
        let _ = embed_missing(app, &state, pid, &cancel).await;
    }
    state.indexes.lock().remove(pid);
}
