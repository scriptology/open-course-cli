# Git Conventions

This is a single-package repo on GitHub (not a monorepo, no ticket tracker), so
conventions are simpler than the multi-package/GitLab pattern this was ported from.

## Format

- Branch: `<type>/<subject-hyphen-simplified>` (e.g. `fix/gemini-empty-tools`).
- Commit message: `<type>(<scope>): <description>` — subject only, no body unless asked.
  `<scope>` is optional (module/view name, e.g. `settings`, `onboarding`). Imperative mood,
  lowercase after the colon (`fix(onboarding): reset stale base url on provider switch`).
- PR title: same format as commit message: `<type>(<scope>): <subject>`.

`type` is one of the conventional-commits types: `feat`, `fix`, `docs`, `style`, `refactor`,
`perf`, `test`, `ci`, `chore`, `revert`.

## Claude-specific instructions

- PR description follows `.github/pull_request_template.md`.
- Use `gh pr create` (this repo is on GitHub, not GitLab — no `glab`).
- No ticket prefix — this repo has no issue tracker tie-in.
- **Never** add `Co-Authored-By` lines to commits.
- Commits simple: subject only, no body. Body only if asked.
- Don't delete the source branch on create; use `gh pr merge --delete-branch` at merge time
  if the user asks to merge.
