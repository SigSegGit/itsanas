# Agents

Three agents work on this repository. Each has one job; none of them replaces
the test suite, which remains the only thing that proves a property holds.

| Agent | Role | Runs where | Entry point |
|---|---|---|---|
| **Claude Code** | Architecture and implementation, test first | developer machine | the session itself; resumes from `docs/HANDOVER.md` |
| **AI code reviewer** (provider-agnostic) | CI gatekeeper: reviews the diff of every PR and every push to `main` | GitHub Actions | `.github/workflows/ai-code-reviewer.yml` → `scripts/ci_code_reviewer.py` |
| **Aider + local Qwen** | Local operations and pre-push review, offline, no per-token cost | developer machine, LM Studio | `scripts/local_ai_helper.sh`, hook in `docs/LOCAL_AI_HOOKS.md` |

## Claude Code

Designs and implements. Every behaviour change starts from a test that fails
without it (`scripts/sabotage.py` checks that the test actually bites). Keeps
`docs/` current in the same change as the code.

**Code self-review is delegated to CI.** The routine critique of a change — the
second reading looking for missing tests, unbounded memory, P2P security
holes — is the AI code reviewer's job on the PR. **Rodin is invoked only when
explicitly asked for**, not as a step of every change.

## AI code reviewer (CI)

- Sends **only** `git diff <base>...HEAD`, never the repository, capped at
  `AI_MAX_DIFF_CHARS` (default 120 000 characters, lockfiles excluded). Over
  the cap, whole files are dropped from the end and the PR comment lists them.
- Reviews three axes: TDD (does a test fail if the change is reverted), memory
  and resources, network/P2P security.
- Provider chosen by repository configuration, through the OpenAI-compatible
  API of any of them — OpenAI, Gemini, DeepSeek, Kimi:

  | Name | Kind | Required | Example |
  |---|---|---|---|
  | `AI_API_KEY` | secret | yes | provider's API key |
  | `AI_MODEL_NAME` | variable | yes | `gpt-4o-mini`, `gemini-2.0-flash`, `deepseek-chat`, `kimi-k2-0905-preview` |
  | `AI_BASE_URL` | variable | no | empty = OpenAI; `https://generativelanguage.googleapis.com/v1beta/openai/`, `https://api.deepseek.com`, `https://api.moonshot.ai/v1` |

- **Fails loudly.** A missing key or model, an API timeout (180 s, two
  retries), an API error, an empty answer or a failed comment post turns the
  job red with the reason in the log. An empty diff is the only green run
  without a review, and the log says so.
- Its findings are **advice**: the job goes red for a broken reviewer, not for
  what the reviewer says. A BLOCKER in its comment is for a human, or Claude
  Code, to answer on the PR.
- A PR from a fork gets no secrets from GitHub, so the job fails on it. That is
  the price of using `pull_request` rather than `pull_request_target`, which
  would hand the repository's key to code from a stranger.

## Aider + local Qwen

`scripts/local_ai_helper.sh` starts Aider on whatever model LM Studio serves at
`http://localhost:1234/v1`; `scripts/local_ai_helper.sh review` reviews the
branch before a push without editing anything. The optional `pre-push` hook is
documented, not installed: [`docs/LOCAL_AI_HOOKS.md`](docs/LOCAL_AI_HOOKS.md).
It never blocks a push.
