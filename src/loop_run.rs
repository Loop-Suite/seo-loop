use crate::generate;
use crate::llm::Llm;
use crate::report;
use crate::score::{self, Scored};
use crate::spec::Spec;
use anyhow::Result;
use std::path::Path;

pub struct LoopOutcome {
    pub best_label: String,
    pub best_doc: String,
    pub best_score: Scored,
    pub first_doc: String,
    pub history: Vec<Scored>,
    pub stop_reason: String,
    /// 길이 인플레이션 경고(점수 대비 분량 증가)
    pub warnings: Vec<String>,
}

pub struct LoopCfg {
    pub target: f64,
    pub max_iter: usize,
    pub rounds: usize,
    /// 직전 최고점 대비 이 값 미만으로 개선되면 정체로 본다.
    pub min_delta: f64,
    /// 정체가 이 횟수 연속이면 조기 종료.
    pub patience: usize,
}

/// 생성 → 채점 → 피드백 반영 재생성 루프.
/// 반환은 마지막 회차가 아니라 전 회차 중 최고점(argmax)이다.
pub fn run(
    gen_llm: &Llm,
    judges: &[Llm],
    spec: &Spec,
    brief: &str,
    out_dir: &Path,
    cfg: &LoopCfg,
    angle: &str,
) -> Result<LoopOutcome> {
    let mut doc = generate::generate(gen_llm, spec, brief, angle)?;
    let mut history: Vec<Scored> = Vec::new();
    let mut docs: Vec<String> = Vec::new();
    let mut best_i = 0usize;
    let mut stall = 0usize;
    let mut stop_reason = format!("최대 반복 {}회 도달", cfg.max_iter.max(1));

    for i in 0..cfg.max_iter.max(1) {
        let label = format!("iter{:02}", i + 1);
        std::fs::write(out_dir.join(format!("{}.md", label)), &doc)?;

        let s = score::score_doc(judges, spec, &label, &doc, cfg.rounds)?;
        report::append_jsonl(out_dir, &s)?;
        println!(
            "  [{}] {:.1}/100  ({}자{})",
            label,
            s.total,
            s.metrics.chars,
            if s.format_issues.is_empty() {
                String::new()
            } else {
                format!(", 형식지적 {}건", s.format_issues.len())
            }
        );

        let prev_best = history.get(best_i).map(|b: &Scored| b.total);
        let improved = match prev_best {
            None => true,
            Some(b) => s.total > b,
        };
        history.push(s.clone());
        docs.push(doc.clone());
        if improved {
            let gain = s.total - prev_best.unwrap_or(f64::NEG_INFINITY);
            best_i = history.len() - 1;
            if prev_best.is_some() && gain < cfg.min_delta {
                stall += 1;
            } else {
                stall = 0;
            }
        } else {
            stall += 1;
        }

        if s.total >= cfg.target && s.format_issues.is_empty() {
            stop_reason = format!("목표 {:.0}점 도달", cfg.target);
            break;
        }
        if stall >= cfg.patience {
            stop_reason = format!(
                "개선 정체({}회 연속 +{:.1}점 미만)",
                cfg.patience, cfg.min_delta
            );
            break;
        }
        if i + 1 == cfg.max_iter.max(1) {
            break;
        }

        let fb = score::feedback_text(&history[history.len() - 1]);
        let weak = score::weak_points(spec, &history[history.len() - 1]);
        doc = generate::revise(gen_llm, spec, brief, &doc, &fb, &weak)?;
    }

    let best_score = history[best_i].clone();
    let best_doc = docs[best_i].clone();
    std::fs::write(out_dir.join("best.md"), &best_doc)?;

    // 길이 인플레이션 canary: 점수 대비 분량이 과도하게 늘면 verbosity gaming 의심.
    let mut warnings = Vec::new();
    let first = &history[0];
    let d_score = best_score.total - first.total;
    let d_chars = best_score.metrics.chars as f64 - first.metrics.chars as f64;
    let growth = if first.metrics.chars > 0 {
        d_chars / first.metrics.chars as f64
    } else {
        0.0
    };
    if growth > 0.25 && d_score < 5.0 {
        warnings.push(format!(
            "길이 canary: 분량 +{:.0}% 인데 점수는 +{:.1}점 → 내용 보강이 아니라 늘려쓰기일 가능성",
            growth * 100.0,
            d_score
        ));
    }
    if best_i + 1 < history.len() {
        warnings.push(format!(
            "마지막 회차({:.1}점)가 최고점이 아님 → best.md는 iter{:02}",
            history.last().map(|h| h.total).unwrap_or(0.0),
            best_i + 1
        ));
    }

    Ok(LoopOutcome {
        best_label: best_score.label.clone(),
        best_doc,
        first_doc: docs[0].clone(),
        best_score,
        history,
        stop_reason,
        warnings,
    })
}
