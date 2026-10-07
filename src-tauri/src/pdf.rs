//! PDF access through pdfium: metadata, per-page text, outline → chapters.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pdfium_render::prelude::*;
use serde::Serialize;

use crate::error::{AppError, AppResult};

static PDFIUM_DIR: OnceLock<PathBuf> = OnceLock::new();
static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

/// Called once at startup with the bundled resources directory.
pub fn set_library_dir(dir: PathBuf) {
    let _ = PDFIUM_DIR.set(dir);
}

fn candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = PDFIUM_DIR.get() {
        dirs.push(d.clone());
    }
    if let Ok(d) = std::env::var("PDFIUM_DYNAMIC_LIB_PATH") {
        dirs.push(PathBuf::from(d));
    }
    // `cargo test` / `tauri dev`: the copy fetched by scripts/fetch-pdfium.sh
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pdfium"));
    dirs
}

pub fn pdfium() -> AppResult<&'static Pdfium> {
    PDFIUM
        .get_or_init(|| {
            let mut last_err = String::from("pdfium library not found");
            for dir in candidates() {
                match Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir)) {
                    Ok(b) => return Ok(Pdfium::new(b)),
                    Err(e) => last_err = format!("{}: {e}", dir.display()),
                }
            }
            match Pdfium::bind_to_system_library() {
                Ok(b) => Ok(Pdfium::new(b)),
                Err(_) => Err(last_err),
            }
        })
        .as_ref()
        .map_err(|e| AppError::Pdf(format!("Could not load pdfium ({e}). Run scripts/fetch-pdfium.sh.")))
}

pub fn is_available() -> bool {
    pdfium().is_ok()
}

fn open<'a>(pdfium: &'a Pdfium, path: &Path) -> AppResult<PdfDocument<'a>> {
    pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| AppError::Pdf(format!("{}: {e}", path.display())))
}

#[derive(Debug, Clone, Default)]
pub struct PdfInfo {
    pub title: Option<String>,
    pub author: Option<String>,
    pub page_count: u32,
}

/// Cheap metadata read (no text extraction).
pub fn read_info(path: &Path) -> AppResult<PdfInfo> {
    let pdfium = pdfium()?;
    let doc = open(pdfium, path)?;
    let meta = doc.metadata();
    let clean = |t: Option<PdfDocumentMetadataTag>| {
        t.map(|v| v.value().trim().to_string()).filter(|s| !s.is_empty())
    };
    Ok(PdfInfo {
        title: clean(meta.get(PdfDocumentMetadataTagType::Title)),
        author: clean(meta.get(PdfDocumentMetadataTagType::Author)),
        page_count: doc.pages().len() as u32,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct OutlineItem {
    pub title: String,
    pub depth: u32,
    /// 1-based page number
    pub page: u32,
}

pub struct Extracted {
    /// Text of each page; index 0 = page 1.
    pub pages: Vec<String>,
    pub outline: Vec<OutlineItem>,
}

pub fn extract(path: &Path, mut on_page: impl FnMut(u32, u32)) -> AppResult<Extracted> {
    let pdfium = pdfium()?;
    let doc = open(pdfium, path)?;
    let total = doc.pages().len() as u32;

    let mut pages = Vec::with_capacity(total as usize);
    for (i, page) in doc.pages().iter().enumerate() {
        let text = page.text().map(|t| t.all()).unwrap_or_default();
        pages.push(normalize_page_text(&text));
        if i % 10 == 0 || i as u32 + 1 == total {
            on_page(i as u32 + 1, total);
        }
    }

    let mut outline = Vec::new();
    if let Some(root) = doc.bookmarks().root() {
        walk_outline(Some(root), 0, &mut outline);
    }
    Ok(Extracted { pages, outline })
}

fn walk_outline(mut node: Option<PdfBookmark<'_>>, depth: u32, out: &mut Vec<OutlineItem>) {
    while let Some(b) = node {
        if out.len() > 10_000 || depth > 8 {
            return;
        }
        let page = b
            .destination()
            .and_then(|d| d.page_index().ok())
            .or_else(|| {
                b.action().and_then(|a| {
                    a.as_local_destination_action()
                        .and_then(|l| l.destination().ok())
                        .and_then(|d| d.page_index().ok())
                })
            });
        if let (Some(title), Some(page)) = (b.title(), page) {
            let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
            if !title.is_empty() {
                out.push(OutlineItem { title, depth, page: page as u32 + 1 });
            }
        }
        walk_outline(b.first_child(), depth + 1, out);
        node = b.next_sibling();
    }
}

fn normalize_page_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    pub idx: u32,
    /// Stable across re-indexing and machines: derived from the start page.
    pub id: String,
    pub title: String,
    pub depth: u32,
    pub start_page: u32,
    pub end_page: u32,
}

const FALLBACK_SECTION_PAGES: u32 = 20;

/// Picks the outline level that best represents chapters and turns it into page ranges.
pub fn chapters_from_outline(outline: &[OutlineItem], page_count: u32) -> Vec<Chapter> {
    let level = |depth: u32| -> Vec<&OutlineItem> {
        let mut items: Vec<&OutlineItem> = outline
            .iter()
            .filter(|o| o.depth == depth && o.page >= 1 && o.page <= page_count)
            .collect();
        items.sort_by_key(|o| o.page);
        items.dedup_by_key(|o| o.page);
        items
    };

    let mut chosen: Option<Vec<&OutlineItem>> = None;
    for depth in 0..=3 {
        let items = level(depth);
        if items.len() < 4 {
            continue;
        }
        // Top level is often "Parts" with real chapters one level down.
        let avg = page_count as f32 / items.len() as f32;
        if avg > 60.0 {
            let deeper = level(depth + 1);
            if deeper.len() as f32 >= items.len() as f32 * 1.5 && deeper.len() <= 120 {
                chosen = Some(deeper);
                break;
            }
        }
        chosen = Some(items);
        break;
    }

    let Some(items) = chosen else {
        return fixed_sections(page_count);
    };

    let mut chapters = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let end = items
            .get(i + 1)
            .map(|n| n.page.saturating_sub(1).max(item.page))
            .unwrap_or(page_count);
        chapters.push(Chapter {
            idx: i as u32,
            id: format!("p{}", item.page),
            title: item.title.clone(),
            depth: item.depth,
            start_page: item.page,
            end_page: end,
        });
    }
    // Front matter before the first outline entry.
    if let Some(first) = chapters.first() {
        if first.start_page > 1 {
            let end = first.start_page - 1;
            chapters.insert(
                0,
                Chapter { idx: 0, id: "p1".into(), title: "Front matter".into(), depth: 0, start_page: 1, end_page: end },
            );
            for (i, c) in chapters.iter_mut().enumerate() {
                c.idx = i as u32;
            }
        }
    }
    chapters
}

/// Outline first; for books without bookmarks, look for running "Chapter N" / "N.M" headers;
/// fixed-size sections as a last resort.
pub fn detect_chapters(outline: &[OutlineItem], pages: &[String]) -> Vec<Chapter> {
    let page_count = pages.len() as u32;
    let from_outline = chapters_from_outline(outline, page_count);
    if !is_fixed(&from_outline) {
        return from_outline;
    }
    let headings = chapters_from_headings(pages);
    if headings.len() >= 4 {
        return chapters_from_outline(&headings, page_count);
    }
    from_outline
}

fn is_fixed(chapters: &[Chapter]) -> bool {
    chapters.iter().all(|c| c.title.starts_with("Pages "))
}

fn tidy_heading(t: &str) -> String {
    let t = t.trim().trim_matches(|c: char| c == '.' || c == ':' || c.is_whitespace());
    if t.chars().any(|c| c.is_lowercase()) {
        return t.to_string();
    }
    // ALL CAPS → Title Case
    t.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c.flat_map(|x| x.to_lowercase())).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// Finds chapter starts from page headers, requiring chapter numbers to appear in order 1, 2, 3…
pub fn chapters_from_headings(pages: &[String]) -> Vec<OutlineItem> {
    let chapter_re = regex::Regex::new(r"(?i)^chapter\s+(\d{1,2})\b[\s.:\-–—]*(.*)$").expect("regex");
    let section_re = regex::Regex::new(r"(?i)^(?:section\s+)?(\d{1,2})\.\d{1,2}\.?(?:\s|$)").expect("regex");
    for (re, use_title) in [(&chapter_re, true), (&section_re, false)] {
        let mut found = Vec::new();
        let mut expected = 1u32;
        for (i, page) in pages.iter().enumerate() {
            let lines: Vec<&str> = page.lines().take(4).collect();
            for (li, line) in lines.iter().enumerate().take(3) {
                let Some(c) = re.captures(line.trim()) else { continue };
                let Ok(n) = c[1].parse::<u32>() else { continue };
                if n != expected {
                    continue;
                }
                let mut title = if use_title { c.get(2).map(|m| tidy_heading(m.as_str())).unwrap_or_default() } else { String::new() };
                // "Chapter 3" on one line, the title on the next
                if use_title && title.is_empty() {
                    if let Some(next) = lines.get(li + 1) {
                        let next = next.trim();
                        let words = next.split_whitespace().count();
                        if (1..=8).contains(&words) && !next.ends_with('.') && !next.starts_with(|c: char| c.is_ascii_digit()) {
                            title = tidy_heading(next);
                        }
                    }
                }
                let title = if title.is_empty() || title.len() > 90 { format!("Chapter {n}") } else { format!("{n}. {title}") };
                found.push(OutlineItem { title, depth: 0, page: i as u32 + 1 });
                expected += 1;
                break;
            }
        }
        if found.len() >= 4 {
            return found;
        }
    }
    vec![]
}

fn fixed_sections(page_count: u32) -> Vec<Chapter> {
    let mut out = Vec::new();
    let mut start = 1;
    while start <= page_count.max(1) {
        let end = (start + FALLBACK_SECTION_PAGES - 1).min(page_count.max(1));
        out.push(Chapter {
            idx: out.len() as u32,
            id: format!("p{start}"),
            title: format!("Pages {start}–{end}"),
            depth: 0,
            start_page: start,
            end_page: end,
        });
        start = end + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, depth: u32, page: u32) -> OutlineItem {
        OutlineItem { title: title.into(), depth, page }
    }

    #[test]
    fn uses_top_level_chapters() {
        let o: Vec<_> = (0..10).map(|i| item(&format!("Ch {i}"), 0, 5 + i * 30)).collect();
        let ch = chapters_from_outline(&o, 320);
        assert_eq!(ch[0].title, "Front matter");
        assert_eq!(ch[1].start_page, 5);
        assert_eq!(ch[1].end_page, 34);
        assert_eq!(ch.last().unwrap().end_page, 320);
    }

    #[test]
    fn descends_from_parts_to_chapters() {
        let mut o = vec![];
        for p in 0..4 {
            o.push(item(&format!("Part {p}"), 0, 1 + p * 200));
            for c in 0..5 {
                o.push(item(&format!("Ch {p}.{c}"), 1, 1 + p * 200 + c * 40));
            }
        }
        let ch = chapters_from_outline(&o, 800);
        assert_eq!(ch.len(), 20);
        assert!(ch[0].title.starts_with("Ch"));
    }

    #[test]
    fn detects_running_chapter_headers() {
        let mut pages: Vec<String> = (0..40).map(|_| "body text".to_string()).collect();
        pages[2] = "Contents\nChapter 1 Intro".into(); // TOC mention
        for (n, p) in [(1, 5), (2, 12), (3, 20), (4, 30)] {
            pages[p] = format!("CHAPTER {n}. LINEAR ALGEBRA\ntext");
            pages[p + 1] = format!("CHAPTER {n}. LINEAR ALGEBRA\nmore");
        }
        let h = chapters_from_headings(&pages);
        assert_eq!(h.iter().map(|o| o.page).collect::<Vec<_>>(), vec![3, 13, 21, 31]);
        assert_eq!(h[1].title, "2. Linear Algebra");
        let ch = detect_chapters(&[], &pages);
        assert_eq!(ch.len(), 5); // front matter + 4
    }

    #[test]
    fn falls_back_to_fixed_sections() {
        let ch = chapters_from_outline(&[], 45);
        assert_eq!(ch.len(), 3);
        assert_eq!(ch[2].title, "Pages 41–45");
    }
}
