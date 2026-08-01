# seo-loop 리서치 서베이

## 1. 개요

`seo-loop`는 Loop-Suite의 `bizplan-loop`가 정립한 "**N개 생성 → 결정론적 룰체크 → LLM 루브릭 채점(de-anchoring) → 절사평균 집계 → 재생성 → held-out 게이트 재채점**" 구조를 SEO 블로그/랜딩페이지 카피 생성 도메인으로 포팅한 Rust CLI다. discourse 기반 익명 교차토론(Code-Review-Loop/research-loop 계열)과는 다른 계열로, 페르소나 간 강제 반박이 아니라 "심사위원 패널(서로 다른 모델·관점) × 여러 라운드 → 통계적 절사평균"으로 분산을 줄이는 접근이다.

이번 조사의 목적은 두 가지다.

1. **초기 라운드(README/랜딩페이지 수준)에서 이미 "구조가 유사하다"고 결론 내렸던 경쟁 OSS(`sour4bh/auto-seo`, BlogPilot)를 이번엔 실제 소스 파일을 `gh api`로 직접 읽어 재검증**하고, 결론이 부정확했던 지점은 명시적으로 자기교정한다.
2. 초기 라운드에서 다루지 않았던 **AI SEO 콘텐츠 생성 OSS 프레임워크(LangGraph/CrewAI), 한국어 가독성 지표 OSS, `pulldown-cmark` 재도입 여부, reward-hacking 방지 학술 근거**를 새로 조사한다.

방법론은 research-loop의 리서치 서베이(`Loop-Suite/research-loop/docs/research-and-evidence-survey-2026-07-31.md`)가 §8("후속 조사")에서 실천한 방식 — *README만 보고 결론 내지 않고, 실제 소스 파일(`skills/researcher.py`, `backend/nodes/*.py` 등)을 직접 읽어 아키텍처를 검증하고, 그 과정에서 README와 코드가 어긋나는 지점을 찾아 자기교정하는 것* — 을 그대로 따른다.

---

## 2. 이전 조사 재검증 (자기교정 포함)

### 2.1 `sour4bh/auto-seo` — "구조가 가장 유사하다"는 초기 결론의 재검증

**초기 결론(재검증 대상)**: "룰 7개 + LLM평가 6개 hybrid 스코어링, Claude+Gemini 멀티모델 합의 — 우리와 구조가 가장 유사하다."

**이번에 실제로 읽은 파일**: `gh api repos/sour4bh/auto-seo/contents/app/article/scorer.py`(342줄), `app/article/pipeline.py`(1291줄) 중 `score_step`(L319-422), `review_step`(L425-494), `edit_step`(L497-539), `_merge_score_dimensions`(L812-829), `run_pipeline`의 edit 루프(L1244-1276).

검증 결과, 표면적 유사성("rule + LLM 합의")은 맞지만 **세 가지 구조적 차이가 소스 레벨에서 명확히 확인됐다** — 초기 라운드는 이 세 가지를 검증하지 않은 채 "가장 유사"라고 단정했으므로 이 부분을 정정한다.

| 항목 | 초기 라운드 추정 | 소스 재검증 결과 |
|---|---|---|
| 초안 생성 | (확인 안 함, "hybrid 스코어링"에만 집중) | `writePost`/`writer.ts`류가 아니라 `pipeline.py`의 `generate_step`(코드는 안 봤지만 `run_pipeline`의 `STEP_SEQUENCE`가 `generate → score → review → (edit↔score↔review 루프)`임을 `L1230-1274`에서 확인)이 **문서 1건을 생성해 그 문서를 제자리에서 계속 고쳐 쓴다.** `seo-loop`의 `generate.rs`처럼 각도(angle)를 바꿔 **N개의 독립 초안**을 만들고 그중 최고점을 고르는 구조가 아니다 |
| 집계 방식 | "hybrid 스코어링" | `score_step`(L360-414)이 3개 LLM 프롬프트 × 컨실(council) 모든 provider를 병렬 호출(`asyncio.gather`, L374)한 뒤 `_merge_score_dimensions`(L812-829)에서 **동일 이름 차원(dimension)의 점수를 단순 평균**한다(`avg = sum(d.score for d in group) / len(group)`). `score.rs::trimmed_mean`처럼 n≥4일 때 최댓값·최솟값을 버리는 절사평균이 아니다 |
| held-out 검증 | (확인 안 함) | `run_pipeline`의 edit 루프(L1244-1276)는 매 반복마다 **같은 `council`**(전역 `get_llm_council()`)이 다시 점수를 매긴다. 루프에 한 번도 참여하지 않은 별도 모델이 최초본·최고본만 재채점하는 `--gate-model`류 장치는 코드 어디에도 없다 |

**자기교정**: 초기 라운드는 "룰 7개 + LLM 6개, 멀티모델 합의"라는 README/문서 수준 서술만으로 "구조가 가장 유사하다"고 판단했다. 소스를 읽어보니 유사성은 "결정론적 룰 체크 + LLM 채점을 병행한다"는 상위 수준에 그치고, **auto-seo에는 (1) N개 독립 초안 생성·선택, (2) 절사평균, (3) held-out 게이트라는 seo-loop의 핵심 반-reward-hacking 장치 세 가지가 모두 없다.** "가장 유사한 경쟁 프로젝트"라는 표현은 과장이었고, 정확히는 "hybrid 채점이라는 상위 개념만 공유하고 reward-hacking 방지 메커니즘은 없는 프로젝트"로 정정한다.

추가로 확인한 세부사항(초기 조사에 없던 내용): `score_readability`/`score_humanity`(scorer.py L147-252)는 `job.language == "en"`일 때만 실행된다(`pipeline.py` L346-348). seo-loop의 라틴 문자 비중 50% 휴리스틱(`checks.rs::readability`, `latin/total < 0.5 → None`)과 달리 **단일 언어 플래그** 기반이라는 점에서 더 거칠다 — 영어가 아니면 무조건 건너뛰고, "영어인데 라틴 문자 비중이 낮은 혼합 문서"는 구분하지 못한다.

### 2.2 BlogPilot — "Flesch 로직 포팅"만 확인했던 것을 전체 구조로 확장 검증

**초기 결론(재검증 대상)**: "39모듈, TF-IDF+E-E-A-T+SERP감지. Flesch 가독성 로직을 포팅함(사실관계 자체는 맞음). 이 저장소에 우리처럼 'N개 생성→다중라운드 채점→held-out gate'가 있는지는 readability.ts만 봤지 확인 안 함."

**이번에 실제로 읽은 파일**: `src/app/api/draft/route.ts`(98줄), `src/lib/seo/writer.ts`(70줄), `src/app/api/content-score/route.ts`(48줄), `src/lib/seo/content-score.ts`(278줄, 함수 목록 grep으로 확인: `grade`, `median`, `jaccardWords`, `competitorDocFrequency`, `competitorMedianCount`, `scoreDraft`).

- `writer.ts::writePost`(L40-70)는 `execute({ methodologies: [...], task: "Write the full post in Markdown following the outline exactly.", ... temperature: 0.7, maxTokens: 6000 })`를 **한 번** 호출해 마크다운 전체를 만든다. provider가 없으면 `fallbackPost`(정적 템플릿)로 대체한다. N개 각도 변형 초안을 만드는 로직은 없다.
- `content-score.ts`에서 LLM 호출 관련 심볼(`execute`, `LlmClient`, `provider.`, `ai/executor` 등)을 `grep`했으나 **한 건도 나오지 않았다.** `scoreDraft`(L109~)는 `jaccardWords`(경쟁사 본문과의 자카드 유사도), `competitorDocFrequency`/`competitorMedianCount`(경쟁사 코퍼스 대비 용어 빈도, TF-IDF 계열), `median`만으로 점수를 낸다 — **LLM 루브릭 채점이 전혀 없는 순수 결정론적 스코어러**다.
- `draft/route.ts`(L33-42)는 `writePost` 결과를 그대로 DB에 저장하고 끝난다. 재채점→재생성 루프 자체가 이 경로에는 없다(별도로 `content-score` API를 사용자가 수동으로 호출해 에디터에서 점수를 보는 구조, `content-score/route.ts` 주석 L7-9: "client passes prefetched corpus; runs scoring locally for fast keystroke updates" — 즉 사람이 타이핑하며 실시간 피드백을 받는 **에디터 보조 도구**이지, 자동 재생성 루프가 아니다).

**자기교정**: 초기 라운드의 "TF-IDF+E-E-A-T+SERP감지"라는 서술 자체는 틀리지 않았지만, "readability.ts만 봤지 전체 구조는 안 봤을 수 있다"는 우려가 실제로 맞았다 — **BlogPilot에는 LLM 루브릭 채점 자체가 없다**(콘텐츠 스코어링은 100% 결정론적 TF-IDF/자카드 유사도이고, LLM은 글쓰기·리서치·아웃라인 등 생성 단계에서만 쓰인다). 이는 auto-seo보다도 seo-loop 구조에서 더 멀다 — "N개 생성" 없음, "LLM 루브릭" 없음, "held-out gate" 없음. 초기 결론이 방향은 맞았지만 근거가 얕았던 사례다.

### 2.3 Yoast SEO — "코드는 안 읽었지만 순수 룰기반"이라는 결론의 아키텍처 재확인

**초기 결론(재검증 대상)**: "규칙기반 assessment 모음, LLM 루브릭 없음. 코드는 안 읽었음(라이선스 문제로 포팅 금지)."

라이선스(GPL-2.0)상 코드 포팅은 여전히 불가하므로 **디렉터리 구조·파일명 수준의 아키텍처 확인**만 수행했다(내용을 옮기지 않음, 구조 확인 목적의 열람).

- `Yoast/YoastSEO.js`는 이제 **archived**(단독 레포는 폐기, `Yoast/wordpress-seo` 모노레포로 흡수됨) — 초기 조사에는 없던 사실.
- `Yoast/wordpress-seo` 트리에서 확인: `packages/yoastseo/src/scoring/assessments/`에 `assessment.js`(베이스 클래스) 아래로 `readability/`(`ParagraphTooLongAssessment.js`, `PassiveVoiceAssessment.js`, `SentenceLengthInTextAssessment.js`, `TransitionWordsAssessment.js`, `WordComplexityAssessment.js` 등)와 `inclusiveLanguage/`(`InclusiveLanguageAssessment.js` + 카테고리별 `configuration/*Assessments.js`)가 **개별 규칙 클래스**로 나뉘어 있다.
- 같은 디렉터리에 `SCORING READABILITY.md`, `SCORING SEO.md`, `SCORING SEO PRODUCT.md`, `SCORING TAXONOMY.md`, `KEYPHRASE MATCHING.md`라는 **Yoast 자체 설계 문서**가 있다(이 문서들의 존재 자체가 "점수 산정 방식이 공개된 결정론적 규칙"이라는 정황 근거다 — 내용을 옮기지 않고 존재만 확인).

**결론**: 초기 라운드의 "순수 룰기반, LLM 없음"이라는 결론은 아키텍처 수준에서 재확인된다. 다만 2026년 검색 결과(Yoast 공식 블로그 "SEO 2026 Predictions")에서 "AI-powered features"를 별도로 추가했다는 언급이 나온다 — **핵심 콘텐츠 분석/점수 엔진(`scoring/assessments/*`)은 여전히 규칙 기반이고, AI 기능은 그 위에 얹힌 별도 레이어(예: AI 기반 문구 제안)로 보인다.** 이 구분을 초기 조사는 몰랐으므로 "Yoast = 순수 룰기반"이라는 단정은 "핵심 스코어링 엔진은 룰기반, 최근 AI 보조기능이 레이어로 추가됨"으로 정밀화한다(다만 AI 레이어의 구체 구조는 이번 조사 범위에서 코드 확인 못함 — 불확실).

### 2.4 Surfer / Clearscope / MarketMuse — "AI가 여러 초안을 자체 재채점하는 기능이 실제 있는지" 재확인

**초기 결론(재검증 대상)**: "경쟁사 상위 페이지 스크래핑 기반 커버리지 점수. AI가 여러 초안을 생성하고 자체적으로 다시 채점하는 기능이 실제로 있는지는 마케팅 문구 수준에서만 봤음."

제품 문서/리뷰 수준(공식 API 문서까지는 공개돼 있지 않아 접근 불가, 서드파티 상세 리뷰 기준)으로 재확인:

- **Surfer AI**: 키워드 1개당 GPT-4 Turbo로 **완성된 초안 1개**를 15~20분에 생성하고, Content Editor의 "Content Score"(0-100)는 사람이 타이핑하는 동안 실시간으로 갱신되는 **정적 유사도 채점**(SERP 코퍼스 대비)이다. "여러 초안을 만들어 자동으로 비교·재채점"하는 기능은 검색된 리뷰·가이드 어디에도 없었다.
- **Clearscope**: 초안을 붙여넣으면 F~A++ 등급을 매기고, 사람이 수정해서 다시 붙여넣는 **수동 반복**(paste → grade → edit → paste again)이 워크플로다.
- **MarketMuse**: "First Draft" 기능이 최대 5,000단어 스캐폴드를 **한 번** 생성한다("more basic AI drafting, primarily for supplements"라는 표현 — 보조적 초안 생성으로 위치지어짐).

**결론**: 세 제품 모두 "AI가 N개 후보를 만들고 자동으로 비교해 최고점을 고르는" 기능은 제품 문서/리뷰 수준에서 확인되지 않았다 — **사람이 루프의 일부(수정→재점검)를 수행하는 반자동 구조**이지, seo-loop/auto-seo/BlogPilot 같은 완전 자동 재생성 루프가 아니다. 초기 결론이 재확인됐고, 이번 라운드는 "마케팅 문구가 아니라 실제 워크플로 서술(15~20분 소요, paste-and-grade 등 구체적 UX 묘사)"까지 근거 밀도를 높였다는 차이가 있다.

---

## 3. 신규 조사

### 3.1 AI SEO 콘텐츠 생성 OSS 프레임워크 (LangGraph/CrewAI) — reward-hacking 방지 장치 존재 여부

GitHub 검색(`seo blog generator langgraph`, `seo content crewai`)으로 나온 프로젝트 대부분은 ★0~2의 학습용/토이 프로젝트였다(`renswickd/multi-agent-blog-generator`, `SuyashMohanty/Agentic-AI-Blog-Generator`, `Mahul777/langgraph-multilingual-blog-generator` 등 — 다국어 번역 기능이 핵심 차별점으로 반복 등장할 뿐, 채점/reward-hacking 방지 구조는 부재). 이 중 **구조가 가장 갖춰진** `christancho/blogging-with-langchain`(LangGraph + Claude + Ghost CMS, "approval gate workflow" 명시)을 실제 소스로 검증했다.

**실제 읽은 파일**: `agentic/graph.py`(275줄), `agentic/nodes/editor.py`(244줄).

- `graph.py`(L59-118)의 워크플로: `research → audience_analysis → writer → fact_checker(↔writer 재작성 루프, 최대 3회) → formatter → seo → editor(↔writer 재작성 루프, 최대 3회) → publisher`. **선형 파이프라인 + 조건부 엣지 2개**(`route_fact_check_decision`, `route_editor_decision`)로 이뤄져 있다 — seo-loop처럼 N개 독립 초안을 만들어 비교하는 구조가 아니라 **단일 문서를 계속 고쳐 쓰는** 구조다(BlogPilot/auto-seo와 같은 패턴).
- `editor.py::editor_node`(L16-244)가 승인 게이트다. 그런데 `llm = Config.get_llm()`(L85)으로 **글을 쓴 것과 같은 LLM 설정을 그대로 재사용**해 JSON 1회 호출로 `cohesiveness_score`/`hook_score`/`storytelling_score`/`voice_score`를 매긴다. 루프에 참여하지 않은 별도 모델이 검증하는 held-out 장치는 없다.
- `max_revisions`(기본 3) 초과 시 "FORCING PUBLICATION WITH NOTE"(L177)로 강제 발행한다 — seo-loop의 `--patience`/`stall` 조기종료(`loop_run.rs`)와 목적은 비슷하지만, **독립적 검증 신호 없이 그냥 포기하고 발행**한다는 점에서 다르다.

**결론**: LangGraph/CrewAI 기반 SEO 콘텐츠 생성 OSS 생태계에서 가장 구조가 갖춰진 사례를 소스 레벨로 확인해도, seo-loop의 핵심 4요소(N개 독립 초안 생성, 다중 모델·다중 라운드 절사평균, de-anchoring, held-out 게이트) 중 **어느 것도 발견되지 않았다.** 이는 §2.1/§2.2의 결론과 정확히 같은 패턴이다 — "같은 모델이 자기가 쓴 글을 자기가 승인한다"는 self-preference 구조(bizplan-loop DESIGN.md §10이 경고하는 바로 그 문제)가 OSS SEO 생성 파이프라인 전반의 공통 패턴이다.

### 3.2 Flesch 외 한국어(비영문) 가독성 지표 오픈소스

`textstat`(auto-seo가 쓰는 파이썬 가독성 라이브러리, Flesch 계열)도, BlogPilot의 `readability.ts`도 영어 전용이다. seo-loop는 라틴 문자 비중 50% 미만이면 `None`(N/A)을 반환해 한국어에는 아예 적용하지 않는다(`checks.rs::readability`).

GitHub 코드 검색(`가독성 language:Python filename:readability`)에서 발견한 `naaaayeonn/AI-literacy-care-Agent`(★3, Python — 문해력 취약계층 지원용 프로젝트로 추정)에 **한국어 전용 가독성 공식 구현체와 그 설계 문서**가 있었다.

**실제 읽은 파일**: `2. Content & RAG Agent/docs/READABILITY_FORMULA.md`(147줄).

공식(문서 원문 그대로):

```
readability_score =
  100
  - (avg_words_per_sentence × 1.015)
  - (avg_syllables_per_word × 8.0)
  - (technical_term_ratio × 35.0)
```

- `avg_words_per_sentence`: 공백 기준 어절 수(한국어는 형태소가 아니라 어절이 기본 단위이므로 영어 word-split과 다른 전제) / 문장 수(`다.`, `요.`, `.`, `!`, `?` 뒤 공백으로 분리).
- `avg_syllables_per_word`: `가-힣` 유니코드 범위 문자 1개 = 음절 1개로 세는 방식(영어처럼 모음 패턴을 추정할 필요가 없어 오히려 정확함 — 한글은 음절 블록 문자라서 문자 수 = 음절 수가 정확히 성립한다).
- `technical_term_ratio`: 영어 3자 이상 단어(`LLM`, `API`) + 한국어 전문용어 접미사 패턴(`화·율·성·도·적·론·법·형·식·계·기·학`)의 어절 대비 비율.

**평가**: 이 공식은 Flesch의 상수(206.835, 1.015, 84.6)를 그대로 가져온 게 아니라 **한글 음절=문자라는 특성을 살려 재설계**했고, 자체 검증 예시(문서 §5)와 해석표(§4)도 갖추고 있다. 다만 (1) ★3, 커밋 이력이 짧은 개인/학생 프로젝트로 보이고, (2) 계수(1.015/8.0/35.0/100)에 대한 **동료검토된 학술적 근거를 문서 스스로 밝히지 않는다**(§7 "한계"에 "구어체/신조어 미반영", "문맥 무시"를 자인) — 즉 **경험적으로 튜닝된 휴리스틱**이지 검증된 공식은 아니다.

별도로 KCI(한국학술지인용색인) 검색에서 "한국어 읽기 텍스트의 난이도 측정 공식 개발에 관한 연구"라는 **동료검토 학술 논문**(문법 난이도 + 어휘 난이도 결합 모형)이 존재함을 확인했으나, 이를 구현한 오픈소스 코드는 발견하지 못했다. 즉 "학술적으로 검증된 공식은 있지만 OSS 구현이 없고, OSS 구현은 있지만 학술적 검증이 없다"는 간극이 현재 상태다 — §5 백로그에 반영.

### 3.3 `pulldown-cmark` 도입 여부 재검토

**초기 결론(재검증 대상)**: "불필요."

이번 조사 시점의 `checks.rs::scan_links`/`find_matching_bracket_close`(L131-188)를 다시 읽었다. README의 "Multi-lens review findings applied" 절(L200-201)에 따르면 이번 리뷰 라운드에서 **중첩 대괄호(`[a[b]c](url)`) depth 추적과 이스케이프(`\[`/`\]`) 처리가 새로 추가됐다** — 즉 수작업 마크다운 토크나이저가 두 번째로 "직접 만든 파서의 엣지 케이스를 손으로 패치"하는 사이클을 겪은 것이다(첫 번째는 CRLF 프론트매터 오프셋 버그 수정).

재검토 결과:

- `scan_links`가 처리해야 하는 입력은 **LLM이 생성한 단일 문서**(spec의 frontmatter + H1~H6 + 표준 `[text](url)`/`![alt](url)` 문법)로 범위가 좁다. reference-style 링크(`[text][ref]`), 각주, 표, 인라인 HTML, 오토링크(`<https://...>`)는 spec/README 어디에도 요구사항으로 없다.
- `pulldown-cmark`(현재 crates.io 최신 `0.13.4`, `cargo search` 확인)를 도입해도 `checks.rs`가 필요로 하는 것은 여전히 (1) 프론트매터 바이트 오프셋 분리(cmark의 범위 밖 — CommonMark는 YAML frontmatter를 표준으로 다루지 않음, `pulldown-cmark-frontmatter`라는 **별도 크레이트**가 필요), (2) heading level/text, (3) link/image의 `(is_image, label, url)` 3튜플, (4) 가독성 계산을 위한 "마크다운 제거 후 순수 프로즈" 텍스트다. `Event` 스트림에서 이 4가지를 재조립하는 코드는 여전히 손으로 짜야 하므로, **의존성 하나(+ frontmatter용 크레이트 하나)를 추가하는 비용 대비 순수 코드량 절감은 크지 않다.**

**결론(재확인, 단 조건부)**: 현재 시점에는 "불필요" 결론이 유지된다. 그러나 이번이 **두 번째** 수작업 엣지케이스 패치였다는 사실은 기록해둘 가치가 있다 — 초기 조사처럼 "불필요"라고 단정만 하지 않고, **전환 트리거 조건을 명시**한다: 만약 이후 리뷰에서 (a) reference-style 링크, (b) 코드 스팬 안의 `]` 문자, (c) 오토링크 중 하나라도 실제 버그로 발견되면, 그 시점부터는 "직접 만든 토크나이저를 계속 패치"하는 대신 `pulldown-cmark` 도입을 재검토해야 한다. 지금은 "결론 재확인"이지만 판단 근거를 매번 처음부터 다시 만들지 않도록 조건을 문서화하는 것이 이번 재검증의 실질적 산출물이다.

### 3.4 de-anchoring / LLM-as-judge reward-hacking 방지 추가 학술 근거

seo-loop README와 bizplan-loop `DESIGN.md`가 이미 인용 중인 근거(중복 확인, 새로 찾은 것 아님): de-anchoring과 held-out 게이트의 1차 근거는 **arXiv:2607.05904**("More Convincing, Not More Correct: Self-Play Reward Hacking of Reference-Free LLM Judges")로, `bizplan-loop/DESIGN.md` §4·§5가 이미 정확히 이 논문을 인용하고 있다(judge가 후보를 보기 전에 기준을 먼저 쓰게 하면 false positive rate 0.719→0.012, self-play로 judge 통과율 0.716→0.938인데 실제 정확도는 0.209→0.202로 정체). 이번 조사에서 이 논문을 다시 찾았다는 것 자체가 — **기존 근거가 여전히 최신·최적임을 재확인**했다는 뜻이다.

과제 지시대로 **추가로 하나 더** 찾은 근거는 다음과 같다.

- **Reward Bias Substitution: Single-Axis Bias Mitigations Redirect Optimization Pressure** (Lamparth, Fein, Haupt, Hussing, Kochenderfer — arXiv:2605.27996). 핵심 주장(초록 원문 인용): *"Single-axis mitigations of reward-model biases (e.g., reducing proxy reliance on length, sycophancy, or style) can rotate optimization pressure onto correlated proxies."* 즉 **길이 같은 편향 축 하나를 억제하면, 최적화 압력이 그와 상관된 다른(관측되지 않는) 축으로 옮겨간다.** 논문이 드는 RLHF 사례: 길이 페널티를 걸면 응답은 짧아지지만 모델이 대신 **과신감(overconfidence)** 쪽으로 편향된다.

**seo-loop에 대한 함의**: `loop_run.rs`의 길이 인플레이션 canary(L109-125, "분량 +25%인데 점수 +5 미만이면 경고")는 정확히 이 논문이 말하는 "single-axis mitigation"이다. 지금은 **분량이라는 축 하나만** 감시한다 — 만약 생성 모델이 "분량을 늘리면 걸린다"는 걸 학습하면, Reward Bias Substitution 논문의 논리대로 **관측되지 않는 다른 축**(예: `keyword_naturalness` 루브릭이 허용하는 한계까지 키워드를 욱여넣기, `citation_required` 60점 상한을 피하기 위해 품질과 무관하게 링크 개수만 채우기, 화려한 형용사·과장된 수치 남발)으로 게이밍이 옮겨갈 수 있다는 것이 이 논문의 예측이다. 현재 seo-loop의 방어는 길이 축 하나뿐이므로, 이는 실제 갭이다 — §5 백로그에 반영.

---

## 4. 종합 결론 — 우리 아키텍처가 실제로 차별점이 있는가 (정직한 평가)

이번 라운드에서 소스 레벨로 재검증한 6개 프로젝트/제품(auto-seo, BlogPilot, Yoast, Surfer, Clearscope, MarketMuse)과 새로 조사한 LangGraph/CrewAI SEO 생성 OSS 생태계(blogging-with-langchain 포함) **전체를 통틀어, seo-loop의 5개 요소 — (1) N개 각도-변형 독립 초안 생성, (2) 다중 모델·다중 라운드 절사평균 집계, (3) de-anchoring(채점 전 기준 선-서술), (4) 루프에 참여하지 않은 held-out 게이트 모델, (5) 링크 실측 기반 인용-부족 하드캡 — 을 동시에 갖춘 사례는 하나도 발견되지 않았다.**

다만 정직하게 짚어야 할 점이 세 가지 있다.

1. **"아무도 안 만들었다"는 것이 곧 "우리가 발명했다"는 뜻은 아니다.** 5개 요소 중 (2)(3)(4)는 seo-loop가 독자 개발한 게 아니라 `bizplan-loop`의 `DESIGN.md`가 이미 인용한 LLM-as-judge 학술 문헌(TrustJudge, Rulers, arXiv:2607.05904, Nine Judges Two Effective Votes 등)에서 가져온 것이고, seo-loop는 이를 **SEO 도메인에 이식**했을 뿐이다. 차별점은 "새로운 기법의 발명"이 아니라 "학술적으로 검증된 반-reward-hacking 기법 조합을 실제 OSS SEO 생성기 중 최초로 통합 적용했다"는 것에 있다.
2. **조사는 표본이지 전수조사가 아니다.** GitHub 검색·`gh api` 트리 탐색으로 찾을 수 있는 범위(공개 레포, 영어/한국어 키워드로 검색되는 것)에 한정된다. 비공개 SaaS 내부 구현(Surfer/Clearscope/MarketMuse의 실제 엔진)은 코드 접근이 불가능해 제품 문서·리뷰 수준으로만 확인했다 — "기능이 없다"가 아니라 "공개된 문서 수준에서 확인되지 않는다"로 읽어야 한다.
3. **우리 스스로의 반-reward-hacking 장치도 완전하지 않다.** §3.4에서 확인했듯, 현재 길이 canary는 단일 축 방어다. Reward Bias Substitution 논문의 논리를 그대로 적용하면, 우리 루프도 길이 축을 막으면 다른 축(키워드 밀도 경계값 악용, 저품질 링크로 인용 개수만 채우기 등)으로 게이밍이 이동할 수 있다는 **동일한 취약점을 잠재적으로 가진다.** 이번 조사에서 발견한 가장 실질적인 갭은 "경쟁사가 우리보다 못하다"가 아니라 "우리 자신의 방어도 한 축뿐"이라는 점이다.

**요약**: 차별점은 실재한다(조사된 범위에서 유일한 조합형 구현), 그러나 (a) 그 차별점의 구성 요소 대부분은 이미 공개된 학술 근거의 이식이고, (b) 검증 범위는 공개 OSS + 문서 수준으로 제한적이며, (c) 우리 자신의 방어 장치도 개선 여지(다축 canary)가 있다는 세 가지 유보를 함께 기록한다.

---

## 5. 다음 단계 제안 (백로그)

우선순위 표시: 🔴 실질적 갭(신뢰도 위험) · 🟡 개선 기회 · ⚪ 조건부 보류(트리거 대기)

| 우선순위 | 항목 | 근거 | 상태 |
|---|---|---|---|
| 🔴 | **다축 reward-hacking canary 추가**: 길이 외에 키워드 밀도/과장 형용사 밀도/저품질 인용 링크 채우기 같은 축을 최소 1개 더 감시 | §3.4, arXiv:2605.27996(Reward Bias Substitution) | 미구현 |
| 🟡 | **한국어 가독성 휴리스틱 opt-in 추가**: `naaaayeonn/AI-literacy-care-Agent`의 어절/음절/전문용어비율 공식을 참고하되, "검증되지 않은 휴리스틱"임을 리포트에 명시(N/A 대신 무근거 확신을 주면 안 됨). 가능하면 KCI 논문("한국어 읽기 텍스트의 난이도 측정 공식 개발에 관한 연구")급 학술 공식과 대조 검증 후 도입 | §3.2 | 미구현 |
| 🟡 | **`score_doc`의 라운드 내부 병렬화 검토**: 현재 `main.rs::par_map`은 문서 간(N개 초안 간) 병렬화만 하고, `score.rs::score_doc`의 `for i in 0..rounds` 루프는 한 문서 안에서 라운드마다 순차 호출이다. auto-seo의 `score_step`(L360-374)이 `asyncio.gather`로 라운드·프롬프트를 한 번에 병렬 발사하는 패턴은 `--rounds`를 늘릴수록 지연시간 이득이 커질 수 있다(단, `--concurrency` 예산 내 스레드 경합은 별도 검토 필요 — 미검증) | §2.1, §3.1 | 미구현/검토 필요 |
| ⚪ | **`pulldown-cmark` 도입은 보류, 단 트리거 조건 문서화**: reference-style 링크·코드스팬 내 `]`·오토링크 버그가 실제로 하나라도 나오면 그때 재검토 | §3.3 | 결정 유지(조건부) |
| ⚪ | **FacTool류 결정론적 인용 검증**: 현재 `citation_links`는 "URL 개수"만 세고 실제로 그 URL이 살아있는지/관련 있는지는 보지 않는다. research-loop가 조사한 FacTool(도구증강 검증)·CITETRACER 패턴처럼 인용 URL을 실제로 fetch해 문자열/도메인 신뢰도를 대조하는 2차 결정론적 검사를 고려할 수 있으나, 네트워크 호출이 추가되므로 CLI의 "재현성/오프라인 우선" 철학과 상충 — 별도 `--verify-citations` 플래그 같은 opt-in으로만 검토 | research-loop §5(CITETRACER, FacTool) 상속 | 아이디어 단계 |
| 🟡 | **Yoast AI 레이어 재조사**: 이번엔 "AI 기능이 별도 레이어로 추가됐다"는 사실만 확인했고 그 구조는 못 봤다. 상용 SEO 툴의 AI 레이어가 어디까지 자동화됐는지(단순 문구 제안 vs 재생성 루프) 후속 조사 가치 있음 | §2.3 | 후속 조사 필요 |
