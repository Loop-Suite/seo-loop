# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.1.0] - 2026-08-10

Initial release.

### Added

- `seo gen` / `seo score` / `seo loop` CLI commands: generate SEO content drafts via `claude -p`,
  score them against a spec-driven rubric (deterministic checks + multi-round, multi-lens LLM
  judge panel), and iteratively regenerate against review feedback.
- Deterministic format/on-page checks (`checks.rs`): front matter parsing, heading hierarchy,
  image alt-text, internal/citation link counting, keyword placement, paragraph length, Flesch
  Reading Ease / Flesch-Kincaid readability (English), and an opt-in, explicitly-unvalidated
  Korean readability heuristic.
- Judge scoring: trimmed-mean aggregation across rounds/models, a deterministic 60-point cap for
  `citation_required` criteria when source links are insufficient, and a held-out gate model
  (`--gate-model`) to cross-check loop-mode score gains against a judge that never scored the loop.
- Reward-hacking canaries in loop mode: a length-inflation warning and a keyword-density-spike
  warning when volume/keyword count grows much faster than score.
- `evals/README.md`: an empirical record of two static-review rounds and one real live-execution
  round (issues #2-#9), including real API cost figures for the live-execution verification.
- Dependabot configuration for dependency updates.

### Fixed

- #2 — `generate::build_prompt` no longer hardcodes "Write in Korean"; the model now follows
  whatever language the brief/spec content is actually written in.
- #3 — Keyword matching (`contains_kw`/`count_kw_occurrences`) now respects word boundaries
  instead of matching across a whitespace-stripped substring (e.g. "running shoe sale" no longer
  false-positives on "running shoes").
- #4 — `missing_sections()` section-title matching is now case-insensitive, consistent with
  keyword matching elsewhere in `checks.rs`.
- #5 — The self-scoring warning now also fires when `--judge-model` is explicitly the same single
  model as `--model` (previously only fired when `--judge-model` was omitted entirely).
- #7 — `missing_sections()` now matches required sections by word tokens instead of raw substring
  containment, fixing false positives from unrelated short headings (e.g. a heading "As" no longer
  satisfies a required "Frequently Asked Questions" section).
- #8 — Self-scoring detection (`judge_model_matches_gen`) now catches a repeated-same-model
  `--judge-model` panel (e.g. `sonnet,sonnet`), not just a single-entry exact match.
- #9 — `Llm::text()` now strips a stray outer ` ``` `/` ```lang ` code fence wrapping an entire
  response (`strip_wrapping_fence`), fixing broken title/meta/keyword-in-title detection and null
  Flesch scores that occurred when a model (observed live with `haiku`) wrapped its whole answer
  in a single outer fence.
- #15 — `strip_wrapping_fence()` now recognizes variable-length (4+ backtick) fence markers,
  closing a gap where a CommonMark-valid nested fence — used when the body legitimately contains
  its own ` ``` ` code sample — silently defeated the #9 fix.

### Security

- #6 — `is_internal_url()` now treats non-http(s)-scheme and protocol-relative absolute URLs
  (`mailto:`, `ftp:`, `tel:`, `//host/path`) as external instead of misclassifying them as
  internal, closing a host-spoofing gap in the internal/citation link classifier used for E-E-A-T
  scoring.
- #16 — `is_internal_url()` now normalizes whitespace and backslashes before classification,
  closing a browser-equivalent variant of the #6 host-spoofing gap (per the WHATWG URL Standard,
  browsers treat `\` identically to `/` for http(s) URLs, so `\\evil.example.org/a` and similar
  were still reaching the same "treated as internal" outcome #6 closed for the forward-slash form).
- #17 — `scan_links()` no longer has O(n²) worst-case time complexity on adversarial markdown (a
  long run of unmatched `[`, or many unclosed `[x](` groups both previously triggered repeated
  full-document re-scans); bracket and closing-paren matching are now precomputed once in two
  linear passes. Closes a CPU-exhaustion DoS reachable via any document passed to
  `seo score`/`seo gen`.

### Changed

- Bumped `clap` 4.6.5 → 4.6.6, `toml` 0.8.23 → 1.1.4+spec-1.1.0, `actions/checkout` 4 → 7.
- Added a real-world validation summary (live `seo gen` run findings) to README.

### Testing

- Test suite grown from an initial baseline through the #2-#9 review/fix cycle to 39 tests
  covering every issue above, then expanded to 73 tests with edge-case coverage for empty input,
  large/adversarial documents (including timing regressions guarding the #17 fix), further code
  fence variants, and further URL scheme extremes.
