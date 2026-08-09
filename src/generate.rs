use crate::spec::Spec;
use anyhow::Result;
use crate::llm::Llm;

pub const SYSTEM: &str = "You are an expert SEO copywriter. You write blog posts and \
landing page copy that precisely match search intent, naturally weave in keywords \
(without over-optimization), and carry E-E-A-T signals (first-person experience, \
concrete figures, sources). You do not exaggerate unverified figures or claims of \
effectiveness, and you attach source links to claims that have supporting evidence.";

/// Initial generation prompt.
pub fn build_prompt(spec: &Spec, brief: &str, angle: &str) -> String {
    let mut p = String::new();
    p.push_str("# Task\nWrite an SEO content draft in Korean according to the conditions below.\n\n");
    p.push_str(&format!("## Content type: {}\n{}\n\n", spec.name, spec.context));
    p.push_str(&format!("## Target keyword\n{}\n\n", spec.keyword));
    if !angle.is_empty() {
        p.push_str(&format!("## Content angle for this draft\n{}\n\n", angle));
    }
    p.push_str(&format!("## Content brief (source material)\n{}\n\n", brief));
    p.push_str(&format!("## Recommended outline (H2+)\n{}\n\n", spec.sections_prompt()));
    p.push_str(&format!("## Scoring criteria (keep these in mind while writing)\n{}\n\n", spec.rubric_prompt()));
    p.push_str(&format!(
        "## Output format (must follow)\n\
         - Put front matter at the very top of the document:\n\
           ```\n\
           ---\n\
           title: \"a title between {title_min}~{title_max} characters\"\n\
           meta_description: \"a meta description between {meta_min}~{meta_max} characters\"\n\
           ---\n\
           ```\n\
         - Markdown body below that. Use exactly one `# H1`, and don't skip heading levels from H2 to H6 (no jumping straight from H1 to H3).\n\
         - Place the target keyword naturally at least once each in the title, the H1, and within the first 100 characters of the intro.\n\
         - If you use images, use the `![alt text](url)` format and always fill in the alt text (no empty alt text).\n\
         - Place {links_min}~{links_max} internal links (in `[text](/path)` form).\n\
         - Place at least {min_citations} external authoritative links for sourcing/citation purposes (in `[text](https://...)` form, from official bodies, research, or primary sources) — this is an E-E-A-T signal, and without it the related scoring item is capped at 60 points.\n\
         - Output only the document body, with no intro, explanation, or meta-commentary.\n\
         - If a fact is uncertain, don't make it up — replace it with 'estimated' or a source link.\n",
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

/// Regeneration prompt that incorporates scoring feedback.
pub fn build_revise_prompt(
    spec: &Spec,
    brief: &str,
    prev_doc: &str,
    feedback: &str,
    weak: &str,
) -> String {
    let mut p = String::new();
    p.push_str("# Task\nImprove the SEO content draft below according to the review feedback and output the entire revised document again.\n\n");
    p.push_str(&format!("## Content type: {}\n{}\n\n", spec.name, spec.context));
    p.push_str(&format!("## Target keyword\n{}\n\n", spec.keyword));
    p.push_str(&format!("## Content brief (source material)\n{}\n\n", brief));
    p.push_str(&format!("## Current draft\n{}\n\n", prev_doc));
    p.push_str(&format!("## Review feedback (must be addressed)\n{}\n\n", feedback));
    if !weak.is_empty() {
        p.push_str(&format!("## Items with especially low scores\n{}\n\n", weak));
    }
    p.push_str(&format!("## Scoring criteria\n{}\n\n", spec.rubric_prompt()));
    p.push_str(
        "## Output rules\n\
         - Output the entire improved document (including front matter) as markdown. No change summaries or meta-commentary.\n\
         - Keep following the rules for front matter title/meta_description character-count ranges, exactly one H1, heading hierarchy, keyword placement, alt text, \
           and internal/source link counts.\n\
         - Keep the parts that are well-written, and substantively strengthen only the parts that were flagged.\n\
         - Don't fabricate new unsupported figures. If you can't back up a claim, remove it or scale it back to 'estimated'.\n\
         - Don't respond by padding the length. Keep the overall length within ±15% of the current draft, improving by replacing weak sentences.\n",
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

/// If angles are insufficient, fill with default angles and return n of them.
pub fn angles_for(spec: &Spec, n: usize) -> Vec<String> {
    let defaults = [
        "Foreground the execution steps in a step-by-step how-to guide format.",
        "Make it easy to skim by listing comparison items in a listicle format.",
        "Foreground the selection criteria in a comparison/alternatives (vs) format.",
        "Foreground resolving specific questions in an FAQ structure.",
        "Foreground credibility with real case studies and data.",
        "Structure it around concept explanations that even beginners can understand.",
    ];
    let pool: Vec<String> = if spec.angles.is_empty() {
        defaults.iter().map(|s| s.to_string()).collect()
    } else {
        spec.angles.clone()
    };
    (0..n).map(|i| pool[i % pool.len()].clone()).collect()
}
