#!/usr/bin/env bash
# Fixture agent for the scratch session. Runs no model, opens no network and
# reads no tokens. It exists only so a pane has a process herdr recognises as an
# agent, and it keeps the pane text to fixture lines.
#
# Usage: agent.sh <number> <state> <session> <pane>
# Env: HERDR_BIN, PENDANT_AGENT_NAME, TICK_PANE, REFRESH_SECONDS.
set -u

number="$1"
state="$2"
session="$3"
pane="$4"
herdr="${HERDR_BIN:-herdr}"
agent_name="${PENDANT_AGENT_NAME:-scratch-$number}"
tick_pane="${TICK_PANE:-0}"
refresh="${REFRESH_SECONDS:-2}"

# Typed input is reported, not echoed twice.
stty -echo 2>/dev/null || true

# Erase the operator prompt and the command echo before the first fixture line.
printf '\033[3J\033[2J\033[H'
printf 'PENDANT-SCRATCH-MARKER-%s\n' "$number"

# Lifecycle state is transient, so it is re-reported for as long as the pane
# lives. The loop is quiet: no output, no job-control noise.
(
    while :; do
        "$herdr" --session "$session" pane report-agent "$pane" \
            --source custom:pendant-scratch --agent "$agent_name" \
            --state "$state" >/dev/null 2>&1
        sleep "$refresh"
    done
) >/dev/null 2>&1 &
reporter=$!
trap 'kill "$reporter" 2>/dev/null' EXIT INT TERM

bar() {
    printf 'PENDANT-SCRATCH-APPROVE [y/n]\n'
}
[ "$state" = "blocked" ] && bar

tick=0
while :; do
    if IFS= read -r -t "$refresh" line; then
        printf 'PENDANT-SCRATCH-INPUT: %s\n' "$line"
        if [ "$state" = "blocked" ]; then
            case "$line" in
                y | Y)
                    printf 'PENDANT-SCRATCH-DECISION: approve\n'
                    bar
                    ;;
                n | N)
                    printf 'PENDANT-SCRATCH-DECISION: deny\n'
                    bar
                    ;;
            esac
        fi
    fi
    if [ "$number" = "$tick_pane" ]; then
        tick=$((tick + 1))
        printf 'PENDANT-SCRATCH-TICK-%s\n' "$tick"
    fi
done
