//! Deterministic checks. Items that would only add variance if left to an LLM
//! (character counts, link counts, heading hierarchy, etc.) are all handled here with rule-based logic.
//! (Rationale: evaluation cost hierarchy — assertions/code rules are cheaper and more stable than an LLM judge.)
//!
//! Document format assumption: a `---` front matter block at the top containing `title:` / `meta_description:`,
//! followed by the markdown body (exactly one H1 + H2-H6 hierarchy, images, links).
//!
//! The readability metric (Flesch) was rewritten and ported to Rust from BlogPilot's (MIT license,
//! IamRamgarhia/BlogPilot-Open-Source-AI-SEO-Content-Studio, src/lib/seo/readability.ts) formula/heuristics —
//! not a code copy-paste, only the logic of the standard Flesch-Kincaid formula + syllable-count estimation heuristic was taken. See README for details.

use crate::spec::Spec;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct Metrics {
    pub title_chars: usize,
    pub meta_chars: usize,
    pub h1_count: usize,
    /// Number of points where a heading level was skipped (e.g. H1 followed directly by H3)
    pub heading_skips: usize,
    pub images_total: usize,
    pub images_missing_alt: usize,
    pub internal_links: usize,
    /// Number of external authority links used for citations/sources (E-E-A-T)
    pub citation_links: usize,
    /// Character count of the body (excluding front matter)
    pub chars: usize,
    /// Whether the keyword is present in each of title/H1/first 100 chars of the intro
    pub keyword_in_title: bool,
    pub keyword_in_h1: bool,
    pub keyword_in_intro: bool,
    /// Number of times the target keyword appears across the whole body (whitespace/case normalized).
    /// Used by the multi-axis reward-hacking canary (loop_run.rs, §3.4 arXiv:2605.27996) to monitor for
    /// sudden spikes across iterations.
    pub keyword_occurrences: usize,
    /// Flesch Reading Ease / Flesch-Kincaid Grade. For documents with a low proportion of Latin characters
    /// (e.g. Korean), the formula does not hold, so this is None (see README for limitations).
    pub flesch_reading_ease: Option<f64>,
    pub flesch_kincaid_grade: Option<f64>,
    /// Korean-only readability heuristic (opt-in; None if Latin ratio is 50% or higher — mutually exclusive with Flesch).
    /// This is an **unvalidated heuristic** — the coefficients (1.015/8.0/35.0) cited by the research doc (§3.2)
    /// are, by the original source's own admission, empirically tuned values with no peer-reviewed academic basis.
    /// Do not treat it with the same confidence as Flesch (a standard formula). Always surface it in reports with a warning label.
    pub korean_readability_heuristic: Option<f64>,
}

/// Splits the front matter (`title`/`meta_description` wrapped in `---`) from the body.
///
/// Byte offsets are computed with `split('\n')`, not `.lines()` (which strips the newline char) —
/// with `.lines()`, `line.len()+1` assumes LF only, so on CRLF documents it under-counts by 1 byte
/// per line, and that drift used to cause out-of-char-boundary slicing (panic) or a body with its
/// leading part cut off. `split('\n')` leaves `\r` attached to the preceding line, so
/// `line.len()+1` is always the exact number of bytes consumed.
pub fn parse_front_matter(doc: &str) -> (Option<String>, Option<String>, &str) {
    let trimmed = doc.trim_start_matches('\u{feff}');
    if !trimmed.starts_with("---") {
        return (None, None, doc);
    }
    let after_marker = &trimmed[3..];
    let first_nl = match after_marker.find('\n') {
        Some(i) => i,
        None => return (None, None, doc), // no newline at all after "---" → not front matter
    };
    let rest = &after_marker[first_nl + 1..];

    let mut title = None;
    let mut meta = None;
    let mut offset = 0usize;
    let mut close_end: Option<usize> = None;
    for line in rest.split('\n') {
        let consumed_this_line = line.len() + 1; // with split('\n') the separator is always exactly 1 byte
        let t = line.trim_end_matches('\r').trim();
        if t == "---" {
            close_end = Some((offset + consumed_this_line).min(rest.len()));
            break;
        }
        if let Some(v) = t.strip_prefix("title:") {
            title = Some(unquote(v.trim()));
        } else if let Some(v) = t.strip_prefix("meta_description:") {
            meta = Some(unquote(v.trim()));
        }
        offset += consumed_this_line;
    }
    let Some(close_end) = close_end else {
        return (None, None, doc); // no closing "---" → format_issues warns about this separately
    };
    let body = &rest[close_end..];
    (title, meta, body.trim_start_matches(['\n', '\r']))
}

/// Case where front matter started with `---` but had no closing `---`, so parsing was abandoned entirely.
/// (On parse failure, `parse_front_matter` returns the original `doc` unchanged, so the pointer/length equal doc's.)
fn front_matter_unclosed(doc: &str) -> bool {
    let trimmed = doc.trim_start_matches('\u{feff}');
    if !trimmed.starts_with("---") {
        return false;
    }
    let (title, meta, body) = parse_front_matter(doc);
    title.is_none() && meta.is_none() && body.len() == doc.len()
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// List of (level 1-6, text) headings.
pub fn headings(body: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            let level = t.chars().take_while(|&c| c == '#').count();
            if (1..=6).contains(&level)
                && t.as_bytes()
                    .get(level)
                    .map(|b| *b == b' ')
                    .unwrap_or(level == t.len())
            {
                let text = t.trim_start_matches('#').trim().to_string();
                out.push((level, text));
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct MdLink {
    pub is_image: bool,
    pub label: String,
    pub url: String,
}

/// Manually scans markdown images (`![alt](url)`) / links (`[text](url)`) without regex.
/// Tracks nested `[...]` inside the label (e.g. `[a[b]c](url)`) by depth to find the matching `]`,
/// and does not count escaped `\[`/`\]` as brackets.
pub fn scan_links(text: &str) -> Vec<MdLink> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        if chars[i] == '\\' && i + 1 < n {
            i += 2;
            continue;
        }
        let is_image = chars[i] == '!' && i + 1 < n && chars[i + 1] == '[';
        let bracket_start = if is_image { i + 1 } else { i };
        if chars[i] == '[' || is_image {
            if let Some(close) = find_matching_bracket_close(&chars, bracket_start + 1) {
                if close + 1 < n && chars[close + 1] == '(' {
                    if let Some(paren_close) = find_char(&chars, close + 2, ')') {
                        let label: String = chars[bracket_start + 1..close].iter().collect();
                        let url: String = chars[close + 2..paren_close].iter().collect();
                        out.push(MdLink {
                            is_image,
                            label,
                            url,
                        });
                        i = paren_close + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&j| chars[j] == target)
}

/// Starting right after `[` (at `from`), tracks nested `[...]` by depth to find the matching `]`.
/// Does not count `\]`/`\[` as brackets (escaped).
fn find_matching_bracket_close(chars: &[char], from: usize) -> Option<usize> {
    let mut depth = 1i32;
    let mut j = from;
    while j < chars.len() {
        if chars[j] == '\\' && j + 1 < chars.len() {
            j += 2;
            continue;
        }
        match chars[j] {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// Determines whether an absolute URL's host is exactly `site_domain` or one of its subdomains.
/// (Previously this only checked substring containment with `contains`, so hosts like `notexample.com`
/// or `example.com.evil.com` could be misclassified as an "internal link".)
fn is_internal_url(url: &str, site_domain: &str) -> bool {
    let low = url.to_lowercase();
    let rest = low
        .strip_prefix("https://")
        .or_else(|| low.strip_prefix("http://"));
    match rest {
        Some(rest) => {
            if site_domain.is_empty() {
                return false;
            }
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            let host = host.split(':').next().unwrap_or(host); // strip the port
            let domain = site_domain.to_lowercase();
            host == domain || host.ends_with(&format!(".{domain}"))
        }
        None => true, // relative paths without a scheme are treated as internal links
    }
}

fn norm_kw(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

fn contains_kw(haystack: &str, keyword: &str) -> bool {
    let kw = norm_kw(keyword);
    if kw.is_empty() {
        return false;
    }
    norm_kw(haystack).contains(&kw)
}

/// Number of (non-overlapping) occurrences of keyword within the normalized haystack.
fn count_kw_occurrences(haystack: &str, keyword: &str) -> usize {
    let kw = norm_kw(keyword);
    if kw.is_empty() {
        return 0;
    }
    norm_kw(haystack).matches(&kw).count()
}

/// Concatenates only the plain prose (excluding heading lines) from the body and takes the first `limit` chars (an approximation of the intro).
fn intro_text(body: &str, limit: usize) -> String {
    let mut acc = String::new();
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        acc.push_str(t);
        acc.push(' ');
        if acc.chars().count() >= limit {
            break;
        }
    }
    acc.chars().take(limit).collect()
}

pub fn metrics(doc: &str, spec: &Spec) -> Metrics {
    let (title, meta, body) = parse_front_matter(doc);
    let title = title.unwrap_or_default();
    let meta = meta.unwrap_or_default();
    let heads = headings(body);
    let h1_count = heads.iter().filter(|(l, _)| *l == 1).count();
    let h1_text = heads
        .iter()
        .find(|(l, _)| *l == 1)
        .map(|(_, t)| t.clone())
        .unwrap_or_default();

    let mut heading_skips = 0usize;
    let mut prev = 0usize;
    for (level, _) in &heads {
        if *level > prev + 1 {
            heading_skips += 1;
        }
        prev = *level;
    }

    let links = scan_links(body);
    let images_total = links.iter().filter(|l| l.is_image).count();
    let images_missing_alt = links
        .iter()
        .filter(|l| l.is_image && l.label.trim().is_empty())
        .count();
    let internal_links = links
        .iter()
        .filter(|l| !l.is_image && is_internal_url(&l.url, &spec.site_domain))
        .count();
    let citation_links = links
        .iter()
        .filter(|l| !l.is_image && !is_internal_url(&l.url, &spec.site_domain))
        .count();

    let intro = intro_text(body, 100);

    let lang = readability(body);
    let korean = korean_readability(body);

    Metrics {
        title_chars: title.chars().count(),
        meta_chars: meta.chars().count(),
        h1_count,
        heading_skips,
        images_total,
        images_missing_alt,
        internal_links,
        citation_links,
        chars: body.chars().count(),
        keyword_in_title: contains_kw(&title, &spec.keyword),
        keyword_in_h1: contains_kw(&h1_text, &spec.keyword),
        keyword_in_intro: contains_kw(&intro, &spec.keyword),
        keyword_occurrences: count_kw_occurrences(body, &spec.keyword),
        flesch_reading_ease: lang.map(|r| r.0),
        flesch_kincaid_grade: lang.map(|r| r.1),
        korean_readability_heuristic: korean,
    }
}

/// Required/recommended sections (H2 or higher) that are missing from the body.
pub fn missing_sections(spec: &Spec, doc: &str) -> Vec<String> {
    let (_, _, body) = parse_front_matter(doc);
    let heads = headings(body);
    let norm = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    spec.sections
        .iter()
        .filter(|s| s.required)
        .filter(|s| {
            let want = norm(&s.title);
            !heads.iter().any(|(_, h)| {
                let hn = norm(h);
                !hn.is_empty() && (hn.contains(&want) || want.contains(&hn))
            })
        })
        .map(|s| s.title.clone())
        .collect()
}

/// Deterministic findings related to formatting and on-page SEO.
pub fn format_issues(spec: &Spec, doc: &str) -> Vec<String> {
    let mut issues = Vec::new();

    if front_matter_unclosed(doc) {
        issues.push(
            "Front matter does not end with a closing `---` → title/meta_description could not be \
             recognized and the entire document is treated as body text (this can corrupt readability/heading checks) → check for the closing `---`"
                .to_string(),
        );
    }

    let m = metrics(doc, spec);

    if m.title_chars == 0 {
        issues.push("Missing title (front matter needs `title:`) → add one".to_string());
    } else if m.title_chars < spec.title_min || m.title_chars > spec.title_max {
        issues.push(format!(
            "title length {} chars (recommended {}-{} chars) → adjust length",
            m.title_chars, spec.title_min, spec.title_max
        ));
    }

    if m.meta_chars == 0 {
        issues.push(
            "Missing meta_description (front matter needs `meta_description:`) → add one"
                .to_string(),
        );
    } else if m.meta_chars < spec.meta_min || m.meta_chars > spec.meta_max {
        issues.push(format!(
            "meta_description length {} chars (recommended {}-{} chars) → adjust length",
            m.meta_chars, spec.meta_min, spec.meta_max
        ));
    }

    if m.h1_count == 0 {
        issues.push("No H1 → add one `# Title` heading to the body".to_string());
    } else if m.h1_count > 1 {
        issues.push(format!(
            "{} H1 headings found → keep exactly one and demote the rest to H2 or lower",
            m.h1_count
        ));
    }

    if m.heading_skips > 0 {
        issues.push(format!(
            "{} heading level skip(s) found (e.g. H1 followed directly by H3) → insert headings in hierarchical order",
            m.heading_skips
        ));
    }

    if !spec.keyword.trim().is_empty() {
        if !m.keyword_in_title {
            issues.push(format!(
                "Target keyword '{}' not found in title → place it there",
                spec.keyword
            ));
        }
        if !m.keyword_in_h1 {
            issues.push(format!(
                "Target keyword '{}' not found in H1 → place it there",
                spec.keyword
            ));
        }
        if !m.keyword_in_intro {
            issues.push(format!("Target keyword '{}' not found within the first 100 chars of the intro → place it there", spec.keyword));
        }
    }

    if m.images_total > 0 && m.images_missing_alt > 0 {
        issues.push(format!(
            "{} image(s) missing alt text (out of {} total) → fill in alt text",
            m.images_missing_alt, m.images_total
        ));
    }

    if m.internal_links < spec.internal_links_min || m.internal_links > spec.internal_links_max {
        issues.push(format!(
            "Internal links: {} (recommended {}-{}) → adjust the count — this range varies a lot depending on site structure and article length, so treat it as a guideline",
            m.internal_links, spec.internal_links_min, spec.internal_links_max
        ));
    }

    if m.citation_links < spec.min_citations {
        issues.push(format!(
            "Citation/source links: {} → at least {} required (E-E-A-T; the related scoring criterion is capped at 60 points)",
            m.citation_links, spec.min_citations
        ));
    }

    for m2 in missing_sections(spec, doc) {
        issues.push(format!("Recommended section '{}' is missing → add it", m2));
    }

    let (_, _, body) = parse_front_matter(doc);
    issues.extend(paragraph_length_issues(body));

    issues
}

/// Flags paragraphs that are too long (hurting scan readability).
/// Idea source: CyberCraftBD/power-seo (MIT) `paragraph-length.ts` (based on an English word count of 120-150).
/// Since this project also handles non-English content such as Korean, it was redesigned to use
/// character count instead of word count (the 600-char threshold is a value chosen arbitrarily for this
/// project, not carried over directly from power-seo's original figure).
const PARAGRAPH_CHAR_LIMIT: usize = 600;

fn paragraph_length_issues(body: &str) -> Vec<String> {
    let mut issues = Vec::new();
    let mut para = String::new();
    let flush = |para: &mut String, issues: &mut Vec<String>| {
        let n = para.chars().count();
        if n > PARAGRAPH_CHAR_LIMIT {
            issues.push(format!(
                "Paragraph is {n} chars, too long (recommended ≤{PARAGRAPH_CHAR_LIMIT} chars) → split it for scan readability"
            ));
        }
        para.clear();
    };
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            if !para.trim().is_empty() {
                flush(&mut para, &mut issues);
            } else {
                para.clear();
            }
            continue;
        }
        para.push_str(t);
        para.push(' ');
    }
    if !para.trim().is_empty() {
        flush(&mut para, &mut issues);
    }
    issues
}

// ---- Readability (Flesch) ----------------------------------------------------
// Formula/heuristics rewritten in Rust from BlogPilot (MIT, src/lib/seo/readability.ts).
// Not copy-pasted code, only the algorithm was ported: markdown stripping → sentence/word splitting →
// syllable-count estimation → standard Flesch-Kincaid Grade / Reading Ease formulas.
// Since this formula is based on English syllable rules, it is not computed for documents
// with a low proportion of Latin characters, such as Korean (see README for limitations).

fn strip_markdown_to_prose(body: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let t = t.trim_start_matches('#').trim();
        // Links/images are parsed with scan_links, leaving only the text.
        let links = scan_links(t);
        if links.is_empty() {
            out.push_str(t);
        } else {
            // Images are removed, links keep only their label, concatenated in order (approximate original order is good enough).
            let mut rest = t.to_string();
            for l in &links {
                let full = if l.is_image {
                    format!("![{}]({})", l.label, l.url)
                } else {
                    format!("[{}]({})", l.label, l.url)
                };
                let repl = if l.is_image {
                    String::new()
                } else {
                    l.label.clone()
                };
                rest = rest.replacen(&full, &repl, 1);
            }
            out.push_str(&rest);
        }
        out.push('\n');
    }
    out.chars()
        .filter(|c| !matches!(c, '*' | '_' | '~' | '>' | '`'))
        .collect()
}

fn count_syllables(word: &str) -> usize {
    let w: String = word
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .collect();
    if w.is_empty() {
        return 0;
    }
    if w.len() <= 3 {
        return 1;
    }
    let mut cleaned = w.clone();
    for suf in ["es", "ed", "e"] {
        if cleaned.ends_with(suf) && cleaned.len() > suf.len() + 2 {
            // only strip when it's a consonant+suffix shape (rough approximation)
            let cut = cleaned.len() - suf.len();
            let before = cleaned.as_bytes()[cut - 1] as char;
            if !"aeiouy".contains(before) {
                cleaned.truncate(cut);
                break;
            }
        }
    }
    let vowels = "aeiouy";
    let mut count = 0usize;
    let mut prev_vowel = false;
    for c in cleaned.chars() {
        let is_v = vowels.contains(c);
        if is_v && !prev_vowel {
            count += 1;
        }
        prev_vowel = is_v;
    }
    count.max(1)
}

/// (flesch_reading_ease, flesch_kincaid_grade). None if the Latin character ratio is under half.
fn readability(body: &str) -> Option<(f64, f64)> {
    let prose = strip_markdown_to_prose(body);
    let total_chars = prose.chars().filter(|c| !c.is_whitespace()).count();
    if total_chars == 0 {
        return None;
    }
    let latin = prose.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if (latin as f64 / total_chars as f64) < 0.5 {
        return None; // non-English content such as Korean — the English syllable-based formula does not apply
    }

    let words: Vec<&str> = prose
        .split_whitespace()
        .filter(|w| w.chars().any(|c| c.is_ascii_alphabetic()))
        .collect();
    if words.is_empty() {
        return None;
    }
    let word_count = words.len();
    let syllables: usize = words.iter().map(|w| count_syllables(w)).sum();

    // Sentence splitting: based on whitespace after '.', '!', '?'.
    let mut sentence_count = 0usize;
    let mut chars_iter = prose.chars().peekable();
    let mut prev_boundary = true;
    while let Some(c) = chars_iter.next() {
        if matches!(c, '.' | '!' | '?') {
            if let Some(&next) = chars_iter.peek() {
                if next.is_whitespace() {
                    sentence_count += 1;
                    prev_boundary = true;
                    continue;
                }
            } else {
                sentence_count += 1;
            }
        }
        prev_boundary = prev_boundary && c.is_whitespace();
    }
    let sentence_count = sentence_count.max(1);

    let avg_sentence_len = word_count as f64 / sentence_count as f64;
    let syll_per_word = syllables as f64 / word_count as f64;
    let grade = 0.39 * avg_sentence_len + 11.8 * syll_per_word - 15.59;
    let ease = 206.835 - 1.015 * avg_sentence_len - 84.6 * syll_per_word;
    Some(((ease * 10.0).round() / 10.0, (grade * 10.0).round() / 10.0))
}

// ---- Readability (Korean, opt-in) ----------------------------------------------
// Rewritten in Rust (not a code port) referencing only the formula from
// `naaaayeonn/AI-literacy-care-Agent` (Python, ★3, doc `READABILITY_FORMULA.md`)
// investigated by the research doc §3.2.
// Formula: 100 - (avg words/sentence × 1.015) - (avg syllables/word × 8.0) - (technical term ratio × 35.0)
//
// **Important**: unlike Flesch, this formula's coefficients (1.015/8.0/35.0) are not a standardized,
// peer-reviewed formula — the original source itself admits the limitation that it "doesn't account for
// colloquial speech/neologisms and ignores context"; it's an empirical heuristic.
// It must never be treated with the same confidence as Flesch (English); always expose it as a separate field and
// attach an "unvalidated heuristic" warning in the report (see README/Metrics docs).
//
// Not computed when the Latin character ratio is 50% or higher (mutually exclusive with readability()'s Flesch calculation).

/// Sentence count: counts a sentence boundary wherever `.`/`!`/`?` is followed by whitespace or the end of the document.
/// (Korean sentence endings like `다.`/`요.` already end with `.`, so no special handling is needed.)
fn count_korean_sentences(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut count = 0usize;
    for i in 0..n {
        if matches!(chars[i], '.' | '!' | '?') && (i + 1 >= n || chars[i + 1].is_whitespace()) {
            count += 1;
        }
    }
    count
}

/// Whether a word is a "technical term": either an English word of 3+ letters (entirely ASCII alphabetic), or
/// it ends with a Korean technical-term suffix (화/율/성/도/적/론/법/형/식/계/기/학), in which case it's true.
/// Punctuation (e.g. periods) around the word is trimmed before judging.
fn is_technical_word(word: &str) -> bool {
    const TECH_SUFFIXES: [char; 12] = [
        '화', '율', '성', '도', '적', '론', '법', '형', '식', '계', '기', '학',
    ];
    let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric() && !('가'..='힣').contains(&c));
    if trimmed.is_empty() {
        return false;
    }
    let ascii_letters = trimmed.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if ascii_letters >= 3 && ascii_letters == trimmed.chars().count() {
        return true; // English word of 3+ letters (e.g. LLM, API)
    }
    trimmed
        .chars()
        .last()
        .map(|c| TECH_SUFFIXES.contains(&c))
        .unwrap_or(false)
}

fn korean_readability(body: &str) -> Option<f64> {
    let prose = strip_markdown_to_prose(body);
    let total_chars = prose.chars().filter(|c| !c.is_whitespace()).count();
    if total_chars == 0 {
        return None;
    }
    let latin = prose.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if (latin as f64 / total_chars as f64) >= 0.5 {
        return None; // high Latin character ratio → handled by the Flesch side (mutually exclusive)
    }

    let words: Vec<&str> = prose.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let word_count = words.len();

    let sentence_count = count_korean_sentences(&prose).max(1);
    let avg_words_per_sentence = word_count as f64 / sentence_count as f64;

    let syllables = prose.chars().filter(|c| ('가'..='힣').contains(c)).count();
    let avg_syllables_per_word = syllables as f64 / word_count as f64;

    let tech_count = words.iter().filter(|w| is_technical_word(w)).count();
    let technical_term_ratio = tech_count as f64 / word_count as f64;

    let score = 100.0
        - (avg_words_per_sentence * 1.015)
        - (avg_syllables_per_word * 8.0)
        - (technical_term_ratio * 35.0);
    Some((score * 10.0).round() / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{Criterion, Spec};

    fn test_spec() -> Spec {
        Spec {
            name: "t".into(),
            context: String::new(),
            keyword: "running shoes".into(),
            scoring_source: String::new(),
            site_domain: "example.com".into(),
            title_min: 50,
            title_max: 60,
            meta_min: 120,
            meta_max: 160,
            internal_links_min: 3,
            internal_links_max: 5,
            min_citations: 1,
            angles: vec![],
            bands: vec![],
            sections: vec![],
            criteria: vec![Criterion {
                id: "x".into(),
                name: "x".into(),
                weight: 1.0,
                guide: String::new(),
                citation_required: false,
            }],
        }
    }

    #[test]
    fn front_matter_parses() {
        let doc = "---\ntitle: \"Hello\"\nmeta_description: \"Description\"\n---\n# H1\nBody";
        let (t, m, b) = parse_front_matter(doc);
        assert_eq!(t.as_deref(), Some("Hello"));
        assert_eq!(m.as_deref(), Some("Description"));
        assert!(b.contains("# H1"));
    }

    #[test]
    fn heading_skip_detected() {
        let body = "# H1\n### H3\n";
        let h = headings(body);
        assert_eq!(h, vec![(1, "H1".to_string()), (3, "H3".to_string())]);
    }

    #[test]
    fn link_scan_separates_images_and_links() {
        let links =
            scan_links("body ![alt text](img.png) and [internal](/a) [external](https://ex.com)");
        assert_eq!(links.len(), 3);
        assert!(links[0].is_image);
    }

    #[test]
    fn link_scan_handles_nested_brackets_in_label() {
        let links = scan_links("[a[b]c](https://ex.com)");
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].label, "a[b]c");
        assert_eq!(links[0].url, "https://ex.com");
    }

    #[test]
    fn link_scan_ignores_escaped_brackets() {
        let links = scan_links(r"\[this is not a link\](url) and [real](https://ex.com)");
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].url, "https://ex.com");
    }

    #[test]
    fn keyword_placement_flags_missing() {
        let spec = test_spec();
        let doc = "---\ntitle: \"Spring new sneaker recommendations\"\nmeta_description: \"short\"\n---\n# How to choose sneakers\nToday, let's talk about shoes.";
        let issues = format_issues(&spec, doc);
        assert!(issues.iter().any(|i| i.contains("running shoes")));
    }

    #[test]
    fn front_matter_parses_with_crlf() {
        let doc =
            "---\r\ntitle: \"Hello\"\r\nmeta_description: \"Description\"\r\n---\r\n# H1\r\nBody";
        let (t, m, b) = parse_front_matter(doc);
        assert_eq!(t.as_deref(), Some("Hello"));
        assert_eq!(m.as_deref(), Some("Description"));
        assert!(b.contains("H1"), "{b:?}");
        assert!(
            !b.contains("title:"),
            "front matter lines must not leak into the body: {b:?}"
        );
    }

    #[test]
    fn front_matter_unclosed_falls_back_and_is_flagged() {
        let spec = test_spec();
        let doc = "---\ntitle: X\nthis is body content, but there's no closing ---";
        let (t, m, _) = parse_front_matter(doc);
        assert!(t.is_none() && m.is_none());
        let issues = format_issues(&spec, doc);
        assert!(issues.iter().any(|i| i.contains("closing")), "{issues:?}");
    }

    #[test]
    fn internal_url_spoofing_is_rejected() {
        assert!(!is_internal_url("https://notexample.com/a", "example.com"));
        assert!(!is_internal_url(
            "https://example.com.evil.com/a",
            "example.com"
        ));
        assert!(is_internal_url("https://example.com/a", "example.com"));
        assert!(is_internal_url("https://blog.example.com/a", "example.com"));
        assert!(is_internal_url("/relative/path", "example.com"));
    }

    #[test]
    fn readability_returns_none_for_korean() {
        let body = "# 제목\n\n이것은 한국어로 작성된 본문입니다. 영어 음절 기반 공식이 적용되지 않아야 합니다.";
        assert!(readability(body).is_none());
    }

    #[test]
    fn korean_readability_computes_expected_score() {
        // Verify the coefficients by hand using a document with 2 sentences × 2 words each, 3 syllables per word, 0 technical terms.
        // avg_words_per_sentence=2.0, avg_syllables_per_word=3.0, technical_term_ratio=0.0
        // → 100 - 2.0*1.015 - 3.0*8.0 - 0 = 73.97 → rounds to 74.0
        let body = "가나다 라마바. 사아자 차카타.";
        let score = korean_readability(body).expect("Korean document should yield Some");
        assert!((score - 74.0).abs() < 0.1, "score={score}");
    }

    #[test]
    fn korean_readability_none_for_latin_dominant_text() {
        // If the Latin character ratio is 50% or higher, this must be None since it's mutually exclusive with Flesch.
        let body = "This is a fully English sentence used to test the exclusivity rule.";
        assert!(korean_readability(body).is_none());
    }

    #[test]
    fn korean_readability_field_matches_flesch_exclusivity_in_metrics() {
        let spec = test_spec();
        let doc = "---\ntitle: \"러닝화 고르는 법에 대한 상세 가이드 전체 안내\"\nmeta_description: \"러닝화를 고르는 방법에 대한 아주 상세하고 도움이 되는 설명입니다 충분히 길게\"\n---\n# 러닝화 고르는 법\n러닝화를 고를 때는 발볼과 쿠셔닝을 함께 확인해야 합니다. 전문성 있는 선택이 중요합니다.\n";
        let m = metrics(doc, &spec);
        assert!(m.flesch_reading_ease.is_none());
        assert!(m.korean_readability_heuristic.is_some());
    }

    #[test]
    fn keyword_occurrence_count() {
        let body = "Running shoes story. Running shoes are good. RunningShoe and running shoes.";
        assert_eq!(count_kw_occurrences(body, "running shoes"), 3);
    }

    #[test]
    fn unquote_handles_bare_value() {
        let doc =
            "---\ntitle: Title without quotes\nmeta_description: \"Description\"\n---\n# H1\nBody";
        let (t, _, _) = parse_front_matter(doc);
        assert_eq!(t.as_deref(), Some("Title without quotes"));
    }

    #[test]
    fn whitespace_only_alt_counts_as_missing() {
        let links = scan_links("![ ](img.png)");
        assert_eq!(links.len(), 1);
        assert!(links[0].label.trim().is_empty());
    }

    #[test]
    fn long_paragraph_is_flagged() {
        let long = "a".repeat(601);
        let body = format!("# T\n\n{long}\n");
        let issues = paragraph_length_issues(&body);
        assert!(issues.iter().any(|i| i.contains("too long")), "{issues:?}");
    }

    #[test]
    fn short_paragraph_is_not_flagged() {
        let body = "# T\n\nThis is a short paragraph.\n";
        assert!(paragraph_length_issues(body).is_empty());
    }

    #[test]
    fn internal_and_citation_link_counts() {
        let spec = test_spec();
        let doc = "---\ntitle: \"A complete, detailed guide to choosing running shoes\"\nmeta_description: \"A very detailed and helpful explanation of how to choose running shoes, long enough\"\n---\n# How to choose running shoes\nRunning shoes story. [internal1](https://example.com/a) [internal2](/b) [external](https://other.com/c)\n";
        let m = metrics(doc, &spec);
        assert_eq!(m.internal_links, 2);
        assert_eq!(m.citation_links, 1);
    }
}
