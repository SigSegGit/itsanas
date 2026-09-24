# Local AI: Aider + LM Studio, and a pre-push hook

A local model reviews what you are about to push, costs nothing per token, and
sends nothing off the machine. It is an **advisor, not a gate**: the gate is
the CI job `.github/workflows/ai-code-reviewer.yml` (see [`AGENTS.md`](../AGENTS.md)).

## 1. Manual use

Prerequisites: LM Studio with its server started (`lms server start`, or the
Developer tab → Start Server) and a model loaded, e.g. Qwen2.5-Coder; Aider
installed (`python -m pip install aider-install && aider-install`).

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
| `LMSTUDIO_URL` | `http://localhost:1234/v1` | |
| `LOCAL_AI_MODEL` | first model LM Studio lists | the id as `curl $LMSTUDIO_URL/models` shows it |
| `LOCAL_AI_MAX_DIFF_CHARS` | `60000` | the diff is cut there, and the model is told so |

Exit codes: `0` done, `2` LM Studio unreachable or no model loaded, `3` Aider
not installed. The check against LM Studio has a 2-second timeout.

On Windows, run it from Git Bash (it is a bash script; `curl` and `git` must be
on the `PATH`, which Git for Windows provides).

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
