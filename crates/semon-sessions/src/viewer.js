"use strict";

const app = document.getElementById("app");
if (location.search.includes("t=")) history.replaceState(null, "", location.pathname);

function element(tag, text, className) {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = text;
  if (className) node.className = className;
  return node;
}

function link(text, href) {
  const node = element("a", text);
  node.href = href;
  return node;
}

function sessionUrl(harness, id) {
  return "/s/" + encodeURIComponent(harness) + "/" + encodeURIComponent(id);
}

async function json(url) {
  const response = await fetch(url, { credentials: "same-origin", cache: "no-store" });
  if (!response.ok) throw new Error("Request failed: " + response.status);
  return response.json();
}

function showError(error) {
  app.replaceChildren(element("p", error.message || String(error)));
}

function drawNode(node) {
  const card = element("section", undefined, "node");
  const heading = element("div", undefined, "node-heading");
  heading.append(link(node.label || node.id, sessionUrl(node.harness, node.id)));
  const live = node.pid != null || node.state === "running";
  heading.append(element("span", node.state, "state" + (live ? " live" : "")));
  card.append(heading);
  const details = [node.harness, node.kind, node.models.join(", "), node.last_activity_age_seconds == null ? "" : node.last_activity_age_seconds + "s ago", node.tokens.total + " tokens"];
  if (node.open_tools.length) details.push("open: " + node.open_tools.map(tool => tool.name).join(", "));
  card.append(element("div", details.filter(Boolean).join(" · "), "meta"));
  for (const child of node.children) card.append(drawNode(child));
  return card;
}

async function tree() {
  const data = await json("/api/tree");
  const title = element("h1", "Session tree");
  const nodes = data.roots.length ? data.roots.map(drawNode) : [element("p", "No sessions in this window.")];
  app.replaceChildren(title, ...nodes);
}

function entryKey(item) { return item.offset + ":" + item.block; }

async function transcript(harness, id) {
  const base = "/api/transcript?harness=" + encodeURIComponent(harness) + "&id=" + encodeURIComponent(id);
  const items = new Map();
  let before = null;
  let end = 0;
  let live = false;
  let children = [];
  let loading = false;
  let initial = true;
  const title = element("h1", harness + " · " + id);
  const toolbar = element("div", undefined, "toolbar");
  const earlier = element("button", "Load earlier");
  toolbar.append(earlier);
  const status = element("span", "", "muted");
  toolbar.append(status);
  const childList = element("div", undefined, "children");
  const conversation = element("div");
  app.replaceChildren(title, toolbar, childList, conversation);

  function drawEntry(item) {
    const card = element("section", undefined, "entry " + item.kind);
    const label = item.name ? item.kind + " · " + item.name : item.kind + (item.kind === "tool_result" && item.tool_id ? " · " + item.tool_id : "");
    card.append(element("h2", label));
    const body = element("pre", item.text);
    if (item.collapsed) {
      const details = element("details");
      details.append(element("summary", item.truncated ? "Show preview (truncated)" : "Show content"), body);
      card.append(details);
    } else {
      card.append(body);
    }
    if (item.link) card.append(link("Open linked session", item.link));
    if (item.truncated) {
      const expand = element("button", "Expand entry");
      expand.addEventListener("click", async () => {
        expand.disabled = true;
        try {
          const url = "/api/entry?harness=" + encodeURIComponent(harness) + "&id=" + encodeURIComponent(id) + "&offset=" + item.offset + "&block=" + item.block;
          const full = await json(url);
          body.textContent = full.text;
          expand.textContent = full.truncated ? "Capped at 8 MB" : "Expanded";
        } catch (error) { expand.textContent = error.message; }
      });
      card.append(expand);
    }
    return card;
  }

  function redraw() {
    const calls = new Map();
    const ordered = [...items.values()].sort((a, b) => a.offset - b.offset || a.block - b.block);
    const cards = [];
    for (const item of ordered) {
      const card = drawEntry(item);
      if (item.kind === "tool_use" && item.tool_id) calls.set(item.tool_id, card);
      if (item.kind === "tool_result" && item.tool_id && calls.has(item.tool_id)) calls.get(item.tool_id).append(card);
      else cards.push(card);
    }
    conversation.replaceChildren(...cards);
    earlier.hidden = before == null;
    status.textContent = live ? "Live · follows new entries" : "Ended";
    childList.replaceChildren(...children.map(child => link(child.label || child.id, sessionUrl(child.harness, child.id))));
  }

  async function load(url, mode = "follow") {
    if (loading) return;
    loading = true;
    try {
      const page = await json(url);
      for (const item of page.entries) items.set(entryKey(item), item);
      if (mode === "earlier" || initial) before = page.before;
      if (mode !== "earlier") end = page.end;
      initial = false;
      live = page.live;
      children = page.children;
      redraw();
    } finally { loading = false; }
  }
  earlier.addEventListener("click", () => { if (before != null) load(base + "&before=" + before, "earlier").catch(showError); });
  await load(base, "initial");
  function follow() { if (live && !document.hidden) load(base + "&after=" + end).catch(showError); }
  setInterval(follow, 5000);
  document.addEventListener("visibilitychange", () => { if (!document.hidden) follow(); });
}

const parts = location.pathname.split("/").filter(Boolean);
if (parts.length === 0) {
  tree().catch(showError);
  function refreshTree() { if (!document.hidden) tree().catch(showError); }
  setInterval(refreshTree, 5000);
  document.addEventListener("visibilitychange", () => { if (!document.hidden) refreshTree(); });
} else if (parts.length === 3 && parts[0] === "s") {
  transcript(decodeURIComponent(parts[1]), decodeURIComponent(parts[2])).catch(showError);
} else {
  showError(new Error("Not found"));
}
