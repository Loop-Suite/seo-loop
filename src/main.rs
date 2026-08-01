mod checks;
mod generate;
mod llm;
mod loop_run;
mod report;
mod score;
mod spec;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use llm::Llm;
use score::Scored;
use spec::Spec;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "seo",
    version,
    about = "Claude Code(`claude -p`)를 백엔드로 SEO 콘텐츠(블로그 글/랜딩페이지 카피)를 생성하고 루브릭으로 채점하는 CLI"
)]
struct Cli {
    /// claude 실행 파일 경로
    #[arg(long, default_value = "claude", global = true)]
    claude_bin: String,
    /// 생성 모델 (opus/sonnet/haiku/fable 또는 전체 모델 ID)
    #[arg(long, global = true)]
    model: Option<String>,
    /// 채점 모델. 쉼표로 여러 개 지정하면 패널로 순환 사용 (예: sonnet,haiku)
    #[arg(long, global = true)]
    judge_model: Option<String>,
    /// LLM 호출 재시도 횟수
    #[arg(long, default_value_t = 2, global = true)]
    retries: u32,
    /// 호출 1건 타임아웃(초)
    #[arg(long, default_value_t = 600, global = true)]
    timeout_secs: u64,
    /// 호출 1건당 최대 비용(USD). claude --max-budget-usd 로 전달
    #[arg(long, global = true)]
    max_budget_usd: Option<f64>,
    /// 실행 디렉터리의 CLAUDE.md·플러그인·훅을 로드 (기본은 --safe-mode로 차단)
    #[arg(long, global = true)]
    load_context: bool,
    /// 재시도·실패 로그 출력
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// 초안 N개 생성 + 채점 + 랭킹 리포트
    Gen {
        #[arg(long)]
        spec: PathBuf,
        /// 콘텐츠 브리프 파일(md/txt) — 타깃 독자, 제품/서비스 정보, 참고 자료
        #[arg(long)]
        brief: PathBuf,
        #[arg(short = 'n', long, default_value_t = 3)]
        count: usize,
        #[arg(long, default_value = "runs")]
        out: PathBuf,
        /// 문서당 채점 횟수(모델·관점 순환 후 절사평균)
        #[arg(long = "rounds", alias = "judges", default_value_t = 2)]
        rounds: usize,
        #[arg(long, default_value_t = 1)]
        concurrency: usize,
        /// 생성만 하고 채점 생략
        #[arg(long)]
        no_score: bool,
    },
    /// 기존 문서 채점만 수행
    Score {
        #[arg(long)]
        spec: PathBuf,
        /// 채점 대상 파일 또는 디렉터리(*.md, *.txt)
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value = "runs")]
        out: PathBuf,
        #[arg(long = "rounds", alias = "judges", default_value_t = 2)]
        rounds: usize,
        #[arg(long, default_value_t = 1)]
        concurrency: usize,
    },
    /// 생성→채점→피드백 재생성 자기개선 루프
    Loop {
        #[arg(long)]
        spec: PathBuf,
        #[arg(long)]
        brief: PathBuf,
        #[arg(long, default_value = "runs")]
        out: PathBuf,
        /// 목표 점수(0-100). 도달 시 조기 종료
        #[arg(long, default_value_t = 85.0)]
        target: f64,
        /// 최대 반복. 문헌상 이득 대부분이 1~2회차에 발생하므로 기본 4
        #[arg(long, default_value_t = 4)]
        max_iter: usize,
        #[arg(long = "rounds", alias = "judges", default_value_t = 2)]
        rounds: usize,
        /// 직전 최고점 대비 이 값 미만 개선이면 정체로 간주
        #[arg(long, default_value_t = 2.0)]
        min_delta: f64,
        /// 정체가 이 횟수 연속이면 조기 종료
        #[arg(long, default_value_t = 2)]
        patience: usize,
        /// 시작 초안의 접근 각도(미지정 시 스펙 기본값)
        #[arg(long, default_value = "")]
        angle: String,
        /// 루프에 참여하지 않는 검증용 채점 모델. 종료 후 최초본 vs 최고본을 이 모델로 재채점
        #[arg(long)]
        gate_model: Option<String>,
    },
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("에러: {e:#}");
        std::process::exit(1);
    }
}

fn build_llm(cli: &Cli, model: Option<String>) -> Llm {
    let mut l = Llm::new(cli.claude_bin.clone(), model);
    l.retries = cli.retries;
    l.verbose = cli.verbose;
    l.timeout = Duration::from_secs(cli.timeout_secs);
    l.max_budget_usd = cli.max_budget_usd;
    l.load_context = cli.load_context;
    l
}

fn judge_panel(cli: &Cli) -> Vec<Llm> {
    match &cli.judge_model {
        Some(list) => list
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|m| build_llm(cli, Some(m.to_string())))
            .collect(),
        None => vec![build_llm(cli, cli.model.clone())],
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let gen_llm = build_llm(&cli, cli.model.clone());
    let judges = judge_panel(&cli);
    if cli.judge_model.is_none() {
        eprintln!(
            "주의: 생성 모델과 채점 모델이 동일합니다. 자기 문체를 후하게 평가하는 편향이 있으므로 \
             --judge-model 로 다른 모델을 지정하는 편이 낫습니다."
        );
    }

    match &cli.cmd {
        Cmd::Gen { spec, brief, count, out, rounds, concurrency, no_score } => {
            let sp = Spec::load(spec)?;
            let brief_text = read_text(brief)?;
            let out_dir = prepare_out(out)?;
            let angles = generate::angles_for(&sp, *count);

            println!("생성 {}건 — {}", count, sp.name);
            let items: Vec<(usize, String)> = angles.into_iter().enumerate().collect();
            let requested = items.len();
            let (docs, failed) = par_map(*concurrency, items, |(i, angle)| {
                let d = generate::generate(&gen_llm, &sp, &brief_text, &angle)?;
                let label = format!("cand{:02}", i + 1);
                std::fs::write(out_dir.join(format!("{}.md", label)), &d)?;
                println!("  생성 완료: {} ({}자)", label, d.chars().count());
                Ok((label, d))
            });
            if failed > 0 {
                eprintln!(
                    "경고: 생성 {failed}건 실패 (요청 {requested}건 중 {}건 성공)",
                    docs.len()
                );
            }
            anyhow::ensure!(!docs.is_empty(), "생성 실패: 요청한 {requested}건 모두 실패");

            if *no_score {
                println!("출력: {}  (누적 ${:.4})", out_dir.display(), llm::total_cost_usd());
                return Ok(());
            }
            let scored = score_many(&judges, &sp, docs, *rounds, *concurrency, &out_dir);
            finish(&out_dir, &sp, &scored)
        }

        Cmd::Score { spec, input, out, rounds, concurrency } => {
            let sp = Spec::load(spec)?;
            let out_dir = prepare_out(out)?;
            let files = collect_docs(input)?;
            anyhow::ensure!(!files.is_empty(), "채점 대상 문서 없음: {}", input.display());
            println!("채점 {}건 — {}", files.len(), sp.name);

            let mut docs: Vec<(String, String)> = Vec::new();
            for f in files {
                let label = f
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| f.display().to_string());
                docs.push((label, read_text(&f)?));
            }
            let scored = score_many(&judges, &sp, docs, *rounds, *concurrency, &out_dir);
            finish(&out_dir, &sp, &scored)
        }

        Cmd::Loop {
            spec, brief, out, target, max_iter, rounds, min_delta, patience, angle, gate_model,
        } => {
            let sp = Spec::load(spec)?;
            let brief_text = read_text(brief)?;
            let out_dir = prepare_out(out)?;
            let angle = if angle.is_empty() {
                generate::angles_for(&sp, 1).remove(0)
            } else {
                angle.clone()
            };
            let cfg = loop_run::LoopCfg {
                target: *target,
                max_iter: *max_iter,
                rounds: *rounds,
                min_delta: *min_delta,
                patience: *patience,
            };
            println!("루프 시작 — 목표 {:.0}점, 최대 {}회", target, max_iter);
            let r = loop_run::run(&gen_llm, &judges, &sp, &brief_text, &out_dir, &cfg, &angle)?;

            // held-out 게이트: 루프에 참여하지 않은 모델로 최초본과 최고본만 재채점
            let mut gate_pair: Option<(Scored, Scored)> = None;
            if let Some(gm) = gate_model {
                println!("held-out 검증 ({gm})…");
                let g = vec![build_llm(&cli, Some(gm.clone()))];
                let f = score::score_doc(&g, &sp, "gate-first", &r.first_doc, 1)?;
                let b = score::score_doc(&g, &sp, "gate-best", &r.best_doc, 1)?;
                println!("  최초본 {:.1} → 최고본 {:.1} (held-out)", f.total, b.total);
                gate_pair = Some((f, b));
            }

            let path = report::write_loop_report(
                &out_dir,
                &sp,
                &r.history,
                &r.stop_reason,
                &r.warnings,
                gate_pair.as_ref().map(|(f, b)| (f, b)),
            )?;
            println!(
                "\n종료: {} · 최고 {:.1}/100 ({})",
                r.stop_reason, r.best_score.total, r.best_label
            );
            for w in &r.warnings {
                println!("  ⚠ {}", w);
            }
            println!("최종본: {}", out_dir.join("best.md").display());
            println!("리포트: {}", path.display());
            println!("누적 비용: ${:.4}", llm::total_cost_usd());
            Ok(())
        }
    }
}

fn finish(out_dir: &Path, sp: &Spec, scored: &[Scored]) -> Result<()> {
    anyhow::ensure!(!scored.is_empty(), "채점 성공한 문서가 없음");
    let path = report::write_report(out_dir, sp, scored)?;
    let mut ranked: Vec<&Scored> = scored.iter().collect();
    ranked.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));
    println!("\n순위");
    for (i, s) in ranked.iter().enumerate() {
        println!("  {}. {} — {:.1}/100", i + 1, s.label, s.total);
    }
    println!("리포트: {}", path.display());
    println!("누적 비용: ${:.4}", llm::total_cost_usd());
    Ok(())
}

fn score_many(
    judges: &[Llm],
    sp: &Spec,
    docs: Vec<(String, String)>,
    rounds: usize,
    concurrency: usize,
    out_dir: &Path,
) -> Vec<Scored> {
    let requested = docs.len();
    let (scored, failed) = par_map(concurrency, docs, |(label, doc)| {
        let s = score::score_doc(judges, sp, &label, &doc, rounds)?;
        println!("  채점 완료: {} — {:.1}/100", s.label, s.total);
        Ok(s)
    });
    if failed > 0 {
        eprintln!(
            "경고: 채점 {failed}건 실패 (요청 {requested}건 중 {}건 성공)",
            scored.len()
        );
    }
    for s in &scored {
        if let Err(e) = report::append_jsonl(out_dir, s) {
            eprintln!("경고: results.jsonl 기록 실패 — {e:#}");
        }
    }
    scored
}

/// concurrency 만큼 묶어 병렬 실행. 실패한 항목은 경고 후 건너뛰되, 실패 개수를 세어
/// 호출부에 반환한다(전체 중단은 하지 않음 — 전부 실패했는지 판단은 호출부 책임).
fn par_map<T, R, F>(concurrency: usize, items: Vec<T>, f: F) -> (Vec<R>, usize)
where
    T: Send,
    R: Send,
    F: Fn(T) -> Result<R> + Sync,
{
    let c = concurrency.max(1);
    let mut out: Vec<R> = Vec::new();
    let mut failed = 0usize;
    let mut rest = items;
    while !rest.is_empty() {
        let take = c.min(rest.len());
        let chunk: Vec<T> = rest.drain(..take).collect();
        let results: Vec<Result<R>> = std::thread::scope(|s| {
            let handles: Vec<_> = chunk.into_iter().map(|item| s.spawn(|| f(item))).collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("작업 스레드 패닉"))))
                .collect()
        });
        for r in results {
            match r {
                Ok(v) => out.push(v),
                Err(e) => {
                    eprintln!("경고: 항목 실패 — {e:#}");
                    failed += 1;
                }
            }
        }
    }
    (out, failed)
}

fn read_text(p: &Path) -> Result<String> {
    std::fs::read_to_string(p).with_context(|| format!("파일 읽기 실패: {}", p.display()))
}

fn prepare_out(p: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(p).with_context(|| format!("출력 디렉터리 생성 실패: {}", p.display()))?;
    Ok(p.to_path_buf())
}

fn collect_docs(input: &Path) -> Result<Vec<PathBuf>> {
    if input.is_file() {
        return Ok(vec![input.to_path_buf()]);
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir(input)
        .with_context(|| format!("디렉터리 읽기 실패: {}", input.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.extension().map(|e| e == "md" || e == "txt").unwrap_or(false)
                && p.file_name().map(|n| n != "report.md").unwrap_or(true)
        })
        .collect();
    v.sort();
    Ok(v)
}
