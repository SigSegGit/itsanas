# Local AI: Aider + LM Studio, and a pre-push hook

A local model reviews what you are about to push, costs nothing per token, and
sends nothing off the machine. It is an **advisor, not a gate**: the gate is
the CI job `.github/workflows/ai-code-reviewer.yml` (see [`AGENTS.md`](../AGENTS.md)).

## 1. Manual use

Prerequisites: LM Studio with its server started (`lms server start`, or the
Developer tab → Start Server) and the model downloaded — it does not need to be
loaded, LM Studio loads it on the first request; Aider installed
(`python -m pip install aider-install && aider-install`, which puts `aider` in
`~/.local/bin` inside its own Python 3.12 environment).

```bash
# Interactive Aider session on the loaded model. Any Aider argument passes through.
scripts/local_ai_helper.sh crates/itsanas-core/src/lib.rs

# One-shot review of your branch against its upstream (else origin/main).
# Edits nothing: --dry-run, no commits.
scripts/local_ai_helper.sh review
scripts/local_ai_helper.sh review origin/main
```

| Variable | Default | |
|---|---|---|
| `LMSTUDIO_URL` | the port `lms server status` reports, else `http://localhost:1234/v1` | not a fixed 1234: on Nicolas's laptop the server is on 54321, for itsaresume |
| `LOCAL_AI_MODEL` | `qwen/qwen3-coder-30b` | an id from `lms ls`; MoE with 3B active, usable on 8 GB of VRAM with RAM offload |
| `LOCAL_AI_MAX_DIFF_CHARS` | `60000` | the diff is cut there, and the model is told so |
| `LOCAL_AI_TIMEOUT` | `180` | seconds before `review` is killed |

Exit codes: `0` done, `2` LM Studio unreachable or the model not available,
`3` Aider not installed, `124` `review` ran past `LOCAL_AI_TIMEOUT`. The check
against LM Studio has a 2-second timeout.

**What to expect from `review`**, measured on 2026-09-24 on the laptop (RTX 4070
8 GB, 64 GB RAM): about 30 s on a clean 25 kB diff, answering "nothing found";
about 2 minutes on an 863-byte diff with a planted flaw — a node-claim signature
check turned into "log and accept" — which it named at the right `file:line` in
three runs out of three. Before the answer was capped at 1,500 tokens, two of
those three runs found the flaw and then repeated themselves for over eight
minutes; that is why the timeout exists. It answers in French whatever the
prompt asks: Aider follows the system locale.

What it is not: a proof that a diff is safe. One planted flaw caught three times
is evidence the review can see an obvious hole, not that it sees subtle ones.

On Windows, run it from **Git Bash**, not WSL: `curl` and `git` come with Git
for Windows, and under WSL's default NAT networking `localhost` is the Linux VM,
not the Windows host LM Studio listens on.

## 2. As a `pre-push` hook, fail-safe

Not installed by anything in this repository — installing it is your choice.

What "fail-safe" means here: **the hook never blocks a push.** It asks, waits 5
seconds, and defaults to *no review* if you do not answer; it skips silently
when there is no terminal (GUI clients, CI), when LM Studio is off, or when
Aider is missing; and it always exits 0, so even a crashed review lets the push
through. It only informs you — reading the review and pressing Ctrl-C to abort
the push is up to you.

Save as `.git/hooks/pre-push`:

```bash
#!/usr/bin/env bash
# Optional local AI review before a push. Always exits 0: it never blocks.

helper="$(git rev-parse --show-toplevel)/scripts/local_ai_helper.sh"
[ -x "$helper" ] || exit 0

# Git feeds the refs being pushed on stdin, so the question has to be read from
# the terminal. No terminal (GUI, IDE, CI): skip without asking.
{ exec 3</dev/tty; } 2>/dev/null || exit 0

printf 'Local AI review before push? [y/N] (5 s) ' >&2
answer=""
read -r -t 5 answer <&3 || true
exec 3<&-
echo >&2

case "$answer" in
    y|Y|yes|YES) ;;
    *) exit 0 ;;
esac

# `$1` is the remote name. Review against its main branch.
"$helper" review "$1/main"
status=$?
case $status in
    0) ;;
    2|3) echo "pre-push: local AI unavailable (exit $status), push continues" >&2 ;;
    124) echo "pre-push: local AI review timed out, push continues" >&2 ;;
    *)   echo "pre-push: local AI review failed (exit $status), push continues" >&2 ;;
esac
exit 0
```

Then:

```bash
chmod +x .git/hooks/pre-push
```

**Why the default is No, not Yes.** A `[Y/n]` that defaults to *yes* after a
timeout starts a review of several minutes every time you push and look away;
with *no* as the default, silence costs nothing. If you prefer the opposite,
change the `case` to treat an empty answer as yes — the rest of the hook stays
safe because LM Studio being off still skips in 2 seconds.

To skip it once: `git push --no-verify`. To remove it: delete the file.
