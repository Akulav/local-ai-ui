//! Text extraction for project files (code, text, PDF, DOCX).

use anyhow::{anyhow, Result};
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;

pub const TEXT_EXT: &[&str] = &[
    "txt", "md", "markdown", "rst", "org", "tex", "csv", "tsv", "json", "jsonl", "yaml", "yml", "toml", "ini", "cfg",
    "conf", "xml", "html", "htm", "css", "scss", "sass", "less", "js", "mjs", "cjs", "jsx", "ts", "tsx", "vue",
    "svelte", "py", "pyi", "ipynb", "rs", "go", "java", "kt", "kts", "scala", "c", "h", "cc", "cpp", "cxx", "hpp",
    "hh", "cs", "fs", "vb", "swift", "m", "mm", "rb", "php", "pl", "lua", "r", "jl", "dart", "sh", "bash", "zsh",
    "ps1", "psm1", "bat", "cmd", "sql", "graphql", "proto", "gradle", "cmake", "make", "mk", "dockerfile", "tf",
    "hcl", "nix", "zig", "nim", "ex", "exs", "erl", "hs", "ml", "clj", "elm", "sol", "asm", "s", "glsl", "hlsl",
    "wgsl", "env", "gitignore", "editorconfig", "log", "srt", "vtt", "bib", "properties", "lock",
];

pub fn is_supported(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
    if matches!(name.as_str(), "dockerfile" | "makefile" | "license" | "readme" | "cmakelists.txt" | "procfile") {
        return true;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    ext == "pdf" || ext == "docx" || TEXT_EXT.contains(&ext.as_str())
}

pub fn is_code(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    !matches!(ext.as_str(), "pdf" | "docx" | "txt" | "md" | "markdown" | "rst" | "org" | "tex" | "html" | "htm" | "csv" | "tsv" | "srt" | "vtt" | "")
}

pub fn extract(path: &Path) -> Result<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "pdf" => extract_pdf(path),
        "docx" => extract_docx(path),
        _ => {
            let bytes = std::fs::read(path)?;
            if bytes.iter().take(4096).any(|&b| b == 0) {
                return Err(anyhow!("binary file"));
            }
            Ok(String::from_utf8_lossy(&bytes).replace("\r\n", "\n"))
        }
    }
}

fn extract_pdf(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    // pdf-extract can panic on malformed files; contain it.
    let r = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(&bytes));
    match r {
        Ok(Ok(t)) => {
            let cleaned = t.replace('\u{c}', "\n\n").replace("\r\n", "\n");
            if cleaned.trim().is_empty() {
                Err(anyhow!("PDF has no extractable text (scanned?)"))
            } else {
                Ok(cleaned)
            }
        }
        Ok(Err(e)) => Err(anyhow!("pdf: {e}")),
        Err(_) => Err(anyhow!("pdf parser crashed on this file")),
    }
}

fn extract_docx(path: &Path) -> Result<String> {
    let f = std::fs::File::open(path)?;
    let mut z = zip::ZipArchive::new(f)?;
    let mut xml = String::new();
    z.by_name("word/document.xml")?.read_to_string(&mut xml)?;
    let xml = xml.replace("</w:p>", "\n").replace("<w:tab/>", "\t").replace("<w:br/>", "\n");
    static TAG_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"<[^>]+>").unwrap());
    let text = TAG_RE.replace_all(&xml, "");
    Ok(text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'"))
}
