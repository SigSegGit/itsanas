# Agents

Three agents work on this repository. Each has one job; none of them replaces
the test suite, which remains the only thing that proves a property holds.

| Agent | Role | Runs where | Entry point |
|---|---|---|---|
| **Claude Code** | Architecture and implementation, test first | developer machine | the session itself; resumes from `docs/HANDOVER.md` |
| **AI code reviewer** (provider-agnostic) | CI gatekeeper: reviews the diff of every PR and every push to `main` | GitHub Actions | `.github/workflows/ai-code-reviewer.yml` → `scripts/ci_code_reviewer.py` |
| **Aider + local Qwen** | Local operations and pre-push review, offline, no per-token cost | developer machine, LM Studio | `scripts/local_ai_helper.sh`, hook in `docs/LOCAL_AI_HOOKS.md` |

## Public repository: what no agent writes here

This repository is public, and its owner may publish, license or sell the
project. Every agent, every commit, pull request, issue comment and release
note follows these rules. Each one exists because breaking it cannot be undone
once it is published.

1. **Legal texts carry no caveat.** CLA.md and any terms, privacy or licence
   text state their terms and nothing about their own status: no draft
   marker, no pending review, no doubt about validity. Such remarks go to the
   owner in private.
2. **No rights are granted.** Nothing enters under a licence the owner could
   not relicense (no copyleft other than the project's own AGPL, nothing
   "non-commercial", no code copied without a licence). The licence does not
   change. Contributors are promised nothing: contributions come in only under
   [CLA.md](CLA.md).
3. **No promise to users.** Public text says what a named test proves, as a
   fact. It gives no warranty, availability, legality or safety guarantee.
4. **Nothing against the project's interest.** No text says the network
   knowingly holds unlawful content, that abuse cannot be handled, or that the
   owner can read members' data.
5. **No private infrastructure, no secret.** No new hostname, port, account,
   key path or address of the owner's machines; no token.
6. **The name is the owner's.** Nothing invites a fork to use "ITSaNAS".

`scripts/check-public-claims.py` enforces 1 and 2 (licences and the CLA) on
every push. The rest is checked by whoever writes: read the diff, the commit
message and the pull request body before sending them.

## Claude Code

Designs and implements. Every behaviour change starts from a test that fails
without it (`scripts/sabotage.py` checks that the test actually bites). Keeps
`docs/` current in the same change as the code.

**Test output is read by an agent, so it is sized for one.** Run the suite with
`cargo nextest run --profile ci --workspace --all-features`: a green run is six
lines, and slow or failed tests still come out in full. The default profile
prints about 800 `PASS` lines that say nothing the summary does not. No test in
CI may run longer than three minutes; `scripts/check-test-budget.py` enforces
it on every nextest profile CI uses.

**Code self-review is delegated to CI.** The routine critique of a change — the
second reading looking for missing tests, unbounded memory, P2P security
holes — is the AI code reviewer's job on the PR, not a Claude Code pass per
commit.

**Merging with planned skips.** A PR's CI is green when every check is a
success or a *skip the CI planned*: skipped jobs count as green only if
"What this run tests" planned them (selective CI) and "No warnings
anywhere in this run" is a success. Any other skip, a pending check or a
red one is not green. (Nicolas, 2026-10-07; a docs-only PR skips six jobs
by design and was otherwise never mergeable by a session.)

**Merge without asking: GitHub does it.** `main` requires the single check
*No warnings anywhere in this run* (which passes only on the rule above), and
the repository allows auto-merge (Nicolas, 2026-10-13: no session should be
woken just to press the button). So a session that opens a PR marks it ready
and enables auto-merge, squash, in the same breath (`enable_pr_auto_merge`,
or `gh pr merge N --auto --squash`), and does not come back for the merge.
It stays on the PR only to fix a red CI or answer a review. Before this, when
a PR's CI was green by the rule above, Claude Code merged it itself, whoever
opened it, marking a draft ready first; that still applies to a PR opened
before auto-merge, or one whose auto-merge was turned off.
It asks Nicolas only when it has a concrete reason to doubt the merge, and
then says what that reason is. Asking for a routine merge costs him a round
trip for nothing. (Nicolas, 2026-10-12.)

**Rodin before each major step, once.** A major step is a `docs/HANDOVER.md` §8
item, or anything Nicolas calls one. Rodin critiques the *plan* before the code
is written, where a wrong design is cheapest to drop; he is not re-run per
commit or per CI run, which is where the tokens went. Outside major steps, only
when Nicolas asks. (Nicolas, 2026-09-24; this replaced "only on explicit
request" from the same day.)

**Subagents, when a job is worth a cold start.** Versioned in
[`.claude/agents/`](.claude/agents/README.md), shared with itsaresume, so
cloud sessions have them too: `locator` (before code: exact places and
names, so the main context stops reading large files blind), `redteam` (the
finished diff, before push; Rodin has had the plan), `docs-sweeper` (after
the code is final: catalogue, counts, stale prose, gates), `persona-user`
(the docs followed literally, before a release). None of them merges, pushes
or decides. Nicolas's user-level roster in `~/.claude/agents/` is superseded
by these where names overlap; `itsanas-lead` and `agents-curator` were
judged redundant with Rodin on 2026-10-04.

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
  | `AI_MODEL_NAME` | variable | yes | a list, e.g. `gemini-flash-lite-latest,gemma-4-31b-it,gemini-3.8-flash` |
  | `AI_BASE_URL` | variable | no | empty = OpenAI; `https://generativelanguage.googleapis.com/v1beta/openai/`, `https://api.deepseek.com`, `https://api.moonshot.ai/v1` |

  Dependabot PRs see only the **Dependabot** secret store, not Actions secrets
  or variables: the same three names are copied there, all as secrets, and the
  workflow reads `vars.X || secrets.X`. Rotate the key in both stores.

- **Fails loudly.** A missing key, an API error, an empty answer or a failed
  comment post turns the job red with the reason in the log. `AI_MODEL_NAME` is
  a comma-separated list tried in order: a model that is overloaded (503/429,
  after retries at 10 and 20 s), missing (404) or slow (90 s) hands over to the
  next, and when none answers the job goes red and **lists the models the key
  can use**. Gemini's free tier kept three flash models at 503 for minutes on
  its first evening, which is why the list exists and why light models come
  first. An empty diff is the only green run without a review, and the log
  says so.
- Its findings are **advice**: the job goes red for a broken reviewer, not for
  what the reviewer says. A BLOCKER in its comment is for a human, or Claude
  Code, to answer on the PR.
- A PR from a fork gets no secrets from GitHub, so the job fails on it. That is
  the price of using `pull_request` rather than `pull_request_target`, which
  would hand the repository's key to code from a stranger.

## Aider + local Qwen

`scripts/local_ai_helper.sh` starts Aider on `qwen/qwen3-coder-next` served by
LM Studio, on whatever port `lms server status` reports;
`scripts/local_ai_helper.sh review` reviews the branch before a push without
editing anything, and is killed after 180 s. The optional `pre-push` hook is
documented, not installed: [`docs/LOCAL_AI_HOOKS.md`](docs/LOCAL_AI_HOOKS.md).
It never blocks a push.
