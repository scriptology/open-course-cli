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
| `n` | **New topic** — start a session with the next untouched curriculum topic. If the curriculum is empty, opens the Curriculum view. If every topic has already been practiced, the curriculum is extended by 3 new topics. |
| `r` | **Review** — open a list of all touched topics (topics with progress). |
| `d` | **Docs** — browse touched topics and view their theory docs. |
| `c` | **Curriculum** — view the full curriculum. |
| `s` | **Settings** — edit profile, provider, batch size, hint mode. |
| `q` | Quit. |

If the app starts and there is no curriculum, it redirects to the Curriculum view automatically.

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
| `m` | Change the current model (opens Settings > Provider > Model). |
| `Enter` | Open the selected topic in Docs. |
| `Esc` | Back to Dashboard. |

Curriculum generation runs in parallel by CEFR level. The screen shows per-level progress: each level displays whether it is waiting, thinking/writing, or complete.

---

## New Topic Session (`n`)

1. App looks at the curriculum and progress.
2. It picks the first topic that has no progress record (`last_practiced` is None).
3. If all curriculum topics are already touched, the app asks the LLM to generate 3 new topics that continue the existing curriculum, taking into account the user's progress and weak areas. These topics are appended to the curriculum.
4. A batch of exercises is generated for the selected topic (using `batch_size` from preferences). All exercises in the batch are generated in a single LLM request.
5. The user sees translation sentences one by one, types the translation, and presses `Enter` to move to the next exercise.

During a session:

- `Enter` submits the answer and advances.
- `Esc` cancels the session and returns to the Dashboard.
- After the last exercise, the app sends the full batch to the LLM for analysis.

---

## Review (`r`)

Shows all touched topics (topics with `last_practiced` set), not only weak ones.

- Default sort: most recently practiced first.
- Press `s` to toggle sort mode:
  - **Last practiced** — descending.
  - **Lowest score** — ascending, so the weakest topics appear first.
- Each line shows topic name, difficulty, current score, and last practiced date.
- Press `Enter` on a topic to start a review session for that topic.
- Press `Esc` to return to the dashboard.

---

## Docs (`d`)

- Lists touched topics (or all curriculum topics if nothing has been practiced yet).
- `s` toggles sort.
- `Enter` opens the selected topic's AI-generated explanation.
- In the topic view, `↑/↓` scrolls, `e` regenerates the review, and `p` starts a practice session for that topic.
- Cache is used when available; otherwise the explanation is generated live.
- Press `Esc` to go back.

---

## Session Report

After all answers are submitted, the report shows:

- Overall session score.
- For each exercise: the user's answer, the correct translation, errors, and commentary/feedback.
- Updated topic scores and a list of weak topics.
- New topics discovered during analysis.

Available keys:

| Key | Action |
|-----|--------|
| `n` | **New topic** — start a session with the next untouched topic. |
| `r` | **Repeat** — start a new session with the same topic. |
| `d` | **Docs** — open the theory documentation for the current topic. |
| `Esc` | Return to the dashboard. |
| `↑/↓` | Scroll if the report is long. |

---

## Settings (`s`)

Sections:

- **Provider**: provider, API key, base URL, endpoint, model.
- **Profile**: native/target language, age, CEFR.
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
