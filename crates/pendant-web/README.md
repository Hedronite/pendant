# pendant-web — embedded PWA assets

Static PWA shell (S1–S5) served same-origin by `pendant-bridge` via `rust-embed`
(`#[folder = "static/"]`). Phone-first 390px. Not a terminal emulator.

**Vault SoT:** `foundry/pendant/GOAL-spike-b.md` (Obsidian Atrium). Scope freeze:
`foundry/pendant/SCOPE-spike-b.md`. Branding deferred to Evan.

## API contract (same-origin `/v1`)

| Method | Path | herdr verb |
| --- | --- | --- |
| GET | `/v1/snapshot` | `session.snapshot` |
| GET | `/v1/agents/:id` | `agent.get` + `agent.read` (output + blocked flag) |
| POST | `/v1/agents/:id/prompt` | `agent.prompt` |
| POST | `/v1/agents/:id/keys` | `agent.send_keys` |
| POST | `/v1/worktrees` | `worktree.create` |
| GET | `/v1/events` | inbox (blocked/done) |
| GET | `/v1/events/stream` | SSE (v1) |

`api.base()` defaults to `""` (same-origin). Override in **More** for standalone dev.

## Screens (Sati)

- S1 Agents: list + status chips + needs-you badge
- S2 Agent detail: transcript + compose + blocked sticky Approve/Deny bar
  (`pane.send_keys`; never silent auto-approve)
- S3 Worktrees: list + New worktree CTA
- S4 New worktree: `worktree.create` into existing workspace
- S5 Inbox: in-app blocked/done (poll; OS push = v1)
- More: bridge URL (dev), machine chip, power-only escape hatch

MOCK fallback uses a real herdr 0.9.0 snapshot shape. Not a terminal. No `0.0.0.0`.
No cloud. Branding (icon, palette, wordmark) deferred to Evan.
