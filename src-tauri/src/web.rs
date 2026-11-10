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
        "wikipedia" => wikipedia(state, query, n).await,
        // default, and the keyless fallback when a provider is missing its key / URL
        _ => duckduckgo(state, query, n).await,
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

/// DuckDuckGo's official keyless Instant Answer API. It returns topic abstracts (mostly Wikipedia) and
/// official sites, not general web results, so the remaining slots are filled from Wikipedia search.
/// (DuckDuckGo has no keyless full-web API; scraping its result pages is not allowed here.)
async fn duckduckgo(state: &AppState, query: &str, n: usize) -> Result<Vec<SearchHit>> {
    let v: Result<Value> = async {
        Ok(state
            .http
            .get("https://api.duckduckgo.com/")
            .header("User-Agent", LUMEN_UA)
            .query(&[("q", query), ("format", "json"), ("no_html", "1"), ("skip_disambig", "1"), ("t", "lumen")])
            .timeout(Duration::from_secs(15))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
    .await;
    let mut out = v.as_ref().map(instant_answer_hits).unwrap_or_default();
    out.truncate(n);
    if out.len() < n {
        match wikipedia(state, query, n).await {
            Ok(wiki) => {
                for h in wiki {
                    if out.len() >= n {
                        break;
                    }
                    if !out.iter().any(|o| o.url == h.url) {
                        out.push(h);
                    }
                }
            }
            // keep what DuckDuckGo found; report the error only when there is nothing at all
            Err(e) if out.is_empty() => return Err(v.err().unwrap_or(e)),
            Err(_) => {}
        }
    }
    Ok(out)
}

/// Abstract (with its source page) and official-site results from an Instant Answer response.
fn instant_answer_hits(v: &Value) -> Vec<SearchHit> {
    let mut out = Vec::new();
    let heading = v["Heading"].as_str().unwrap_or_default();
    if let Some(url) = v["AbstractURL"].as_str().filter(|u| u.starts_with("http")) {
        let source = v["AbstractSource"].as_str().unwrap_or_default();
        out.push(SearchHit {
            title: if source.is_empty() { heading.to_string() } else { format!("{heading} – {source}") },
            url: url.to_string(),
            snippet: v["AbstractText"].as_str().unwrap_or_default().to_string(),
        });
    }
    for r in v["Results"].as_array().into_iter().flatten() {
        let Some(url) = r["FirstURL"].as_str().filter(|u| u.starts_with("http")) else { continue };
        if out.iter().any(|o| o.url == url) {
            continue;
        }
        let text = strip_tags(r["Text"].as_str().unwrap_or(url));
        out.push(SearchHit { title: if heading.is_empty() { text.clone() } else { format!("{heading} – {text}") }, url: url.to_string(), snippet: text });
    }
    out
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
    let url = url.trim();
    if !reqwest::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some()) {
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
        let code = resp.status().as_u16();
        // (GitHub answers 404 for a private repository too: say so, so the model can tell the user)
        let hint = if matches!(code, 401 | 403 | 404) { " - the page doesn't exist or needs a login (a private repository, for example); Lumen can only read public pages" } else { "" };
        return Err(anyhow!("HTTP {}{hint}", resp.status()));
    }
    let final_url = resp.url().to_string();
    let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    // read at most MAX_PAGE bytes: a huge (or endless) response must not fill the memory first
    const MAX_PAGE: usize = 25 * 1024 * 1024;
    if resp.content_length().is_some_and(|l| l > MAX_PAGE as u64) {
        return Err(anyhow!("page too large"));
    }
    let mut resp = resp;
    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if bytes.len() + chunk.len() > MAX_PAGE {
            return Err(anyhow!("page too large"));
        }
        bytes.extend_from_slice(&chunk);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instant_answer_abstract_and_official_site() {
        // trimmed real response of api.duckduckgo.com for "rust programming language"
        let v = json!({
            "Heading": "Rust (programming language)",
            "AbstractURL": "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            "AbstractSource": "Wikipedia",
            "AbstractText": "Rust is a general-purpose programming language.",
            "Results": [
                {"FirstURL": "https://www.rust-lang.org/", "Text": "<b>Official site</b>"},
                {"FirstURL": "https://en.wikipedia.org/wiki/Rust_(programming_language)", "Text": "dup"}
            ],
            "RelatedTopics": [{"FirstURL": "https://duckduckgo.com/c/Rust", "Text": "topic"}]
        });
        let hits = instant_answer_hits(&v);
        assert_eq!(hits.len(), 2, "abstract + official site, duplicate dropped, DDG topic links ignored");
        assert_eq!(hits[0].url, "https://en.wikipedia.org/wiki/Rust_(programming_language)");
        assert_eq!(hits[0].title, "Rust (programming language) – Wikipedia");
        assert_eq!(hits[1].url, "https://www.rust-lang.org/");
        assert_eq!(hits[1].snippet, "Official site");
    }

    #[test]
    fn instant_answer_empty_for_open_questions() {
        let v = json!({"Heading": "", "AbstractURL": "", "Results": [], "RelatedTopics": []});
        assert!(instant_answer_hits(&v).is_empty());
    }
}
