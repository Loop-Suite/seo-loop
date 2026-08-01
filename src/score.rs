use crate::checks::{self, Metrics};
use crate::llm::Llm;
use crate::spec::Spec;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

pub const JUDGE_SYSTEM: &str = "당신은 Google 품질평가 가이드라인(E-E-A-T)에 정통한 SEO 콘텐츠 심사자다. \
문서의 작성자는 알 수 없으며, 저자를 추측하지 않는다. \
검색 의도와 무관한 미사여구, 근거 없는 수치, 과도한 키워드 반복(키워드 스터핑)은 감점 사유다. \
관대하게 채점하지 않으며, 모든 점수에 문서 원문 인용을 근거로 붙인다.";

/// 심사 관점. 회차마다 순환.
/// (동일 모델 반복은 오차가 상관되므로 관점 분리만으로 독립 표본이 되지는 않는다.
///  실질적 독립성은 서로 다른 모델 패널에서 나온다.)
pub const LENSES: &[&str] = &[
    "검색 의도 부합도와 전체 완성도를 균형 있게 본다.",
    "키워드 자연스러움과 과최적화(스터핑) 여부를 특히 까다롭게 본다.",
    "E-E-A-T(경험·전문성·권위성·신뢰성) 신호와 출처의 검증 가능성을 특히 까다롭게 본다.",
    "검색 결과에서 클릭해 처음 3초~10초간 읽었을 때의 스캔 가독성과 구조를 본다.",
    "경쟁 상위 노출 콘텐츠 대비 차별성과 실질 정보 밀도를 본다.",
    "제목·메타·헤딩 등 온페이지 요소가 실제 클릭률/이해도에 기여하는지를 본다.",
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
    /// 0-100 가중 합산
    pub total: f64,
    /// 항목별 집계 점수(0-100, 절사평균, 인용 부족 시 60점 상한 반영 후)
    pub per_criterion: BTreeMap<String, f64>,
    /// 항목별 원점수 전체(심사위원별)
    pub raw: BTreeMap<String, Vec<f64>>,
    /// 항목별 최대-최소 폭(판정 불안정 지표)
    pub spread: BTreeMap<String, f64>,
    pub missing_sections: Vec<String>,
    /// 결정론적 형식/온페이지 검사 결과
    pub format_issues: Vec<String>,
    pub metrics: Metrics,
    pub improvements: Vec<String>,
    pub comments: Vec<String>,
    pub rounds: usize,
    pub models: Vec<String>,
    /// citation_required 항목이 출처 부족으로 60점 상한 처리됐는지
    pub citation_capped: Vec<String>,
}

fn judge_schema(spec: &Spec) -> serde_json::Value {
    let ids: Vec<String> = spec.criteria.iter().map(|c| c.id.clone()).collect();
    // 필드 순서 = 생성 순서. 채점 전에 기준(winning_conditions)을 먼저 쓰게 해
    // 문서에 앵커링되는 것을 줄인다(de-anchoring).
    json!({
        "type": "object",
        "properties": {
            "winning_conditions": {
                "type": "array",
                "minItems": 3,
                "items": {"type": "string"},
                "description": "문서를 읽기 전에, 이 검색 의도에서 상위 노출될 콘텐츠가 갖춰야 할 조건 3~6개"
            },
            "criteria": {
                "type": "array",
                "minItems": ids.len(),
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string", "enum": ids},
                        "evidence": {"type": "string", "description": "문서 원문 직접 인용(30자 이상)"},
                        "why_not_higher": {"type": "string", "description": "왜 더 높은 점수가 아닌지"},
                        "score": {"type": "integer", "minimum": 0, "maximum": 100}
                    },
                    "required": ["id", "evidence", "why_not_higher", "score"],
                    "additionalProperties": false
                }
            },
            "improvements": {
                "type": "array", "minItems": 3, "maxItems": 8,
                "items": {"type": "string", "description": "즉시 실행 가능한 수정 지시문"}
            },
            "comment": {"type": "string"}
        },
        "required": ["winning_conditions", "criteria", "improvements", "comment"],
        "additionalProperties": false
    })
}

fn build_judge_prompt(spec: &Spec, doc: &str, lens: &str) -> String {
    format!(
        "# 과제\n제출된 SEO 콘텐츠를 채점 기준에 따라 심사하라.\n\n\
         ## 콘텐츠 유형: {name}\n{ctx}\n\n\
         ## 타깃 키워드\n{kw}\n\n\
         ## 이번 심사자의 관점\n{lens}\n\n\
         ## 채점 기준(각 항목 0~100점 정수)\n{rubric}\n\n\
         ## 점수대 기준\n{bands}\n\n\
         ## 절차\n\
         1. 문서를 채점하기 전에 winning_conditions에 '이 검색 의도에서 상위 노출될 콘텐츠의 조건'을 먼저 3~6개 적는다.\n\
         2. 그 다음 각 항목을 채점한다. 항목마다 evidence에 문서 원문을 직접 인용하고, why_not_higher에 더 높은 점수를 주지 않은 이유를 적는다.\n\
         3. 인용할 근거를 찾지 못하면 그 항목은 60점을 넘길 수 없다.\n\
         4. title/meta 글자수, 헤딩 계층, 링크 개수, alt 텍스트 등 형식·온페이지 요소는 별도 자동 검사에서 처리하므로 채점에 반영하지 말고 내용·설득력만 평가한다.\n\n\
         ## 채점 대상 문서(프론트매터 + 마크다운 본문)\n<document>\n{doc}\n</document>\n",
        name = spec.name,
        ctx = spec.context,
        kw = spec.keyword,
        lens = lens,
        rubric = spec.rubric_prompt(),
        bands = spec.bands_prompt(),
        doc = doc
    )
}

/// 절사평균. n>=4면 최소·최대 1개씩 제거 후 평균, 아니면 단순 평균.
/// (0~100 정수 다수 표본에서 중앙값은 tie를 과다 생성해 미세 개선을 감지하지 못한다)
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

/// 문서 1건 채점. rounds 회 반복하며 모델·관점을 순환한다.
pub fn score_doc(
    judges: &[Llm],
    spec: &Spec,
    label: &str,
    doc: &str,
    rounds: usize,
) -> Result<Scored> {
    anyhow::ensure!(!judges.is_empty(), "채점 모델 없음");
    let rounds = rounds.max(1);
    let schema = judge_schema(spec);

    // 라운드 병렬화: main.rs::par_map이 문서 간(N개 초안 간) 병렬화에 쓰는
    // std::thread::scope 패턴을 그대로 라운드 단위에 적용한다. `--concurrency`는
    // 이미 문서 단위 병렬 예산이므로, 여기서는 별도 옵션 없이 단순하게 라운드 수만큼만
    // 스레드를 스폰한다(§5 백로그: auto-seo의 asyncio.gather 패턴 참고, 단 예산 관리는
    // 미검증이라 단순 구현으로 제한). 결과는 인덱스 순서를 유지해 반환하므로
    // trimmed_mean 등 이후 집계 로직은 영향받지 않는다.
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
                        .with_context(|| format!("채점 실패 ({label}, round {})", i + 1))?;
                    let jr: JudgeResult = serde_json::from_value(v)
                        .with_context(|| format!("채점 결과 스키마 불일치 ({label})"))?;
                    Ok((jr, llm.label()))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("채점 스레드 패닉 ({label})"))))
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
        // citation_required 항목: 출처/인용 링크가 spec.min_citations 미만이면
        // 결정론적으로 60점 상한(bizplan-loop의 "인용 못하면 60점 상한"과 동일 발상이나,
        // 인용 개수는 마크다운 링크로 실측 가능하므로 프롬프트가 아니라 여기서 하드캡한다).
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

/// 재생성 프롬프트용 피드백. 점수 자체는 넘기지 않는다(점수 최적화 유인 억제).
pub fn feedback_text(s: &Scored) -> String {
    let mut out = String::from("[반드시 반영할 수정 지시]\n");
    for i in &s.improvements {
        out.push_str(&format!("- {}\n", i));
    }
    if !s.comments.is_empty() {
        out.push_str("\n[심사 총평]\n");
        for c in &s.comments {
            out.push_str(&format!("- {}\n", c));
        }
    }
    out
}

/// 가장 낮은 항목 2개.
pub fn weak_points(spec: &Spec, s: &Scored) -> String {
    let mut v: Vec<(&str, f64)> = spec
        .criteria
        .iter()
        .map(|c| (c.name.as_str(), s.per_criterion.get(&c.id).copied().unwrap_or(0.0)))
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
