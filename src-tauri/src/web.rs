//! Web search through official APIs (Brave, Tavily, SearXNG; Wikipedia as the keyless fallback)
//! and readable page extraction.

use anyhow::{anyhow, Result};
use scraper::{ElementRef, Html, Node, Selector};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::LazyLock;
use std::time::Duration;

use crate::state::{AppState, LUMEN_UA};

static TAG_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"<[^>]+>").unwrap());

#[derive(Clone, Debug, Serialize)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub async fn search(state: &AppState, query: &str, n: usize) -> Result<Vec<SearchHit>> {
    let (provider, key, searx) = {
        let s = state.settings.read();
        (s.search_provider.clone(), s.search_key.trim().to_string(), s.searxng_url.trim().trim_end_matches('/').to_string())
    };
    let r = match provider.as_str() {
        "brave" if !key.is_empty() => brave(state, &key, query, n).await,
        "tavily" if !key.is_empty() => tavily(state, &key, query, n).await,
        "searxng" if !searx.is_empty() => searxng(state, &searx, query, n).await,
        _ => wikipedia(state, query, n).await,
    };
    match r {
        Ok(v) if !v.is_empty() => Ok(v),
        Ok(_) => Err(anyhow!("web search returned no results")),
        Err(e) => Err(e),
    }
}

fn hits(arr: Option<&Vec<Value>>, title: &str, url: &str, snippet: &str, n: usize) -> Vec<SearchHit> {
    arr.into_iter()
        .flatten()
        .filter_map(|r| {
            let u = r[url].as_str()?.to_string();
            Some(SearchHit {
                title: strip_tags(r[title].as_str().unwrap_or(&u)),
                snippet: strip_tags(r[snippet].as_str().unwrap_or("")),
                url: u,
            })
        })
        .take(n)
        .collect()
}

fn strip_tags(s: &str) -> String {
    TAG_RE.replace_all(s, "").replace("&quot;", "\"").replace("&amp;", "&").replace("&#39;", "'")
}

async fn brave(state: &AppState, key: &str, query: &str, n: usize) -> Result<Vec<SearchHit>> {
    let v: Value = state
        .http
        .get("https://api.search.brave.com/res/v1/web/search")
        .query(&[("q", query), ("count", &n.min(20).to_string())])
        .header("X-Subscription-Token", key)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(hits(v["web"]["results"].as_array(), "title", "url", "description", n))
}

async fn tavily(state: &AppState, key: &str, query: &str, n: usize) -> Result<Vec<SearchHit>> {
    let v: Value = state
        .http
        .post("https://api.tavily.com/search")
        .json(&json!({"api_key": key, "query": query, "max_results": n.min(20)}))
        .timeout(Duration::from_secs(20))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(hits(v["results"].as_array(), "title", "url", "content", n))
}

async fn searxng(state: &AppState, base: &str, query: &str, n: usize) -> Result<Vec<SearchHit>> {
    let v: Value = state
        .http
        .get(format!("{base}/search"))
        .query(&[("q", query), ("format", "json")])
        .timeout(Duration::from_secs(20))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(hits(v["results"].as_array(), "title", "url", "content", n))
}

async fn wikipedia(state: &AppState, query: &str, n: usize) -> Result<Vec<SearchHit>> {
    // Romanian-looking queries also search ro.wikipedia.org
    let ro = query.chars().any(|c| "ăâîșțşţĂÂÎȘȚ".contains(c));
    let langs: &[&str] = if ro { &["ro", "en"] } else { &["en"] };
    let mut out = Vec::new();
    for lang in langs {
        let mut attempt = 0;
        let v: Value = loop {
            let resp = state
                .http
                .get(format!("https://{lang}.wikipedia.org/w/api.php"))
                // Wikimedia API etiquette: identify the client honestly
                .header("User-Agent", LUMEN_UA)
                .query(&[("action", "query"), ("list", "search"), ("srsearch", query), ("format", "json"), ("srlimit", &n.to_string())])
                .timeout(Duration::from_secs(15))
                .send()
                .await?;
            // back off politely when rate limited
            if resp.status().as_u16() == 429 && attempt < 3 {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(1500 * attempt)).await;
                continue;
            }
            break resp.error_for_status()?.json().await?;
        };
        for r in v["query"]["search"].as_array().into_iter().flatten() {
            let title = r["title"].as_str().unwrap_or_default();
            out.push(SearchHit {
                title: title.to_string(),
                url: format!("https://{lang}.wikipedia.org/wiki/{}", urlencoding::encode(&title.replace(' ', "_"))),
                snippet: strip_tags(r["snippet"].as_str().unwrap_or("")),
            });
        }
    }
    out.truncate(n);
    Ok(out)
}

fn text_of(e: ElementRef) -> String {
    e.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub url: String,
    pub title: String,
    pub text: String,
}

pub async fn fetch(state: &AppState, url: &str) -> Result<Page> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(anyhow!("only http(s) URLs are supported"));
    }
    let mut req = state.http.get(url);
    if url.contains(".wikipedia.org/") {
        req = req.header("User-Agent", LUMEN_UA);
    }
    let resp = req
        .header("Accept", "text/html,application/xhtml+xml,application/pdf;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9,ro;q=0.8")
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow!("HTTP {}", resp.status()));
    }
    let final_url = resp.url().to_string();
    let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    let bytes = resp.bytes().await?;
    if bytes.len() > 25 * 1024 * 1024 {
        return Err(anyhow!("page too large"));
    }
    if ctype.contains("pdf") || final_url.to_lowercase().ends_with(".pdf") {
        let b = bytes.to_vec();
        let text = tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(&b)).ok().and_then(|r| r.ok()).unwrap_or_default()
        })
        .await?;
        return Ok(Page { url: final_url.clone(), title: final_url, text: squeeze(&text) });
    }
    let html = String::from_utf8_lossy(&bytes).to_string();
    if !ctype.contains("html") && !html.trim_start().starts_with('<') {
        return Ok(Page { url: final_url.clone(), title: final_url, text: squeeze(&html) });
    }
    let (title, text) = readable(&html);
    Ok(Page { url: final_url, title, text })
}

const SKIP: &[&str] = &["script", "style", "noscript", "nav", "footer", "header", "aside", "form", "svg", "iframe", "button", "select", "template", "canvas"];
const BLOCK: &[&str] = &["p", "div", "section", "article", "main", "br", "li", "tr", "h1", "h2", "h3", "h4", "h5", "h6", "pre", "blockquote", "table", "ul", "ol", "dd", "dt", "figcaption"];

/// Minimal readability: prefer <article>/<main>, drop chrome, keep block structure and headings.
pub fn readable(html: &str) -> (String, String) {
    let doc = Html::parse_document(html);
    let title = Selector::parse("title")
        .ok()
        .and_then(|s| doc.select(&s).next().map(text_of))
        .unwrap_or_default();
    let root = ["article", "main", "[role=main]", "body"]
        .iter()
        .filter_map(|s| Selector::parse(s).ok())
        .find_map(|s| doc.select(&s).max_by_key(|e| e.text().map(|t| t.len()).sum::<usize>()))
        .filter(|e| e.text().map(|t| t.len()).sum::<usize>() > 400)
        .or_else(|| Selector::parse("body").ok().and_then(|s| doc.select(&s).next()));
    let mut out = String::new();
    if let Some(r) = root {
        walk(r, &mut out);
    }
    (title, squeeze(&out))
}

fn walk(e: ElementRef, out: &mut String) {
    for child in e.children() {
        match child.value() {
            Node::Text(t) => out.push_str(t),
            Node::Element(el) => {
                let name = el.name();
                if SKIP.contains(&name) {
                    continue;
                }
                let cls = el.attr("class").unwrap_or("").to_lowercase();
                if cls.contains("cookie") || cls.contains("advert") || cls.contains("newsletter") || cls.contains("sidebar") {
                    continue;
                }
                let block = BLOCK.contains(&name);
                if block {
                    out.push('\n');
                }
                if name.len() == 2 && name.starts_with('h') {
                    out.push_str("## ");
                }
                if name == "li" {
                    out.push_str("- ");
                }
                if let Some(ce) = ElementRef::wrap(child) {
                    walk(ce, out);
                }
                if block {
                    out.push('\n');
                } else if name == "td" || name == "th" {
                    out.push_str(" | ");
                }
            }
            _ => {}
        }
    }
}

fn squeeze(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.lines() {
        let l = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if l.is_empty() {
            blank += 1;
            if blank == 1 {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(&l);
            out.push('\n');
        }
    }
    out.trim().to_string()
}
