# Empirical review findings

This directory records what actually happened when this repo was reviewed and, separately,
actually run — as opposed to what a design doc claims should happen. Same spirit as
[Code-Review-Loop's `evals/README.md`](https://github.com/Loop-Suite/Code-Review-Loop/blob/main/evals/README.md):
real numbers from real work, not estimates, with the caveats stated plainly.

This repo has no `promptfoo` golden-set harness (that's Code-Review-Loop's own eval scaffold,
not something this project has). What's documented here instead is three rounds of actual review
work against this codebase — two static code-review passes and one live CLI execution pass — all
performed in a single review session on 2026-08-09/10, with every issue filed also fixed and
verified in this same session.

## TL;DR

| Round | Method | Issues filed | Issues fixed | Real cost |
|---|---|---|---|---|
| 1. Static review | manual code reading, no LLM calls | #2, #3, #4, #5 (4) | 4/4 | $0 |
| 2. Deeper static review | manual code reading, no LLM calls | #6, #7, #8 (3) | 3/3 | $0 |
| 3. Real CLI execution | `claude -p --model haiku` via `seo gen` (2 real generations) | #9 (1) | 1/1 | $0.3941 |
| **Total** | | **8** | **8/8** | **$0.3941** |

**What this bought:**

- **Static review alone found 7 real bugs across two passes, all confirmed by reading the actual
  code** (not speculative) — no LLM calls needed, no cost, because these are logic bugs
  (word-boundary handling, case sensitivity, string comparison) verifiable by inspection and unit
  test, not behavior that requires running the tool against a live model.
- **One bug (#5) was fixed incompletely the first time, and the second review pass caught the
  gap.** #5 fixed the self-scoring warning for the omitted-`--judge-model` case; #8, filed in the
  very next review pass, found that the fix's own condition (`Iterator::eq` against a
  single-element array) silently failed to warn for `--judge-model sonnet,sonnet` — same model,
  repeated, still 100% self-scored, just with a list length that no longer matched `[m]`. Recorded
  here specifically because "the fix for the last finding has its own gap" is the kind of thing a
  second independent pass catches and a single pass doesn't.
- **Static review, however thorough, could not find #9** — a bug that only exists in the
  *interaction* between this tool's code and a live model's actual output shape. Only running the
  real `seo gen` pipeline against `claude -p --model haiku` surfaced it: haiku wrapped its entire
  response in a stray outer code fence, silently breaking three separate downstream checks. This
  is exactly the class of bug the reference doc above calls out as invisible to static review —
  confirmed the same way here, on a different codebase.
- **#9's root cause is a real asymmetry, not a one-off parsing slip**: the JSON response path
  (`llm::json()`/`extract_json()`) already tolerated a stray wrapping code fence as a documented
  fallback; the plain-text path (`Llm::text()`, used by both `generate()` and `revise()`) had no
  equivalent handling and returned the model's raw text unprocessed. The fix (`strip_wrapping_fence()`)
  brings the text path up to the same tolerance the JSON path already had — closing a gap between
  two code paths that should have had matching behavior from the start.
- **The fix for #9 was verified by re-running the same real pipeline, not just by reading the
  diff.** A second live `seo gen` run against the same spec/brief, after the fix, shows
  `title_chars`/`meta_chars` correctly populated (56/139 and 50/149, vs. 0/0 before) and Flesch
  scores correctly computed (62.5/61.5, vs. `null`/"N/A (non-English content)" before) — see
  Round 3 below for the exact before/after data.
- Real cost across the whole review: **$0.3941** — both dollars spent were in Round 3's two live
  generations (`$0.1748` reproducing the bug, `$0.2193` verifying the fix); Rounds 1 and 2 cost
  nothing because they never called a model.

## Round 1: static review — 4 issues (#2–#5)

Manual read-through of the codebase against its own README claims. All four confirmed by reading
the relevant function and, where applicable, a hand-traced repro — no LLM calls, no cost.

### #2 — `generate::build_prompt` hardcodes "Write in Korean"

`src/generate.rs:15` unconditionally injected `"Write an SEO content draft in Korean according to
the conditions below."` into every generation prompt, regardless of the spec/brief's actual
language. Directly contradicts the README's claim that "the CLI itself is language-agnostic."
Running the README's own quickstart — an entirely English spec/brief
(`specs/example-blogpost.toml`, `brief.example.md`) — still forced a Korean draft, which also
silently skipped the English-only Flesch readability checks for every draft generated this way.

Fixed in `938edde`: hardcoded language directive removed; the model now infers the language from
the brief/spec content instead.

### #3 — keyword matching strips all whitespace, causing false-positive substring matches

`checks::norm_kw` stripped *all* whitespace before lowercasing, and `contains_kw`/
`count_kw_occurrences` did a plain substring search on the fully-concatenated result. Example:
`"This is a running shoe sale event."` normalizes to `"...runningshoesaleevent"`, which contains
`"runningshoes"` (the normalized keyword) purely because "shoe" + "sale" glue together into
"shoesale" — even though the text says "running shoe sale," not "running shoes." This can suppress
a legitimate "keyword missing" warning and distort `Metrics::keyword_occurrences`, which feeds a
loop-mode keyword-density safety canary.

Fixed in `7a02e11`: keyword matching now tokenizes into words and compares word-boundary-respecting
token sequences instead of stripping whitespace and doing a raw substring search.

### #4 — `missing_sections()` does case-sensitive title matching

`checks::missing_sections`'s `norm` closure stripped whitespace but did **not** lowercase, unlike
`norm_kw` elsewhere in the same file. A required section titled `"FAQ"` was reported "missing" even
when the document had it as `## faq` — inconsistent with the rest of the deterministic checks, and
capable of feeding a false "missing section" instruction back into every regeneration prompt in
loop mode.

Fixed in `3a6829c`: `norm` now lowercases too, matching `norm_kw`'s convention.

### #5 — self-scoring warning only checks whether `--judge-model` was omitted

README's Limitations section says the CLI warns when generation and judge model are the same. The
actual check (`src/main.rs`) only fired on `cli.judge_model.is_none()` — so `--model sonnet
--judge-model sonnet` (explicitly the same model, genuinely self-scoring) printed no warning at
all, contradicting the documented behavior.

Fixed in `4a22c11`: the condition also warns when `--judge-model` resolves to a single model equal
to `--model`. (Its own gap is what #8, below, later caught.)

## Round 2: deeper static review — 3 issues (#6–#8)

A second, more adversarial static pass over the same codebase, after Round 1's fixes landed —
looking specifically for the same bug classes (substring matching, boundary conditions) recurring
elsewhere, and for incomplete fixes.

### #6 — `is_internal_url` misclassifies non-http(s)-scheme absolute URLs as internal

`checks::is_internal_url()` only stripped `https://`/`http://` prefixes before checking the host.
Anything else — protocol-relative URLs (`//evil.example.org/a`), `mailto:`, `ftp:`, `tel:` — fell
through to the `None` branch, which is meant for genuine relative paths (`/about`) and silently
treated all of these as internal links instead. This is the same class of host-spoofing hole
already fixed for `notexample.com`/`example.com.evil.com` substring matching, reached through a
different code path (missing scheme prefix, not a substring match). Affects `seo score` on
arbitrary user-supplied documents, where it can push the reported internal/citation link counts in
the wrong direction on both metrics simultaneously.

Fixed in `cd29f9e`: protocol-relative URLs now get the same host check as `http(s)://`, and any
other explicit URL scheme is treated as external instead of falling through to the relative-path
default.

### #7 — `missing_sections()` false-positives on unrelated short headings via substring containment

Even after #4's case-insensitivity fix, `missing_sections()` still matched via plain substring
containment in either direction (`hn.contains(&want) || want.contains(&hn)`). A required section
titled `"Frequently Asked Questions"` normalizes to `"frequentlyaskedquestions"`; a completely
unrelated heading `"## As"` normalizes to `"as"`, and `"frequentlyaskedquestions".contains("as")`
is true (`ask`**ed** contains `as`) — so the required FAQ section was reported present when the
document had no FAQ section at all. Same bug class already fixed for keyword matching in #3, never
applied to this function. Feeds directly into `format_issues()` and loop mode's stop condition, so
a false "present" match can let the loop stop early on a document actually missing a required
section.

Fixed in `0adb7e1`: reuses the word-boundary tokenizer from the #3 fix, matching on contiguous word
tokens instead of raw substring containment.

### #8 — self-scoring warning misses a repeated-same-model `--judge-model` panel

Follow-up on #5's own fix. The fixed condition compared the judge-model list to a one-element
array via `Iterator::eq`, which requires equal *length*. `--model sonnet --judge-model sonnet` was
correctly caught (1 entry, matches `[m]`), but `--model sonnet --judge-model sonnet,sonnet` (or any
repeated-N-times variant) was **not** — `["sonnet","sonnet"].eq(["sonnet"])` is `false` purely
because the lengths differ (2 vs. 1), even though every entry in the panel is still identically the
generation model, i.e. still fully self-scored. This is worth calling out as a pattern: the first
fix addressed the literal case named in the bug report (single same model) but the underlying
intent — "warn whenever the judge panel has zero real diversity from the generation model" — wasn't
fully captured until this second pass re-examined the fix itself, not just the original bug.

Fixed in `36f90f7`: extracted into a standalone, unit-tested `judge_model_matches_gen()` that
requires every (trimmed, non-empty) judge-model entry to equal `--model`, regardless of list
length.

## Round 3: real CLI execution — 1 issue (#9), real cost $0.3941

The two static-review rounds above found real bugs, but static review has a ceiling: it cannot see
what an actual model actually returns. This round ran the real pipeline —
`./target/release/seo --model haiku --judge-model haiku gen --spec specs/example-blogpost.toml
--brief brief.example.md -n 2 --rounds 1 --concurrency 1` — against a live `claude -p` backend, at
real API cost, specifically to check whether the tool's assumptions about model output actually
hold.

### #9 — `generate()`/`revise()` don't strip a stray outer code fence

**What happened:** in 2 of 2 real generations, haiku wrapped its entire response — frontmatter and
body together — in a single outer ` ``` ` fence, despite the prompt explicitly saying "Output only
the document body, with no intro, explanation, or meta-commentary." `Llm::text()` returned this raw
text unmodified, so the saved candidate files started with a literal ` ``` ` line before the `---`
frontmatter delimiter.

**Cascading effect, all visible in the same run's `report.md`/`results.jsonl`:**
- `title_chars: 0`, `meta_chars: 0` for both documents, even though both had valid `title:`/
  `meta_description:` frontmatter — `parse_front_matter()` only recognizes frontmatter when the
  document *starts* with `"---"`, and here it started with `` ``` `` instead.
- False format issues: "Missing title," "Missing meta_description," and "Target keyword ... not
  found in title" — all wrong; the title clearly contained the keyword.
- `flesch_reading_ease: null` / `flesch_kincaid_grade: null`, reported as **"Flesch: N/A
  (non-English content — see README limitations)"** for content that was 100% English. Root cause:
  `strip_markdown_to_prose()` toggles an `in_fence` flag on every ` ``` `-starting line; with only
  one opening fence and no matching close inside the body, `in_fence` stayed `true` for the entire
  document, so every line was skipped and the resulting near-empty "prose" pushed the Latin-char
  ratio below the language-detection threshold.

**Why static review couldn't have caught this — the interesting part:** the JSON response path
(`llm::json()`/`extract_json()` in `src/llm.rs`) already had documented fallback handling for a
stray wrapping code fence around a JSON reply. The plain-text path (`Llm::text()`, used by both
`generate()` and `revise()`) had no equivalent — it returned `r.text` completely unprocessed. This
is a genuine asymmetry between two code paths that should have behaved the same way, and it's
invisible from reading the code alone: it only manifests when the backend model actually decides to
wrap its answer, which requires observing a live response, not inspecting the prompt or the parser
in isolation. (`generate::build_prompt()`'s own instructions demonstrate the required frontmatter
shape *inside* a ` ``` ` fence, which may be priming some models to wrap the whole answer the same
way — plausible contributing factor, not the fix.)

**Reproduction data (`runs/blog-test/`, pre-fix, real haiku output):**

| | cand01 | cand02 |
|---|---|---|
| `title_chars` | 0 | 0 |
| `meta_chars` | 0 | 0 |
| `keyword_in_title` | false | false |
| `flesch_reading_ease` | null | null |

**Fix (`279a116`):** added `strip_wrapping_fence()` to `src/llm.rs`, giving `Llm::text()` the same
fence tolerance `extract_json()` already had — strips a single matching ` ``` `/` ```lang ` … ` ``` `
pair only when it wraps the *entire* trimmed response, so a fence used legitimately inside real
content (e.g. a code sample partway through a document) is left untouched.

**Verification (`runs/blog-verify/`, post-fix, second real haiku run, same spec/brief):**

| | cand01 | cand02 |
|---|---|---|
| `title_chars` | 56 | 50 |
| `meta_chars` | 139 | 149 |
| `keyword_in_title` | true | true |
| `flesch_reading_ease` | 62.5 | 61.5 |

Title/meta detection and Flesch scoring both now work correctly against real model output, and the
Round 1 fix for #2 (language forcing) was independently confirmed still holding — both real runs
produced English drafts with no Korean-forcing regression.

**Real cost:** reproduction run (2 generations, haiku gen + haiku judge, 1 round) —
`Cumulative API cost: $0.1748` (from `runs/blog-test/report.md`). Fix-verification run, same
parameters — `Cumulative API cost: $0.2193` (from `runs/blog-verify/report.md`). **Total: $0.3941**
across the two real `claude -p` calls this round required.

## Caveats

- This is not a golden-set benchmark like Code-Review-Loop's `evals/` — no fixed diff corpus, no
  automated pass/fail grading, no CI wiring. It is a record of one review session's actual findings
  and one pipeline's actual execution, at n=1 for the live-execution round.
- Round 3 exercised exactly one model (`haiku`) and one spec/brief pair (the bundled quickstart
  example). Whether other models wrap responses in stray fences the same way, or whether other
  specs trigger different failure modes, is untested here.
- Rounds 1 and 2 were manual code review, not an automated static-analysis tool — thorough for the
  bug classes actually looked for (substring/boundary/case-sensitivity issues), but not exhaustive
  by construction.
