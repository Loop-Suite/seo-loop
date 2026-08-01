use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Spec {
    /// 콘텐츠 유형 이름 (예: "제품 랜딩페이지", "How-to 블로그 글")
    pub name: String,
    /// 사이트/브랜드/타깃 독자 맥락. 프롬프트에 그대로 삽입됨.
    #[serde(default)]
    pub context: String,
    /// 타깃 키워드(주 키워드 1개). title/H1/도입부 배치 검사에 사용.
    pub keyword: String,
    /// 배점 근거 메모(내부 가이드라인 등). 리포트에 표시.
    #[serde(default)]
    pub scoring_source: String,
    /// 이 사이트의 도메인(예: "example.com"). 내부/외부 링크 판정에 사용. 비우면
    /// http(s):// 로 시작하지 않는 링크를 내부 링크로 간주.
    #[serde(default)]
    pub site_domain: String,

    #[serde(default = "default_title_min")]
    pub title_min: usize,
    #[serde(default = "default_title_max")]
    pub title_max: usize,
    #[serde(default = "default_meta_min")]
    pub meta_min: usize,
    #[serde(default = "default_meta_max")]
    pub meta_max: usize,
    #[serde(default = "default_links_min")]
    pub internal_links_min: usize,
    #[serde(default = "default_links_max")]
    pub internal_links_max: usize,
    /// 출처/인용(외부 권위 사이트) 링크 최소 개수. E-E-A-T 검사 및 60점 상한 판정에 사용.
    #[serde(default = "default_min_citations")]
    pub min_citations: usize,

    /// 생성 다양성을 위한 콘텐츠 각도(how-to/listicle/비교 등).
    #[serde(default)]
    pub angles: Vec<String>,
    /// 점수대 서술자(0~100). 미지정 시 기본값 사용.
    #[serde(default)]
    pub bands: Vec<String>,
    /// 권장 H2 아웃라인(선택). required=true면 누락 시 형식 지적.
    #[serde(default)]
    pub sections: Vec<Section>,
    pub criteria: Vec<Criterion>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Section {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub guide: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Criterion {
    pub id: String,
    pub name: String,
    /// 가중치. 합이 1이 아니어도 내부에서 정규화.
    pub weight: f64,
    #[serde(default)]
    pub guide: String,
    /// true면 본문에 출처/인용 링크가 `min_citations` 미만일 때 이 항목 점수를 60점으로
    /// 강제 상한한다(코드로 결정론적 적용 — bizplan-loop의 "인용 못하면 60점 상한"과
    /// 같은 발상이나, 여기서는 인용 개수를 마크다운 링크로 실측 가능하므로 프롬프트
    /// 지시가 아니라 score.rs에서 하드캡으로 구현했다).
    #[serde(default)]
    pub citation_required: bool,
}

fn default_title_min() -> usize {
    50
}
fn default_title_max() -> usize {
    60
}
fn default_meta_min() -> usize {
    120
}
fn default_meta_max() -> usize {
    160
}
fn default_links_min() -> usize {
    3
}
fn default_links_max() -> usize {
    5
}
fn default_min_citations() -> usize {
    1
}

pub const DEFAULT_BANDS: &[&str] = &[
    "90~100: 검색 의도에 정확히 부합하고, 키워드가 자연스럽게 녹아 있으며, E-E-A-T 신호(경험·출처)가 충분하고 구조가 명확함.",
    "75~89: 대체로 부합하나 일부 근거가 얕거나 구조가 약간 산만함.",
    "60~74: 기본은 갖췄으나 검색 의도 충족이 피상적이고 근거·출처가 부족함.",
    "40~59: 일반론 위주. 키워드가 부자연스럽거나(과최적화/미배치) 근거가 거의 없음.",
    "0~39: 검색 의도와 무관하거나 내용이 비어 있음.",
];

impl Spec {
    pub fn load(path: &Path) -> Result<Spec> {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("스펙 파일 읽기 실패: {}", path.display()))?;
        let spec: Spec = toml::from_str(&s)
            .with_context(|| format!("스펙 TOML 파싱 실패: {}", path.display()))?;
        anyhow::ensure!(!spec.keyword.trim().is_empty(), "keyword 비어 있음");
        anyhow::ensure!(!spec.criteria.is_empty(), "criteria 비어 있음");
        anyhow::ensure!(
            spec.criteria.iter().all(|c| c.weight > 0.0),
            "criteria weight는 모두 0보다 커야 함"
        );
        anyhow::ensure!(
            spec.title_min <= spec.title_max,
            "title_min은 title_max보다 클 수 없음"
        );
        anyhow::ensure!(
            spec.meta_min <= spec.meta_max,
            "meta_min은 meta_max보다 클 수 없음"
        );
        anyhow::ensure!(
            spec.internal_links_min <= spec.internal_links_max,
            "internal_links_min은 internal_links_max보다 클 수 없음"
        );
        let mut ids: Vec<&str> = spec.criteria.iter().map(|c| c.id.as_str()).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        anyhow::ensure!(ids.len() == n, "criteria id 중복");
        Ok(spec)
    }

    pub fn weight_sum(&self) -> f64 {
        self.criteria.iter().map(|c| c.weight).sum()
    }

    pub fn bands_prompt(&self) -> String {
        if self.bands.is_empty() {
            DEFAULT_BANDS.join("\n")
        } else {
            self.bands.join("\n")
        }
    }

    pub fn sections_prompt(&self) -> String {
        if self.sections.is_empty() {
            return "(권장 아웃라인 미지정 — 검색 의도에 맞춰 자유롭게 H2~H6 구성)".to_string();
        }
        self.sections
            .iter()
            .map(|s| {
                let mut line = format!("## {}\n- 작성지침: {}", s.title, s.guide);
                if s.required {
                    line.push_str("\n- 필수 섹션");
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    pub fn rubric_prompt(&self) -> String {
        let sum = self.weight_sum();
        self.criteria
            .iter()
            .map(|c| {
                format!(
                    "- id=\"{}\" | {} (배점 비중 {:.0}%) : {}{}",
                    c.id,
                    c.name,
                    c.weight / sum * 100.0,
                    c.guide,
                    if c.citation_required {
                        " [본문에 출처/인용 링크가 없으면 이 항목은 자동으로 60점 상한]"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
