# Open Course CLI — User Flow

This document describes the intended user flow for the Rust TUI implementation of Open Course CLI.

---

## Onboarding

First launch shows a step-by-step onboarding wizard:

1. **Native language** — ISO 639-1 code (e.g. `ru`, `en`).
2. **Target language** — ISO 639-1 code (e.g. `es`, `de`).
3. **Age** — optional number used to pick age-appropriate contexts and avoid school/kindergarten topics for adults.
4. **CEFR level** — required selector: `A1`, `A2`, `B1`, `B2`, `C1`, `C2`.
5. **Batch size** — required selector: `2`, `3`, `4`, `5` exercises per session.
6. **Provider** — select LLM provider (OpenAI, Anthropic, Google, DeepSeek, Mistral, OpenRouter, Ollama, Custom).
7. **API key** — enter API key (optional where provider allows).
8. **Base URL** — required for Custom/Ollama; skipped for others.
9. **Model** — pick from discovered models or type model ID manually.

After saving the config, the app runs **Model diagnostics** (connectivity, streaming, exercise generation, analysis, topic review). Once diagnostics finish, the app opens the **Curriculum** view, where you press `g` to generate the initial curriculum.

---

## Dashboard

Main hub after onboarding. Available keys:

| Key | Action |
|-----|--------|
| `n` | **New topic** — start the next balanced session: a due review (weakest decayed topic) or the next untouched curriculum topic; every 3rd session is a review when topics are due. If the curriculum is empty, opens the Curriculum view; if there is nothing to review or learn, the curriculum is extended by 5 new topics. |
| `d` | **Docs** — browse touched topics and view their theory docs. |
| `c` | **Curriculum** — view the full curriculum. |
| `p` | **Pairs** — switch between language pairs or add a new one. |
| `s` | **Settings** — edit profile, provider, batch size, hint mode. |
| `q` | Quit. |
| `↑`/`↓` | Show and move the selector in the **Weak topics** block (top 5 weakest topics). |
| `Enter` | Start a review session for the selected weak topic. |
| `Esc` | Hide the weak-topics selector. |

If the app starts and there is no curriculum, it redirects to the Curriculum view automatically.

The **Activity** block is a GitHub-style calendar of the current month: quiet days have no background, active days are shaded in four greens computed on a relative scale (the 90th percentile of the user's own daily totals, so a single spike does not flatten the rest), and today is lightly highlighted.

---

## Language pairs (`p`)

Switch between language pairs or add a new one.

- Each pair has its own isolated database folder: `.open-course-cli/pairs/{native-target}/db`.
- Progress, session history, curriculum, and reviews are stored per pair.
- Provider settings and preferences are shared across all pairs.

Available keys:

| Key | Action |
|-----|--------|
| `↑`/`↓` or `j`/`k` | Select pair |
| `Enter` | Switch to the selected pair |
| `a` | Add a new pair (asks for native/target language, age, CEFR) |
| `Esc` | Back to Dashboard |

After switching, the dashboard and all sections reflect the active pair's data. If the newly selected pair has no curriculum, the app redirects to the Curriculum view so you can generate it.

---

## Curriculum (`c`)

Lists all generated curriculum topics.

When empty:

```
Curriculum

No curriculum loaded. Press 'g' to generate.
```

Available keys:

| Key | Action |
|-----|--------|
| `g` | Generate the full curriculum. |
| `r` | Reset curriculum, progress, and reviews, then regenerate (only when non-empty). |
| `a` | Add 5 new topics extending the existing curriculum (only when non-empty). |
| `s` | Toggle sort: progression vs score (only when non-empty). |
| `Enter` | Open the selected topic in Docs (`Esc` there returns to this list). |
| `Esc` | Back to Dashboard. |

Curriculum generation runs in parallel by CEFR level. The screen shows per-level progress: each level displays whether it is waiting, thinking/writing, or complete.

---

## New Topic Session (`n`)

1. App looks at the curriculum and progress.
2. It balances new material against spaced review (`pick_next_session_topic`):
   - **Review candidate** — practiced topics whose effective mastery decayed below 50 (mastery decays ~5%/day since `last_practiced`), weakest first.
   - **New candidate** — the first curriculum topic with no progress record (`last_practiced` is None).
   - With both candidates present, every 3rd session is a review (every 2nd when 5+ topics are due); with only one kind, that kind is taken.
3. If there is nothing to review and nothing new, the app asks the LLM to generate 5 new topics that continue the existing curriculum, taking into account the user's progress and weak areas. These topics are appended to the curriculum.
4. The loading screen shows what was picked: "Review: {topic}" or "New topic: {topic}".
5. A batch of exercises is generated for the selected topic (using `batch_size` from preferences). All exercises in the batch are generated in a single LLM request.
6. The user sees translation sentences one by one, types the translation, and presses `Enter` to move to the next exercise.

During a session:

- `Enter` submits the answer and advances.
- `Esc` cancels the session and returns to the Dashboard.
- After the last exercise, the app sends the full batch to the LLM for analysis.

---

## Docs (`d`)

- Lists touched topics (or all curriculum topics if nothing has been practiced yet).
- `s` toggles sort.
- `Enter` opens the selected topic's AI-generated explanation.
- In the topic view, `↑/↓` or the mouse wheel scrolls, `e` regenerates the review, and `p` starts a practice session for that topic.
- Cache is used when available; otherwise the explanation is generated live.
- Press `Esc` to go back. When Docs was opened directly from another screen (Enter on a topic in Curriculum, or `d` on the session report), `Esc` returns to that screen instead of the docs list.

### Mouse: scroll vs text selection

On the Dashboard, Report, Docs, and Curriculum screens the mouse wheel scrolls the content (or moves the list selection), while the command bar stays pinned at the bottom. Because the terminal sends all mouse activity to the app in this mode, plain native drag-selection is disabled; you have two ways to copy text:

- Press `m` to toggle mouse capture off — drag and copy natively, then press `m` again to get wheel scrolling back. The hint bar shows which mode is active.
- In most terminals, Shift+drag (Option+drag in iTerm2) selects text even while mouse capture is on.

---

## Session Report

After all answers are submitted, the report shows:

- Overall session score.
- For each exercise: the user's answer, the correct translation, errors, and commentary/feedback.
- Updated topic scores and a list of weak topics.
- New topics discovered during analysis. Only generalizable patterns (agreement rules, word-order patterns, conjugation classes) become curriculum topics; word-specific confusions (e.g. `Adjective: Caro vs Rico`) never do.
- **Learning items** — small concrete targets (e.g. a word pair or micro-pattern such as `pequeño/pequeña`) that do not deserve a full topic. These are stored separately in the `learning_items` table, listed in the report under "Added to review", and automatically sprinkled into future exercises so weak micro-points keep reappearing. Re-discovering an existing item does not reset its score.

Available keys:

| Key | Action |
|-----|--------|
| `n` | **New topic** — start a session with the next untouched topic. |
| `r` | **Repeat** — start a new session with the same topic. |
| `d` | **Docs** — open the theory documentation for the current topic. |
| `m` | **Mouse mode** — toggle between wheel scrolling and native text selection. |
| `Esc` | Return to the dashboard. |
| `↑/↓` or wheel | Scroll if the report is long. |

---

## Settings (`s`)

Sections:

- **Provider**: provider, API key, base URL, endpoint, model.
- **Profile**: age and CEFR for the active pair. To change languages, add a new pair from the Pairs screen.
- **Session**: batch size (2–5), hint mode (auto/on-demand).
- **Data**: reset progress, history, curriculum, reviews, or all data.

Changes are saved immediately. Press `Esc` to return to the dashboard.

---

## Dynamic Learning Plan

The curriculum is not static. When the user keeps requesting new topics (`n`), the app:

1. Tracks which topics have been practiced.
2. Once every topic in the current curriculum has been touched, it asks the LLM for 3 new topics that logically continue the plan.
3. The extension request includes existing topics, their difficulty, and the user's progress/scores, so the LLM can reinforce weak areas or introduce prerequisites for difficult topics.

You can also manually extend the curriculum from the Curriculum view with `a` (add 5 topics) or regenerate it from scratch with `r`.
