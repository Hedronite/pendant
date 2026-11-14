# Scratch herdr session

`seed.sh` starts a named herdr session that the bridge can read, with made-up
content only. It exists so the app-shell checks have four agents, one of them
blocked, without touching a live session or real notes.

```bash
scripts/scratch/seed.sh                # session pendant-shell-g1
scripts/scratch/teardown.sh            # stop and delete it
```

The session lives at `~/.config/herdr/sessions/<name>/herdr.sock`. `seed.sh`
prints the socket path and the agent summary, and refuses to seed a session
that is already running.

Both scripts take the name from the first argument, else
`PENDANT_SCRATCH_SESSION`, else `pendant-shell-g1`, and then refuse anything
that does not start with `pendant-scratch` or `pendant-shell`, plus an empty
name (`lib.sh`). They stop and delete sessions, so a typo must never reach a
live one.

What it creates:

- a made-up git repo at `/tmp/pendant-scratch-repo` (one commit, one fixture
  file, a scratch identity) so the workspace can create a worktree;
- one workspace labelled `pendant-scratch` on that repo, four panes in a 2x2 split;
- a marker line `PENDANT-SCRATCH-MARKER-<n>` in each pane;
- four detected agents, all labelled with the harness name herdr recognises, with
  the pane-2 agent blocked (ids are the pane ids `w1:p1` … `w1:p4`);
- a `PENDANT-SCRATCH-TICK-<n>` line in pane 1 every couple of seconds, so
  herdr keeps emitting frames for the WebSocket link;
- on the blocked pane, a `PENDANT-SCRATCH-APPROVE [y/n]` bar, and
  `PENDANT-SCRATCH-DECISION: approve|deny` when a key arrives;
- `PENDANT-SCRATCH-INPUT: <line>` for anything sent to a pane; a line without a
  trailing newline is reported on the next idle tick.

herdr only treats a pane as an agent it can prompt when the foreground process
matches a known harness. `agent.sh` is that process: a bash fixture that runs no
model, opens no network and reads no tokens. Each seed makes a fresh
`mktemp -d` directory holding one symlink to it, named after a supported agent,
and prepends only that directory to the pane's `env -i` PATH; teardown removes
the directory. Detection is asynchronous, so `seed.sh` waits for four agents.
Reports carry the detected label: reporting a different name stops herdr treating
the pane as a promptable agent.

The agent re-reports its lifecycle state every couple of seconds. The state is
transient by design: without the loop herdr drops a reported agent once the pane
settles. Before its first line the agent erases the operator prompt, the echo of
the command that started it, and any startup noise, and it never returns to a
shell — so a pane's text is only fixture lines: no username, hostname, home path
or binary path.

Run the bridge against the session with:

```bash
PENDANT_HERDR_SOCK=~/.config/herdr/sessions/pendant-shell-g1/herdr.sock \
PENDANT_SERVE_ORIGIN=https://<serve-host>:8443 \
  cargo run -p pendant
```
