# seo-loop Research & Evidence Survey

## 1. Overview

`seo-loop` is a Rust CLI that ports the "**generate N → deterministic rule checks → LLM rubric scoring (de-anchoring) → trimmed-mean aggregation → regenerate → held-out gate re-scoring**" structure established by Loop-Suite's `bizplan-loop` into the SEO blog/landing-page copy generation domain. It belongs to a different lineage than discourse-based anonymous cross-debate (the Code-Review-Loop/research-loop family) — rather than forced rebuttal between personas, it's an approach that reduces variance via "a judge panel (different models/perspectives) × multiple rounds → statistical trimmed mean."

This survey has two goals.

1. **Re-verify the competing OSS projects (`sour4bh/auto-seo`, BlogPilot) that an earlier round (at README/landing-page level) had already concluded were "structurally similar," this time by directly reading actual source files via `gh api`**, and explicitly self-correct any points where the earlier conclusion was inaccurate.
2. Investigate, for the first time, topics the earlier round didn't cover: **AI SEO content-generation OSS frameworks (LangGraph/CrewAI), Korean-language readability-metric OSS, whether to reintroduce `pulldown-cmark`, and academic evidence for reward-hacking prevention.**

The methodology follows exactly the approach that research-loop's research survey (`Loop-Suite/research-loop/docs/research-and-evidence-survey-2026-07-31.md`) practiced in §8 ("Follow-up Investigation") — *not concluding from the README alone, but directly reading actual source files (`skills/researcher.py`, `backend/nodes/*.py`, etc.) to verify the architecture, and self-correcting any points along the way where the README and the code diverge.*

---

## 2. Re-verifying the Earlier Survey (Including Self-Correction)

### 2.1 `sour4bh/auto-seo` — Re-verifying the Earlier Conclusion That "the Structure Is Most Similar"

**Earlier conclusion (subject to re-verification)**: "Hybrid scoring with 7 rules + 6 LLM evaluations, Claude+Gemini multi-model consensus — most structurally similar to ours."

**Files actually read this time**: `gh api repos/sour4bh/auto-seo/contents/app/article/scorer.py` (342 lines); from `app/article/pipeline.py` (1291 lines): `score_step` (L319-422), `review_step` (L425-494), `edit_step` (L497-539), `_merge_score_dimensions` (L812-829), and the edit loop in `run_pipeline` (L1244-1276).

The verification confirms the surface-level similarity ("rule + LLM consensus") holds, but **three structural differences were clearly confirmed at the source level** — the earlier round asserted "most similar" without verifying these three points, so this is corrected here.

| Item | Earlier-round assumption | Source re-verification result |
|---|---|---|
| Draft generation | (not checked; focus was only on "hybrid scoring") | Not a `writePost`/`writer.ts`-style function — `pipeline.py`'s `generate_step` (the code itself wasn't read, but `run_pipeline`'s `STEP_SEQUENCE` was confirmed at `L1230-1274` to be `generate → score → review → (edit↔score↔review loop)`) **generates a single document and keeps rewriting that same document in place.** It is not a structure that, like `seo-loop`'s `generate.rs`, varies the angle to produce **N independent drafts** and picks the highest-scoring one |
| Aggregation method | "hybrid scoring" | `score_step` (L360-414) fires 3 LLM prompts × all providers in the council in parallel (`asyncio.gather`, L374), then `_merge_score_dimensions` (L812-829) **simply averages the scores of dimensions with the same name** (`avg = sum(d.score for d in group) / len(group)`). This is not a trimmed mean like `score.rs::trimmed_mean`, which drops the max and min when n≥4 |
| Held-out verification | (not checked) | `run_pipeline`'s edit loop (L1244-1276) has **the same `council`** (the global `get_llm_council()`) re-score on every iteration. There is no `--gate-model`-style mechanism anywhere in the code where a separate model that never participated in the loop re-scores only the first and best drafts |

**Self-correction**: The earlier round judged "structurally most similar" based only on README/documentation-level description — "7 rules + 6 LLMs, multi-model consensus." Reading the source shows the similarity stays at the high-level "runs deterministic rule checks alongside LLM scoring," and **auto-seo has none of the three core anti-reward-hacking mechanisms of seo-loop: (1) generating and selecting from N independent drafts, (2) trimmed-mean aggregation, and (3) a held-out gate.** The phrase "most similar competing project" was an overstatement; it is corrected to, more precisely, "a project that shares only the high-level concept of hybrid scoring, with no reward-hacking-prevention mechanism."

Additional detail confirmed (not in the earlier survey): `score_readability`/`score_humanity` (scorer.py L147-252) run only when `job.language == "en"` (`pipeline.py` L346-348). Unlike seo-loop's 50%-Latin-character-share heuristic (`checks.rs::readability`, `latin/total < 0.5 → None`), this is cruder in that it's based on a **single language flag** — it unconditionally skips anything not English, and cannot distinguish "an English document with a low Latin-character share due to mixed content."

### 2.2 BlogPilot — Expanding Verification from "Flesch Logic Porting Only" to the Full Architecture

**Earlier conclusion (subject to re-verification)**: "39 modules, TF-IDF + E-E-A-T + SERP detection. Ported the Flesch readability logic (this fact itself is correct). Whether this repo has our 'generate N → multi-round scoring → held-out gate' pattern wasn't checked — only readability.ts was read."

**Files actually read this time**: `src/app/api/draft/route.ts` (98 lines), `src/lib/seo/writer.ts` (70 lines), `src/app/api/content-score/route.ts` (48 lines), `src/lib/seo/content-score.ts` (278 lines; function list confirmed via grep: `grade`, `median`, `jaccardWords`, `competitorDocFrequency`, `competitorMedianCount`, `scoreDraft`).

- `writer.ts::writePost` (L40-70) calls `execute({ methodologies: [...], task: "Write the full post in Markdown following the outline exactly.", ... temperature: 0.7, maxTokens: 6000 })` **once** to produce the entire Markdown post. If no provider is available, it falls back to `fallbackPost` (a static template). There is no logic for producing N angle-varied drafts.
- Grepping `content-score.ts` for LLM-call-related symbols (`execute`, `LlmClient`, `provider.`, `ai/executor`, etc.) turned up **zero matches.** `scoreDraft` (L109 onward) scores using only `jaccardWords` (Jaccard similarity against competitor body text), `competitorDocFrequency`/`competitorMedianCount` (term frequency against a competitor corpus, TF-IDF family), and `median` — a **purely deterministic scorer with no LLM rubric scoring whatsoever.**
- `draft/route.ts` (L33-42) simply saves the `writePost` result to the DB and stops. There is no re-score → regenerate loop in this path at all (instead, the user manually calls the separate `content-score` API to see a score in the editor; `content-score/route.ts` comment L7-9 says: "client passes prefetched corpus; runs scoring locally for fast keystroke updates" — i.e., it's an **editor-assist tool** giving real-time feedback as a human types, not an automatic regeneration loop).

**Self-correction**: The earlier round's description — "TF-IDF + E-E-A-T + SERP detection" — was not itself wrong, but the worry that "only readability.ts was read, so the full architecture might have been missed" turned out to be correct — **BlogPilot has no LLM rubric scoring at all** (content scoring is 100% deterministic TF-IDF/Jaccard similarity; the LLM is used only in generation stages such as writing, research, and outlining). This is even further from seo-loop's structure than auto-seo — no "generate N," no "LLM rubric," no "held-out gate." A case where the earlier conclusion pointed the right direction but rested on shallow evidence.

### 2.3 Yoast SEO — Re-confirming the Architecture Behind the "Pure Rule-Based, Code Unread" Conclusion

**Earlier conclusion (subject to re-verification)**: "A collection of rule-based assessments, no LLM rubric. Code was not read (porting is prohibited due to license issues)."

Since the license (GPL-2.0) still prohibits porting code, only an **architecture check at the directory-structure/filename level** was performed (no content was transcribed; the read was for structural confirmation only).

- `Yoast/YoastSEO.js` is now **archived** (the standalone repo was retired and absorbed into the `Yoast/wordpress-seo` monorepo) — a fact not present in the earlier survey.
- Confirmed in the `Yoast/wordpress-seo` tree: under `packages/yoastseo/src/scoring/assessments/`, below the `assessment.js` base class, `readability/` (`ParagraphTooLongAssessment.js`, `PassiveVoiceAssessment.js`, `SentenceLengthInTextAssessment.js`, `TransitionWordsAssessment.js`, `WordComplexityAssessment.js`, etc.) and `inclusiveLanguage/` (`InclusiveLanguageAssessment.js` + per-category `configuration/*Assessments.js`) are split into **individual rule classes**.
- The same directory contains **Yoast's own design documents**: `SCORING READABILITY.md`, `SCORING SEO.md`, `SCORING SEO PRODUCT.md`, `SCORING TAXONOMY.md`, `KEYPHRASE MATCHING.md` (the mere existence of these documents is circumstantial evidence that "the scoring method is a published set of deterministic rules" — only their existence was confirmed, no content transcribed).

**Conclusion**: The earlier round's conclusion — "pure rule-based, no LLM" — is re-confirmed at the architecture level. However, a 2026 search result (Yoast's official blog, "SEO 2026 Predictions") mentions that "AI-powered features" were added separately — **the core content-analysis/scoring engine (`scoring/assessments/*`) is still rule-based, and the AI functionality appears to be a separate layer bolted on top (e.g., AI-driven phrasing suggestions).** The earlier survey didn't know this distinction, so the blanket statement "Yoast = pure rule-based" is refined to "the core scoring engine is rule-based; a recent AI-assist feature was added as a layer" (though the concrete structure of the AI layer could not be confirmed in code within this survey's scope — uncertain).

### 2.4 Surfer / Clearscope / MarketMuse — Re-confirming Whether "AI Actually Re-Scores Multiple Drafts Itself"

**Earlier conclusion (subject to re-verification)**: "Coverage scoring based on scraping competitors' top-ranking pages. Whether AI actually generates multiple drafts and re-scores them itself was checked only at the marketing-copy level."

Re-confirmed at the product-documentation/review level (official API docs aren't public, so inaccessible; based on detailed third-party reviews):

- **Surfer AI**: Generates **one finished draft** per keyword using GPT-4 Turbo in 15-20 minutes; the Content Editor's "Content Score" (0-100) is a **static similarity score** (against a SERP corpus) that updates in real time as a person types. No feature for "generating multiple drafts and automatically comparing/re-scoring them" appeared in any review or guide found.
- **Clearscope**: Pasting in a draft returns an F-through-A++ grade; the workflow is a **manual iteration** (paste → grade → edit → paste again) where a human revises and pastes it back in.
- **MarketMuse**: The "First Draft" feature generates a scaffold of up to 5,000 words **once** (described as "more basic AI drafting, primarily for supplements" — positioned as supplementary draft generation).

**Conclusion**: For all three products, a feature where "AI generates N candidates and automatically compares them to pick the top score" was not confirmed at the product-documentation/review level — they are **semi-automatic structures where a human performs part of the loop (edit → re-check)**, not fully automatic regeneration loops like seo-loop/auto-seo/BlogPilot. The earlier conclusion is re-confirmed; the difference this round is that the evidence density was raised to "actual workflow description rather than marketing copy" (concrete UX details such as the 15-20 minute duration, paste-and-grade, etc.).

---

## 3. New Investigation

### 3.1 AI SEO Content-Generation OSS Frameworks (LangGraph/CrewAI) — Whether Reward-Hacking-Prevention Mechanisms Exist

Most projects turned up by GitHub search (`seo blog generator langgraph`, `seo content crewai`) were ★0-2 learning/toy projects (`renswickd/multi-agent-blog-generator`, `SuyashMohanty/Agentic-AI-Blog-Generator`, `Mahul777/langgraph-multilingual-blog-generator`, etc. — multilingual translation kept recurring as the headline differentiator, with no scoring or reward-hacking-prevention structure). Of these, the **most structurally developed** — `christancho/blogging-with-langchain` (LangGraph + Claude + Ghost CMS, explicitly described as an "approval gate workflow") — was verified against actual source.

**Files actually read**: `agentic/graph.py` (275 lines), `agentic/nodes/editor.py` (244 lines).

- `graph.py`'s (L59-118) workflow: `research → audience_analysis → writer → fact_checker (↔writer rewrite loop, up to 3x) → formatter → seo → editor (↔writer rewrite loop, up to 3x) → publisher`. It consists of a **linear pipeline plus 2 conditional edges** (`route_fact_check_decision`, `route_editor_decision`) — not a structure that, like seo-loop, produces N independent drafts and compares them, but one that **keeps rewriting a single document** (the same pattern as BlogPilot/auto-seo).
- `editor.py::editor_node` (L16-244) is the approval gate. However, it **reuses the exact same LLM configuration that wrote the piece** via `llm = Config.get_llm()` (L85), scoring `cohesiveness_score`/`hook_score`/`storytelling_score`/`voice_score` with a single JSON call. There is no held-out mechanism where a separate model that didn't participate in the loop verifies the result.
- When `max_revisions` (default 3) is exceeded, it force-publishes with "FORCING PUBLICATION WITH NOTE" (L177) — similar in purpose to seo-loop's `--patience`/stall early-stop (`loop_run.rs`), but different in that it **simply gives up and publishes with no independent verification signal.**

**Conclusion**: Even checking the most structurally developed case in the LangGraph/CrewAI-based SEO content-generation OSS ecosystem at the source level, **none** of seo-loop's four core elements (generating N independent drafts, multi-model/multi-round trimmed-mean, de-anchoring, held-out gate) were found. This is exactly the same pattern as the conclusions in §2.1/§2.2 — the self-preference structure where "the same model approves the piece it wrote itself" (precisely the problem bizplan-loop's `DESIGN.md` §10 warns about) is a common pattern across the OSS SEO generation pipeline landscape.

### 3.2 Non-English (Korean) Readability Metrics Beyond Flesch, in Open Source

Both `textstat` (the Python readability library auto-seo uses, part of the Flesch family) and BlogPilot's `readability.ts` are English-only. seo-loop returns `None` (N/A) when the Latin-character share is under 50%, so it doesn't apply the Flesch formula to Korean at all (`checks.rs::readability`).

A GitHub code search (`가독성 language:Python filename:readability`) turned up `naaaayeonn/AI-literacy-care-Agent` (★3, Python — presumably a project supporting people with low literacy), which contains a **Korean-specific readability formula implementation and its design document.**

**File actually read**: `2. Content & RAG Agent/docs/READABILITY_FORMULA.md` (147 lines).

Formula (verbatim from the document):

```
readability_score =
  100
  - (avg_words_per_sentence × 1.015)
  - (avg_syllables_per_word × 8.0)
  - (technical_term_ratio × 35.0)
```

- `avg_words_per_sentence`: whitespace-delimited eojeol count (in Korean, the basic unit is the eojeol — a space-delimited word/phrase group — not the morpheme, a different premise from English word-splitting) / sentence count (split on whitespace following `다.`, `요.`, `.`, `!`, `?`).
- `avg_syllables_per_word`: counts 1 character in the `가-힣` Unicode range as 1 syllable (this is actually more accurate than English, since there's no need to estimate vowel patterns — Hangul characters are syllable blocks, so character count exactly equals syllable count).
- `technical_term_ratio`: the ratio, relative to total eojeol count, of English words of 3+ characters (`LLM`, `API`) plus words matching Korean technical-term suffix patterns (`화·율·성·도·적·론·법·형·식·계·기·학`).

**Assessment**: This formula isn't simply Flesch's constants (206.835, 1.015, 84.6) carried over as-is — it was **redesigned to exploit the Hangul-syllable-equals-character property**, and it comes with its own validation examples (document §5) and an interpretation table (§4). That said, (1) it appears to be a ★3 personal/student project with a short commit history, and (2) the document itself **does not cite peer-reviewed academic backing** for its coefficients (1.015/8.0/35.0/100) (§7 "Limitations" admits it "doesn't account for colloquial speech/neologisms" and "ignores context") — in other words, it's an **empirically tuned heuristic**, not a validated formula.

Separately, a KCI (Korea Citation Index) search confirmed the existence of a **peer-reviewed academic paper** titled "A Study on Developing a Formula for Measuring the Difficulty of Korean Reading Texts" (a model combining grammatical difficulty and lexical difficulty), but no open-source code implementing it was found. In other words, the current state is a gap: "an academically validated formula exists but has no OSS implementation, and an OSS implementation exists but has no academic validation" — reflected in the §5 backlog.

### 3.3 Reconsidering Whether to Adopt `pulldown-cmark`

**Earlier conclusion (subject to re-verification)**: "Unnecessary."

Re-read `checks.rs::scan_links`/`find_matching_bracket_close` (L131-188) as of this survey. Per the README's "Multi-lens review findings applied" section (L200-201), this review round newly added **nested-bracket (`[a[b]c](url)`) depth tracking and escape (`\[`/`\]`) handling** — meaning the hand-rolled Markdown tokenizer has now gone through a second cycle of "hand-patching an edge case in a self-built parser" (the first was the CRLF frontmatter-offset bug fix).

Re-examination results:

- The input `scan_links` needs to handle is narrow in scope: **a single LLM-generated document** (spec frontmatter + H1-H6 + standard `[text](url)`/`![alt](url)` syntax). Reference-style links (`[text][ref]`), footnotes, tables, inline HTML, and autolinks (`<https://...>`) are not requirements anywhere in the spec/README.
- Even adopting `pulldown-cmark` (currently `0.13.4` on crates.io, confirmed via `cargo search`), `checks.rs` would still need: (1) byte-offset splitting of frontmatter (outside cmark's scope — CommonMark doesn't standardly handle YAML frontmatter, requiring a **separate crate**, `pulldown-cmark-frontmatter`), (2) heading level/text, (3) the `(is_image, label, url)` 3-tuple for links/images, and (4) "prose with Markdown stripped" text for the readability calculation. The code to reassemble these four things from the `Event` stream would still have to be hand-written, so **the reduction in raw code volume isn't large relative to the cost of adding a dependency (plus a frontmatter crate).**

**Conclusion (reconfirmed, but conditional)**: The "unnecessary" conclusion holds as of now. However, it's worth recording that this was the **second** hand-patch of an edge case — rather than simply asserting "unnecessary" as the earlier survey did, this time a **switch-trigger condition is spelled out**: if any of (a) reference-style links, (b) a `]` character inside a code span, or (c) autolinks is found to be an actual bug in a future review, adopting `pulldown-cmark` should be reconsidered at that point instead of continuing to patch the hand-built tokenizer. The result is "conclusion reconfirmed" for now, but documenting the trigger condition so the reasoning doesn't have to be rebuilt from scratch each time is the practical output of this re-verification.

### 3.4 Additional Academic Evidence for De-anchoring / LLM-as-Judge Reward-Hacking Prevention

Evidence already cited by the seo-loop README and bizplan-loop's `DESIGN.md` (duplicate-check, not newly found): the primary evidence for de-anchoring and the held-out gate is **arXiv:2607.05904** ("More Convincing, Not More Correct: Self-Play Reward Hacking of Reference-Free LLM Judges"), which `bizplan-loop/DESIGN.md` §4/§5 already cites precisely (having the judge write its criteria before seeing the candidates drops the false-positive rate from 0.719 to 0.012; self-play raises the judge pass rate from 0.716 to 0.938 while actual accuracy stagnates from 0.209 to 0.202). Finding this paper again in this survey itself means — **the existing evidence has been reconfirmed as still current and optimal.**

Per the task's instruction to find **one additional** piece of evidence, the following was found.

- **Reward Bias Substitution: Single-Axis Bias Mitigations Redirect Optimization Pressure** (Lamparth, Fein, Haupt, Hussing, Kochenderfer — arXiv:2605.27996). Core claim (quoted verbatim from the abstract): *"Single-axis mitigations of reward-model biases (e.g., reducing proxy reliance on length, sycophancy, or style) can rotate optimization pressure onto correlated proxies."* In other words, **suppressing one bias axis, such as length, shifts optimization pressure onto another (unobserved) axis correlated with it.** The RLHF example the paper cites: applying a length penalty makes responses shorter, but the model instead becomes biased toward **overconfidence.**

**Implication for seo-loop**: The length-inflation canary in `loop_run.rs` (L109-125, "warn if length grows +25% while score gains under +5") is exactly the "single-axis mitigation" this paper describes. Right now it monitors **only the single axis of length** — if the generation model learns that "growing the length gets caught," the paper's logic predicts that gaming could shift to an **unobserved other axis** (e.g., stuffing keywords right up to the limit the `keyword_naturalness` rubric will tolerate, padding link count regardless of quality to dodge the `citation_required` 60-point cap, or piling on flashy adjectives and exaggerated figures). Since seo-loop's current defense is only the single length axis, this is a real gap — reflected in the §5 backlog.

---

## 4. Overall Conclusion — Does Our Architecture Actually Have a Differentiator? (An Honest Assessment)

**Across the entire set** of 6 projects/products re-verified at the source level this round (auto-seo, BlogPilot, Yoast, Surfer, Clearscope, MarketMuse) plus the newly investigated LangGraph/CrewAI SEO-generation OSS ecosystem (including blogging-with-langchain), **not a single case was found that simultaneously has all 5 of seo-loop's elements** — (1) generating N angle-varied independent drafts, (2) multi-model/multi-round trimmed-mean aggregation, (3) de-anchoring (writing criteria before scoring), (4) a held-out gate model that didn't participate in the loop, and (5) a hard cap for citation shortfalls based on actual link measurement.

That said, three points need to be honestly noted.

1. **"No one else built it" doesn't mean "we invented it."** Of the 5 elements, (2), (3), and (4) weren't independently developed by seo-loop — they were taken from LLM-as-judge academic literature already cited by `bizplan-loop`'s `DESIGN.md` (TrustJudge, Rulers, arXiv:2607.05904, Nine Judges Two Effective Votes, etc.); seo-loop merely **ported them into the SEO domain.** The differentiator isn't "inventing a new technique" but "being the first, among actual OSS SEO generators, to integrate a combination of academically validated anti-reward-hacking techniques."
2. **The survey is a sample, not an exhaustive census.** It's limited to what can be found via GitHub search and `gh api` tree exploration (public repos, searchable by English/Korean keywords). Private SaaS internal implementations (the actual engines behind Surfer/Clearscope/MarketMuse) had no code access, so they were confirmed only at the product-documentation/review level — this should be read as "not confirmed at the level of public documentation," not as "the feature doesn't exist."
3. **Our own anti-reward-hacking mechanisms aren't complete either.** As confirmed in §3.4, the current length canary is a single-axis defense. Applying the Reward Bias Substitution paper's logic directly, our loop **potentially carries the same vulnerability** — if the length axis is blocked, gaming could shift to another axis (exploiting the keyword-density threshold, padding citation count with low-quality links, etc.). The most substantive gap found in this survey is not "competitors are worse than us" but "our own defense is also only one axis."

**Summary**: The differentiator is real (the only combined implementation within the surveyed scope), but three caveats are recorded alongside it: (a) most of the differentiator's components are ports of already-published academic evidence, (b) the verification scope is limited to public OSS plus documentation level, and (c) our own defense mechanism still has room for improvement (a multi-axis canary).

---

## 5. Proposed Next Steps (Backlog)

Priority key: 🔴 substantive gap (credibility risk) · 🟡 improvement opportunity · ⚪ conditionally deferred (awaiting trigger)

| Priority | Item | Basis | Status |
|---|---|---|---|
| 🔴 | **Add a multi-axis reward-hacking canary**: monitor at least one more axis besides length — e.g., keyword density, exaggerated-adjective density, or padding with low-quality citation links | §3.4, arXiv:2605.27996 (Reward Bias Substitution) | Not implemented |
| 🟡 | **Add an opt-in Korean readability heuristic**: reference `naaaayeonn/AI-literacy-care-Agent`'s eojeol/syllable/technical-term-ratio formula, but explicitly mark it in the report as an "unvalidated heuristic" (must not give unfounded confidence in place of N/A). Ideally cross-validate against an academic-grade formula such as the KCI paper ("A Study on Developing a Formula for Measuring the Difficulty of Korean Reading Texts") before adopting | §3.2 | Not implemented |
| 🟡 | **Review parallelizing rounds inside `score_doc`**: currently `main.rs::par_map` parallelizes only across documents (across the N drafts), while the `for i in 0..rounds` loop in `score.rs::score_doc` makes sequential calls round by round within a single document. auto-seo's `score_step` (L360-374) pattern of firing rounds/prompts in parallel at once via `asyncio.gather` could yield growing latency gains as `--rounds` increases (though thread contention within the `--concurrency` budget needs separate review — unverified) | §2.1, §3.1 | Not implemented / needs review |
| ⚪ | **Defer adopting `pulldown-cmark`, but document the trigger condition**: reconsider if even one of reference-style links, `]` inside a code span, or an autolink bug actually surfaces | §3.3 | Decision maintained (conditional) |
| ⚪ | **FacTool-style deterministic citation verification**: `citation_links` currently only counts "number of URLs" and doesn't check whether those URLs are actually live/relevant. A secondary deterministic check that actually fetches citation URLs and cross-checks string/domain trustworthiness — like the FacTool (tool-augmented verification)/CITETRACER patterns research-loop investigated — could be considered, but it adds a network call, conflicting with the CLI's "reproducibility/offline-first" philosophy — to be reviewed only as an opt-in, e.g. a separate `--verify-citations` flag | Inherited from research-loop §5 (CITETRACER, FacTool) | Idea stage |
| 🟡 | **Follow-up investigation of Yoast's AI layer**: this round only confirmed the fact that "AI functionality was added as a separate layer" without seeing its structure. Worth a follow-up investigation into how far commercial SEO tools' AI layers are automated (simple phrasing suggestions vs. a regeneration loop) | §2.3 | Follow-up investigation needed |
