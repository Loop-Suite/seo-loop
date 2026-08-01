use crate::spec::Spec;
use anyhow::Result;
use crate::llm::Llm;

pub const SYSTEM: &str = "당신은 SEO 카피라이터 전문가다. 검색 의도에 정확히 부합하고, \
키워드를 자연스럽게(과최적화 없이) 녹여 넣으며, E-E-A-T 신호(1인칭 경험, 구체적 수치, 출처)를 \
담은 블로그 글·랜딩페이지 카피를 작성한다. 확인되지 않은 수치나 효능은 과장하지 않고, \
근거가 있는 주장에는 출처 링크를 붙인다.";

/// 최초 생성 프롬프트.
pub fn build_prompt(spec: &Spec, brief: &str, angle: &str) -> String {
    let mut p = String::new();
    p.push_str("# 과제\n아래 조건에 맞춰 SEO 콘텐츠 초안을 한국어로 작성하라.\n\n");
    p.push_str(&format!("## 콘텐츠 유형: {}\n{}\n\n", spec.name, spec.context));
    p.push_str(&format!("## 타깃 키워드\n{}\n\n", spec.keyword));
    if !angle.is_empty() {
        p.push_str(&format!("## 이번 초안의 콘텐츠 각도\n{}\n\n", angle));
    }
    p.push_str(&format!("## 콘텐츠 브리프(원본 자료)\n{}\n\n", brief));
    p.push_str(&format!("## 권장 아웃라인(H2 이상)\n{}\n\n", spec.sections_prompt()));
    p.push_str(&format!("## 채점 기준(작성 시 반드시 의식할 것)\n{}\n\n", spec.rubric_prompt()));
    p.push_str(&format!(
        "## 출력 형식(반드시 지킬 것)\n\
         - 문서 맨 위에 프론트매터를 넣는다:\n\
           ```\n\
           ---\n\
           title: \"{title_min}~{title_max}자 사이의 title\"\n\
           meta_description: \"{meta_min}~{meta_max}자 사이의 meta description\"\n\
           ---\n\
           ```\n\
         - 그 아래 마크다운 본문. `# H1`을 정확히 1개 사용하고, H2~H6은 레벨을 건너뛰지 말 것(H1 다음 바로 H3 금지).\n\
         - title, H1, 그리고 도입부 첫 100자 이내에 타깃 키워드를 각각 최소 1회 자연스럽게 배치할 것.\n\
         - 이미지를 쓸 경우 `![대체텍스트](url)` 형식으로, alt 텍스트를 반드시 채울 것(빈 대체텍스트 금지).\n\
         - 내부링크(`[텍스트](/경로)` 형태)를 {links_min}~{links_max}개 배치할 것.\n\
         - 출처/인용 목적의 외부 권위 링크(`[텍스트](https://...)` 형태, 공식 기관·연구·1차 자료)를 최소 {min_citations}개 배치할 것 — E-E-A-T 신호이며 없으면 관련 채점 항목이 60점 상한 처리된다.\n\
         - 서론·설명·메타코멘트 없이 문서 본문만 출력.\n\
         - 사실이 불확실하면 지어내지 말고 '추정' 또는 출처 링크로 대체.\n",
        title_min = spec.title_min,
        title_max = spec.title_max,
        meta_min = spec.meta_min,
        meta_max = spec.meta_max,
        links_min = spec.internal_links_min,
        links_max = spec.internal_links_max,
        min_citations = spec.min_citations,
    ));
    p
}

/// 채점 피드백 반영 재생성 프롬프트.
pub fn build_revise_prompt(
    spec: &Spec,
    brief: &str,
    prev_doc: &str,
    feedback: &str,
    weak: &str,
) -> String {
    let mut p = String::new();
    p.push_str("# 과제\n아래 SEO 콘텐츠 초안을 심사 피드백에 따라 개선하여 전체를 다시 출력하라.\n\n");
    p.push_str(&format!("## 콘텐츠 유형: {}\n{}\n\n", spec.name, spec.context));
    p.push_str(&format!("## 타깃 키워드\n{}\n\n", spec.keyword));
    p.push_str(&format!("## 콘텐츠 브리프(원본 자료)\n{}\n\n", brief));
    p.push_str(&format!("## 현재 초안\n{}\n\n", prev_doc));
    p.push_str(&format!("## 심사 피드백(반드시 반영)\n{}\n\n", feedback));
    if !weak.is_empty() {
        p.push_str(&format!("## 특히 점수가 낮은 항목\n{}\n\n", weak));
    }
    p.push_str(&format!("## 채점 기준\n{}\n\n", spec.rubric_prompt()));
    p.push_str(
        "## 출력 규칙\n\
         - 개선된 문서 전체(프론트매터 포함)를 마크다운으로 출력. 변경 요약이나 메타코멘트 금지.\n\
         - 프론트매터의 title/meta_description 글자수 범위, H1 1개, 헤딩 계층, 키워드 배치, alt 텍스트, \
           내부링크·출처링크 개수 규칙은 계속 지킬 것.\n\
         - 잘 작성된 부분은 유지하고, 지적된 부분만 실질적으로 보강.\n\
         - 근거 없는 수치를 새로 지어내지 말 것. 근거를 못 대면 그 주장을 삭제하거나 '추정'으로 축소.\n\
         - 분량을 늘려서 대응하지 말 것. 전체 길이는 현재 초안 대비 ±15% 이내로 유지하고, 약한 문장을 교체하는 방식으로 개선.\n",
    );
    p
}

pub fn generate(llm: &Llm, spec: &Spec, brief: &str, angle: &str) -> Result<String> {
    let prompt = build_prompt(spec, brief, angle);
    llm.text(&prompt, Some(SYSTEM))
}

pub fn revise(
    llm: &Llm,
    spec: &Spec,
    brief: &str,
    prev_doc: &str,
    feedback: &str,
    weak: &str,
) -> Result<String> {
    let prompt = build_revise_prompt(spec, brief, prev_doc, feedback, weak);
    llm.text(&prompt, Some(SYSTEM))
}

/// angles가 부족하면 기본 각도로 채워 n개 반환.
pub fn angles_for(spec: &Spec, n: usize) -> Vec<String> {
    let defaults = [
        "단계별 how-to 가이드 형식으로 실행 순서를 전면에 세운다.",
        "리스트형(listicle)으로 비교 항목을 나열해 훑어보기 쉽게 만든다.",
        "비교/대안(vs) 형식으로 선택 기준을 전면에 세운다.",
        "자주 묻는 질문(FAQ) 구조로 구체적 궁금증 해소를 전면에 세운다.",
        "실제 사례·데이터 중심으로 신뢰도를 전면에 세운다.",
        "초보자도 이해할 수 있는 개념 설명 중심으로 구성한다.",
    ];
    let pool: Vec<String> = if spec.angles.is_empty() {
        defaults.iter().map(|s| s.to_string()).collect()
    } else {
        spec.angles.clone()
    };
    (0..n).map(|i| pool[i % pool.len()].clone()).collect()
}
