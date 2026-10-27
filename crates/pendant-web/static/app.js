const view = document.getElementById("view");
const title = document.getElementById("page-title");
const conn = document.getElementById("conn");

const titles = {
  agents: "Agents",
  agent: "Agent",
  worktrees: "Worktrees",
  inbox: "Inbox",
};

function route() {
  const hash = location.hash.replace(/^#\/?/, "");
  const [page, id] = hash.split("/");
  document.querySelectorAll(".tabs a").forEach((a) => {
    a.classList.toggle("active", a.dataset.nav === (page || "agents") || (page === "agent" && a.dataset.nav === "agents"));
  });
  if (!page || page === "agents") return showAgents();
  if (page === "agent" && id) return showAgent(decodeURIComponent(id));
  if (page === "worktrees") return showWorktrees();
  if (page === "inbox") return showInbox();
  return showAgents();
}

async function api(path, options) {
  const res = await fetch(path, {
    headers: { Accept: "application/json", "Content-Type": "application/json" },
    ...options,
  });
  const text = await res.text();
  let body = null;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = { error: { message: text || res.statusText } };
  }
  if (!res.ok) {
    const message = body?.error?.message || res.statusText;
    throw new Error(message);
  }
  return body;
}

function setConn(error) {
  if (!error) {
    conn.hidden = true;
    conn.textContent = "";
    return;
  }
  conn.hidden = false;
  conn.textContent = error;
}

function chip(status) {
  const s = status || "unknown";
  return `<span class="chip ${s}">${s}</span>`;
}

function empty(message) {
  return `<div class="empty">${message}</div>`;
}

async function showAgents() {
  title.textContent = titles.agents;
  view.innerHTML = empty("Loading agents…");
  try {
    const data = await api("/v1/snapshot");
    setConn(null);
    const agents = data.agents || [];
    if (!agents.length) {
      view.innerHTML = empty("No agents in the current herdr snapshot.");
      return;
    }
    view.innerHTML = agents
      .map((agent) => {
        const id = encodeURIComponent(agent.id);
        return `<a class="card" href="#/agent/${id}">
          <div class="row">
            <h2>${escapeHtml(agent.agent || agent.name || agent.id)}</h2>
            ${chip(agent.status)}
          </div>
          <p class="meta">${escapeHtml(agent.title || agent.pane_id)} · ${escapeHtml(agent.workspace_id || "—")}</p>
        </a>`;
      })
      .join("");
  } catch (err) {
    setConn(err.message);
    view.innerHTML = empty("Can't reach herdr. Is the local server running?");
  }
}

async function showAgent(id) {
  title.textContent = titles.agent;
  view.innerHTML = empty("Loading agent…");
  try {
    const data = await api(`/v1/agents/${encodeURIComponent(id)}`);
    setConn(null);
    const agent = data.agent || {};
    const output = extractOutput(data.output);
    const blocked = Boolean(data.blocked);
    view.innerHTML = `
      <section class="card">
        <div class="row">
          <h2>${escapeHtml(agent.agent || agent.name || id)}</h2>
          ${chip(agent.status)}
        </div>
        <p class="meta">${escapeHtml(agent.title || agent.pane_id || id)} · ${escapeHtml(agent.workspace_id || "")}</p>
      </section>
      <section class="output">${escapeHtml(output || "No recent output.")}</section>
      <form class="form" id="compose">
        <label>Send a prompt
          <textarea name="prompt" required placeholder="Ask the agent…"></textarea>
        </label>
        <button type="submit">Send</button>
        <p class="note">Uses herdr agent.prompt. If the agent is blocked, herdr will refuse the prompt.</p>
      </form>
      ${
        blocked
          ? `<div class="blocked-bar">
              <p>Blocked — stub bar. Approve sends Enter; Deny sends Esc. Not a structured approve RPC.</p>
              <button type="button" id="approve">Approve</button>
              <button type="button" class="secondary" id="deny">Deny</button>
            </div>`
          : ""
      }
    `;
    document.getElementById("compose").addEventListener("submit", async (event) => {
      event.preventDefault();
      const prompt = new FormData(event.target).get("prompt");
      try {
        await api(`/v1/agents/${encodeURIComponent(id)}/prompt`, {
          method: "POST",
          body: JSON.stringify({ prompt }),
        });
        event.target.reset();
        showAgent(id);
      } catch (err) {
        setConn(err.message);
      }
    });
    document.getElementById("approve")?.addEventListener("click", () => sendKeys(id, ["enter"]));
    document.getElementById("deny")?.addEventListener("click", () => sendKeys(id, ["esc"]));
  } catch (err) {
    setConn(err.message);
    view.innerHTML = empty("Agent detail unavailable.");
  }
}

async function sendKeys(id, keys) {
  const label = keys.join("+");
  if (!confirm(`Send ${label} to this agent?`)) return;
  try {
    await api(`/v1/agents/${encodeURIComponent(id)}/keys`, {
      method: "POST",
      body: JSON.stringify({ keys }),
    });
    showAgent(id);
  } catch (err) {
    setConn(err.message);
  }
}

function extractOutput(output) {
  if (!output) return "";
  if (typeof output === "string") return output;
  return output.text || output.output || output.result?.text || JSON.stringify(output, null, 2);
}

async function showWorktrees() {
  title.textContent = titles.worktrees;
  view.innerHTML = empty("Loading worktrees…");
  let listHtml = empty("Worktree list unavailable until herdr answers.");
  let workspaces = [];
  try {
    const snap = await api("/v1/snapshot");
    workspaces = snap.workspaces || [];
    const listed = await api("/v1/worktrees");
    const items = listed.worktrees || listed.items || listed.result?.worktrees || [];
    if (Array.isArray(items) && items.length) {
      listHtml = items
        .map((item) => {
          const branch = item.branch || item.name || "worktree";
          const path = item.path || item.cwd || "";
          return `<article class="card">
            <h2>${escapeHtml(branch)}</h2>
            <p class="meta">${escapeHtml(path)} · ${escapeHtml(item.workspace_id || "")}</p>
          </article>`;
        })
        .join("");
    } else if (workspaces.some((ws) => ws.worktree)) {
      listHtml = workspaces
        .filter((ws) => ws.worktree)
        .map((ws) => `<article class="card"><h2>${escapeHtml(ws.label || ws.workspace_id)}</h2><p class="meta">${escapeHtml(ws.cwd || "")}</p></article>`)
        .join("");
    } else {
      listHtml = empty("No worktrees listed. Create one into an existing workspace.");
    }
    setConn(null);
  } catch (err) {
    setConn(err.message);
  }

  const workspaceOptions = workspaces
    .map((ws) => `<option value="${escapeHtml(ws.workspace_id)}">${escapeHtml(ws.label || ws.workspace_id)}</option>`)
    .join("");

  view.innerHTML = `
    ${listHtml}
    <form class="form" id="create-wt">
      <h2>Create worktree</h2>
      <label>Workspace
        ${
          workspaceOptions
            ? `<select name="workspace_id">${workspaceOptions}</select>`
            : `<input name="workspace_id" required placeholder="w1" />`
        }
      </label>
      <label>Branch
        <input name="branch" required placeholder="worktree/phone" />
      </label>
      <label>Base (optional)
        <input name="base" placeholder="HEAD" />
      </label>
      <button type="submit">Create</button>
      <p class="note">Calls herdr worktree.create into an existing workspace.</p>
    </form>
  `;
  document.getElementById("create-wt").addEventListener("submit", async (event) => {
    event.preventDefault();
    const data = Object.fromEntries(new FormData(event.target).entries());
    const body = {
      workspace_id: data.workspace_id || undefined,
      branch: data.branch,
      base: data.base || undefined,
      focus: false,
    };
    try {
      await api("/v1/worktrees", { method: "POST", body: JSON.stringify(body) });
      showWorktrees();
    } catch (err) {
      setConn(err.message);
    }
  });
}

async function showInbox() {
  title.textContent = titles.inbox;
  view.innerHTML = empty("Loading inbox…");
  try {
    const data = await api("/v1/events");
    setConn(null);
    const items = data.items || [];
    if (!items.length) {
      view.innerHTML = empty("Nothing blocked or done right now.");
      return;
    }
    view.innerHTML = items
      .map((item) => {
        const id = encodeURIComponent(item.id);
        return `<a class="card" href="#/agent/${id}">
          <div class="row">
            <h2>${escapeHtml(item.agent || item.id)}</h2>
            ${chip(item.status)}
          </div>
          <p class="meta">${escapeHtml(item.title || item.kind)} · ${escapeHtml(item.workspace_id || "")}</p>
        </a>`;
      })
      .join("");
  } catch (err) {
    setConn(err.message);
    view.innerHTML = empty("Inbox needs a live herdr snapshot (poll of blocked/done). OS push notifications are not supported yet.");
  }
}

function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js").catch(() => {});
}

window.addEventListener("hashchange", route);
route();
