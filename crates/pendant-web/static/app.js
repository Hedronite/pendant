// Pendant — phone shell over the embedded loopback bridge. Spike B. Not a terminal.
// Served same-origin by pendant-bridge (rust-embed). API prefix is /v1 (see pendant-bridge routes).
// Contract (pendant-herdr AgentRecord/WorkspaceRecord/InboxItem):
//   GET  /v1/snapshot            -> {agents[], workspaces[], raw}
//   GET  /v1/agents/:id          -> {id, agent, get, output, blocked}
//   POST /v1/agents/:id/prompt   {prompt}            -> ok
//   POST /v1/agents/:id/keys     {keys:[...]}       -> ok
//   POST /v1/worktrees           CreateWorktree body -> {workspace_id,...}
//   GET  /v1/events              -> {items:[InboxItem]}
// If the bridge is unreachable the app runs in MOCK mode using a real herdr 0.9.0
// snapshot shape, so the spike is gradeable on a Pixel without the bridge.

const STORE = {
  bridge: localStorage.getItem("pendant.bridge") || "",   // "" = same-origin (embedded)
  mock: localStorage.getItem("pendant.mock") === "1",
  machine: localStorage.getItem("pendant.machine") || "castle",
};

const $ = (s, r = document) => r.querySelector(s);
const el = (t, cls, html) => { const e = document.createElement(t); if (cls) e.className = cls; if (html != null) e.innerHTML = html; return e; };
const routeEl = () => $("#route");
const toast = (msg, kind = "") => { const t = $("#toast"); t.textContent = msg; t.className = "toast " + kind; t.hidden = false; clearTimeout(toast._t); toast._t = setTimeout(() => (t.hidden = true), 2200); };
const escapeHtml = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

// ---------- pendant client (same-origin /v1) ----------
const api = {
  base() { return STORE.bridge || ""; },
  async snapshot() {
    if (STORE.mock) return mockSnapshot();
    try {
      const r = await fetch(this.base() + "/v1/snapshot", { headers: { Accept: "application/json" } });
      if (!r.ok) throw new Error("HTTP " + r.status);
      const j = await r.json();
      return j.snapshot || j;   // scaffold returns {agents, workspaces, raw}
    } catch (e) {
      if (!STORE.mock) { STORE.mock = true; $("#mock-badge").hidden = false; setDot("warn"); toast("Bridge down — MOCK mode", "err"); }
      return mockSnapshot();
    }
  },
  async agentDetail(target) {
    if (STORE.mock) return mockAgentDetail(target);
    const r = await fetch(this.base() + "/v1/agents/" + encodeURIComponent(target), { headers: { Accept: "application/json" } });
    if (!r.ok) throw new Error("agent HTTP " + r.status);
    return r.json();
  },
  async prompt(target, text) {
    if (STORE.mock) { mockPrompt(target, text); return { ok: true }; }
    const r = await fetch(this.base() + "/v1/agents/" + encodeURIComponent(target) + "/prompt", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ prompt: text }) });
    if (!r.ok) throw new Error("prompt HTTP " + r.status);
    return r.json();
  },
  async sendKeys(target, keys) {
    if (STORE.mock) { toast("MOCK send-keys: " + keys.join(" ")); return { ok: true }; }
    const r = await fetch(this.base() + "/v1/agents/" + encodeURIComponent(target) + "/keys", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ keys }) });
    if (!r.ok) throw new Error("send-keys HTTP " + r.status);
    return r.json();
  },
  async worktreeCreate({ cwd, branch, base, label }) {
    if (STORE.mock) return mockWorktreeCreate({ cwd, branch, base, label });
    const r = await fetch(this.base() + "/v1/worktrees", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ cwd, branch, base, label }) });
    const j = await r.json();
    if (!r.ok) throw new Error(j?.error?.message || ("worktree HTTP " + r.status));
    return j;
  },
  async inbox() {
    if (STORE.mock) return mockInbox();
    try {
      const r = await fetch(this.base() + "/v1/events", { headers: { Accept: "application/json" } });
      if (!r.ok) throw new Error("HTTP " + r.status);
      const j = await r.json(); return j.items || [];
    } catch (e) { return []; }
  },
};

// ---------- state ----------
let state = { snap: null, route: "agents", target: null, focusPane: null };
const STATUS_ORDER = ["blocked", "working", "done", "idle", "unknown"];

function setDot(kind) { const d = $("#ts-dot"); d.className = "dot " + kind + (kind === "ok" ? " live" : ""); }
function agents() { return (state.snap && state.snap.agents) || []; }
function workspaces() { return (state.snap && state.snap.workspaces) || []; }
function wsLabel(id) { const w = workspaces().find((x) => x.workspace_id === id); return (w && w.label) || id; }
function blockedAgents() { return agents().filter((a) => a.status === "blocked"); }
function doneAgents() { return agents().filter((a) => a.status === "done"); }

function renderChrome() {
  $("#machine-name").textContent = STORE.machine;
  $("#needs-you").hidden = blockedAgents().length === 0;
  $("#inbox-dot").hidden = blockedAgents().length === 0 && doneAgents().length === 0;
}

async function refresh() {
  routeEl().classList.add("spin");
  try {
    state.snap = await api.snapshot();
    if (!STORE.mock) { $("#mock-badge").hidden = true; setDot("ok"); $("#poll-badge").hidden = false; }
    renderChrome(); rerender();
  } catch (e) { setDot("bad"); toast("refresh failed", "err"); }
  finally { routeEl().classList.remove("spin"); }
}

// ---------- router ----------
function go(r) { location.hash = "#/" + r; }
function currentTarget() { return state.target; }
function activeTab() { const r = state.route; if (r === "agent") return "agents"; if (r === "worktree") return "worktrees"; return r; }
window.addEventListener("hashchange", () => dispatch(location.hash));
function dispatch(hash) {
  const path = hash.replace(/^#\/?/, "");
  const parts = path.split("/");
  state.route = parts[0] || "agents";
  state.target = parts[1] || null;
  document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("active", t.dataset.route === activeTab()));
  hideCompose(); rerender();
}
function rerender() {
  const r = state.route;
  if (r === "agents") renderAgents();
  else if (r === "agent") renderAgentDetail(currentTarget());
  else if (r === "worktrees") renderWorktrees();
  else if (r === "worktree") renderNewWorktree();
  else if (r === "inbox") renderInbox();
  else if (r === "more") renderMore();
  else renderAgents();
}

// ---------- S1: Agents ----------
function renderAgents() {
  const r = routeEl(); r.innerHTML = "";
  const list = agents().slice().sort((a, b) => STATUS_ORDER.indexOf(a.status) - STATUS_ORDER.indexOf(b.status));
  if (!list.length) { r.appendChild(el("div", "empty", '<span class="big">▦</span>No agents. Start herdr on castle.')); return; }
  r.appendChild(el("div", "h", "Agents · " + list.length));
  for (const a of list) r.appendChild(agentCard(a));
}
function agentCard(a) {
  const c = el("div", "card");
  c.append(el("div", "avatar", glyph(a)));
  const body = el("div", "body");
  const row1 = el("div", "row1");
  row1.append(el("div", "name", escapeHtml(labelOf(a))), el("span", "chip " + a.status, a.status));
  body.append(row1, el("div", "ws", wsLabel(a.workspace_id) + " · " + a.pane_id), el("div", "last", escapeHtml(a.title || a.workspace_id || "")));
  c.append(body);
  c.onclick = () => go("agent/" + encodeURIComponent(a.id));
  return c;
}
function glyph(a) { const m = { grok: "G", omp: "π", omapi: "π", cursor: "C", codex: "C", claude: "✦", pi: "π" }; return m[a.agent] || (a.agent ? a.agent[0].toUpperCase() : "?"); }
function labelOf(a) { return a.name || a.title || a.agent || a.id; }

// ---------- S4: New worktree ----------
function field(label, ph, val) {
  const wrap = el("div", "field");
  wrap.append(el("label", "", label));
  const input = el("input"); input.type = "text"; input.placeholder = ph; if (val) input.value = val;
  wrap.append(input);
  return { wrap, input };
}
function renderNewWorktree() {
  const r = routeEl(); r.innerHTML = "";
  const back = el("button", "back", "‹"); back.onclick = () => go("worktrees");
  r.append(back, el("div", "h", "New worktree"));
  const form = el("form");
  const cwdF = field("Repo / workspace path", "~/Projects/geode", "");
  const baseF = field("Base ref (optional)", "main", "");
  const branchF = field("Branch name", "feat/spike", "");
  const labelF = field("Label (optional)", "", "");
  form.append(cwdF.wrap, baseF.wrap, branchF.wrap, labelF.wrap);
  const err = el("div", "form-error", ""); form.append(err);
  const submit = el("button", "btn primary block", "Create worktree"); submit.type = "submit";
  form.append(submit);
  form.onsubmit = async (e) => {
    e.preventDefault(); err.textContent = ""; submit.disabled = true;
    try {
      const res = await api.worktreeCreate({ cwd: cwdF.input.value.trim(), base: baseF.input.value.trim() || undefined, branch: branchF.input.value.trim() || undefined, label: labelF.input.value.trim() || undefined });
      toast("worktree created: " + (res.workspace_id || "ok"), "ok");
      go("worktrees"); setTimeout(refresh, 400);
    } catch (e2) {
      const msg = String(e2.message || e2);
      err.textContent = msg.includes("exist") ? "That branch or path is already taken — pick another."
        : msg.includes("not a git") ? "That folder isn't a git repo — check the path."
        : msg.includes("busy") ? "That path is busy right now — try again."
        : "Couldn't create the worktree: " + msg;
    } finally { submit.disabled = false; }
  };
  r.append(form);
}

// ---------- S5: Inbox ----------
async function renderInbox() {
  const r = routeEl(); r.innerHTML = "";
  r.append(el("div", "h", "Inbox · in-app (v0)"));
  let items = [];
  try { items = await api.inbox(); } catch (e) { items = []; }
  if (!items.length) {
    // fall back to deriving from snapshot
    const derived = [];
    for (const a of blockedAgents()) derived.push({ id: a.id, agent: a.agent, status: "blocked", workspace_id: a.workspace_id, title: a.title, kind: "blocked" });
    for (const a of doneAgents()) derived.push({ id: a.id, agent: a.agent, status: "done", workspace_id: a.workspace_id, title: a.title, kind: "done" });
    items = derived;
  }
  if (!items.length) { r.appendChild(el("div", "empty", '<span class="big">◷</span>Nothing waiting. Agents working.')); return; }
  items.sort((x, y) => (x.kind === "blocked" ? -1 : 1) - (y.kind === "blocked" ? -1 : 1));
  for (const it of items) {
    const row = el("div", "card inbox-item" + (it.kind === "blocked" ? " unread" : ""));
    row.append(el("span", "dot " + it.kind, ""));
    const body = el("div", "body");
    body.append(el("div", "t", escapeHtml(it.title || it.agent || it.id)));
    body.append(el("div", "last", it.kind === "blocked" ? "Blocked — needs your call" : "Done"));
    row.append(body);
    row.append(el("span", "when", it.status || it.kind));
    row.onclick = () => go("agent/" + encodeURIComponent(it.id));
    r.append(row);
  }
  r.append(el("div", "h", "OS push deferred to v1"));
}

// ---------- More ----------
function renderMore() {
  const r = routeEl(); r.innerHTML = "";
  r.append(el("div", "h", "Session"));
  const kv = (k, v) => { const x = el("div", "kv"); x.append(el("span", "k", k), el("span", "v", v)); return x; };
  r.append(kv("Machine", STORE.machine), kv("Bridge", STORE.bridge || "(same-origin /v1)"), kv("Mode", STORE.mock ? "MOCK" : "live"));
  r.append(el("div", "h", "Bridge URL (dev only)"));
  const f = el("div", "field");
  const inp = el("input"); inp.type = "text"; inp.placeholder = "https://castle.<tailnet>.ts.net"; inp.value = STORE.bridge;
  f.append(el("label", "", "Empty = same-origin (embedded). Tailscale Serve only — never 0.0.0.0"), inp);
  const save = el("button", "btn primary block", "Save"); save.style.marginTop = "8px";
  save.onclick = () => { STORE.bridge = inp.value.trim(); localStorage.setItem("pendant.bridge", STORE.bridge); toast("saved — reconnecting", "ok"); STORE.mock = false; localStorage.removeItem("pendant.mock"); refresh(); };
  f.append(save); r.append(f);
  r.append(el("div", "h", "Machine name"));
  const mf = el("div", "field");
  const minp = el("input"); minp.type = "text"; minp.value = STORE.machine;
  mf.append(el("label", "", "Chip label in top bar"), minp);
  const msave = el("button", "btn block", "Save"); msave.style.marginTop = "8px";
  msave.onclick = () => { STORE.machine = minp.value.trim() || "castle"; localStorage.setItem("pendant.machine", STORE.machine); renderChrome(); toast("saved", "ok"); };
  mf.append(msave); r.append(mf);
  r.append(el("div", "h", "Escape hatch"));
  const term = el("button", "btn block", "Terminal attach (power)");
  term.onclick = () => { if (confirm("Open herdr attach docs? The terminal path is a power-user escape hatch, not the product.")) window.open("https://herdr.dev/agent-guide.md", "_blank"); };
  r.append(term);
  r.append(el("div", "h", "Danger"));
  const reset = el("button", "btn block danger", "Clear settings & force MOCK");
  reset.onclick = () => { if (confirm("Clear bridge URL and return to MOCK?")) { localStorage.removeItem("pendant.bridge"); localStorage.setItem("pendant.mock", "1"); STORE.bridge = ""; STORE.mock = true; $("#mock-badge").hidden = false; refresh(); } };
  r.append(reset);
}

// ---------- mock data (real herdr 0.9.0 snapshot shape; scaffold field names) ----------
function mockSnapshot() {
  return {
    raw: {},
    agents: [
      { id: "cursor-attack", name: null, agent: "cursor", status: "blocked", pane_id: "wN:p5", workspace_id: "wN", tab_id: "wN:t1", title: "Geode Spec Attack — waiting for approval" },
      { id: "grok", name: null, agent: "grok", status: "working", pane_id: "wN:p1", workspace_id: "wN", tab_id: "wN:t1", title: "Geode bootstrap, TUI spec — grok" },
      { id: "fe-geode", name: "fe-geode", agent: "omp", status: "working", pane_id: "wN:p3", workspace_id: "wN", tab_id: "wN:t3", title: "Polish clap help vault recipients" },
      { id: "omp", name: null, agent: "omp", status: "working", pane_id: "wN:p2", workspace_id: "wN", tab_id: "wN:t2", title: "Resume geode TUI peer review" },
      { id: "omp-g0b", name: null, agent: "omapi", status: "idle", pane_id: "wN:p4", workspace_id: "wN", tab_id: "wN:t4", title: "Implement geode G0b backend v010" },
      { id: "grok-fleet", name: null, agent: "grok", status: "idle", pane_id: "wP:p1", workspace_id: "wP", tab_id: "wP:t1", title: "12-seat fleet scene live on test.hedronite" },
      { id: "fe-kimi", name: "fe-kimi", agent: "omp", status: "done", pane_id: "wP:p4", workspace_id: "wP", tab_id: "wP:t4", title: "Clinic polish hero instrumentation pass" },
      { id: "omp-push", name: null, agent: "omp", status: "done", pane_id: "wP:p3", workspace_id: "wP", tab_id: "wP:t3", title: "Push G1b commit to origin" },
      { id: "grok-heraldi", name: null, agent: "grok", status: "working", pane_id: "wQ:p1", workspace_id: "wQ", tab_id: "wQ:t1", title: "Halo PM: Delta trial — grok" },
    ],
    workspaces: [
      { workspace_id: "wN", label: "geode", cwd: "~/agents/halo" },
      { workspace_id: "wP", label: "hedronite.com", cwd: "~" },
      { workspace_id: "wQ", label: "heraldi", cwd: "~" },
    ],
  };
}
function mockAgentDetail(target) {
  const a = agents().find((x) => x.id === target || x.pane_id === target);
  return { id: target, agent: a || null, get: null, output: mockRead(target), blocked: a ? a.status === "blocked" : false };
}
function mockRead(target) {
  return [
    "[" + target + "] herdr 0.9.0 · protocol 22",
    "  working: wN:p1, wN:p2, wN:p3, wQ:p1",
    "  blocked: wN:p5  <- needs you",
    "  done:   wP:p3, wP:p4",
    "",
    "Approve? [y] yes  [n] no  [1-3] option",
  ].join("\n");
}
function mockInbox() {
  const items = [];
  for (const a of blockedAgents()) items.push({ id: a.id, agent: a.agent, status: "blocked", workspace_id: a.workspace_id, title: a.title, kind: "blocked" });
  for (const a of doneAgents()) items.push({ id: a.id, agent: a.agent, status: "done", workspace_id: a.workspace_id, title: a.title, kind: "done" });
  return items;
}
function mockPrompt(target, text) { toast("MOCK prompt -> " + target + ": " + text.slice(0, 40)); }
function mockWorktreeCreate({ cwd, branch }) {
  if (!cwd) throw new Error("not_git_worktree: Herdr worktree actions require a workspace inside a Git work tree");
  if (branch === "main") throw new Error("branch_exists: That branch or path is already taken — pick another.");
  return { workspace_id: "wR", pane_id: "wR:p1", branch, cwd };
}

// ---------- init ----------
function bindTabs() { document.querySelectorAll(".tab").forEach((t) => t.addEventListener("click", () => go(t.dataset.route))); }
$("#refresh-btn").addEventListener("click", refresh);
bindTabs();
if (STORE.mock) { $("#mock-badge").hidden = false; setDot("warn"); } else setDot("bad");
if ("serviceWorker" in navigator) navigator.serviceWorker.register("./sw.js").catch(() => {});
dispatch(location.hash);
refresh();
setInterval(() => { if (!document.hidden) refresh(); }, 8000);
