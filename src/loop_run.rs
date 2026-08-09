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
    /// Length inflation warning (volume increase relative to score)
    pub warnings: Vec<String>,
}

pub struct LoopCfg {
    pub target: f64,
    pub max_iter: usize,
    pub rounds: usize,
    /// If improvement over the previous best is below this value, it's considered stalled.
    pub min_delta: f64,
    /// Early stop if stalled this many times in a row.
    pub patience: usize,
}

/// Generate → score → regenerate with feedback loop.
/// The return value is the best score across all iterations (argmax), not the last one.
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
    let mut stop_reason = format!("Reached max iterations ({} rounds)", cfg.max_iter.max(1));

    for i in 0..cfg.max_iter.max(1) {
        let label = format!("iter{:02}", i + 1);
        std::fs::write(out_dir.join(format!("{}.md", label)), &doc)?;

        let s = score::score_doc(judges, spec, &label, &doc, cfg.rounds)?;
        report::append_jsonl(out_dir, &s)?;
        println!(
            "  [{}] {:.1}/100  ({} chars{})",
            label,
            s.total,
            s.metrics.chars,
            if s.format_issues.is_empty() {
                String::new()
            } else {
                format!(", {} format issues", s.format_issues.len())
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
            stop_reason = format!("Reached target score ({:.0} points)", cfg.target);
            break;
        }
        if stall >= cfg.patience {
            stop_reason = format!(
                "Improvement stalled ({} consecutive rounds below +{:.1} points)",
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

    // Length inflation canary: if volume grows disproportionately to score, suspect verbosity gaming.
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
            "Length canary: volume +{:.0}% but score only +{:.1} points → likely padding rather than substantive improvement",
            growth * 100.0,
            d_score
        ));
    }

    // Keyword density canary: guards against multi-axis reward-hacking (§3.4, arXiv:2605.27996
    // "Reward Bias Substitution: Single-Axis Bias Mitigations Redirect Optimization Pressure").
    // The paper's core claim is that suppressing only a single bias axis (length) redirects
    // optimization pressure toward other, unobserved axes — the length canary above is exactly
    // that kind of single-axis defense, so we monitor at least one more axis here.
    //
    // Why we chose "a spike in target keyword occurrences across rounds" over "unnatural
    // link density relative to citation_links count": checks.rs already has norm_kw/contains_kw
    // keyword-normalization logic we can simply reuse (a cleaner implementation), whereas
    // flagging "low-quality links" would require actually fetching URLs to judge their
    // trustworthiness, which we explicitly excluded from this scope for the same reason as
    // the FacTool-type items in the §5 backlog (conflicts with the offline/reproducibility-first
    // philosophy and adds a network dependency).
    let d_kw = best_score.metrics.keyword_occurrences as f64 - first.metrics.keyword_occurrences as f64;
    if first.metrics.keyword_occurrences > 0 {
        let kw_growth = d_kw / first.metrics.keyword_occurrences as f64;
        if kw_growth > 0.5 && d_score < 5.0 {
            warnings.push(format!(
                "Keyword density canary: '{}' occurrence count {}→{} (+{:.0}%) but score only +{:.1} points → suspect keyword stuffing",
                spec.keyword,
                first.metrics.keyword_occurrences,
                best_score.metrics.keyword_occurrences,
                kw_growth * 100.0,
                d_score
            ));
        }
    }

    if best_i + 1 < history.len() {
        warnings.push(format!(
            "Last iteration ({:.1} points) is not the best score → best.md is iter{:02}",
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
