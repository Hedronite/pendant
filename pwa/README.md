# Pendant PWA — phone shell (SPIKE B)

Thin phone shell for herdr agents, worktrees, and approvals. **Not a terminal emulator.**
Phone-first 390px (capped 480). Vanilla JS + CSS, no build step, installable PWA.

**Vault SoT:** `foundry/pendant/GOAL-spike-b.md` (Obsidian Atrium). Scope freeze: `foundry/pendant/SCOPE-spike-b.md`. This repo is the product tree; the vault pack is the planning SoT. Branding deferred to Evan.

## Screens (S1–S5)

| Screen | What |
| --- | --- |
| **S1 Agents** | Vertical list of agent cards: kind glyph · name · workspace chip · status chip (`working` / `blocked` / `done` / `idle`). Floating "needs you" badge if any `blocked`. Tap → S2. |
| **S2 Agent detail** | Transcript (`agent.read`, last N lines — not live PTY paint). Compose sheet (FAB) → `agent.prompt`. If `blocked`: sticky **Approve / Deny / 1–3** bar via `pane.send_keys`. Overflow: Focus · Interrupt (Ctrl-C) · Worktrees. Never silent auto-approve. |
| **S3 Worktrees** | List of workspaces (path · branch · linked panes). Primary CTA **New worktree** → S4. |
| **S4 New worktree** | Fields: repo/cwd · base ref · branch · label. Submit → `worktree.create` into an **existing** workspace. Stranger-first error copy. |
| **S5 Inbox** | Chronological: `blocked` then `done`. One-tap → S2 action bar. v0 in-app only; OS push deferred to v1. |
| **More** | Bridge URL config · machine chip `castle` · "Terminal attach (power)" escape hatch (labeled, not the product) · clear settings. |

Shell chrome (every screen): machine chip (`castle`) + Tailscale/bridge health dot, `MOCK` badge when the bridge is unreachable, `live` badge when polling. Bottom tab bar: Agents · Worktrees · Inbox · More.

## Run

No build step. Vanilla JS + CSS, installable PWA.

```bash
cd pwa
python3 -m http.server 8090   # or: npx --yes serve -l 8090
# open http://localhost:8090/  (or the Tailscale Serve URL backend applies)
```

Install to home screen from a Pixel browser for the standalone shell.

## Bridge contract (fullstack lane implements)

The PWA calls a localhost-only HTTP bridge that proxies the herdr socket API
(`~/.config/herdr/herdr.sock`, `srw-------`, herdr 0.9.0). Reachable only via
Tailscale Serve HTTPS, mesh-only. **Never bind the bridge to `0.0.0.0` / public WAN.**

| Method | Path | herdr verb | Returns |
| --- | --- | --- | --- |
| GET | `/api/snapshot` | `session.snapshot` | `{snapshot:{agents[],workspaces[],tabs[]}}` |
| GET | `/api/agent/read/:target?lines=N` | `agent.read` | `{text:string}` |
| POST | `/api/agent/prompt` | `agent.prompt` | `{ok:true}` |
| POST | `/api/agent/send-keys` | `pane.send_keys` | `{ok:true}` |
| POST | `/api/worktree/create` | `worktree.create` | `{workspace_id,pane_id}` |

`target` / `:target` is a herdr pane id (e.g. `wN:p3`). SSE (`events.subscribe`)
for blocked/done is v1; the spike polls `/api/snapshot` every 8s.

**Default bridge:** `http://127.0.0.1:8780` until Tailscale Serve consent. Override in **More**.

### MOCK mode

If the bridge is unreachable the app falls back to **MOCK mode** using a real
herdr 0.9.0 `session.snapshot` shape (3 workspaces / 9 panes), so the spike is
gradeable on a Pixel without the bridge. The `MOCK` badge shows in the top bar.
Set the bridge URL in **More** to go live; clear settings to force MOCK.

## Spike DoD (Evan GO → done)

1. Pixel on Tailscale opens Pendant → live agent list from castle `api snapshot` (status chips). ✅ wired (S1)
2. One button: `worktree.create` into an existing workspace. ✅ (S4)
3. Agent detail: read last output + compose Send (`agent.prompt`); if `blocked`, sticky Approve/Deny tap bar via `send_keys`. ✅ (S2)
4. Inbox: in-app list of blocked/done (poll); OS push deferred to v1. ✅ (S5)
5. Not Termux-as-product; not cloud. ✅ (escape hatch labeled "power", MOCK is local)

Stretch (not required): S6 diff skim (file→hunk cards).

## Non-goals (spike)

Cloud runners · full mobile editor · PR merge UI · forking ADE/Arboretum/C3
(steal shape only) · phone sshd / Magisk · binding any proxy to `0.0.0.0` ·
patching herdr upstream first · structured approve RPC (herdr 0.9.0 has none;
spike injects keys/prompt) · native Android (C).

## Files

```
manifest.webmanifest   installable PWA manifest (name "Pendant")
icon.svg               maskable icon (placeholder — branding Evan)
sw.js                  service worker (cache "pendant-v2"; never caches /api)
index.html             shell + routes + compose sheet — <title>Pendant</title>
styles.css             phone-first 390px, dark cards, sticky bars
app.js                 router + herdr client + mock + S1–S5/More
```

No `node_modules`. No build. Depends on stock herdr 0.9.x — no omp fork, no
Omahedron OS work. Branding (icon, palette, wordmark) deferred to Evan.
