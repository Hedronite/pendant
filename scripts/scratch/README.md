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

- one workspace labelled `pendant-scratch`, four panes in a 2x2 split;
- a marker line `PENDANT-SCRATCH-MARKER-<n>` in each pane;
- four reported agents `scratch-1` … `scratch-4`, with `scratch-2` blocked;
- a `PENDANT-SCRATCH-TICK-<n>` line in pane 1 every couple of seconds, so
  herdr keeps emitting frames for the WebSocket link.

Each pane runs a reporter loop that re-reports its lifecycle state every
couple of seconds. The state is transient by design: without the loop herdr
drops a reported agent once the pane settles back at a plain shell prompt.

Run the bridge against the session with:

```bash
PENDANT_HERDR_SOCK=~/.config/herdr/sessions/pendant-shell-g1/herdr.sock \
PENDANT_SERVE_ORIGIN=https://<serve-host>:8443 \
  cargo run -p pendant
```
