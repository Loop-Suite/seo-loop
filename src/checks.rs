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
///
/// 바이트 오프셋은 `.lines()`(개행 문자 제거)가 아니라 `split('\n')`로 계산한다 —
/// `.lines()` 기준 `line.len()+1`은 LF만 가정해서 CRLF 문서에서 줄마다 1바이트씩
/// 과소 계산되고, 그 오차가 문자 경계 밖 슬라이싱(panic)이나 앞부분이 잘려나간
/// 본문으로 이어지는 버그가 있었다. `split('\n')`은 `\r`을 앞 줄에 남겨두므로
/// `line.len()+1`이 항상 정확한 소비 바이트 수가 된다.
pub fn parse_front_matter(doc: &str) -> (Option<String>, Option<String>, &str) {
    let trimmed = doc.trim_start_matches('\u{feff}');
    if !trimmed.starts_with("---") {
        return (None, None, doc);
    }
    let after_marker = &trimmed[3..];
    let first_nl = match after_marker.find('\n') {
        Some(i) => i,
        None => return (None, None, doc), // "---" 뒤에 개행조차 없음 → 프론트매터 아님
    };
    let rest = &after_marker[first_nl + 1..];

    let mut title = None;
    let mut meta = None;
    let mut offset = 0usize;
    let mut close_end: Option<usize> = None;
    for line in rest.split('\n') {
        let consumed_this_line = line.len() + 1; // split('\n')이라 구분자는 항상 정확히 1바이트
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
        return (None, None, doc); // 닫는 "---" 없음 → format_issues에서 별도로 경고
    };
    let body = &rest[close_end..];
    (title, meta, body.trim_start_matches(['\n', '\r']))
}

/// 프론트매터가 `---`로 시작했지만 닫는 `---`가 없어 파싱이 통째로 포기된 경우.
/// (파싱 실패 시 `parse_front_matter`는 원본 `doc`을 그대로 반환하므로 포인터/길이가 doc과 같다)
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
/// 라벨 안에 중첩된 `[...]`(예: `[a[b]c](url)`)를 depth로 추적해 짝이 맞는 `]`를 찾고,
/// `\[`/`\]`(이스케이프)는 괄호로 세지 않는다.
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

/// `[` 바로 다음 위치(`from`)부터 중첩 `[...]`를 depth로 추적하며 짝이 맞는 `]`를 찾는다.
/// `\]`/`\[`는 괄호로 세지 않는다(이스케이프).
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

/// 절대 URL의 호스트가 `site_domain`과 정확히 같거나 그 서브도메인인지 판정한다.
/// (이전에는 `contains`로 부분 문자열만 검사해 `notexample.com`이나
/// `example.com.evil.com` 같은 호스트도 "내부 링크"로 오분류될 수 있었다.)
fn is_internal_url(url: &str, site_domain: &str) -> bool {
    let low = url.to_lowercase();
    let rest = low.strip_prefix("https://").or_else(|| low.strip_prefix("http://"));
    match rest {
        Some(rest) => {
            if site_domain.is_empty() {
                return false;
            }
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            let host = host.split(':').next().unwrap_or(host); // 포트 제거
            let domain = site_domain.to_lowercase();
            host == domain || host.ends_with(&format!(".{domain}"))
        }
        None => true, // 스킴 없는 상대경로는 내부 링크로 취급
    }
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

    if front_matter_unclosed(doc) {
        issues.push(
            "프론트매터가 닫는 `---`로 끝나지 않음 → title/meta_description을 인식하지 못하고 \
             문서 전체가 본문으로 처리됨(가독성·헤딩 검사가 오염될 수 있음) → 닫는 `---` 확인"
                .to_string(),
        );
    }

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

    let (_, _, body) = parse_front_matter(doc);
    issues.extend(paragraph_length_issues(body));

    issues
}

/// 지나치게 긴 문단(스캔 가독성 저해) 지적.
/// 아이디어 출처: CyberCraftBD/power-seo(MIT) `paragraph-length.ts`(영문 단어수 120~150 기준).
/// 이 프로젝트는 한국어 등 비영문 콘텐츠도 다루므로 단어수 대신 글자수 기준으로
/// 재설계했다(임계값 600자는 이 프로젝트에서 임의로 정한 값, power-seo 원 수치를
/// 그대로 옮긴 게 아니다).
const PARAGRAPH_CHAR_LIMIT: usize = 600;

fn paragraph_length_issues(body: &str) -> Vec<String> {
    let mut issues = Vec::new();
    let mut para = String::new();
    let flush = |para: &mut String, issues: &mut Vec<String>| {
        let n = para.chars().count();
        if n > PARAGRAPH_CHAR_LIMIT {
            issues.push(format!(
                "문단이 {n}자로 너무 김(권장 {PARAGRAPH_CHAR_LIMIT}자 이하) → 스캔 가독성을 위해 문단 분리"
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
    fn link_scan_handles_nested_brackets_in_label() {
        let links = scan_links("[a[b]c](https://ex.com)");
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].label, "a[b]c");
        assert_eq!(links[0].url, "https://ex.com");
    }

    #[test]
    fn link_scan_ignores_escaped_brackets() {
        let links = scan_links(r"\[이건 링크 아님\](url) 그리고 [진짜](https://ex.com)");
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].url, "https://ex.com");
    }

    #[test]
    fn keyword_placement_flags_missing() {
        let spec = test_spec();
        let doc = "---\ntitle: \"봄 신상 운동화 추천\"\nmeta_description: \"짧음\"\n---\n# 운동화 고르는 법\n오늘은 신발 이야기.";
        let issues = format_issues(&spec, doc);
        assert!(issues.iter().any(|i| i.contains("러닝화")));
    }

    #[test]
    fn front_matter_parses_with_crlf() {
        let doc = "---\r\ntitle: \"안녕\"\r\nmeta_description: \"설명\"\r\n---\r\n# H1\r\n본문";
        let (t, m, b) = parse_front_matter(doc);
        assert_eq!(t.as_deref(), Some("안녕"));
        assert_eq!(m.as_deref(), Some("설명"));
        assert!(b.contains("H1"), "{b:?}");
        assert!(!b.contains("title:"), "프론트매터 줄이 본문에 새어나가면 안 됨: {b:?}");
    }

    #[test]
    fn front_matter_unclosed_falls_back_and_is_flagged() {
        let spec = test_spec();
        let doc = "---\ntitle: X\n본문인데 닫는 --- 가 없음";
        let (t, m, _) = parse_front_matter(doc);
        assert!(t.is_none() && m.is_none());
        let issues = format_issues(&spec, doc);
        assert!(issues.iter().any(|i| i.contains("닫는")), "{issues:?}");
    }

    #[test]
    fn internal_url_spoofing_is_rejected() {
        assert!(!is_internal_url("https://notexample.com/a", "example.com"));
        assert!(!is_internal_url("https://example.com.evil.com/a", "example.com"));
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
    fn unquote_handles_bare_value() {
        let doc = "---\ntitle: 따옴표 없는 제목\nmeta_description: \"설명\"\n---\n# H1\n본문";
        let (t, _, _) = parse_front_matter(doc);
        assert_eq!(t.as_deref(), Some("따옴표 없는 제목"));
    }

    #[test]
    fn whitespace_only_alt_counts_as_missing() {
        let links = scan_links("![ ](img.png)");
        assert_eq!(links.len(), 1);
        assert!(links[0].label.trim().is_empty());
    }

    #[test]
    fn long_paragraph_is_flagged() {
        let long = "가".repeat(601);
        let body = format!("# T\n\n{long}\n");
        let issues = paragraph_length_issues(&body);
        assert!(issues.iter().any(|i| i.contains("너무 김")), "{issues:?}");
    }

    #[test]
    fn short_paragraph_is_not_flagged() {
        let body = "# T\n\n짧은 문단입니다.\n";
        assert!(paragraph_length_issues(body).is_empty());
    }

    #[test]
    fn internal_and_citation_link_counts() {
        let spec = test_spec();
        let doc = "---\ntitle: \"러닝화 고르는 법에 대한 상세 가이드 전체 안내\"\nmeta_description: \"러닝화를 고르는 방법에 대한 아주 상세하고 도움이 되는 설명입니다 충분히 길게\"\n---\n# 러닝화 고르는 법\n러닝화 이야기. [내부1](https://example.com/a) [내부2](/b) [외부](https://other.com/c)\n";
        let m = metrics(doc, &spec);
        assert_eq!(m.internal_links, 2);
        assert_eq!(m.citation_links, 1);
    }
}
