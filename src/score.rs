use crate::checks::{self, Metrics};
use crate::llm::Llm;
use crate::spec::Spec;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

pub const JUDGE_SYSTEM: &str = "You are an SEO content judge well-versed in Google's Quality Rater Guidelines (E-E-A-T). \
The document's author is unknown; do not guess at authorship. \
Flowery language unrelated to search intent, unsupported figures, and excessive keyword repetition (keyword stuffing) are grounds for deduction. \
Do not grade generously — back every score with a direct quotation from the document.";

/// Judging lenses. Rotated per round.
/// (Repeating the same model correlates its errors, so lens separation alone does not produce
///  independent samples. Real independence comes from using a panel of different models.)
pub const LENSES: &[&str] = &[
    "Weighs search-intent alignment and overall completeness in balance.",
    "Scrutinizes keyword naturalness and over-optimization (stuffing) especially strictly.",
    "Scrutinizes E-E-A-T (Experience, Expertise, Authoritativeness, Trustworthiness) signals and source verifiability especially strictly.",
    "Evaluates scan readability and structure as experienced in the first 3-10 seconds after clicking through from search results.",
    "Evaluates differentiation and genuine information density versus top-ranking competing content.",
    "Evaluates whether on-page elements like title, meta description, and headings actually contribute to click-through rate/comprehension.",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriterionScore {
    pub id: String,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub why_not_higher: String,
    pub score: f64, // 0-100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeResult {
    #[serde(default)]
    pub winning_conditions: Vec<String>,
    #[serde(default)]
    pub criteria: Vec<CriterionScore>,
    #[serde(default)]
    pub improvements: Vec<String>,
    #[serde(default)]
    pub comment: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scored {
    pub label: String,
    /// Weighted sum, 0-100
    pub total: f64,
    /// Per-criterion aggregate score (0-100, trimmed mean, after applying the 60-point cap for insufficient citations)
    pub per_criterion: BTreeMap<String, f64>,
    /// All raw per-criterion scores (per judge)
    pub raw: BTreeMap<String, Vec<f64>>,
    /// Per-criterion max-min spread (a judgment-instability indicator)
    pub spread: BTreeMap<String, f64>,
    pub missing_sections: Vec<String>,
    /// Deterministic format/on-page check results
    pub format_issues: Vec<String>,
    pub metrics: Metrics,
    pub improvements: Vec<String>,
    pub comments: Vec<String>,
    pub rounds: usize,
    pub models: Vec<String>,
    /// Whether citation_required criteria were capped at 60 points due to insufficient citations
    pub citation_capped: Vec<String>,
}

fn judge_schema(spec: &Spec) -> serde_json::Value {
    let ids: Vec<String> = spec.criteria.iter().map(|c| c.id.clone()).collect();
    // Field order = generation order. Having the model write out criteria (winning_conditions) before
    // scoring reduces anchoring to the document (de-anchoring).
    json!({
        "type": "object",
        "properties": {
            "winning_conditions": {
                "type": "array",
                "minItems": 3,
                "items": {"type": "string"},
                "description": "3-6 conditions that top-ranking content for this search intent should meet, written before reading the document"
            },
            "criteria": {
                "type": "array",
                "minItems": ids.len(),
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string", "enum": ids},
                        "evidence": {"type": "string", "description": "Direct quotation from the document (30+ characters)"},
                        "why_not_higher": {"type": "string", "description": "Why not a higher score"},
                        "score": {"type": "integer", "minimum": 0, "maximum": 100}
                    },
                    "required": ["id", "evidence", "why_not_higher", "score"],
                    "additionalProperties": false
                }
            },
            "improvements": {
                "type": "array", "minItems": 3, "maxItems": 8,
                "items": {"type": "string", "description": "Immediately actionable revision instructions"}
            },
            "comment": {"type": "string"}
        },
        "required": ["winning_conditions", "criteria", "improvements", "comment"],
        "additionalProperties": false
    })
}

fn build_judge_prompt(spec: &Spec, doc: &str, lens: &str) -> String {
    format!(
        "# Task\nJudge the submitted SEO content according to the scoring criteria.\n\n\
         ## Content type: {name}\n{ctx}\n\n\
         ## Target keyword\n{kw}\n\n\
         ## This judge's lens\n{lens}\n\n\
         ## Scoring criteria (integer 0-100 for each item)\n{rubric}\n\n\
         ## Score band guidelines\n{bands}\n\n\
         ## Procedure\n\
         1. Before scoring the document, write 3-6 'conditions content must meet to rank for this search intent' in winning_conditions.\n\
         2. Then score each criterion. For each item, quote the document verbatim in evidence, and explain in why_not_higher why it didn't receive a higher score.\n\
         3. If you cannot find supporting evidence to quote, that item cannot exceed 60 points.\n\
         4. Format/on-page elements such as title/meta character counts, heading hierarchy, link counts, and alt text are handled by separate automated checks — do not factor them into scoring; evaluate only content and persuasiveness.\n\n\
         ## Document to score (front matter + markdown body)\n<document>\n{doc}\n</document>\n",
        name = spec.name,
        ctx = spec.context,
        kw = spec.keyword,
        lens = lens,
        rubric = spec.rubric_prompt(),
        bands = spec.bands_prompt(),
        doc = doc
    )
}

/// Trimmed mean. If n>=4, drop one min and one max then average; otherwise plain average.
/// (With many integer 0-100 samples, the median produces too many ties and misses small improvements)
fn trimmed_mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    if v.len() < 4 {
        return v.iter().sum::<f64>() / v.len() as f64;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let inner = &s[1..s.len() - 1];
    inner.iter().sum::<f64>() / inner.len() as f64
}

/// Score a single document. Repeats `rounds` times, rotating models and lenses.
pub fn score_doc(
    judges: &[Llm],
    spec: &Spec,
    label: &str,
    doc: &str,
    rounds: usize,
) -> Result<Scored> {
    anyhow::ensure!(!judges.is_empty(), "No scoring models available");
    let rounds = rounds.max(1);
    let schema = judge_schema(spec);

    // Round-level parallelism: applies the same std::thread::scope pattern that main.rs::par_map
    // uses to parallelize across documents (N drafts) to rounds instead. `--concurrency` already
    // budgets parallelism at the document level, so here we simply spawn one thread per round with
    // no extra option (§5 backlog: cf. auto-seo's asyncio.gather pattern, but its budget management
    // is unverified, so we keep this a simple implementation). Results are returned in index order,
    // so downstream aggregation logic like trimmed_mean is unaffected.
    let round_results: Vec<Result<(JudgeResult, String)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..rounds)
            .map(|i| {
                let llm = &judges[i % judges.len()];
                let lens = LENSES[i % LENSES.len()];
                let prompt = build_judge_prompt(spec, doc, lens);
                let schema = &schema;
                scope.spawn(move || -> Result<(JudgeResult, String)> {
                    let v = llm
                        .json(&prompt, Some(JUDGE_SYSTEM), schema)
                        .with_context(|| format!("Scoring failed ({label}, round {})", i + 1))?;
                    let jr: JudgeResult = serde_json::from_value(v)
                        .with_context(|| format!("Scoring result schema mismatch ({label})"))?;
                    Ok((jr, llm.label()))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Scoring thread panicked ({label})")))
            })
            .collect()
    });

    let mut results: Vec<JudgeResult> = Vec::with_capacity(rounds);
    let mut models: Vec<String> = Vec::with_capacity(rounds);
    for r in round_results {
        let (jr, model) = r?;
        results.push(jr);
        models.push(model);
    }

    let m = checks::metrics(doc, spec);

    let mut per_criterion: BTreeMap<String, f64> = BTreeMap::new();
    let mut raw: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut spread: BTreeMap<String, f64> = BTreeMap::new();
    let mut citation_capped: Vec<String> = Vec::new();
    for c in &spec.criteria {
        let vals: Vec<f64> = results
            .iter()
            .filter_map(|r| r.criteria.iter().find(|x| x.id == c.id))
            .map(|x| x.score.clamp(0.0, 100.0))
            .collect();
        let lo = vals.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        spread.insert(c.id.clone(), if vals.is_empty() { 0.0 } else { hi - lo });
        let mut agg = trimmed_mean(&vals);
        // citation_required criteria: if source/citation links fall below spec.min_citations,
        // deterministically cap at 60 points (same idea as bizplan-loop's "can't cite → capped at 60",
        // but since citation count is measurable from markdown links, we hard-cap it here rather than in the prompt).
        if c.citation_required && m.citation_links < spec.min_citations && agg > 60.0 {
            agg = 60.0;
            citation_capped.push(c.id.clone());
        }
        per_criterion.insert(c.id.clone(), agg);
        raw.insert(c.id.clone(), vals);
    }

    let wsum = spec.weight_sum();
    let total: f64 = spec
        .criteria
        .iter()
        .map(|c| per_criterion.get(&c.id).copied().unwrap_or(0.0) * (c.weight / wsum))
        .sum();

    let format_issues = checks::format_issues(spec, doc);
    let missing = checks::missing_sections(spec, doc);

    let mut improvements: Vec<String> = format_issues.clone();
    for r in &results {
        for imp in &r.improvements {
            let t = imp.trim().to_string();
            if !t.is_empty() && !improvements.contains(&t) {
                improvements.push(t);
            }
        }
    }

    Ok(Scored {
        label: label.to_string(),
        total: (total * 10.0).round() / 10.0,
        per_criterion,
        raw,
        spread,
        missing_sections: missing,
        format_issues,
        metrics: m,
        improvements,
        comments: results.iter().map(|r| r.comment.clone()).collect(),
        rounds,
        models,
        citation_capped,
    })
}

/// Feedback for the regeneration prompt. Does not pass along the score itself (to discourage optimizing for the score).
pub fn feedback_text(s: &Scored) -> String {
    let mut out = String::from("[Revision instructions that must be applied]\n");
    for i in &s.improvements {
        out.push_str(&format!("- {}\n", i));
    }
    if !s.comments.is_empty() {
        out.push_str("\n[Judge's overall comments]\n");
        for c in &s.comments {
            out.push_str(&format!("- {}\n", c));
        }
    }
    out
}

/// The two lowest-scoring criteria.
pub fn weak_points(spec: &Spec, s: &Scored) -> String {
    let mut v: Vec<(&str, f64)> = spec
        .criteria
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                s.per_criterion.get(&c.id).copied().unwrap_or(0.0),
            )
        })
        .collect();
    v.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    v.iter()
        .take(2)
        .map(|(n, sc)| format!("- {} : {:.0}/100", n, sc))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trimmed_mean_drops_outliers() {
        assert_eq!(trimmed_mean(&[70.0, 72.0, 74.0, 100.0]), 73.0);
        assert_eq!(trimmed_mean(&[80.0]), 80.0);
        assert!((trimmed_mean(&[70.0, 80.0]) - 75.0).abs() < 1e-9);
    }
}
