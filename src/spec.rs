use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Spec {
    /// Content type name (e.g. "Product landing page", "How-to blog post")
    pub name: String,
    /// Site/brand/target-audience context. Inserted verbatim into the prompt.
    #[serde(default)]
    pub context: String,
    /// Target keyword (single primary keyword). Used to check placement in title/H1/intro.
    pub keyword: String,
    /// Note on scoring basis (internal guidelines, etc). Shown in the report.
    #[serde(default)]
    pub scoring_source: String,
    /// This site's domain (e.g. "example.com"). Used to determine internal vs external links. If empty,
    /// links that don't start with http(s):// are treated as internal links.
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
    /// Minimum number of source/citation (external authoritative site) links. Used for E-E-A-T checks and the 60-point cap decision.
    #[serde(default = "default_min_citations")]
    pub min_citations: usize,

    /// Content angles for generation diversity (how-to/listicle/comparison, etc).
    #[serde(default)]
    pub angles: Vec<String>,
    /// Score band descriptors (0-100). Uses defaults if unspecified.
    #[serde(default)]
    pub bands: Vec<String>,
    /// Recommended H2 outline (optional). If required=true, flagged as a format issue when missing.
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
    /// Weight. Normalized internally even if the sum isn't 1.
    pub weight: f64,
    #[serde(default)]
    pub guide: String,
    /// If true, forces this criterion's score to a 60-point cap when the body has fewer than
    /// `min_citations` source/citation links (deterministically enforced in code — same idea as
    /// bizplan-loop's "can't cite → capped at 60", but since citation count is measurable from
    /// markdown links here, it's implemented as a hard cap in score.rs rather than a prompt instruction).
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
    "90-100: Precisely matches search intent, keywords are woven in naturally, E-E-A-T signals (experience, sources) are sufficient, and structure is clear.",
    "75-89: Mostly matches, but some evidence is shallow or structure is somewhat unfocused.",
    "60-74: Meets the basics, but satisfies search intent only superficially and lacks evidence/sources.",
    "40-59: Mostly generic statements. Keywords are unnatural (over-optimized/misplaced) or there's little supporting evidence.",
    "0-39: Unrelated to search intent, or content is empty.",
];

impl Spec {
    pub fn load(path: &Path) -> Result<Spec> {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read spec file: {}", path.display()))?;
        let spec: Spec = toml::from_str(&s)
            .with_context(|| format!("Failed to parse spec TOML: {}", path.display()))?;
        anyhow::ensure!(!spec.keyword.trim().is_empty(), "keyword is empty");
        anyhow::ensure!(!spec.criteria.is_empty(), "criteria is empty");
        anyhow::ensure!(
            spec.criteria.iter().all(|c| c.weight > 0.0),
            "criteria weight must all be greater than 0"
        );
        anyhow::ensure!(
            spec.title_min <= spec.title_max,
            "title_min cannot be greater than title_max"
        );
        anyhow::ensure!(
            spec.meta_min <= spec.meta_max,
            "meta_min cannot be greater than meta_max"
        );
        anyhow::ensure!(
            spec.internal_links_min <= spec.internal_links_max,
            "internal_links_min cannot be greater than internal_links_max"
        );
        let mut ids: Vec<&str> = spec.criteria.iter().map(|c| c.id.as_str()).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        anyhow::ensure!(ids.len() == n, "duplicate criteria id");
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
            return "(No recommended outline specified — structure H2-H6 freely to match search intent)".to_string();
        }
        self.sections
            .iter()
            .map(|s| {
                let mut line = format!("## {}\n- Writing guide: {}", s.title, s.guide);
                if s.required {
                    line.push_str("\n- Required section");
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
                    "- id=\"{}\" | {} (weight {:.0}%) : {}{}",
                    c.id,
                    c.name,
                    c.weight / sum * 100.0,
                    c.guide,
                    if c.citation_required {
                        " [If the body has no source/citation links, this item is automatically capped at 60 points]"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp_toml(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("seo-loop-spec-test-{name}.toml"));
        std::fs::write(&path, contents).expect("write temp spec file");
        path
    }

    const VALID_MIN: &str = r#"
name = "Test content type"
keyword = "running shoes"
[[criteria]]
id = "x"
name = "X"
weight = 1.0
"#;

    #[test]
    fn load_handles_empty_file() {
        let path = write_temp_toml("empty-file", "");
        let err = Spec::load(&path).unwrap_err();
        assert!(format!("{err:#}").contains("Failed to parse"), "{err:#}");
    }

    #[test]
    fn load_rejects_empty_keyword() {
        let toml = r#"
name = "Test content type"
keyword = ""
[[criteria]]
id = "x"
name = "X"
weight = 1.0
"#;
        let path = write_temp_toml("empty-keyword", toml);
        let err = Spec::load(&path).unwrap_err();
        assert!(format!("{err:#}").contains("keyword is empty"), "{err:#}");
    }

    #[test]
    fn load_rejects_whitespace_only_keyword() {
        let toml = r#"
name = "Test content type"
keyword = "   "
[[criteria]]
id = "x"
name = "X"
weight = 1.0
"#;
        let path = write_temp_toml("whitespace-keyword", toml);
        let err = Spec::load(&path).unwrap_err();
        assert!(format!("{err:#}").contains("keyword is empty"), "{err:#}");
    }

    #[test]
    fn load_rejects_empty_criteria_list() {
        let toml = "name = \"Test content type\"\nkeyword = \"running shoes\"\ncriteria = []\n";
        let path = write_temp_toml("empty-criteria", toml);
        let err = Spec::load(&path).unwrap_err();
        assert!(format!("{err:#}").contains("criteria is empty"), "{err:#}");
    }

    #[test]
    fn load_rejects_zero_weight_criterion() {
        let toml = r#"
name = "Test content type"
keyword = "running shoes"
[[criteria]]
id = "x"
name = "X"
weight = 0.0
"#;
        let path = write_temp_toml("zero-weight", toml);
        let err = Spec::load(&path).unwrap_err();
        assert!(
            format!("{err:#}").contains("weight must all be greater than 0"),
            "{err:#}"
        );
    }

    #[test]
    fn load_rejects_duplicate_criteria_ids() {
        let toml = r#"
name = "Test content type"
keyword = "running shoes"
[[criteria]]
id = "x"
name = "X"
weight = 1.0
[[criteria]]
id = "x"
name = "Y"
weight = 1.0
"#;
        let path = write_temp_toml("dup-ids", toml);
        let err = Spec::load(&path).unwrap_err();
        assert!(
            format!("{err:#}").contains("duplicate criteria id"),
            "{err:#}"
        );
    }

    #[test]
    fn load_rejects_inverted_title_range() {
        // title_min/title_max must come before [[criteria]] — TOML would otherwise attach them
        // as (ignored) extra fields on the last table-array entry instead of top-level keys.
        let toml = r#"
name = "Test content type"
keyword = "running shoes"
title_min = 60
title_max = 50
[[criteria]]
id = "x"
name = "X"
weight = 1.0
"#;
        let path = write_temp_toml("inverted-title", toml);
        let err = Spec::load(&path).unwrap_err();
        assert!(
            format!("{err:#}").contains("title_min cannot be greater than title_max"),
            "{err:#}"
        );
    }

    #[test]
    fn load_accepts_minimal_valid_spec_and_fills_defaults() {
        let path = write_temp_toml("valid-minimal", VALID_MIN);
        let spec = Spec::load(&path).expect("minimal spec should load");
        assert_eq!(spec.keyword, "running shoes");
        assert_eq!(spec.title_min, 50);
        assert_eq!(spec.title_max, 60);
        assert!(spec.sections.is_empty());
        assert!(spec.angles.is_empty());
    }

    #[test]
    fn sections_prompt_handles_empty_sections() {
        let spec_toml = write_temp_toml("sections-prompt-empty", VALID_MIN);
        let spec = Spec::load(&spec_toml).unwrap();
        assert!(spec.sections_prompt().contains("No recommended outline"));
    }

    #[test]
    fn rubric_prompt_handles_single_criterion() {
        let spec_toml = write_temp_toml("rubric-prompt-single", VALID_MIN);
        let spec = Spec::load(&spec_toml).unwrap();
        assert!(spec.rubric_prompt().contains("weight 100%"));
    }
}
