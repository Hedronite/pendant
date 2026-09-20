// Pendant — phone shell over a thin herdr bridge. Spike B. Not a terminal.
// Bridge contract (fullstack lane implements; see README.md):
//   GET  /api/snapshot                       -> {snapshot:{agents[],workspaces[],tabs[]}}
//   GET  /api/agent/read/:target?lines=N      -> {text:string}
//   POST /api/agent/prompt      {target,text} -> {ok:true}
//   POST /api/agent/send-keys   {target,keys} -> {ok:true}
//   POST /api/worktree/create   {cwd,branch,base,label} -> {workspace_id,pane_id}
// If the bridge is unreachable the app runs in MOCK mode using a real herdr 0.9.0
// snapshot shape, so the spike is gradeable on a Pixel without the bridge.

const STORE = {
  bridge: localStorage.getItem("pendant.bridge") || "http://127.0.0.1:8780",
  mock: localStorage.getItem("pendant.mock") === "1",
  machine: localStorage.getItem("pendant.machine") || "castle",
};

const $ = (s, r = document) => r.querySelector(s);
const el = (t, cls, html) => { const e = document.createElement(t); if (cls) e.className = cls; if (html != null) e.innerHTML = html; return e; };
const routeEl = () => $("#route");
const toast = (msg, kind = "") => { const t = $("#toast"); t.textContent = msg; t.className = "toast " + kind; t.hidden = false; clearTimeout(toast._t); toast._t = setTimeout(() => (t.hidden = true), 2200); };
const escapeHtml = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

// ---------- herdr client ----------
const api = {
  base() { return STORE.bridge || ""; },
  async snapshot() {
    if (STORE.mock) return mockSnapshot();
    try {
      const r = await fetch(this.base() + "/api/snapshot", { headers: { Accept: "application/json" } });
      if (!r.ok) throw new Error("HTTP " + r.status);
      const j = await r.json();
      return j.snapshot || j.result?.snapshot || j;
    } catch (e) {
      if (!STORE.mock) { STORE.mock = true; $("#mock-badge").hidden = false; setDot("warn"); toast("Bridge down — MOCK mode", "err"); }
      return mockSnapshot();
    }
  },
  async read(target, lines = 200) {
    if (STORE.mock) return mockRead(target);
    const r = await fetch(this.base() + `/api/agent/read/${encodeURIComponent(target)}?lines=${lines}`, { headers: { Accept: "application/json" } });
    if (!r.ok) throw new Error("read HTTP " + r.status);
    const j = await r.json(); return j.text || j.result?.text || "";
  },
  async prompt(target, text) {
    if (STORE.mock) { mockPrompt(target, text); return { ok: true }; }
    const r = await fetch(this.base() + "/api/agent/prompt", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ target, text }) });
    if (!r.ok) throw new Error("prompt HTTP " + r.status);
    return r.json();
  },
  async sendKeys(target, keys) {
    if (STORE.mock) { toast("MOCK send-keys: " + keys.join(" ")); return { ok: true }; }
    const r = await fetch(this.base() + "/api/agent/send-keys", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ target, keys }) });
    if (!r.ok) throw new Error("send-keys HTTP " + r.status);
    return r.json();
  },
  async worktreeCreate({ cwd, branch, base, label }) {
    if (STORE.mock) return mockWorktreeCreate({ cwd, branch, base, label });
    const r = await fetch(this.base() + "/api/worktree/create", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ cwd, branch, base, label }) });
    const j = await r.json();
    if (!r.ok) throw new Error(j?.error?.message || ("worktree HTTP " + r.status));
    return j;
  },
};

// ---------- state ----------
let state = { snap: null, route: "agents", target: null, focusPane: null };
const STATUS_ORDER = ["blocked", "working", "done", "idle", "unknown"];

function setDot(kind) { const d = $("#ts-dot"); d.className = "dot " + kind + (kind === "ok" ? " live" : ""); }
function agents() { return (state.snap && state.snap.agents) || []; }
function workspaces() { return (state.snap && state.snap.workspaces) || []; }
function wsLabel(id) { const w = workspaces().find((x) => x.workspace_id === id); return w ? w.label : id; }
function blockedAgents() { return agents().filter((a) => a.agent_status === "blocked"); }
function doneAgents() { return agents().filter((a) => a.agent_status === "done"); }

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
    renderChrome();
    rerender();
  } catch (e) { setDot("bad"); toast("refresh failed", "err"); }
  finally { routeEl().classList.remove("spin"); }
}

// ---------- router ----------
function go(r) { location.hash = "#/" + r; }
function currentTarget() { return state.target; }
function activeTab() {
  const r = state.route;
  if (r === "agent") return "agents";
  if (r === "worktree") return "worktrees";
  return r;
}
window.addEventListener("hashchange", () => dispatch(location.hash));
function dispatch(hash) {
  const path = hash.replace(/^#\/?/, "");
  const parts = path.split("/");
  state.route = parts[0] || "agents";
  state.target = parts[1] || null;
  document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("active", t.dataset.route === activeTab()));
  hideCompose();
  rerender();
}
function rerender() {
  const r = routeEl();
  if (state.route === "agents") renderAgents();
  else if (state.route === "agent") renderAgentDetail(currentTarget());
  else if (state.route === "worktrees") renderWorktrees();
  else if (state.route === "worktree") renderNewWorktree();
  else if (state.route === "inbox") renderInbox();
  else if (state.route === "more") renderMore();
  else renderAgents();
}

// ---------- S1: Agents ----------
function renderAgents() {
  const r = routeEl(); r.innerHTML = "";
  const list = agents().slice().sort((a, b) => STATUS_ORDER.indexOf(a.agent_status) - STATUS_ORDER.indexOf(b.agent_status));
  if (!list.length) { r.appendChild(el("div", "empty", '<span class="big">▦</span>No agents. Start herdr on castle.')); return; }
  r.appendChild(el("div", "h", "Agents · " + list.length));
  for (const a of list) r.appendChild(agentCard(a));
}
function agentCard(a) {
  const c = el("div", "card");
  c.append(el("div", "avatar", glyph(a)));
  const body = el("div", "body");
  const row1 = el("div", "row1");
  row1.append(el("div", "name", escapeHtml(labelOf(a))), el("span", "chip " + a.agent_status, a.agent_status));
  body.append(row1, el("div", "ws", wsLabel(a.workspace_id) + " · " + a.pane_id), el("div", "last", escapeHtml(a.terminal_title_stripped || a.cwd || "")));
  c.append(body);
  c.onclick = () => go("agent/" + a.pane_id);
  return c;
}
function glyph(a) { const m = { grok: "G", omp: "π", omapi: "π", cursor: "C", codex: "C", claude: "✦", pi: "π" }; return m[a.agent] || (a.agent ? a.agent[0].toUpperCase() : "?"); }
function labelOf(a) { return a.name || a.terminal_title_stripped || a.agent || a.pane_id; }

// ---------- S2: Agent detail ----------
async function renderAgentDetail(target) {
  state.focusPane = target;
  const a = agents().find((x) => x.pane_id === target);
  const r = routeEl(); r.innerHTML = "";
  const back = el("button", "back", "‹"); back.onclick = () => go("agents");
  const head = el("div", "detail-head");
  head.append(el("div", "avatar", a ? glyph(a) : "?"));
  const m = el("div", "meta");
  m.append(el("div", "name", escapeHtml(a ? labelOf(a) : target)), el("div", "sub", a ? wsLabel(a.workspace_id) + " · " + a.pane_id : ""));
  head.append(m);
  if (a) head.append(el("span", "chip " + a.agent_status, a.agent_status));
  r.append(back, head);

  const t = el("div", "transcript");
  t.appendChild(el("span", "empty-t", "loading…"));
  r.append(t);
  api.read(target, 200).then((txt) => {
    t.innerHTML = "";
    (txt || "(no output)").split("\n").forEach((l) => t.appendChild(el("span", "ln", escapeHtml(l))));
    t.scrollTop = t.scrollHeight;
  }).catch((e) => { t.innerHTML = ""; t.appendChild(el("span", "empty-t", "read failed: " + e.message)); });

  const overflow = el("div", "overflow-row");
  const focus = el("button", "btn", "Focus"); focus.onclick = () => api.sendKeys(target, ["focus"]).then(() => toast("focused", "ok")).catch((e) => toast(e.message, "err"));
  const interrupt = el("button", "btn", "Interrupt"); interrupt.onclick = () => { if (confirm("Send Ctrl-C to " + target + "?")) api.sendKeys(target, ["ctrl-c"]).then(() => toast("interrupted", "ok")).catch((e) => toast(e.message, "err")); };
  const openWt = el("button", "btn", "Worktrees"); openWt.onclick = () => go("worktrees");
  overflow.append(focus, interrupt, openWt);
  r.append(overflow);

  const composeBtn = el("button", "fab", "✎"); composeBtn.onclick = () => showCompose(target);
  r.append(composeBtn);

  if (a && a.agent_status === "blocked") r.appendChild(actionBar(a));
}

function actionBar(a) {
  const bar = el("div", "action-bar");
  bar.append(el("div", "ab-title", "Needs you — " + a.pane_id));
  const row = el("div", "ab-row");
  const deny = el("button", "btn deny", "Deny"); deny.onclick = () => approve(a, "n");
  const approveB = el("button", "btn approve", "Approve"); approveB.onclick = () => approve(a, "y");
  row.append(deny, approveB); bar.append(row);
  const opts = el("div", "overflow-row"); opts.style.marginTop = "8px";
  for (const n of [1, 2, 3]) { const b = el("button", "btn", String(n)); b.onclick = () => approve(a, String(n)); opts.append(b); }
  bar.append(opts);
  return bar;
}
async function approve(a, key) {
  try { await api.sendKeys(a.pane_id, [key, "enter"]); toast("sent “" + key + "”", "ok"); setTimeout(refresh, 600); }
  catch (e) { toast(e.message, "err"); }
}

function showCompose(target) {
  const s = $("#compose-sheet"); s.hidden = false; const ta = $("#compose-text"); ta.value = ""; ta.focus();
  $("#compose-send").onclick = async () => {
    const text = ta.value.trim(); if (!text) return;
    $("#compose-send").disabled = true;
    try { await api.prompt(target, text); hideCompose(); toast("sent", "ok"); setTimeout(refresh, 700); }
    catch (e) { toast(e.message, "err"); } finally { $("#compose-send").disabled = false; }
  };
  $("#compose-cancel").onclick = hideCompose;
}
function hideCompose() { $("#compose-sheet").hidden = true; }

// ---------- S3: Worktrees ----------
function renderWorktrees() {
  const r = routeEl(); r.innerHTML = "";
  r.append(el("div", "h", "Worktrees"));
  const ws = workspaces();
  if (!ws.length) { r.appendChild(el("div", "empty", '<span class="big">⎇</span>Spawn a worktree on castle — agents keep your machine.')); }
  else {
    for (const w of ws) {
      const c = el("div", "card");
      c.append(el("div", "avatar", "⎇"));
      const body = el("div", "body");
      const row1 = el("div", "row1");
      row1.append(el("span", "name", escapeHtml(w.label)), el("span", "ws", w.workspace_id + " · " + w.tab_count + " tabs"));
      const panes = agents().filter((a) => a.workspace_id === w.workspace_id).map((a) => a.pane_id).join(", ");
      body.append(row1, el("div", "last", "panes: " + (panes || "—")));
      c.append(body); c.onclick = () => go("agents"); r.append(c);
    }
  }
  const cta = el("button", "btn primary block", "＋ New worktree"); cta.style.marginTop = "14px"; cta.onclick = () => go("worktree/new");
  r.append(cta);
}

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
      toast("worktree created: " + (res.workspace_id || res.pane_id || "ok"), "ok");
      go("worktrees"); setTimeout(refresh, 400);
    } catch (e2) {
      // stranger-first copy, no fleet jargon
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
function renderInbox() {
  const r = routeEl(); r.innerHTML = "";
  r.append(el("div", "h", "Inbox · in-app (v0)"));
  const items = [];
  for (const a of blockedAgents()) items.push({ a, kind: "blocked", when: "now" });
  for (const a of doneAgents()) items.push({ a, kind: "done", when: "done" });
  if (!items.length) { r.appendChild(el("div", "empty", '<span class="big">◷</span>Nothing waiting. Agents working.')); return; }
  // blocked first
  items.sort((x, y) => (x.kind === "blocked" ? -1 : 1) - (y.kind === "blocked" ? -1 : 1));
  for (const it of items) {
    const row = el("div", "card inbox-item" + (it.kind === "blocked" ? " unread" : ""));
    row.append(el("span", "dot " + it.kind, ""));
    const body = el("div", "body");
    body.append(el("div", "t", escapeHtml(labelOf(it.a)) + " · " + it.a.pane_id));
    body.append(el("div", "last", it.kind === "blocked" ? "Blocked — needs your call" : "Done"));
    row.append(body);
    row.append(el("span", "when", it.when));
    row.onclick = () => go("agent/" + it.a.pane_id);
    r.append(row);
  }
  const note = el("div", "h", "OS push deferred to v1");
  r.append(note);
}

// ---------- S7 lite: More ----------
function renderMore() {
  const r = routeEl(); r.innerHTML = "";
  r.append(el("div", "h", "Session"));
  const kv = (k, v) => { const x = el("div", "kv"); x.append(el("span", "k", k), el("span", "v", v)); return x; };
  r.append(kv("Machine", STORE.machine), kv("Bridge", STORE.bridge || "(same-origin /api)"), kv("Mode", STORE.mock ? "MOCK" : "live"));
  r.append(el("div", "h", "Bridge URL"));
  const f = el("div", "field");
  const inp = el("input"); inp.type = "text"; inp.placeholder = "https://castle.<tailnet>.ts.net"; inp.value = STORE.bridge;
  f.append(el("label", "", "https://…  (Tailscale Serve only — never 0.0.0.0)"), inp);
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

// ---------- mock data (real herdr 0.9.0 snapshot shape) ----------
function mockSnapshot() {
  return {
    protocol: 22, version: "0.9.0",
    focused_workspace_id: "wN", focused_pane_id: "wN:p3",
    workspaces: [
      { workspace_id: "wN", label: "geode", number: 1, tab_count: 5, pane_count: 5, agent_status: "working", active_tab_id: "wN:t3" },
      { workspace_id: "wP", label: "hedronite.com", number: 2, tab_count: 5, pane_count: 5, agent_status: "done", active_tab_id: "wN:t1", focused: true },
      { workspace_id: "wQ", label: "heraldi", number: 3, tab_count: 1, pane_count: 1, agent_status: "working", active_tab_id: "wQ:t1" },
    ],
    agents: [
      { agent: "grok", agent_status: "working", pane_id: "wN:p1", workspace_id: "wN", cwd: "~/agents/halo", terminal_title_stripped: "Geode bootstrap, TUI spec — grok" },
      { agent: "omp", agent_status: "working", pane_id: "wN:p2", workspace_id: "wN", cwd: "~/agents/halo", terminal_title_stripped: "Resume geode TUI peer review" },
      { agent: "omp", agent_status: "working", pane_id: "wN:p3", workspace_id: "wN", cwd: "~/agents/halo", terminal_title_stripped: "Polish clap help vault recipients", name: "fe-geode" },
      { agent: "omapi", agent_status: "idle", pane_id: "wN:p4", workspace_id: "wN", cwd: "~/agents/halo", terminal_title_stripped: "Implement geode G0b backend v010" },
      { agent: "cursor", agent_status: "blocked", pane_id: "wN:p5", workspace_id: "wN", cwd: "~/agents/halo", terminal_title_stripped: "Geode Spec Attack — waiting for approval" },
      { agent: "grok", agent_status: "idle", pane_id: "wP:p1", workspace_id: "wP", cwd: "~", terminal_title_stripped: "12-seat fleet scene live on test.hedronite…", focused: true },
      { agent: "omp", agent_status: "done", pane_id: "wP:p4", workspace_id: "wP", cwd: "~", terminal_title_stripped: "Clinic polish hero instrumentation pass", name: "fe-kimi" },
      { agent: "omp", agent_status: "done", pane_id: "wP:p3", workspace_id: "wP", cwd: "~", terminal_title_stripped: "Push G1b commit to origin" },
      { agent: "grok", agent_status: "working", pane_id: "wQ:p1", workspace_id: "wQ", cwd: "~", terminal_title_stripped: "Halo PM: Delta trial — grok" },
    ],
  };
}
function mockRead(target) {
  return [
    "[" + target + "] herdr 0.9.0 · protocol 22",
    "› reviewing: " + target,
    "",
    "  ✓ loaded 3 workspaces / 9 panes",
    "  ⠋ working: wN:p1, wN:p2, wN:p3, wQ:p1",
    "  ● blocked: wN:p5  ← needs you",
    "  ◷ done:   wP:p3, wP:p4",
    "",
    "Approve? [y] yes  [n] no  [1-3] option",
  ].join("\n");
}
function mockPrompt(target, text) { toast("MOCK prompt → " + target + ": " + text.slice(0, 40)); }
function mockWorktreeCreate({ cwd, branch }) {
  if (!cwd) throw new Error("not_git_worktree: Herdr worktree actions require a workspace inside a Git work tree");
  if (branch === "main") throw new Error("branch_exists: That branch or path is already taken — pick another.");
  return { workspace_id: "wR", pane_id: "wR:p1", branch, cwd };
}

// ---------- init ----------
function bindTabs() {
  document.querySelectorAll(".tab").forEach((t) => t.addEventListener("click", () => go(t.dataset.route)));
}
$("#refresh-btn").addEventListener("click", refresh);
bindTabs();
if (STORE.mock) { $("#mock-badge").hidden = false; setDot("warn"); } else setDot("bad");
if ("serviceWorker" in navigator) navigator.serviceWorker.register("./sw.js").catch(() => {});
dispatch(location.hash);
refresh();
// poll every 8s (spike; SSE is v1)
setInterval(() => { if (!document.hidden) refresh(); }, 8000);
