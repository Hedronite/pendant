#!/usr/bin/env bash
# Seed a named scratch herdr session for the app-shell checks.
#
# The session is separate from any live session and holds only made-up content:
# one workspace, four panes, a marker line per pane, and four reported agents
# with one blocked. Each pane runs a small reporter loop that keeps its
# lifecycle state registered while the pane stays open.
#
# Usage: scripts/scratch/seed.sh [session-name]
# Tear down with scripts/scratch/teardown.sh [session-name].
set -euo pipefail

SESSION="${1:-${PENDANT_SCRATCH_SESSION:-pendant-shell-g1}}"
LABEL="${PENDANT_SCRATCH_LABEL:-pendant-scratch}"
HERDR="${HERDR_BIN:-$(command -v herdr)}"
BLOCKED_INDEX=2
REFRESH_SECONDS=2
SOURCE="custom:pendant-scratch"
SOCK="$HOME/.config/herdr/sessions/$SESSION/herdr.sock"

h() { "$HERDR" --session "$SESSION" "$@"; }

if h api snapshot >/dev/null 2>&1; then
    echo "session $SESSION is already running; teardown.sh first" >&2
    exit 1
fi

nohup "$HERDR" --session "$SESSION" server >/dev/null 2>&1 &
for _ in $(seq 1 100); do
    h api snapshot >/dev/null 2>&1 && break
    sleep 0.1
done
if ! h api snapshot >/dev/null 2>&1; then
    echo "session $SESSION did not start" >&2
    exit 1
fi

workspaces="$(h workspace list | jq '.result.workspaces | length')"
if [ "$workspaces" != "0" ]; then
    echo "session $SESSION already has workspaces; teardown.sh first" >&2
    exit 1
fi

root="$(h workspace create --label "$LABEL" --cwd /tmp --no-focus |
    jq -r '.result.root_pane.pane_id')"
split() { h pane split "$1" --direction "$2" --no-focus | jq -r '.result.pane.pane_id'; }
second="$(split "$root" right)"
third="$(split "$root" down)"
fourth="$(split "$second" down)"
panes=("$root" "$second" "$third" "$fourth")

for index in "${!panes[@]}"; do
    pane="${panes[$index]}"
    number=$((index + 1))
    state=idle
    [ "$number" -eq "$BLOCKED_INDEX" ] && state=blocked
    h pane run "$pane" "echo PENDANT-SCRATCH-MARKER-$number" >/dev/null
    h pane run "$pane" \
        "while :; do '$HERDR' --session '$SESSION' pane report-agent '$pane' --source '$SOURCE' --agent 'scratch-$number' --state '$state' >/dev/null 2>&1; sleep $REFRESH_SECONDS; done" \
        >/dev/null
done

for _ in $(seq 1 100); do
    summary="$(h api snapshot | jq -c '{agents: [.result.snapshot.agents[] |
        {agent, pane_id, agent_status}]}')"
    if printf '%s' "$summary" |
        jq -e '.agents | length == 4 and (map(select(.agent_status == "blocked")) | length) == 1' \
            >/dev/null; then
        printf 'session=%s\nsocket=%s\n%s\n' "$SESSION" "$SOCK" "$summary"
        exit 0
    fi
    sleep 0.1
done

echo "session $SESSION started but the agent set did not settle" >&2
h api snapshot | jq -c '{agents: [.result.snapshot.agents[] | {agent, pane_id, agent_status}]}' >&2
exit 1
