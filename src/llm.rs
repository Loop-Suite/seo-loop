use anyhow::{anyhow, Context, Result};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Accumulated cost (micro-dollars). Sums total_cost_usd from claude responses.
static COST_MICROS: AtomicU64 = AtomicU64::new(0);

pub fn total_cost_usd() -> f64 {
    COST_MICROS.load(Ordering::Relaxed) as f64 / 1_000_000.0
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub text: String,
    /// Validated structured output when --json-schema is used
    pub structured: Option<serde_json::Value>,
}

/// Backend that invokes the Claude Code CLI (`claude -p`) as a subprocess.
///
/// Applies the following by default (all verified against `claude --help`):
/// - `--safe-mode`      : disables CLAUDE.md / skills / plugins / hooks / MCP (auth still works normally)
/// - `--tools ""`       : disables all built-in tools → pure text generation, no file access
/// - `--no-session-persistence` : no session file is created (avoids contention during parallel runs)
/// - `--output-format json`     : collects result / structured_output / total_cost_usd
///
/// `--bare` is not used. It doesn't read OAuth/keychain, which breaks auth for subscription-login users.
#[derive(Clone, Debug)]
pub struct Llm {
    pub bin: String,
    pub model: Option<String>,
    pub retries: u32,
    pub verbose: bool,
    pub timeout: Duration,
    pub max_budget_usd: Option<f64>,
    /// If true, omits --safe-mode and loads CLAUDE.md etc. from the execution directory
    pub load_context: bool,
}

impl Llm {
    pub fn new(bin: String, model: Option<String>) -> Self {
        Llm {
            bin,
            model,
            retries: 2,
            verbose: false,
            timeout: Duration::from_secs(600),
            max_budget_usd: None,
            load_context: false,
        }
    }

    pub fn label(&self) -> String {
        self.model.clone().unwrap_or_else(|| "default".to_string())
    }

    fn call_once(&self, prompt: &str, system: Option<&str>, schema: Option<&str>) -> Result<Reply> {
        let mut cmd = Command::new(&self.bin);
        cmd.arg("-p").arg("--output-format").arg("json");
        if !self.load_context {
            cmd.arg("--safe-mode");
        }
        cmd.arg("--no-session-persistence");
        cmd.arg("--tools").arg("");
        if let Some(m) = &self.model {
            cmd.arg("--model").arg(m);
        }
        if let Some(b) = self.max_budget_usd {
            cmd.arg("--max-budget-usd").arg(format!("{b}"));
        }
        if let Some(s) = system {
            cmd.arg("--append-system-prompt").arg(s);
        }
        if let Some(js) = schema {
            cmd.arg("--json-schema").arg(js);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().with_context(|| {
            format!("Failed to run `{}` (check installation and PATH)", self.bin)
        })?;

        // Writing stdin and reading stdout/stderr must happen concurrently to avoid deadlock from pipe buffer saturation.
        let mut sin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to open stdin"))?;
        let payload = prompt.to_string();
        let t_in = std::thread::spawn(move || {
            let _ = sin.write_all(payload.as_bytes());
            // drop(sin) → EOF
        });
        let mut sout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to open stdout"))?;
        let t_out = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = sout.read_to_string(&mut s);
            s
        });
        let mut serr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Failed to open stderr"))?;
        let t_err = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = serr.read_to_string(&mut s);
            s
        });

        let started = Instant::now();
        let status = loop {
            match child.try_wait()? {
                Some(st) => break st,
                None => {
                    if started.elapsed() > self.timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(anyhow!(
                            "Timeout exceeded {} seconds",
                            self.timeout.as_secs()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(150));
                }
            }
        };
        let _ = t_in.join();
        let stdout = t_out.join().unwrap_or_default();
        let stderr = t_err.join().unwrap_or_default();

        if !status.success() {
            return Err(anyhow!(
                "claude exited with code {:?}: {}",
                status.code(),
                truncate(stderr.trim(), 300)
            ));
        }

        let v: serde_json::Value = serde_json::from_str(stdout.trim()).with_context(|| {
            format!(
                "Failed to parse claude JSON output: {}",
                truncate(&stdout, 300)
            )
        })?;
        if v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false) {
            return Err(anyhow!(
                "claude error response (subtype={}): {}",
                v.get("subtype").and_then(|s| s.as_str()).unwrap_or("?"),
                truncate(v.get("result").and_then(|r| r.as_str()).unwrap_or(""), 300)
            ));
        }
        let cost = v
            .get("total_cost_usd")
            .and_then(|c| c.as_f64())
            .unwrap_or(0.0);
        if cost > 0.0 {
            COST_MICROS.fetch_add((cost * 1_000_000.0) as u64, Ordering::Relaxed);
        }
        let text = v
            .get("result")
            .and_then(|r| r.as_str())
            .unwrap_or_default()
            .to_string();
        let structured = v.get("structured_output").cloned().filter(|s| !s.is_null());
        if text.trim().is_empty() && structured.is_none() {
            return Err(anyhow!("Empty response"));
        }
        Ok(Reply { text, structured })
    }

    fn with_retry<T>(&self, what: &str, mut f: impl FnMut() -> Result<T>) -> Result<T> {
        let mut last: Option<anyhow::Error> = None;
        for attempt in 0..=self.retries {
            match f() {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if self.verbose {
                        eprintln!(
                            "[{} retry {}/{}] {}",
                            what,
                            attempt + 1,
                            self.retries,
                            truncate(&format!("{e:#}"), 200)
                        );
                    }
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("Unknown failure")))
    }

    /// Generate text.
    pub fn text(&self, prompt: &str, system: Option<&str>) -> Result<String> {
        let r = self.with_retry("generate", || self.call_once(prompt, system, None))?;
        Ok(strip_wrapping_fence(&r.text))
    }

    /// Structured generation enforcing a JSON Schema. Schema validation is performed by the claude CLI, and
    /// the result comes back in the response's structured_output field. If absent, attempts extraction from the body.
    pub fn json(
        &self,
        prompt: &str,
        system: Option<&str>,
        schema: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let schema_str = schema.to_string();
        self.with_retry("judge", || {
            let r = self.call_once(prompt, system, Some(&schema_str))?;
            match r.structured {
                Some(v) => Ok(v),
                None => extract_json(&r.text),
            }
        })
    }
}

/// Strips a stray outer ```/```lang ... ``` wrapper around an entire text response, if present.
///
/// Some models wrap the whole document (frontmatter + body) in a single code fence despite being
/// told to output only the document body — possibly primed by build_prompt()'s own frontmatter
/// example, which is itself shown inside a ``` fence. Left unstripped, this breaks
/// checks::parse_front_matter() (which requires the document to start with "---") and
/// checks::readability() (whose strip_markdown_to_prose() treats the whole body as one open code
/// fence, since it never sees a closing fence to pair with the opening one, and drops all of it).
///
/// Mirrors the tolerance extract_json() already has for the JSON path (fixes #9). Deliberately
/// conservative: only strips when the *entire* trimmed response is bounded by a single matching
/// fence pair (first line is a run of 3+ backticks, optionally followed by a language tag; last
/// line is a run of backticks at least as long), so a fence that legitimately appears inside real
/// content (e.g. a code sample partway through the doc) is left alone.
///
/// The fence length is deliberately not hardcoded to exactly 3 backticks (fixes #15): per
/// CommonMark, a fence that needs to wrap content which itself already contains a ``` code block
/// uses 4+ backticks so the two don't collide — a model asked to output a document containing a
/// real code sample, while also (still) wrapping its whole answer in an outer fence, would
/// sensibly reach for this. The old exactly-3-backtick check silently failed to strip that case,
/// reproducing the exact #9 breakage in precisely the "fence within fence" scenario the fix was
/// meant to cover.
fn strip_wrapping_fence(text: &str) -> String {
    let trimmed = text.trim();
    let mut lines = trimmed.lines();
    let Some(first) = lines.next() else {
        return text.to_string();
    };
    let first = first.trim();
    let open_len = first.chars().take_while(|&c| c == '`').count();
    let is_open_fence =
        open_len >= 3 && first[open_len..].chars().all(|c| c.is_ascii_alphanumeric());
    if !is_open_fence {
        return text.to_string();
    }
    let rest: Vec<&str> = lines.collect();
    let Some(last) = rest.last() else {
        return text.to_string();
    };
    let last = last.trim();
    let close_len = last.chars().take_while(|&c| c == '`').count();
    // A valid closer is backticks only (nothing else on the line) and at least as long as the
    // opener — matching CommonMark's own fence-closing rule.
    let is_close_fence = close_len >= open_len && close_len == last.len();
    if !is_close_fence {
        return text.to_string();
    }
    rest[..rest.len() - 1].join("\n")
}

/// Extracts only the JSON object from a response mixed with code fences/chatter (fallback path).
pub fn extract_json(raw: &str) -> Result<serde_json::Value> {
    let t = raw.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
        return Ok(v);
    }
    if let Some(start) = t.find("```") {
        let after = &t[start + 3..];
        let after = after.strip_prefix("json").unwrap_or(after);
        if let Some(end) = after.find("```") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(after[..end].trim()) {
                return Ok(v);
            }
        }
    }
    if let (Some(s), Some(e)) = (t.find('{'), t.rfind('}')) {
        if s < e {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t[s..=e]) {
                return Ok(v);
            }
        }
    }
    Err(anyhow!("Failed to extract JSON: {}", truncate(t, 300)))
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_wrapping_fence_removes_plain_fence() {
        let raw = "```\n---\ntitle: \"x\"\n---\n\n# H1\n\nbody\n```";
        assert_eq!(
            strip_wrapping_fence(raw),
            "---\ntitle: \"x\"\n---\n\n# H1\n\nbody"
        );
    }

    #[test]
    fn strip_wrapping_fence_removes_language_tagged_fence() {
        let raw = "```markdown\n---\ntitle: \"x\"\n---\n\nbody\n```";
        assert_eq!(strip_wrapping_fence(raw), "---\ntitle: \"x\"\n---\n\nbody");
    }

    #[test]
    fn strip_wrapping_fence_leaves_untouched_when_not_wrapped() {
        let raw = "---\ntitle: \"x\"\n---\n\nbody with a ```code``` span";
        assert_eq!(strip_wrapping_fence(raw), raw);
    }

    #[test]
    fn strip_wrapping_fence_leaves_untouched_when_only_opening_fence() {
        // Unclosed fence: don't guess, leave as-is (front_matter_unclosed etc. handle it elsewhere).
        let raw = "```\n---\ntitle: \"x\"\n---\n\nbody";
        assert_eq!(strip_wrapping_fence(raw), raw);
    }

    #[test]
    fn strip_wrapping_fence_leaves_untouched_when_internal_fence_present() {
        // A real embedded code block inside otherwise unwrapped content must survive untouched.
        let raw = "---\ntitle: \"x\"\n---\n\nExample:\n```rust\nfn main() {}\n```\n\nmore text";
        assert_eq!(strip_wrapping_fence(raw), raw);
    }

    #[test]
    fn strip_wrapping_fence_removes_four_backtick_wrapper_around_nested_fence() {
        // Regression test for #15: a model that wraps its whole answer while the body itself
        // legitimately contains a real ``` code sample would sensibly (per CommonMark) use 4+
        // backticks for the outer wrapper so it doesn't collide with the inner one. The old
        // exactly-3-backtick check silently failed to strip this, reproducing #9's breakage.
        let raw = "````\n---\ntitle: \"x\"\n---\n\nExample:\n```rust\nfn main() {}\n```\n\nmore text\n````";
        assert_eq!(
            strip_wrapping_fence(raw),
            "---\ntitle: \"x\"\n---\n\nExample:\n```rust\nfn main() {}\n```\n\nmore text"
        );
    }

    #[test]
    fn strip_wrapping_fence_removes_four_backtick_wrapper_with_language_tag() {
        let raw = "````markdown\n---\ntitle: \"x\"\n---\n\nbody\n````";
        assert_eq!(strip_wrapping_fence(raw), "---\ntitle: \"x\"\n---\n\nbody");
    }

    #[test]
    fn strip_wrapping_fence_leaves_untouched_when_closer_is_shorter_than_opener() {
        // A closer with fewer backticks than the opener doesn't actually close the fence per
        // CommonMark — must not be treated as a valid wrapper boundary.
        let raw = "````\n---\ntitle: \"x\"\n---\n\nbody\n```";
        assert_eq!(strip_wrapping_fence(raw), raw);
    }

    #[test]
    fn strip_wrapping_fence_accepts_longer_closer_than_opener() {
        // A closer with more backticks than the opener is still a valid close per CommonMark.
        let raw = "```\n---\ntitle: \"x\"\n---\n\nbody\n`````";
        assert_eq!(strip_wrapping_fence(raw), "---\ntitle: \"x\"\n---\n\nbody");
    }
}
