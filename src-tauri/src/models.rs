//! Model management: curated catalog for 16 GB cards, local GGUF listing/deletion and Hugging Face
//! search/listing/download.

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

#[derive(Serialize, Clone)]
pub struct CatalogEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub repo: &'static str,
    pub file: &'static str,
    pub size_gb: f32,
    pub tag: &'static str,
    pub desc_en: &'static str,
    pub desc_ro: &'static str,
    pub recommended: bool,
}

/// Curated for a 16 GB AMD card (RX 9070 XT) + 32 GB RAM.
pub fn catalog() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            id: "qwen38-27b-iq4xs",
            name: "Qwen3.8 27B · IQ4_XS",
            repo: "unsloth/Qwen3.8-27B-GGUF",
            file: "Qwen3.8-27B-UD-IQ4_XS.gguf",
            size_gb: 13.27,
            tag: "Best for 16 GB",
            desc_en: "Top quality that fits fully in the 9070 XT's VRAM. Strong reasoning, coding, English & Romanian.",
            desc_ro: "Cea mai bună calitate care încape complet în VRAM-ul 9070 XT. Raționament, cod, engleză și română.",
            recommended: true,
        },
        CatalogEntry {
            id: "qwen38-27b-q3kxl",
            name: "Qwen3.8 27B · Q3_K_XL",
            repo: "unsloth/Qwen3.8-27B-GGUF",
            file: "Qwen3.8-27B-UD-Q3_K_XL.gguf",
            size_gb: 12.24,
            tag: "Long context",
            desc_en: "Slightly smaller quant – leaves more VRAM for a longer context window.",
            desc_ro: "Cuantizare puțin mai mică – lasă mai mult VRAM pentru un context mai lung.",
            recommended: false,
        },
        CatalogEntry {
            id: "qwen38-27b-q4kxl",
            name: "Qwen3.8 27B · Q4_K_XL",
            repo: "unsloth/Qwen3.8-27B-GGUF",
            file: "Qwen3.8-27B-UD-Q4_K_XL.gguf",
            size_gb: 16.35,
            tag: "Max quality",
            desc_en: "Higher quality; a few layers spill into system RAM, so it is noticeably slower.",
            desc_ro: "Calitate mai mare; câteva straturi trec în RAM, deci e vizibil mai lent.",
            recommended: false,
        },
        CatalogEntry {
            id: "qwen36-35b-a3b-iq3s",
            name: "Qwen3.6 35B-A3B MoE · IQ3_S",
            repo: "unsloth/Qwen3.6-35B-A3B-GGUF",
            file: "Qwen3.6-35B-A3B-UD-IQ3_S.gguf",
            size_gb: 12.74,
            tag: "Fastest",
            desc_en: "Mixture-of-experts with only 3B active parameters – very high tokens/s, fully in VRAM.",
            desc_ro: "Mixture-of-experts cu doar 3B parametri activi – foarte rapid, complet în VRAM.",
            recommended: false,
        },
        CatalogEntry {
            id: "qwen36-35b-a3b-q4km",
            name: "Qwen3.6 35B-A3B MoE · Q4_K_M",
            repo: "unsloth/Qwen3.6-35B-A3B-GGUF",
            file: "Qwen3.6-35B-A3B-UD-Q4_K_M.gguf",
            size_gb: 20.61,
            tag: "Fast + RAM offload",
            desc_en: "Better MoE quant; some experts live in system RAM (auto-fitted). Still fast.",
            desc_ro: "Cuantizare MoE mai bună; o parte din experți stau în RAM (ajustat automat). Tot rapid.",
            recommended: false,
        },
        CatalogEntry {
            id: "qwen35-9b-q4km",
            name: "Qwen3.5 9B · Q4_K_M",
            repo: "unsloth/Qwen3.5-9B-GGUF",
            file: "Qwen3.5-9B-Q4_K_M.gguf",
            size_gb: 5.29,
            tag: "Light",
            desc_en: "Small and snappy. Huge context window, leaves room for games/other apps.",
            desc_ro: "Mic și rapid. Context foarte mare, lasă loc pentru alte aplicații.",
            recommended: false,
        },
    ]
}

#[derive(Serialize)]
pub struct LocalModel {
    pub file: String,
    pub size: u64,
    pub modified: i64,
    pub active: bool,
}

/// GGUF files in the models folder (projectors/imatrix files and the embedding subfolder excluded).
pub fn local_models(state: &AppState) -> Vec<LocalModel> {
    let active = state.info.read().model.clone();
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&state.paths.models) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let lower = name.to_lowercase();
            if !p.is_file() || !lower.ends_with(".gguf") || lower.starts_with("mmproj") || lower.contains("imatrix") {
                continue;
            }
            let md = e.metadata().ok();
            v.push(LocalModel {
                active: active.as_deref() == Some(name.as_str()),
                size: md.as_ref().map(|m| m.len()).unwrap_or(0),
                modified: md
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
                file: name,
            });
        }
    }
    v.sort_by_key(|m| std::cmp::Reverse(m.modified));
    v
}

fn safe_file_name(file: &str) -> Result<String> {
    let name = std::path::Path::new(file)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("bad file name"))?;
    if !name.to_lowercase().ends_with(".gguf") {
        return Err(anyhow!("only .gguf files are supported"));
    }
    Ok(name.to_string())
}

pub async fn delete_model(app: &AppHandle, file: &str) -> Result<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let name = safe_file_name(file)?;
    if state.info.read().model.as_deref() == Some(name.as_str()) {
        crate::server::stop_llm(app).await;
    }
    let p = state.paths.models.join(&name);
    tokio::fs::remove_file(&p).await?;
    let cleared = {
        let mut s = state.settings.write();
        let hit = s.last_model.as_deref() == Some(name.as_str());
        if hit {
            s.last_model = None;
        }
        hit
    };
    if cleared {
        state.save_settings();
    }
    Ok(())
}

/// Download `repo/path` (a .gguf inside a Hugging Face repo) into the models folder.
pub async fn download_model(app: &AppHandle, repo: &str, path: &str) -> Result<String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let name = safe_file_name(path)?;
    let mut parts = repo.split('/');
    let valid = |s: Option<&str>| s.is_some_and(|s| !s.is_empty() && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)));
    if !(valid(parts.next()) && valid(parts.next()) && parts.next().is_none()) {
        return Err(anyhow!("`{repo}` is not a Hugging Face repository (owner/name)"));
    }
    let id = format!("model:{name}");
    let cancel = Arc::new(AtomicBool::new(false));
    {
        // two downloads of the same file would write into the same .part
        let mut d = state.downloads.lock();
        if d.contains_key(&id) {
            return Err(anyhow!("{name} is already downloading"));
        }
        d.insert(id.clone(), cancel.clone());
    }
    let url = format!(
        "https://huggingface.co/{repo}/resolve/main/{}?download=true",
        path.split('/').map(|s| urlencoding::encode(s).into_owned()).collect::<Vec<_>>().join("/")
    );
    let dest = state.paths.models.join(&name);
    let r = crate::download::download(app, &state.http, &id, &name, &url, &dest, cancel).await;
    state.downloads.lock().remove(&id);
    if let Err(e) = &r {
        let _ = app.emit("download-progress", json!({"id": id, "label": name, "error": e.to_string()}));
    }
    r.map(|_| name)
}

pub fn cancel_download(state: &AppState, id: &str) {
    if let Some(c) = state.downloads.lock().get(id) {
        c.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Accepts "owner/repo" or any huggingface.co URL and lists the GGUF files in it.
pub async fn list_repo(state: &AppState, input: &str) -> Result<Value> {
    let s = input.trim().trim_end_matches('/');
    let s = s
        .strip_prefix("https://huggingface.co/")
        .or_else(|| s.strip_prefix("http://huggingface.co/"))
        .or_else(|| s.strip_prefix("huggingface.co/"))
        .unwrap_or(s);
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() < 2 {
        return Err(anyhow!("use the form owner/repo"));
    }
    let repo = format!("{}/{}", parts[0], parts[1]);
    let tree: Value = state
        .http
        .get(format!("https://huggingface.co/api/models/{repo}/tree/main?recursive=true"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let files: Vec<Value> = tree
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| {
            let p = f["path"].as_str().unwrap_or("").to_lowercase();
            p.ends_with(".gguf") && !p.contains("mmproj") && !p.contains("imatrix") && !p.contains("-of-0")
        })
        .map(|f| json!({"path": f["path"], "size": f["size"]}))
        .collect();
    Ok(json!({ "repo": repo, "files": files }))
}

/// Search Hugging Face for GGUF repos.
pub async fn search_hf(state: &AppState, query: &str) -> Result<Value> {
    let url = format!(
        "https://huggingface.co/api/models?search={}&filter=gguf&sort=downloads&limit=20",
        urlencoding::encode(query)
    );
    let v: Value = state.http.get(url).send().await?.error_for_status()?.json().await?;
    let out: Vec<Value> = v
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| json!({"id": m["id"], "downloads": m["downloads"], "likes": m["likes"]}))
        .collect();
    Ok(json!(out))
}
