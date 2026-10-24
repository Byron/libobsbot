# Repository workflow

## Commits

- Commit after completing each semantically distinct step, rather than waiting
  until the end of the task.
- Always author agent-created commits as the responsible agent, using its own
  name and email explicitly. Never inherit the configured user's author identity
  or attribute agent work to the user. For Codex, use
  `Codex <codex@openai.com>`; with Tix, pass `--author`.
- Keep each commit meaningful on its own, including the related implementation,
  tests, fixtures, and documentation. Run the checks appropriate to that step
  before committing it.
- Use descriptive Conventional Commit titles with a scope, such as
  `feat(cli): add tracking and zoom commands`.
- Write commit bodies in Markdown, changelog style: explain the resulting
  behavior, list the important changes, and record validation and any relevant
  limitations.
