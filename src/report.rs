use crate::score::Scored;
use crate::spec::Spec;
use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn append_jsonl(out_dir: &Path, s: &Scored) -> Result<()> {
    let path = out_dir.join("results.jsonl");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("{} 열기 실패", path.display()))?;
    writeln!(f, "{}", serde_json::to_string(s)?)?;
    Ok(())
}

fn header(spec: &Spec, rows: &[&Scored]) -> String {
    let mut md = format!("# 채점 리포트 — {}\n\n", spec.name);
    md.push_str(&format!("> 타깃 키워드: {}\n\n", spec.keyword));
    if !spec.scoring_source.is_empty() {
        md.push_str(&format!("> 배점 근거: {}\n\n", spec.scoring_source));
    }
    let rounds = rows.first().map(|r| r.rounds).unwrap_or(0);
    let models = rows.first().map(|r| r.models.join(", ")).unwrap_or_default();
    md.push_str(&format!(
        "문서 {}건 · 문서당 채점 {}회 · 채점 모델: {}\n\n",
        rows.len(),
        rounds,
        if models.is_empty() { "-".into() } else { models }
    ));
    md
}

fn table(spec: &Spec, rows: &[&Scored]) -> String {
    let mut md = String::from("| 순위 | 문서 | 총점 |");
    for c in &spec.criteria {
        md.push_str(&format!(" {} |", c.name));
    }
    md.push_str(" title | meta | H1 | 내부링크 | 출처링크 | 형식지적 |\n|---|---|---|");
    for _ in &spec.criteria {
        md.push_str("---|");
    }
    md.push_str("---|---|---|---|---|---|\n");

    for (i, s) in rows.iter().enumerate() {
        md.push_str(&format!("| {} | {} | **{:.1}** |", i + 1, s.label, s.total));
        for c in &spec.criteria {
            let v = s.per_criterion.get(&c.id).copied().unwrap_or(0.0);
            let sp = s.spread.get(&c.id).copied().unwrap_or(0.0);
            let capped = s.citation_capped.contains(&c.id);
            if sp > 0.0 {
                md.push_str(&format!(" {:.0} (±{:.0}){} |", v, sp / 2.0, if capped { " 🔒60" } else { "" }));
            } else {
                md.push_str(&format!(" {:.0}{} |", v, if capped { " 🔒60" } else { "" }));
            }
        }
        md.push_str(&format!(
            " {}자 | {}자 | {} | {} | {} | {} |\n",
            s.metrics.title_chars,
            s.metrics.meta_chars,
            s.metrics.h1_count,
            s.metrics.internal_links,
            s.metrics.citation_links,
            if s.format_issues.is_empty() { "-".to_string() } else { format!("{}건", s.format_issues.len()) }
        ));
    }
    md
}

fn details(rows: &[&Scored]) -> String {
    let mut md = String::from("\n---\n\n## 문서별 상세\n");
    for s in rows {
        md.push_str(&format!("\n### {} ({:.1}/100)\n\n", s.label, s.total));
        md.push_str(&format!(
            "본문 {}자 · H1 {}개 · 헤딩건너뛰기 {}건 · 이미지 {}개(alt누락 {}개) · 내부링크 {}개 · 출처링크 {}개",
            s.metrics.chars,
            s.metrics.h1_count,
            s.metrics.heading_skips,
            s.metrics.images_total,
            s.metrics.images_missing_alt,
            s.metrics.internal_links,
            s.metrics.citation_links
        ));
        match (s.metrics.flesch_reading_ease, s.metrics.flesch_kincaid_grade) {
            (Some(ease), Some(grade)) => {
                md.push_str(&format!(" · Flesch Reading Ease {:.0} (Grade {:.1})", ease, grade));
            }
            _ => md.push_str(" · Flesch: N/A(비영문 콘텐츠 — README 한계 참고)"),
        }
        if let Some(kr) = s.metrics.korean_readability_heuristic {
            md.push_str(&format!(
                " · 한국어 가독성 휴리스틱 {:.1} (⚠️ 검증되지 않은 휴리스틱 — 학술 근거 없음, README 한계 참고)",
                kr
            ));
        }
        md.push_str("\n\n");

        if !s.citation_capped.is_empty() {
            md.push_str(&format!(
                "🔒 출처 부족으로 60점 상한 적용된 항목: {}\n\n",
                s.citation_capped.join(", ")
            ));
        }

        for c in &s.comments {
            if !c.trim().is_empty() {
                md.push_str(&format!("> {}\n\n", c));
            }
        }
        if !s.format_issues.is_empty() {
            md.push_str("자동 형식/온페이지 검사:\n\n");
            for f in &s.format_issues {
                md.push_str(&format!("- {}\n", f));
            }
            md.push('\n');
        }
        md.push_str("개선 지시:\n\n");
        for imp in s.improvements.iter().filter(|i| !s.format_issues.contains(i)) {
            md.push_str(&format!("- {}\n", imp));
        }
    }
    md
}

/// 랭킹 리포트.
pub fn write_report(out_dir: &Path, spec: &Spec, scored: &[Scored]) -> Result<PathBuf> {
    let mut rows: Vec<&Scored> = scored.iter().collect();
    rows.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));

    let mut md = header(spec, &rows);
    md.push_str(&table(spec, &rows));
    md.push_str(&details(&rows));
    md.push_str(&format!("\n---\n\n누적 API 비용: ${:.4}\n", crate::llm::total_cost_usd()));

    let path = out_dir.join("report.md");
    std::fs::write(&path, md).with_context(|| format!("{} 쓰기 실패", path.display()))?;
    Ok(path)
}

/// 루프 리포트: 회차별 추이 + 경고 + held-out 게이트 결과.
pub fn write_loop_report(
    out_dir: &Path,
    spec: &Spec,
    history: &[Scored],
    stop_reason: &str,
    warnings: &[String],
    gate: Option<(&Scored, &Scored)>, // (최초본, 최고본) held-out 채점 결과
) -> Result<PathBuf> {
    let mut md = format!("# 루프 리포트 — {}\n\n", spec.name);
    md.push_str(&format!("> 타깃 키워드: {}\n\n", spec.keyword));
    if !spec.scoring_source.is_empty() {
        md.push_str(&format!("> 배점 근거: {}\n\n", spec.scoring_source));
    }
    md.push_str(&format!("종료 사유: {}\n\n", stop_reason));

    md.push_str("## 회차별 추이\n\n| 회차 | 총점 | Δ | 분량 | 형식지적 |\n|---|---|---|---|---|\n");
    let mut prev: Option<f64> = None;
    for h in history {
        let d = match prev {
            Some(p) => format!("{:+.1}", h.total - p),
            None => "-".to_string(),
        };
        md.push_str(&format!(
            "| {} | {:.1} | {} | {}자 | {}건 |\n",
            h.label, h.total, d, h.metrics.chars, h.format_issues.len()
        ));
        prev = Some(h.total);
    }

    if let Some((first, best)) = gate {
        let loop_delta =
            history.iter().map(|h| h.total).fold(f64::NEG_INFINITY, f64::max) - history[0].total;
        let gate_delta = best.total - first.total;
        md.push_str(&format!(
            "\n## held-out 검증 (루프 미참여 채점 모델: {})\n\n\
             | 대상 | 루프 채점 | held-out 채점 |\n|---|---|---|\n\
             | 최초본 | {:.1} | {:.1} |\n| 최고본 | {:.1} | {:.1} |\n\n\
             루프 기준 개선폭 {:+.1}점 vs held-out 기준 {:+.1}점\n\n",
            best.models.join(", "),
            history[0].total,
            first.total,
            history.iter().map(|h| h.total).fold(f64::NEG_INFINITY, f64::max),
            best.total,
            loop_delta,
            gate_delta
        ));
        if loop_delta > 0.0 && gate_delta < loop_delta * 0.34 {
            md.push_str(
                "> ⚠ held-out 개선폭이 루프 개선폭의 1/3 미만 → 채점자 최적화(reward hacking) 의심. \
                 실제 콘텐츠 품질이 올랐는지 사람이 직접 확인할 것.\n\n",
            );
        }
    }

    if !warnings.is_empty() {
        md.push_str("\n## 경고\n\n");
        for w in warnings {
            md.push_str(&format!("- {}\n", w));
        }
    }

    let rows: Vec<&Scored> = history.iter().collect();
    md.push_str(&details(&rows));
    md.push_str(&format!("\n---\n\n누적 API 비용: ${:.4}\n", crate::llm::total_cost_usd()));

    let path = out_dir.join("report.md");
    std::fs::write(&path, md).with_context(|| format!("{} 쓰기 실패", path.display()))?;
    Ok(path)
}
