//! 결정론적 검사. LLM에 맡기면 분산만 커지는 항목(글자수, 링크 개수, 헤딩 계층 등)은
//! 전부 여기서 룰 기반으로 처리한다.
//! (근거: 평가 비용 위계 — assertion/코드 규칙이 LLM judge보다 싸고 안정적)
//!
//! 문서 포맷 가정: 맨 위 `---` 프론트매터 블록에 `title:` / `meta_description:`,
//! 그 아래 마크다운 본문(H1 정확히 1개 + H2~H6 계층, 이미지, 링크).
//!
//! 가독성 지표(Flesch)는 BlogPilot(MIT license, IamRamgarhia/BlogPilot-Open-Source-AI-SEO-Content-Studio,
//! src/lib/seo/readability.ts)의 공식·휴리스틱을 Rust로 재작성해 포팅했다 — 코드 복붙이 아니라
//! Flesch-Kincaid 표준 공식 + 음절수 추정 휴리스틱의 로직만 가져왔다. 자세한 내용은 README 참고.

use crate::spec::Spec;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct Metrics {
    pub title_chars: usize,
    pub meta_chars: usize,
    pub h1_count: usize,
    /// 헤딩 레벨을 건너뛴 지점 개수 (예: H1 다음 바로 H3)
    pub heading_skips: usize,
    pub images_total: usize,
    pub images_missing_alt: usize,
    pub internal_links: usize,
    /// 출처/인용용 외부 권위 링크 개수 (E-E-A-T)
    pub citation_links: usize,
    /// 본문(프론트매터 제외) 글자 수
    pub chars: usize,
    /// title/H1/도입부 100자 각각에서 키워드 존재 여부
    pub keyword_in_title: bool,
    pub keyword_in_h1: bool,
    pub keyword_in_intro: bool,
    /// Flesch Reading Ease / Flesch-Kincaid Grade. 라틴 문자 비중이 낮은(한국어 등)
    /// 문서는 공식이 성립하지 않으므로 None (README 한계 참고).
    pub flesch_reading_ease: Option<f64>,
    pub flesch_kincaid_grade: Option<f64>,
}

/// 프론트매터(`---`로 감싼 title/meta_description)와 본문을 분리한다.
pub fn parse_front_matter(doc: &str) -> (Option<String>, Option<String>, &str) {
    let trimmed = doc.trim_start_matches('\u{feff}');
    if !trimmed.starts_with("---") {
        return (None, None, doc);
    }
    let mut lines = trimmed.lines();
    lines.next(); // 첫 "---"
    let mut title = None;
    let mut meta = None;
    let mut consumed = "---\n".len();
    let mut closed = false;
    for line in lines {
        consumed += line.len() + 1;
        let t = line.trim();
        if t == "---" {
            closed = true;
            break;
        }
        if let Some(v) = t.strip_prefix("title:") {
            title = Some(unquote(v.trim()));
        } else if let Some(v) = t.strip_prefix("meta_description:") {
            meta = Some(unquote(v.trim()));
        }
    }
    if !closed {
        return (None, None, doc);
    }
    let body = &trimmed[consumed.min(trimmed.len())..];
    (title, meta, body.trim_start_matches('\n'))
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

/// (레벨 1~6, 텍스트) 헤딩 목록.
pub fn headings(body: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            let level = t.chars().take_while(|&c| c == '#').count();
            if (1..=6).contains(&level) && t.as_bytes().get(level).map(|b| *b == b' ').unwrap_or(level == t.len()) {
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

/// 마크다운 이미지(`![alt](url)`)/링크(`[text](url)`)를 정규식 없이 수동 스캔한다.
pub fn scan_links(text: &str) -> Vec<MdLink> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        let is_image = chars[i] == '!' && i + 1 < n && chars[i + 1] == '[';
        let bracket_start = if is_image { i + 1 } else { i };
        if chars[i] == '[' || is_image {
            if let Some(close) = find_char(&chars, bracket_start + 1, ']') {
                if close + 1 < n && chars[close + 1] == '(' {
                    if let Some(paren_close) = find_char(&chars, close + 2, ')') {
                        let label: String = chars[bracket_start + 1..close].iter().collect();
                        let url: String = chars[close + 2..paren_close].iter().collect();
                        out.push(MdLink { is_image, label, url });
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

fn is_internal_url(url: &str, site_domain: &str) -> bool {
    let low = url.to_lowercase();
    if low.starts_with("http://") || low.starts_with("https://") {
        if site_domain.is_empty() {
            return false;
        }
        return low.contains(&site_domain.to_lowercase());
    }
    true
}

fn norm_kw(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase()
}

fn contains_kw(haystack: &str, keyword: &str) -> bool {
    let kw = norm_kw(keyword);
    if kw.is_empty() {
        return false;
    }
    norm_kw(haystack).contains(&kw)
}

/// 본문에서 헤딩 줄을 제외한 순수 프로즈만 이어 붙여 첫 `limit`자를 뽑는다(도입부 근사).
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
    let h1_text = heads.iter().find(|(l, _)| *l == 1).map(|(_, t)| t.clone()).unwrap_or_default();

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
    let images_missing_alt = links.iter().filter(|l| l.is_image && l.label.trim().is_empty()).count();
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
        flesch_reading_ease: lang.map(|r| r.0),
        flesch_kincaid_grade: lang.map(|r| r.1),
    }
}

/// 필수 권장 섹션(H2 이상) 중 본문에 없는 것.
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

/// 형식·SEO 온페이지 관련 결정론적 지적 사항.
pub fn format_issues(spec: &Spec, doc: &str) -> Vec<String> {
    let mut issues = Vec::new();
    let m = metrics(doc, spec);

    if m.title_chars == 0 {
        issues.push("title 없음(프론트매터에 `title:` 필요) → 추가".to_string());
    } else if m.title_chars < spec.title_min || m.title_chars > spec.title_max {
        issues.push(format!(
            "title 글자수 {}자 (권장 {}~{}자) → 길이 조정",
            m.title_chars, spec.title_min, spec.title_max
        ));
    }

    if m.meta_chars == 0 {
        issues.push("meta_description 없음(프론트매터에 `meta_description:` 필요) → 추가".to_string());
    } else if m.meta_chars < spec.meta_min || m.meta_chars > spec.meta_max {
        issues.push(format!(
            "meta_description 글자수 {}자 (권장 {}~{}자) → 길이 조정",
            m.meta_chars, spec.meta_min, spec.meta_max
        ));
    }

    if m.h1_count == 0 {
        issues.push("H1 없음 → 본문에 `# 제목` 1개 추가".to_string());
    } else if m.h1_count > 1 {
        issues.push(format!("H1이 {}개 → 정확히 1개만 남기고 나머지는 H2 이하로 낮추기", m.h1_count));
    }

    if m.heading_skips > 0 {
        issues.push(format!(
            "헤딩 레벨 건너뛰기 {}건(예: H1 다음 바로 H3) → 계층 순서대로 삽입",
            m.heading_skips
        ));
    }

    if !spec.keyword.trim().is_empty() {
        if !m.keyword_in_title {
            issues.push(format!("타깃 키워드 '{}'가 title에 없음 → 배치", spec.keyword));
        }
        if !m.keyword_in_h1 {
            issues.push(format!("타깃 키워드 '{}'가 H1에 없음 → 배치", spec.keyword));
        }
        if !m.keyword_in_intro {
            issues.push(format!("타깃 키워드 '{}'가 도입부 100자 내에 없음 → 배치", spec.keyword));
        }
    }

    if m.images_total > 0 && m.images_missing_alt > 0 {
        issues.push(format!(
            "alt 텍스트 없는 이미지 {}개(총 {}개 중) → alt 텍스트 채우기",
            m.images_missing_alt, m.images_total
        ));
    }

    if m.internal_links < spec.internal_links_min || m.internal_links > spec.internal_links_max {
        issues.push(format!(
            "내부링크 {}개 (권장 {}~{}개) → 개수 조정 — 이 범위는 사이트 구조·글 길이에 따라 편차가 크므로 참고용",
            m.internal_links, spec.internal_links_min, spec.internal_links_max
        ));
    }

    if m.citation_links < spec.min_citations {
        issues.push(format!(
            "출처/인용 링크 {}개 → 최소 {}개 필요 (E-E-A-T, 관련 채점 항목 60점 상한 적용됨)",
            m.citation_links, spec.min_citations
        ));
    }

    for m2 in missing_sections(spec, doc) {
        issues.push(format!("권장 섹션 '{}' 누락 → 추가", m2));
    }

    issues
}

// ---- 가독성(Flesch) ----------------------------------------------------
// BlogPilot(MIT, src/lib/seo/readability.ts)의 공식·휴리스틱을 Rust로 재작성.
// 코드는 복붙하지 않고 알고리즘만 옮겼다: 마크다운 스트리핑 → 문장/단어 분리 →
// 음절수 추정 → Flesch-Kincaid Grade / Reading Ease 표준 공식.
// 이 공식은 영문 음절 규칙에 기반하므로, 라틴 문자 비중이 낮은 한국어 등의
// 문서에서는 계산하지 않는다(README 한계 참고).

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
        // 링크/이미지는 scan_links로 파싱해 텍스트만 남긴다.
        let links = scan_links(t);
        if links.is_empty() {
            out.push_str(t);
        } else {
            // 이미지→제거, 링크→라벨만 남기고 순서대로 이어붙임(원문 순서는 근사치로 충분).
            let mut rest = t.to_string();
            for l in &links {
                let full = if l.is_image {
                    format!("![{}]({})", l.label, l.url)
                } else {
                    format!("[{}]({})", l.label, l.url)
                };
                let repl = if l.is_image { String::new() } else { l.label.clone() };
                rest = rest.replacen(&full, &repl, 1);
            }
            out.push_str(&rest);
        }
        out.push('\n');
    }
    out.chars().filter(|c| !matches!(c, '*' | '_' | '~' | '>' | '`')).collect()
}

fn count_syllables(word: &str) -> usize {
    let w: String = word.to_lowercase().chars().filter(|c| c.is_ascii_alphabetic()).collect();
    if w.is_empty() {
        return 0;
    }
    if w.len() <= 3 {
        return 1;
    }
    let mut cleaned = w.clone();
    for suf in ["es", "ed", "e"] {
        if cleaned.ends_with(suf) && cleaned.len() > suf.len() + 2 {
            // 자음+suf 형태일 때만 제거(대략적 근사)
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

/// (flesch_reading_ease, flesch_kincaid_grade). 라틴 문자 비중이 절반 미만이면 None.
fn readability(body: &str) -> Option<(f64, f64)> {
    let prose = strip_markdown_to_prose(body);
    let total_chars = prose.chars().filter(|c| !c.is_whitespace()).count();
    if total_chars == 0 {
        return None;
    }
    let latin = prose.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if (latin as f64 / total_chars as f64) < 0.5 {
        return None; // 한국어 등 비영문 콘텐츠 — 영문 음절 기반 공식 부적용
    }

    let words: Vec<&str> = prose.split_whitespace().filter(|w| w.chars().any(|c| c.is_ascii_alphabetic())).collect();
    if words.is_empty() {
        return None;
    }
    let word_count = words.len();
    let syllables: usize = words.iter().map(|w| count_syllables(w)).sum();

    // 문장 분리: '.', '!', '?' 뒤 공백 기준.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{Criterion, Spec};

    fn test_spec() -> Spec {
        Spec {
            name: "t".into(),
            context: String::new(),
            keyword: "러닝화".into(),
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
        let doc = "---\ntitle: \"안녕\"\nmeta_description: \"설명\"\n---\n# H1\n본문";
        let (t, m, b) = parse_front_matter(doc);
        assert_eq!(t.as_deref(), Some("안녕"));
        assert_eq!(m.as_deref(), Some("설명"));
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
        let links = scan_links("본문 ![대체텍스트](img.png) 그리고 [내부](/a) [외부](https://ex.com)");
        assert_eq!(links.len(), 3);
        assert!(links[0].is_image);
    }

    #[test]
    fn keyword_placement_flags_missing() {
        let spec = test_spec();
        let doc = "---\ntitle: \"봄 신상 운동화 추천\"\nmeta_description: \"짧음\"\n---\n# 운동화 고르는 법\n오늘은 신발 이야기.";
        let issues = format_issues(&spec, doc);
        assert!(issues.iter().any(|i| i.contains("러닝화")));
    }
}
