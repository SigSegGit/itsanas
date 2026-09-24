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
| `LOCAL_AI_MODEL` | `qwen/qwen3-coder-next` | an id from `lms ls`; loaded with a 32k context if it is not loaded |
| `LOCAL_AI_MAX_DIFF_CHARS` | `60000` | the diff is cut there, and the model is told so |
| `LOCAL_AI_TIMEOUT` | `180` | seconds before `review` is killed |

Exit codes: `0` done, `2` LM Studio unreachable or the model not available,
`3` Aider not installed, `124` `review` ran past `LOCAL_AI_TIMEOUT`. The check
against LM Studio has a 2-second timeout.

### Which model, measured

Laptop: RTX 4070 8 GB, 64 GB RAM, 2026-09-24. Two flaws planted on throwaway
branches — a node-claim signature check turned into "log and accept"
(`itsanas-coord/src/claim.rs`), and the frame-size check before allocation
removed (`itsanas-wire/src/wire.rs`) — plus the clean 25 kB diff of this change,
which should produce nothing.

| Model | Signature flaw | Frame-size flaw | Clean diff | Per review |
|---|---|---|---|---|
| **`qwen/qwen3-coder-next`** (80B MoE, 3B active) | 2/2 | 3/3, exact line | nothing found | 21–29 s, 76 s clean |
| `qwen/qwen3-coder-30b` (30B MoE, 3B active) | 3/5 | 2/3 | nothing found | 10–36 s |
| `qwen/qwen3.6-35b-a3b` (thinking model) | 2/2, answer unread | — | killed at 180 s | 106–181 s |

`coder-30b` missed each flaw at least once with the same prompt and the same
diff, so a "nothing found" from it means little. `coder-next` is 48.5 GB and
loads in about two minutes into RAM with part on the GPU; with it loaded,
about 23 GB of RAM stayed free. Timings moved when the laptop's power mode
changed mid-measurement; the found/missed verdicts do not depend on it.

Five runs on two flaws is evidence that it sees an obvious hole, not proof that
it sees subtle ones. It answers in French whatever the prompt asks: Aider
follows the system locale.

**Speculative decoding: measured, no gain.** `qwen/qwen3-0.6b` as a draft for
`coder-30b` (it has to be set at load time, `lms load ... --speculative-draft-simple
--speculative-draft-model ...`): 19.7 and 20.7 tokens/s against 18.4 and 20.6
without, which is noise. Expected for a mixture-of-experts model offloaded to
RAM: the cost is moving experts, not sequential decoding. It does not change
the output — the main model verifies every drafted token — so the only price is
VRAM. No draft model exists for `coder-next`'s architecture on this machine.

**Two bounds**, because a local model does not always stop: before the answer
was capped at 1,500 tokens, `coder-30b` found a flaw and then repeated itself
for over eight minutes, twice in three runs. `LOCAL_AI_TIMEOUT` kills anything
else.

**From WSL** it works as your existing `~/.aider.conf.yml` already does: the
Windows host is `10.255.255.254` from WSL, so
`LMSTUDIO_URL=http://10.255.255.254:54321/v1 scripts/local_ai_helper.sh review`.
There is no `lms` in WSL, so the model has to be loaded already. From Git Bash
nothing needs setting.

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
