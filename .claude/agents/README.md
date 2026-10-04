# Project agents

Shared between Nicolas's projects (itsanas, itsaresume); keep the copies
identical. Each reads the project's own `AGENTS.md` and handover rather than
knowing one project. Versioned in the repository so cloud sessions, which do
not see `~/.claude/agents/`, have them too.

| Agent | When | Model |
|---|---|---|
| `locator` | before writing code for a step: where, and the exact names | haiku |
| `redteam` | on the finished diff, before push (the critic, e.g. Rodin, has already seen the plan) | sonnet |
| `docs-sweeper` | after the code is final: catalogue, counts, stale prose, gates | sonnet |
| `persona-user` | before a release: the docs followed literally | sonnet |

None of them commits, pushes, merges or decides. The plan critique stays a
skill (Rodin), once per major step, on the plan.
