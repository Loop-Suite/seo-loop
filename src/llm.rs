use anyhow::{anyhow, Context, Result};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// 누적 비용(마이크로달러). claude 응답의 total_cost_usd 합산.
static COST_MICROS: AtomicU64 = AtomicU64::new(0);

pub fn total_cost_usd() -> f64 {
    COST_MICROS.load(Ordering::Relaxed) as f64 / 1_000_000.0
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub text: String,
    /// --json-schema 사용 시 검증된 구조화 출력
    pub structured: Option<serde_json::Value>,
}

/// Claude Code CLI(`claude -p`)를 서브프로세스로 호출하는 백엔드.
///
/// 기본으로 다음을 적용한다(모두 `claude --help` 실측 확인):
/// - `--safe-mode`      : CLAUDE.md / 스킬 / 플러그인 / 훅 / MCP 비활성 (인증은 정상 동작)
/// - `--tools ""`       : 내장 도구 전면 비활성 → 순수 텍스트 생성, 파일 접근 없음
/// - `--no-session-persistence` : 세션 파일 미생성 (병렬 실행 시 경합 회피)
/// - `--output-format json`     : result / structured_output / total_cost_usd 수집
///
/// `--bare`는 쓰지 않는다. OAuth/키체인을 읽지 않아 구독 로그인 사용자의 인증이 깨진다.
#[derive(Clone, Debug)]
pub struct Llm {
    pub bin: String,
    pub model: Option<String>,
    pub retries: u32,
    pub verbose: bool,
    pub timeout: Duration,
    pub max_budget_usd: Option<f64>,
    /// true면 --safe-mode를 빼고 실행 디렉터리의 CLAUDE.md 등을 로드
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

    fn call_once(
        &self,
        prompt: &str,
        system: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Reply> {
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
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("`{}` 실행 실패 (설치 및 PATH 확인)", self.bin))?;

        // stdin 쓰기와 stdout/stderr 읽기를 동시에 진행해야 파이프 버퍼 포화로 인한 교착을 피한다.
        let mut sin = child.stdin.take().ok_or_else(|| anyhow!("stdin 열기 실패"))?;
        let payload = prompt.to_string();
        let t_in = std::thread::spawn(move || {
            let _ = sin.write_all(payload.as_bytes());
            // drop(sin) → EOF
        });
        let mut sout = child.stdout.take().ok_or_else(|| anyhow!("stdout 열기 실패"))?;
        let t_out = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = sout.read_to_string(&mut s);
            s
        });
        let mut serr = child.stderr.take().ok_or_else(|| anyhow!("stderr 열기 실패"))?;
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
                        return Err(anyhow!("타임아웃 {}초 초과", self.timeout.as_secs()));
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
                "claude 종료코드 {:?}: {}",
                status.code(),
                truncate(stderr.trim(), 300)
            ));
        }

        let v: serde_json::Value = serde_json::from_str(stdout.trim())
            .with_context(|| format!("claude JSON 출력 파싱 실패: {}", truncate(&stdout, 300)))?;
        if v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false) {
            return Err(anyhow!(
                "claude 에러 응답(subtype={}): {}",
                v.get("subtype").and_then(|s| s.as_str()).unwrap_or("?"),
                truncate(v.get("result").and_then(|r| r.as_str()).unwrap_or(""), 300)
            ));
        }
        let cost = v.get("total_cost_usd").and_then(|c| c.as_f64()).unwrap_or(0.0);
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
            return Err(anyhow!("빈 응답"));
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
                            "[{} 재시도 {}/{}] {}",
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
        Err(last.unwrap_or_else(|| anyhow!("알 수 없는 실패")))
    }

    /// 텍스트 생성.
    pub fn text(&self, prompt: &str, system: Option<&str>) -> Result<String> {
        let r = self.with_retry("generate", || self.call_once(prompt, system, None))?;
        Ok(r.text)
    }

    /// JSON Schema를 강제한 구조화 생성. 스키마 검증은 claude CLI가 수행하고
    /// 결과는 응답의 structured_output 필드로 온다. 없으면 본문에서 추출 시도.
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

/// 코드펜스/잡설이 섞인 응답에서 JSON 오브젝트만 추출(폴백 경로).
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
    Err(anyhow!("JSON 추출 실패: {}", truncate(t, 300)))
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}
