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

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SESSION="$(scratch_session_name "$@")" || exit 1
LABEL="${PENDANT_SCRATCH_LABEL:-pendant-scratch}"
REPO_DIR="${PENDANT_SCRATCH_REPO:-/tmp/pendant-scratch-repo}"
HERDR="${HERDR_BIN:-$(command -v herdr)}"
TICKER_INDEX=1
BLOCKED_INDEX=2
# Lifecycle reports must carry the detected agent label, or herdr stops
# treating the pane as a promptable agent. It is the symlink name.
AGENT_NAME=claude
REFRESH_SECONDS=2
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

seed_repo() {
    mkdir -p "$REPO_DIR"
    if [ -d "$REPO_DIR/.git" ]; then
        return 0
    fi
    git -C "$REPO_DIR" init -q -b main
    printf '# Scratch fixture\n\nMade-up content for the worktree check. No real notes.\n' \
        >"$REPO_DIR/FIXTURE.md"
    git -C "$REPO_DIR" -c user.name=scratch -c user.email=scratch@example.invalid \
        add -A
    git -C "$REPO_DIR" -c user.name=scratch -c user.email=scratch@example.invalid \
        commit -qm "scratch fixture"
}
seed_repo

root="$(h workspace create --label "$LABEL" --cwd "$REPO_DIR" --no-focus |
    jq -r '.result.root_pane.pane_id')"
split() { h pane split "$1" --direction "$2" --no-focus | jq -r '.result.pane.pane_id'; }
second="$(split "$root" right)"
third="$(split "$root" down)"
fourth="$(split "$second" down)"
panes=("$root" "$second" "$third" "$fourth")

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AGENT_SCRIPT="$SCRIPT_DIR/agent.sh"
# herdr detects an agent by the foreground process name, so the pane runs the
# fixture through a symlink deliberately named after a supported agent. The
# directory is fresh per seed, holds only that symlink, is prepended to the
# pane PATH only, and teardown removes it.
BIN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pendant-scratch-bin.XXXXXX")"
ln -s "$AGENT_SCRIPT" "$BIN_DIR/claude"
BIN_STATE="$HOME/.config/herdr/sessions/$SESSION/scratch-bin.path"
printf '%s\n' "$BIN_DIR" >"$BIN_STATE"

for index in "${!panes[@]}"; do
    pane="${panes[$index]}"
    number=$((index + 1))
    state=idle
    [ "$number" -eq "$BLOCKED_INDEX" ] && state=blocked
    # The agent erases this typed line and the prompt before printing anything,
    # so the pane holds fixture text only. env -i with the scratch bin first and
    # system directories only; no inherited PATH, no operator shell.
    command="$(printf 'exec env -i HOME=%q PATH=%q HERDR_BIN=%q PENDANT_AGENT_NAME=%q TICK_PANE=%q REFRESH_SECONDS=%q claude %q %q %q %q' \
        "$HOME" "$BIN_DIR:/usr/bin:/bin:/usr/sbin:/sbin" "$HERDR" \
        "$AGENT_NAME" "$TICKER_INDEX" "$REFRESH_SECONDS" \
        "$number" "$state" "$SESSION" "$pane")"
    h pane run "$pane" "$command" >/dev/null
done

# Detection is asynchronous: wait until herdr sees four agents in the session.
for _ in $(seq 1 100); do
    detected="$(h api snapshot | jq '.result.snapshot.agents | length')"
    [ "$detected" = "4" ] && break
    sleep 0.1
done

for _ in $(seq 1 100); do
    summary="$(h api snapshot | jq -c '{agents: [.result.snapshot.agents[] |
        {agent, pane_id, agent_status}]}')"
    if printf '%s' "$summary" |
        jq -e '.agents | length == 4 and (map(select(.agent_status == "blocked")) | length) == 1' \
            >/dev/null; then
        printf 'session=%s\nsocket=%s\nrepo=%s\n%s\n' "$SESSION" "$SOCK" "$REPO_DIR" "$summary"
        exit 0
    fi
    sleep 0.1
done

echo "session $SESSION started but the agent set did not settle" >&2
h api snapshot | jq -c '{agents: [.result.snapshot.agents[] | {agent, pane_id, agent_status}]}' >&2
exit 1
