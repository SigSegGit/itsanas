#!/usr/bin/env bash
# Aider, pointed at a model served locally by LM Studio. See docs/LOCAL_AI_HOOKS.md.
#
#   scripts/local_ai_helper.sh [aider args...]   interactive Aider session
#   scripts/local_ai_helper.sh review [BASE]     one-shot review of BASE...HEAD
#                                                (BASE defaults to the upstream,
#                                                else origin/main); edits nothing
#
# Environment:
#   LMSTUDIO_URL      default http://localhost:1234/v1
#   LOCAL_AI_MODEL    model id as LM Studio lists it; default: the first model
#                     LM Studio reports as available
#   LOCAL_AI_MAX_DIFF_CHARS  cap on the diff sent by `review` (default 60000;
#                     a local model's context is smaller than a hosted one's)
#
# Exit codes: 0 ok, 2 LM Studio unreachable or no model, 3 aider not installed.
# The pre-push hook in docs/LOCAL_AI_HOOKS.md relies on 2 and 3 to skip quietly.

set -euo pipefail

LMSTUDIO_URL="${LMSTUDIO_URL:-http://localhost:1234/v1}"
MAX_DIFF="${LOCAL_AI_MAX_DIFF_CHARS:-60000}"

die() { code=$1; shift; printf 'local_ai_helper: %s\n' "$*" >&2; exit "$code"; }

command -v aider >/dev/null 2>&1 \
    || die 3 "aider is not installed (python -m pip install aider-install && aider-install)"

models_json=$(curl -fsS --max-time 2 "$LMSTUDIO_URL/models" 2>/dev/null) \
    || die 2 "LM Studio is not answering at $LMSTUDIO_URL (start its server: lms server start)"

model="${LOCAL_AI_MODEL:-}"
if [ -z "$model" ]; then
    model=$(printf '%s' "$models_json" | grep -o '"id"[[:space:]]*:[[:space:]]*"[^"]*"' \
        | head -n1 | sed 's/.*"\([^"]*\)"$/\1/')
    [ -n "$model" ] || die 2 "LM Studio at $LMSTUDIO_URL lists no model; load one or set LOCAL_AI_MODEL"
fi

# LM Studio ignores the key, but the OpenAI client Aider uses refuses an empty one.
aider_base=(
    --openai-api-base "$LMSTUDIO_URL"
    --openai-api-key lm-studio
    --model "openai/$model"
    --no-show-model-warnings
    --no-check-update
    --no-analytics
)

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
trap 'rm -f "$msg"' EXIT
{
    echo "Review this diff of ITSaNAS (Rust, P2P zero-knowledge storage). Axes: does each"
    echo "behaviour change have a test that fails if reverted; unbounded memory driven by"
    echo "peer input; P2P security (auth/signature checks, replay, trusting a peer). Terse,"
    echo "file:line per finding, no praise. Do not edit any file. $note"
    echo
    echo '```diff'
    printf '%s\n' "$diff"
    echo '```'
} > "$msg"

echo "local_ai_helper: reviewing $base...HEAD (${#diff} chars) with $model" >&2
# Not exec: the trap must still remove the message file afterwards.
aider "${aider_base[@]}" --dry-run --no-auto-commits --no-git --yes-always \
    --message-file "$msg"
