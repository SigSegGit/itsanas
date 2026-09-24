#!/usr/bin/env bash
# Aider, pointed at a model served locally by LM Studio. See docs/LOCAL_AI_HOOKS.md.
#
#   scripts/local_ai_helper.sh [aider args...]   interactive Aider session
#   scripts/local_ai_helper.sh review [BASE]     one-shot review of BASE...HEAD
#                                                (BASE defaults to the upstream,
#                                                else origin/main); edits nothing
#
# Environment:
#   LMSTUDIO_URL      default: the port `lms server status` reports, else
#                     LM Studio's own default http://localhost:1234/v1
#   LOCAL_AI_MODEL    model id as LM Studio lists it; default qwen/qwen3-coder-30b
#                     (MoE, 3B active: usable on 8 GB of VRAM with RAM offload).
#                     LM Studio loads it on the first request if it is not loaded.
#   LOCAL_AI_MAX_DIFF_CHARS  cap on the diff sent by `review` (default 60000;
#                     a local model's context is smaller than a hosted one's)
#   LOCAL_AI_TIMEOUT  seconds before `review` is killed (default 180)
#
# Exit codes: 0 ok, 2 LM Studio unreachable or no model, 3 aider not installed,
# 124 `review` ran past LOCAL_AI_TIMEOUT. The pre-push hook in
# docs/LOCAL_AI_HOOKS.md lets the push through on every one of them.

set -euo pipefail

# Not a fixed 1234: on Nicolas's laptop the server is on 54321 because
# itsaresume's router is configured for it, and a helper that assumed 1234
# reported "not answering" against a server that was up.
if [ -z "${LMSTUDIO_URL:-}" ]; then
    # `lms` prints its status on stderr, not stdout.
    port=$(lms server status 2>&1 | grep -o 'port [0-9]*' | grep -o '[0-9]*' || true)
    LMSTUDIO_URL="http://localhost:${port:-1234}/v1"
fi
MAX_DIFF="${LOCAL_AI_MAX_DIFF_CHARS:-60000}"
TIMEOUT="${LOCAL_AI_TIMEOUT:-180}"

die() { code=$1; shift; printf 'local_ai_helper: %s\n' "$*" >&2; exit "$code"; }

command -v aider >/dev/null 2>&1 \
    || die 3 "aider is not installed (python -m pip install aider-install && aider-install)"

models_json=$(curl -fsS --max-time 2 "$LMSTUDIO_URL/models" 2>/dev/null) \
    || die 2 "LM Studio is not answering at $LMSTUDIO_URL (start its server: lms server start)"

# A named default, not "the first model listed": LM Studio lists every model
# on disk, in no useful order, and the first one on this laptop was a 1.9B
# speculative-decoding draft model -- it would have answered, badly, and
# nothing would have said so.
model="${LOCAL_AI_MODEL:-qwen/qwen3-coder-30b}"
printf '%s' "$models_json" | grep -qF "\"$model\"" \
    || die 2 "LM Studio at $LMSTUDIO_URL has no model '$model' (lms ls lists them; set LOCAL_AI_MODEL)"

# LM Studio ignores the key, but the OpenAI client Aider uses refuses an empty one.
aider_base=(
    --openai-api-base "$LMSTUDIO_URL"
    --openai-api-key lm-studio
    --model "openai/$model"
    --no-show-model-warnings
    --no-check-update
    --no-analytics
    # Aider offers to fetch every URL it sees in a message, and --yes-always
    # accepts: on the first real run a diff mentioning five URLs made this
    # laptop open a headless browser on each of them. Off.
    --no-detect-urls
)

# Aider prints through Python; the Windows console code page garbles accents.
export PYTHONUTF8=1

cd "$(git rev-parse --show-toplevel)"

if [ "${1:-}" != "review" ]; then
    echo "local_ai_helper: aider on $model via $LMSTUDIO_URL" >&2
    exec aider "${aider_base[@]}" "$@"
fi

base="${2:-}"
if [ -z "$base" ]; then
    base=$(git rev-parse --abbrev-ref --symbolic-full-name '@{u}' 2>/dev/null || echo origin/main)
fi

diff=$(git diff --no-color "$base...HEAD" -- . ':(exclude)Cargo.lock')
if [ -z "$diff" ]; then
    echo "local_ai_helper: nothing to review in $base...HEAD" >&2
    exit 0
fi

note=""
if [ "${#diff}" -gt "$MAX_DIFF" ]; then
    note="NOTE: the diff was cut at $MAX_DIFF of ${#diff} characters; say so in your answer."
    diff="${diff:0:$MAX_DIFF}"
fi

msg=$(mktemp)
# Aider writes its chat and input history into the current directory, which is
# the repository: the first real run left .aider.chat.history.md at its root.
hist=$(mktemp)
settings=$(mktemp)
trap 'rm -f "$msg" "$hist" "$hist.in" "$settings"' EXIT

# Two bounds, because qwen3-coder-30b does not always stop: on 2026-09-24 two
# runs out of three on an 863-character diff found the planted flaw and then
# repeated themselves for over eight minutes. max_tokens bounds the answer;
# `timeout` bounds everything else, including a model that is slow to load.
cat > "$settings" <<EOF
- name: openai/$model
  extra_params:
    max_tokens: 1500
EOF
{
    echo "Review this diff of ITSaNAS (Rust, P2P zero-knowledge storage) for DEFECTS IN"
    echo "THE CHANGED LINES. Look for: a behaviour change with no test that would fail if"
    echo "it were reverted; memory that grows without bound on input from a peer; a P2P"
    echo "security hole (skipped auth or signature check, replay, trusting a peer's word)."
    echo "These are things to look for, not features every file must implement: a doc or"
    echo "shell script that has nothing to do with them is not a finding. Each finding:"
    echo "file:line, what is wrong, how it fails. If there is none, answer exactly"
    echo "'nothing found'. English, terse, no praise. Do not edit any file. $note"
    echo
    echo '```diff'
    printf '%s\n' "$diff"
    echo '```'
} > "$msg"

echo "local_ai_helper: reviewing $base...HEAD (${#diff} chars) with $model" >&2
# Not exec: the trap must still remove the message file afterwards.
# `ask` mode: Aider's default edit mode tells the model to work on files added
# to the chat, and with none added the first real run answered "no content was
# provided" to an 8.9k-token diff.
timeout "$TIMEOUT" aider "${aider_base[@]}" --model-settings-file "$settings" \
    --chat-mode ask --dry-run --no-auto-commits --no-git \
    --yes-always --no-fancy-input --message-file "$msg" \
    --chat-history-file "$hist" --input-history-file "$hist.in"
