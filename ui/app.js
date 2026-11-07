/*
 * Lumen UI (made by Akulav). Plain JS, no framework, no bundler.
 *
 * Data flow:
 *   - `call(cmd, args)` → Rust #[tauri::command]s in src-tauri/src/commands.rs
 *   - Rust pushes updates as events (`listen(...)` at the bottom): server-status, metrics,
 *     download-progress, index-progress and chat-event (streaming tokens, tools, compression…)
 *   - All UI state lives in the single `S` object; render*() functions rebuild their section from it.
 *     The streaming assistant message is the only thing patched in place (see "live streaming").
 */
"use strict";
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// ---------------------------------------------------------------- helpers
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const el = (html) => { const t = document.createElement("template"); t.innerHTML = html.trim(); return t.content.firstElementChild; };
const fmtBytes = (b) => { if (!b) return "0 GB"; const g = b / 1073741824; return g >= 1 ? g.toFixed(g >= 10 ? 1 : 2) + " GB" : (b / 1048576).toFixed(0) + " MB"; };
const fmtK = (n) => (n >= 1000 ? (n / 1000).toFixed(n >= 10000 ? 0 : 1) + "k" : String(n || 0));
const estTokens = (s) => Math.ceil((s || "").length / 3.6);

const ICONS = {
  plus: '<path d="M12 5v14M5 12h14"/>',
  chat: '<path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1-4.6A8 8 0 1 1 21 12z"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  cube: '<path d="M21 16V8l-9-5-9 5v8l9 5 9-5z"/><path d="M3.3 7.5 12 12l8.7-4.5M12 22V12"/>',
  gear: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>',
  brain: '<path d="M9.5 2A2.5 2.5 0 0 0 7 4.5v.3A3 3 0 0 0 4.6 9 3 3 0 0 0 4 14.4 3 3 0 0 0 7 19a2.5 2.5 0 0 0 5 .5V4.5A2.5 2.5 0 0 0 9.5 2zM14.5 2A2.5 2.5 0 0 1 17 4.5v.3A3 3 0 0 1 19.4 9a3 3 0 0 1 .6 5.4A3 3 0 0 1 17 19a2.5 2.5 0 0 1-5 .5"/>',
  globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>',
  telescope: '<path d="m10 14 2 7M14 14l-2 7M4.5 13.5 3 11l14-8 3 5.2-14 8z"/><path d="m13 5 3 5.2"/>',
  send: '<path d="M12 19V5M5 12l7-7 7 7"/>',
  stop: '<rect x="7" y="7" width="10" height="10" rx="2" fill="currentColor"/>',
  copy: '<rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/>',
  check: '<path d="M20 6 9 17l-5-5"/>',
  x: '<path d="M18 6 6 18M6 6l12 12"/>',
  edit: '<path d="M12 20h9M16.5 3.5a2.1 2.1 0 1 1 3 3L7 19l-4 1 1-4z"/>',
  refresh: '<path d="M21 12a9 9 0 1 1-2.6-6.4L21 8"/><path d="M21 3v5h-5"/>',
  trash: '<path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6"/>',
  pin: '<path d="M12 17v5M9 3h6l-1 6 4 4H6l4-4z"/>',
  chevron: '<path d="m6 9 6 6 6-6"/>',
  sparkle: '<path d="M12 3c.5 4.5 2.5 6.5 9 9-6.5 2.5-8.5 4.5-9 9-.5-4.5-2.5-6.5-9-9 6.5-2.5 8.5-4.5 9-9z"/>',
  bolt: '<path d="M13 2 3 14h9l-1 8 10-12h-9z"/>',
  clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
  file: '<path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><path d="M14 3v6h6"/>',
  download: '<path d="M12 3v12M7 10l5 5 5-5M5 21h14"/>',
  link: '<path d="M10 13a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-1 1"/><path d="M14 11a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l1-1"/>',
  more: '<circle cx="5" cy="12" r="1.2"/><circle cx="12" cy="12" r="1.2"/><circle cx="19" cy="12" r="1.2"/>',
  compress: '<path d="M4 14h6v6M20 10h-6V4M14 10l7-7M3 21l7-7"/>',
  undo: '<path d="M9 14 4 9l5-5"/><path d="M4 9h11a5 5 0 0 1 0 10h-3"/>',
  chip: '<rect x="5" y="5" width="14" height="14" rx="2"/><path d="M9 9h6v6H9zM9 1v4M15 1v4M9 19v4M15 19v4M1 9h4M1 15h4M19 9h4M19 15h4"/>',
  list: '<path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"/>',
  book: '<path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20V3H6.5A2.5 2.5 0 0 0 4 5.5z"/><path d="M4 19.5V21h16"/>',
  warn: '<path d="M12 9v4M12 17h.01"/><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"/>',
  power: '<path d="M18.4 6.6a9 9 0 1 1-12.8 0M12 2v10"/>',
  play: '<path d="m6 4 14 8-14 8z"/>',
  layers: '<path d="m12 2 10 6-10 6L2 8z"/><path d="m2 16 10 6 10-6"/>',
  git: '<circle cx="6" cy="6" r="2.5"/><circle cx="6" cy="18" r="2.5"/><circle cx="18" cy="9" r="2.5"/><path d="M6 8.5v7M18 11.5c0 4-6 2.5-10.3 5.3"/>',
  terminal: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="m7 9 3 3-3 3M13 15h4"/>',
  calc: '<rect x="5" y="3" width="14" height="18" rx="2"/><path d="M8 7h8M8 11h.01M12 11h.01M16 11h.01M8 15h.01M12 15h.01M16 15h.01M8 18h.01M12 18h.01M16 18h.01"/>',
  code: '<path d="m8 7-5 5 5 5M16 7l5 5-5 5"/>',
  shield: '<path d="M12 3 4 6v6c0 4.5 3.4 8.3 8 9 4.6-.7 8-4.5 8-9V6z"/><path d="m9 12 2 2 4-4"/>',
  diff: '<path d="M6 3v12M3 6h6M15 18h6"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/>',
};
const icon = (n, cls = "") => `<svg class="i ${cls}" viewBox="0 0 24 24">${ICONS[n] || ""}</svg>`;

// ---------------------------------------------------------------- state
// view: chat | models | settings | project. live: the assistant message currently streaming.
// liveCtxBase: context tokens before the current generation (meter = base + generated tokens).
const S = {
  version: "",
  settings: {}, info: { state: "stopped" }, catalog: [], root: "", runtime: null,
  tab: "chats", view: "chat", chatId: null, chat: null, messages: [], budget: 0, ctxUsed: 0,
  chats: [], projects: [], projectId: null, models: [], downloads: {}, metrics: {},
  busy: {}, live: null, liveCtxBase: 0, showOld: false, filter: "", searchHits: null,
  toggles: { think: true, web: false, research: false }, needsReload: false, lastTps: 0,
  drafts: {}, // unsent composer text per chat id ("" = no chat yet)
  updates: {}, // from updater.rs: { current, app: {version,url}|null, runtime: tag|null, runtime_current, checking, installing, error }
  lastPromptTps: 0, engineOpen: false,
  confirms: {}, // tool_confirm requests still waiting for the user's answer, by id
};
/** Translate key `k` in the current UI language (falls back to English), filling {placeholders}. */
const t = (k, vars) => {
  let s = (I18N[S.settings.ui_lang || "en"] || I18N.en)[k] ?? I18N.en[k] ?? k;
  if (vars) for (const [a, b] of Object.entries(vars)) s = s.replace(`{${a}}`, b);
  return s;
};

function toast(msg, err = false) {
  const n = el(`<div class="toast ${err ? "err" : ""}">${icon(err ? "warn" : "check", err ? "bad-ic" : "ok-ic")}<div>${esc(msg)}</div></div>`);
  $("#toasts").appendChild(n);
  setTimeout(() => n.remove(), err ? 7000 : 3200);
}
/** invoke() that also shows the error as a toast before rethrowing. */
async function call(cmd, args) {
  try { return await invoke(cmd, args); } catch (e) { toast(String(e), true); throw e; }
}
function confirmBox(text, okLabel = "OK") {
  return new Promise((res) => {
    const m = el(`<div class="modal-bg"><div class="modal"><div class="modal-text">${esc(text)}</div><div class="acts"><button class="btn ghost" data-a="no">${t("cancel")}</button><button class="btn primary" data-a="yes">${esc(okLabel)}</button></div></div></div>`);
    m.addEventListener("click", (e) => { const a = e.target.closest("[data-a]")?.dataset.a; if (a || e.target === m) { m.remove(); res(a === "yes"); } });
    m.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); m.remove(); res(true); } });
    document.body.appendChild(m);
    m.querySelector('[data-a="yes"]').focus();
  });
}
/** Closes the top-most modal; returns true if there was one. */
function closeTopModal() {
  const m = $$(".modal-bg").pop();
  if (!m) return false;
  const cancel = m.querySelector('[data-a="no"], [data-a="c"]');
  if (cancel) cancel.click(); else m.remove();
  return true;
}
let openMenuEl = null;
function popMenu(anchor, html, onClick) {
  closeMenu();
  const r = anchor.getBoundingClientRect();
  const m = el(`<div class="menu">${html}</div>`);
  document.body.appendChild(m);
  const w = m.offsetWidth;
  m.style.top = r.bottom + 6 + "px";
  m.style.left = Math.max(8, Math.min(window.innerWidth - w - 8, r.right - w)) + "px";
  m.addEventListener("click", (e) => { const b = e.target.closest("button[data-v]"); if (b) { closeMenu(); onClick(b.dataset.v); } });
  openMenuEl = m;
  setTimeout(() => document.addEventListener("click", closeMenuOutside), 0);
}
function closeMenuOutside(e) { if (openMenuEl && !openMenuEl.contains(e.target)) closeMenu(); }
function closeMenu() { openMenuEl?.remove(); openMenuEl = null; document.removeEventListener("click", closeMenuOutside); }

// ---------------------------------------------------------------- markdown
const mdRenderer = new marked.Renderer();
mdRenderer.code = function ({ text, lang }) {
  const l = (lang || "").split(/\s/)[0];
  let html;
  try { html = l && hljs.getLanguage(l) ? hljs.highlight(text, { language: l }).value : hljs.highlightAuto(text).value; } catch { html = esc(text); }
  return `<div class="code"><div class="code-h"><span>${esc(l || "code")}</span><button class="copy-code">${icon("copy")}${t("copy")}</button></div><pre><code class="hljs">${html}</code></pre></div>`;
};
marked.setOptions({ gfm: true, breaks: false, renderer: mdRenderer });
/** Markdown → sanitized HTML. With research `sources`, [n] markers become clickable citation chips. */
function renderMd(text, sources) {
  let html = marked.parse(text || "");
  if (sources && sources.length) {
    const byN = Object.fromEntries(sources.map((s) => [String(s.n), s]));
    html = html.replace(/\[(\d{1,3})\](?![^<]*<\/code>)/g, (m, n) => byN[n] ? `<a class="cite" href="${esc(byN[n].url)}" title="${esc(byN[n].title)}">${n}</a>` : m);
  }
  return DOMPurify.sanitize(html, { ADD_ATTR: ["target"] });
}
document.addEventListener("click", (e) => {
  const cc = e.target.closest(".copy-code");
  if (cc) {
    const code = cc.closest(".code").querySelector("code").innerText;
    navigator.clipboard.writeText(code);
    cc.innerHTML = icon("check") + t("copied");
    setTimeout(() => (cc.innerHTML = icon("copy") + t("copy")), 1400);
    return;
  }
  const a = e.target.closest("a[href]");
  if (a) {
    const href = a.getAttribute("href");
    e.preventDefault();
    if (/^https?:\/\//i.test(href)) invoke("open_url", { url: href }).catch(() => {});
  }
});

// ---------------------------------------------------------------- sidebar
function renderSidebarStatic() {
  const cp = contextProject();
  $("#newBtn").innerHTML = icon("plus") + esc(S.tab === "projects" ? t("newProject") : cp ? t("newChatIn", { p: cp.name }) : t("newChat"));
  $("#tabChats").innerHTML = icon("chat") + t("chats");
  $("#tabProjects").innerHTML = icon("folder") + t("projects");
  $("#tabChats").classList.toggle("on", S.tab === "chats");
  $("#tabProjects").classList.toggle("on", S.tab === "projects");
  $("#searchIc").innerHTML = icon("search");
  $("#searchInp").placeholder = t("search");
  $("#navModels").innerHTML = icon("cube") + `<span class="t">${t("models")}</span>`;
  $("#navSettings").innerHTML = icon("gear") + `<span class="t">${t("settings")}</span>`;
  $("#navModels").classList.toggle("on", S.view === "models");
  $("#navSettings").classList.toggle("on", S.view === "settings");
  const checking = S.updates?.checking;
  $("#credit").innerHTML = `<span>${t("madeBy")} <b>Akulav</b></span><span class="ver">v${esc(S.version)}<button id="creditCheck" title="${t("checkUpdates")}" ${checking ? "disabled" : ""}>${checking ? '<span class="spinner"></span>' : icon("refresh")}</button></span>`;
  renderUpdateNav();
}
$("#credit").addEventListener("click", (e) => { if (e.target.closest("#creditCheck")) checkUpdatesNow(); });

// ---------------------------------------------------------------- updates
/** Sidebar entries that appear only when GitHub has something newer (same entries as the tray menu). */
function renderUpdateNav() {
  const u = S.updates || {};
  let html = "";
  if (u.app && !u.self_built) html += `<div class="item update" data-upd="app">${icon("download")}<span class="t">${t("updateApp", { v: esc(u.app.version) })}</span></div>`;
  if (u.runtime) html += `<div class="item update rt" data-upd="runtime">${icon("chip")}<span class="t">${t("updateRuntime", { v: esc(u.runtime) })}</span></div>`;
  if (u.installing) html = `<div class="item update"><span class="spinner"></span><span class="t">${t("updateInstalling")}</span></div>`;
  $("#navUpdate").innerHTML = html;
}
$("#navUpdate").addEventListener("click", (e) => {
  const k = e.target.closest("[data-upd]")?.dataset.upd;
  if (k === "app") promptAppUpdate();
  if (k === "runtime") promptRuntimeUpdate();
});
async function promptAppUpdate() {
  const a = S.updates.app;
  if (!a || S.updates.installing) return;
  if (S.updates.self_built) return toast(t("selfBuilt"), true);
  const busy = Object.values(S.busy).some(Boolean);
  const text = t("updateConfirm", { v: a.version }) + (busy ? "\n\n" + t("updateBusy") : "");
  if (!(await confirmBox(text, t("updateNow")))) return;
  toast(t("updateDownloading", { v: a.version }));
  call("install_update").catch(() => {}); // on success Lumen exits and restarts into the new version
}
async function promptRuntimeUpdate() {
  const r = S.updates.runtime;
  if (!r) return;
  if (!(await confirmBox(t("runtimeConfirm", { v: r }), t("updateNow")))) return;
  toast(t("downloading") + " llama.cpp " + r + "…");
  try { S.runtime = await call("update_runtime"); toast("llama.cpp " + S.runtime + " ✓"); } catch {}
  if (S.view === "settings") renderSettings();
}
/** Manual check (Settings button, tray "Check for updates"); the background check is silent. */
async function checkUpdatesNow() {
  toast(t("updateChecking"));
  const u = await call("check_updates");
  if (u.error) toast(u.error, true);
  else if (!u.app && !u.runtime) toast(t("upToDate", { v: u.current }));
}

function dateGroup(ts) {
  const d = new Date(ts), now = new Date();
  const day = (x) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = (day(now) - day(d)) / 86400000;
  if (diff < 1) return t("today");
  if (diff < 2) return t("yesterday");
  if (diff < 8) return t("week");
  return t("older");
}

function renderSidebar() {
  renderSidebarStatic();
  const list = $("#sideList");
  const f = S.filter.toLowerCase();
  let html = "";
  if (S.tab === "chats") {
    // title match, or (from search_chats) a match inside the messages
    const chats = S.chats.filter((c) => !f || c.title.toLowerCase().includes(f) || S.searchHits?.has(c.id));
    let last = null;
    for (const c of chats) {
      const g = c.pinned ? t("pinned") : dateGroup(c.updated);
      if (g !== last) { html += `<div class="group-label">${g}</div>`; last = g; }
      html += chatItem(c);
    }
    if (!chats.length) html = `<div class="empty">${t("noChats")}</div>`;
  } else {
    const ps = S.projects.filter((p) => !f || p.name.toLowerCase().includes(f));
    for (const p of ps) {
      const on = (S.view === "project" && S.projectId === p.id) || (S.view === "chat" && S.chat?.project_id === p.id);
      html += `<div class="item ${on ? "on" : ""}" data-project="${p.id}">${icon("folder")}<div style="flex:1;min-width:0"><div class="t">${esc(p.name)}</div><div class="sub">${esc(p.folder)}</div></div>
        <div class="acts"><button data-act="delproj" data-id="${p.id}" title="${t("del")}">${icon("trash")}</button></div></div>`;
    }
    if (!ps.length) html = `<div class="empty">${t("projectHint")}</div>`;
  }
  list.innerHTML = html;
}
function chatItem(c) {
  const on = S.view === "chat" && S.chatId === c.id;
  return `<div class="item ${on ? "on" : ""}" data-chat="${c.id}">${S.busy[c.id] ? '<span class="busy-dot"></span>' : icon("chat")}<span class="t">${esc(c.title)}</span>
    <div class="acts"><button data-act="pin" data-id="${c.id}" title="${c.pinned ? t("unpin") : t("pin")}">${icon("pin")}</button><button data-act="delchat" data-id="${c.id}" title="${t("del")}">${icon("trash")}</button></div></div>`;
}

$("#sideList").addEventListener("click", async (e) => {
  const act = e.target.closest("[data-act]");
  if (act) {
    e.stopPropagation();
    const id = act.dataset.id;
    if (act.dataset.act === "delchat") {
      await deleteChat(id);
    } else if (act.dataset.act === "pin") {
      const c = S.chats.find((x) => x.id === id);
      await call("pin_chat", { id, pinned: !c?.pinned });
      await refreshChats();
    } else if (act.dataset.act === "delproj") {
      const p = S.projects.find((x) => x.id === id);
      if (!(await confirmBox(t("confirmDeleteProject", { x: p?.name }), t("del")))) return;
      await call("delete_project", { id });
      if (S.projectId === id) { S.projectId = null; showView("chat"); }
      await refreshProjects();
    }
    return;
  }
  const c = e.target.closest("[data-chat]");
  if (c) return openChat(c.dataset.chat);
  const p = e.target.closest("[data-project]");
  if (p) return openProject(p.dataset.project);
});
$$(".tabs button").forEach((b) => b.addEventListener("click", () => { S.tab = b.dataset.tab; renderSidebar(); }));
let searchTimer = null;
$("#searchInp").addEventListener("input", (e) => {
  S.filter = e.target.value;
  renderSidebar();
  // also look inside messages (debounced; the title filter above is instant)
  clearTimeout(searchTimer);
  const q = S.filter.trim();
  if (q.length < 2) { S.searchHits = null; return; }
  searchTimer = setTimeout(async () => {
    const ids = await invoke("search_chats", { query: q }).catch(() => []);
    if (S.filter.trim() !== q) return; // a newer query is on its way
    S.searchHits = new Set(ids);
    renderSidebar();
  }, 220);
});
$("#searchInp").addEventListener("keydown", (e) => { if (e.key === "Escape") { e.stopPropagation(); e.target.value = ""; e.target.dispatchEvent(new Event("input")); e.target.blur(); } });
$("#newBtn").addEventListener("click", () => (S.tab === "projects" ? newProjectModal() : newChat(contextProject()?.id || null)));
/** The project the user is in (its page, or one of its chats): new chats are created there so they see its files. */
function contextProject() {
  const id = S.view === "project" ? S.projectId : S.view === "chat" ? S.chat?.project_id : null;
  return id ? S.projects.find((p) => p.id === id) || null : null;
}
$("#navModels").addEventListener("click", () => showView("models"));
$("#navSettings").addEventListener("click", () => showView("settings"));

async function refreshChats() { S.chats = await call("list_chats", { projectId: null }); renderSidebar(); }
/** Asks, deletes, and clears the view when the open chat was the one deleted. */
async function deleteChat(id) {
  if (!(await confirmBox(t("confirmDeleteChat"), t("del")))) return;
  await call("delete_chat", { id });
  delete S.drafts[id];
  if (S.chatId === id) { S.chatId = null; S.chat = null; S.messages = []; input.value = ""; autosize(); renderChat(); }
  await refreshChats();
  refreshProjectChats();
}
async function exportChat() {
  const path = await call("export_chat", { id: S.chatId });
  if (path) toast(t("exported", { x: path }));
}
async function refreshProjects() { S.projects = await call("list_projects"); renderSidebar(); }

// ---------------------------------------------------------------- views
function showView(v) {
  S.view = v;
  $$(".view").forEach((x) => x.classList.toggle("on", x.id === "view-" + v));
  if (v === "models") { renderModels(); refreshModels(); }
  if (v === "settings") renderSettings();
  if (v === "chat") renderChat();
  renderTopbar();
  renderSidebar();
}

function renderTopbar() {
  const tb = $("#topbar");
  if (S.view === "chat") {
    const proj = S.chat?.project_id ? S.projects.find((p) => p.id === S.chat.project_id) : null;
    const { used, budget, pct, mark } = ctxNumbers();
    tb.innerHTML = `
      ${proj ? `<div class="crumb" data-go-project="${proj.id}">${icon("folder")}${esc(proj.name)} /</div>` : S.chat && S.projects.length ? `<button class="crumb link-proj" id="linkProj" title="${esc(t("linkProjectHint"))}">${icon("folder")}${esc(t("linkProject"))}</button>` : ""}
      <div class="title" id="chatTitle" title="${t("rename")}">${esc(S.chat?.title || t("newChat"))}</div>
      <div class="spacer"></div>
      ${S.chat ? `<div class="ctx" title="${t("ctx")}: ${used} / ${budget} tokens">
        <div class="ctx-top"><span>${t("ctx")}</span><span><b id="ctxNum">${fmtK(used)}</b> / ${fmtK(budget)}</span></div>
        <div class="ctx-bar"><div class="ctx-fill ${pct > mark ? "warn" : ""}" id="ctxFill" style="width:${pct}%"></div><div class="ctx-mark" style="left:${mark}%"></div></div></div>` : ""}
      ${modelPill()}
      ${S.chat ? `<button class="icon-btn" id="chatMore">${icon("more")}</button>` : ""}`;
    $("#chatTitle").addEventListener("dblclick", startRename);
    $("[data-go-project]", tb)?.addEventListener("click", (e) => openProject(e.currentTarget.dataset.goProject));
    $("#chatMore")?.addEventListener("click", (e) => chatMenu(e.currentTarget));
    $("#linkProj")?.addEventListener("click", (e) => projectMenu(e.currentTarget));
  } else {
    const titles = { models: t("models"), settings: t("settings"), project: S.projects.find((p) => p.id === S.projectId)?.name || t("projects") };
    tb.innerHTML = `<div class="title">${esc(titles[S.view])}</div><div class="spacer"></div>${modelPill()}`;
  }
  $("#modelPill")?.addEventListener("click", (e) => modelMenu(e.currentTarget));
}
/** Context meter values: tokens used (live while streaming), window size, fill % and compression mark %. */
function ctxNumbers() {
  const used = S.live ? S.liveCtxBase + (S.live.tokens || 0) : S.ctxUsed;
  const budget = S.budget || S.info.n_ctx || 0;
  const pct = budget ? Math.min(100, (used / budget) * 100) : 0;
  return { used, budget, pct, mark: (S.settings.compress_at || 0.72) * 100 };
}
function updateCtxMeter() {
  if (S.view !== "chat" || !S.chat) return;
  const { used, pct, mark } = ctxNumbers();
  const n = $("#ctxNum"), f = $("#ctxFill");
  if (n) n.textContent = fmtK(used);
  if (f) { f.style.width = pct + "%"; f.classList.toggle("warn", pct > mark); }
}
function shortModel(m) { return (m || "").replace(/\.gguf$/i, "").replace(/-UD-/, " ").replace(/-GGUF/i, ""); }
function modelPill() {
  const st = S.info.state;
  const label = st === "ready" ? shortModel(S.info.model) : st === "starting" ? t("loading") : st === "runtime" ? t("runtime") : st === "error" ? t("error") : t("noModel");
  return `<button class="pill" id="modelPill"><span class="dot ${st}"></span>${esc(label)}${icon("chevron")}</button>`;
}
async function modelMenu(anchor) {
  S.models = await call("list_models");
  let html = `<div class="lbl">${t("installed")}</div>`;
  for (const m of S.models) html += `<button data-v="load:${esc(m.file)}" class="${m.active ? "on" : ""}">${m.active ? icon("check", "ok-ic") : icon("cube")}<span style="flex:1">${esc(shortModel(m.file))}</span><small style="color:var(--text-3)">${fmtBytes(m.size)}</small></button>`;
  if (!S.models.length) html += `<div class="empty">${t("noInstalled")}</div>`;
  html += `<div class="sep"></div>`;
  if (S.info.state === "ready") html += `<button data-v="unload">${icon("power")}${t("unload")}</button>`;
  html += `<button data-v="manage">${icon("layers")}${t("models")}…</button>`;
  popMenu(anchor, html, (v) => {
    if (v === "manage") return showView("models");
    if (v === "unload") return call("unload_model");
    if (v.startsWith("load:")) loadModel(v.slice(5));
  });
}
function loadModel(file) {
  S.needsReload = false;
  call("load_model", { file }).then(() => toast(`${shortModel(file)} · ${t("ready")}`)).catch(() => {});
}
function chatMenu(anchor) {
  const opts = [0, 8192, 16384, 32768, 65536, 131072];
  let html = `<button data-v="compress">${icon("compress")}${t("compressNow")}</button><div class="sep"></div><div class="lbl">${t("ctxLimit")}</div>`;
  for (const o of opts) {
    const on = (S.chat.ctx_limit || 0) === o;
    html += `<button data-v="limit:${o}" class="${on ? "on" : ""}">${on ? icon("check", "ok-ic") : icon("layers")}${o ? t("kTokens", { n: o / 1024 }) : t("ctxAuto")}</button>`;
  }
  html += `<div class="sep"></div>${S.projects.length ? `<button data-v="project">${icon("folder")}${t("moveToProject")}…</button>` : ""}<button data-v="export">${icon("download")}${t("exportMd")}</button><button data-v="rename">${icon("edit")}${t("rename")}</button><button data-v="delete">${icon("trash")}${t("del")}</button>`;
  popMenu(anchor, html, async (v) => {
    if (v === "project") return projectMenu($("#chatMore") || anchor);
    if (v === "compress") return call("compress_chat", { chatId: S.chatId });
    if (v === "export") return exportChat();
    if (v === "rename") return startRename();
    if (v === "delete") return deleteChat(S.chatId);
    if (v.startsWith("limit:")) {
      const limit = +v.slice(6);
      S.budget = await call("set_chat_limit", { id: S.chatId, limit });
      S.chat.ctx_limit = limit; renderTopbar();
    }
  });
}
/** Links the open chat to a project (or unlinks it): the model gets that folder's files and tools from the next message. */
function projectMenu(anchor) {
  const cur = S.chat?.project_id || null;
  let html = `<div class="lbl">${t("moveToProject")}</div>`;
  for (const p of S.projects) html += `<button data-v="p:${esc(p.id)}" class="${p.id === cur ? "on" : ""}">${p.id === cur ? icon("check", "ok-ic") : icon("folder")}<span style="flex:1">${esc(p.name)}</span></button>`;
  if (cur) html += `<div class="sep"></div><button data-v="none">${icon("x")}${t("noProject")}</button>`;
  popMenu(anchor, html, async (v) => {
    const projectId = v === "none" ? null : v.slice(2);
    if (!S.chat || projectId === cur) return;
    await call("set_chat_project", { id: S.chatId, projectId });
    S.chat.project_id = projectId;
    const p = S.projects.find((x) => x.id === projectId);
    toast(p ? t("movedToProject", { p: p.name }) : t("movedOut"));
    await refreshChats();
    if (S.projectId) refreshProjectChats();
    renderTopbar(); renderSidebar(); updatePlaceholder();
  });
}
function startRename() {
  if (!S.chat) return;
  const n = $("#chatTitle");
  n.contentEditable = "true"; n.focus();
  document.getSelection().selectAllChildren(n);
  const done = async (save) => {
    n.contentEditable = "false";
    const title = n.textContent.trim();
    if (save && title && title !== S.chat.title) { await call("rename_chat", { id: S.chatId, title }); S.chat.title = title; refreshChats(); refreshProjectChats(); }
    else n.textContent = S.chat.title;
  };
  n.onkeydown = (e) => { if (e.key === "Enter") { e.preventDefault(); n.blur(); } if (e.key === "Escape") { n.textContent = S.chat.title; n.blur(); } };
  n.onblur = () => done(true);
}

// ---------------------------------------------------------------- chat view
async function newChat(projectId) {
  const c = await call("create_chat", { projectId });
  if (projectId) await refreshProjectChats(); else await refreshChats();
  await openChat(c.id);
  $("#input").focus();
}
async function openChat(id) {
  const r = await call("get_chat", { id });
  if (S.chatId !== id) {
    // keep what was being typed in the previous chat, restore this chat's draft
    S.drafts[S.chatId || ""] = input.value;
    input.value = S.drafts[id] || "";
    autosize();
  }
  S.chatId = id; S.chat = r.chat; S.messages = r.messages; S.budget = r.budget; S.ctxUsed = r.chat.ctx_used;
  S.busy[id] = r.busy; S.live = null; S.showOld = false;
  showView("chat");
  scrollBottom(true);
}
/** Rebuilds the whole thread. Consecutive assistant/tool messages are grouped into one "turn"
 *  (avatar + body); compressed messages are hidden behind a toggle. */
function renderChat() {
  const th = $("#thread");
  th.innerHTML = "";
  updatePlaceholder();
  $("#hint").textContent = t("hint");
  $("#jumpBtn").innerHTML = icon("chevron") + t("jumpLatest");
  renderComposer();
  if (!S.chat || !S.messages.length) { th.appendChild(welcome()); return; }
  const old = S.messages.filter((m) => m.compressed);
  if (old.length) {
    const sep = el(`<div class="compressed-sep"><span>${t("olderCompressed")}</span><button>${S.showOld ? t("hideOld") : t("showOld", { n: old.length })}</button></div>`);
    sep.querySelector("button").onclick = () => { S.showOld = !S.showOld; renderChat(); };
    th.appendChild(sep);
  }
  let turn = null;
  for (const m of S.messages) {
    if (m.compressed && !S.showOld) continue;
    turn = placeMessage(th, m, turn);
  }
  // a tool may still be waiting for permission (e.g. after switching chats)
  Object.values(S.confirms).filter((p) => p.chat_id === S.chatId).forEach(showConfirm);
  markLastTurn();
}
/** Adds a message element to the thread; returns the current assistant turn element (or null). */
function placeMessage(th, m, turn) {
  if (m.role === "user") { th.appendChild(userEl(m)); return null; }
  if (m.role === "event") { th.appendChild(eventEl(m)); return null; }
  if (!turn) turn = th.appendChild(newTurnEl());
  if (m.compressed) turn.classList.add("old");
  const body = turn.querySelector(".turn-body");
  if (m.role === "assistant") { const n = assistantEl(m); if (n) body.appendChild(n); }
  else if (m.role === "tool") body.appendChild(toolEl(m));
  return turn;
}
function currentTurn(create = true) {
  const th = $("#thread");
  const last = th.lastElementChild;
  if (last && last.classList.contains("turn")) return last;
  return create ? th.appendChild(newTurnEl()) : null;
}
const newTurnEl = () => el(`<div class="turn"><div class="avatar"></div><div class="turn-body"></div></div>`);
const fmtTime = (ms) => (ms ? new Date(ms).toLocaleString() : "");
function markLastTurn() {
  $$(".regen-btn").forEach((b) => (b.style.display = "none"));
  const turns = $$("#thread .turn");
  const last = turns[turns.length - 1];
  if (last && !S.busy[S.chatId]) { const b = $$(".regen-btn", last).pop(); if (b) b.style.display = ""; }
}

function welcome() {
  const noModels = !S.models.length;
  const rec = S.catalog.find((c) => c.recommended);
  const dl = rec && S.downloads["model:" + rec.file];
  const w = el(`<div class="welcome"><img src="logo.png" alt=""/><h1>${t("welcomeTitle")}</h1><p>${t("welcomeSub")}</p><div class="made-by">Lumen · ${t("madeBy")} <b>Akulav</b></div></div>`);
  if (noModels && rec) {
    const card = el(`<div class="setup-card"><div class="nm" style="display:flex;gap:10px;align-items:center;font-weight:600">${icon("sparkle")}${t("setupTitle")}</div>
      <div style="color:var(--text-2)">${t("setupBody")}</div>
      <div style="display:flex;gap:10px;align-items:center;flex-wrap:wrap"><b>${esc(rec.name)}</b><span class="tag acc">${esc(rec.tag)}</span><span class="tag">${rec.size_gb.toFixed(1)} GB</span></div>
      <div data-dl="model:${esc(rec.file)}">${dl ? progressHtml(dl) : ""}</div>
      <div style="display:flex;gap:8px"><button class="btn primary" id="dlRec" ${dl ? "disabled" : ""}>${icon("download")}${t("downloadRec")}</button><button class="btn" id="goModels">${t("openModels")}</button></div></div>`);
    card.querySelector("#dlRec").onclick = () => startDownload(rec.repo, rec.file);
    card.querySelector("#goModels").onclick = () => showView("models");
    w.appendChild(card);
  } else {
    const sg = el(`<div class="sugg"></div>`);
    [["s1", "book"], ["s2", "chip"], ["s3", "globe"], ["s4", "telescope"]].forEach(([k, ic], i) => {
      const b = el(`<button>${icon(ic)}<div><div style="color:var(--text);font-weight:500">${t(k)}</div><div style="font-size:12.5px;margin-top:2px">${esc(t(k + "p"))}</div></div></button>`);
      b.onclick = () => { if (i === 3) setToggle("research", true); $("#input").value = t(k + "p"); autosize(); $("#input").focus(); };
      sg.appendChild(b);
    });
    w.appendChild(sg);
  }
  return w;
}

function userEl(m) {
  const n = el(`<div class="msg-user" data-id="${m.id}" title="${esc(fmtTime(m.created))}"><div class="bubble"></div><div class="msg-tools"></div></div>`);
  n.querySelector(".bubble").textContent = m.content;
  const ret = m.meta?.retrieved;
  if (ret && ret.length) {
    const a = el(`<div class="attach"></div>`);
    a.innerHTML = `<span class="chip" style="border-style:dashed">${icon("layers")}${t("retrieved")}</span>` + ret.slice(0, 6).map((r) => `<span class="chip">${icon("file")}${esc(r.path)}:${esc(r.lines)}</span>`).join("");
    n.insertBefore(a, n.querySelector(".msg-tools"));
  }
  if (m.meta?.research) n.querySelector(".bubble").insertAdjacentHTML("afterbegin", `<div style="font-size:11.5px;color:#aee6ff;display:flex;gap:5px;align-items:center;margin-bottom:4px">${icon("telescope")}${t("research")}</div>`);
  const tools = n.querySelector(".msg-tools");
  tools.innerHTML = `<button data-a="copy" title="${t("copy")}">${icon("copy")}</button><button data-a="edit">${icon("edit")}${t("edit")}</button><button data-a="del" title="${t("deleteExchange")}">${icon("trash")}</button>`;
  tools.onclick = (e) => {
    const a = e.target.closest("button")?.dataset.a;
    if (a === "copy") { navigator.clipboard.writeText(m.content); toast(t("copied")); }
    if (a === "edit") editUser(n, m);
    if (a === "del") deleteExchange(m);
  };
  return n;
}
/** Deletes this question and its whole answer (tool steps included), then redraws the chat. */
async function deleteExchange(m) {
  if (S.busy[S.chatId]) return toast(t("waitForReply"), true);
  if (!(await confirmBox(t("confirmDeleteExchange"), t("del")))) return;
  S.ctxUsed = await call("delete_exchange", { chatId: S.chatId, messageId: m.id });
  // drop the exchange locally: this message up to the next question
  const i = S.messages.findIndex((x) => x.id === m.id);
  let j = i + 1;
  while (j < S.messages.length && S.messages[j].role !== "user") j++;
  if (i >= 0) S.messages.splice(i, j - i);
  S.liveCtxBase = S.ctxUsed;
  renderChat();
  updateCtxMeter();
}
function editUser(n, m) {
  if (S.busy[S.chatId]) return;
  const box = el(`<div style="width:100%;min-width:420px"><textarea class="inp" style="width:100%;min-height:90px"></textarea><div style="display:flex;gap:8px;justify-content:flex-end;margin-top:6px"><button class="btn ghost" data-a="c">${t("cancel")}</button><button class="btn primary" data-a="s">${t("save")}</button></div></div>`);
  box.querySelector("textarea").value = m.content;
  n.replaceChildren(box);
  box.querySelector("textarea").focus();
  box.onclick = async (e) => {
    const a = e.target.closest("button")?.dataset.a;
    if (a === "c") return renderChat();
    if (a === "s") {
      const text = box.querySelector("textarea").value.trim();
      if (!text) return;
      await call("edit_and_resend", { chatId: S.chatId, messageId: m.id, text, think: S.toggles.think, web: S.toggles.web });
      const idx = S.messages.findIndex((x) => x.id === m.id);
      S.messages = S.messages.slice(0, idx);
      S.liveCtxBase = S.ctxUsed + estTokens(text);
      renderChat();
    }
  };
}

function thinkEl(reasoning, live, seconds) {
  const n = el(`<div class="think ${live ? "live" : ""}"><div class="think-h">${icon("sparkle", "spark")}<span class="lbl"></span>${icon("chevron", "chev")}</div><div class="think-b"></div></div>`);
  n.querySelector(".lbl").innerHTML = live ? `<span class="shimmer">${t("thinking")}…</span>` : esc(seconds ? t("thoughtFor", { s: seconds }) : t("reasoning"));
  n.querySelector(".think-b").textContent = reasoning || "";
  n.querySelector(".think-h").onclick = () => {
    n.classList.toggle("open");
    if (n.classList.contains("open")) { const b = n.querySelector(".think-b"); b.scrollTop = b.scrollHeight; }
  };
  return n;
}
function statsHtml(meta) {
  if (!meta) return "";
  const parts = [];
  if (meta.tps) parts.push(`<span title="tokens/s">${icon("bolt")}${meta.tps.toFixed(1)} ${t("tps")}</span>`);
  if (meta.tokens) parts.push(`<span>${icon("layers")}${meta.tokens} tok</span>`);
  if (meta.elapsed_ms) parts.push(`<span>${icon("clock")}${(meta.elapsed_ms / 1000).toFixed(1)}s</span>`);
  if (meta.prompt_tps && meta.prompt_evaluated > 1) parts.push(`<span title="${t("promptSpeedHint", { n: meta.prompt_evaluated })}">${icon("list")}${t("promptSpeed", { n: fmtK(Math.round(meta.prompt_tps)) })}</span>`);
  if (meta.cached_tokens) parts.push(`<span title="${t("cacheHit")}">${icon("check")}${fmtK(meta.cached_tokens)} ${t("cached")}</span>`);
  return parts.join("");
}
function assistantEl(m) {
  const hasText = m.content && m.content.trim();
  if (!hasText && !m.reasoning) return null;
  const n = el(`<div class="seg" data-id="${m.id}" style="display:flex;flex-direction:column;gap:10px"></div>`);
  if (m.reasoning && m.reasoning.trim()) n.appendChild(thinkEl(m.reasoning, false, m.meta?.think_s));
  if (hasText) {
    const md = el(`<div class="md"></div>`);
    md.innerHTML = renderMd(m.content, m.meta?.sources);
    n.appendChild(md);
    if (m.meta?.sources?.length) {
      const s = el(`<div class="sources"></div>`);
      s.innerHTML = m.meta.sources.map((x) => `<a class="chip" href="${esc(x.url)}" title="${esc(x.url)}">${x.n}. ${esc(x.title || x.url)}</a>`).join("");
      n.appendChild(s);
    }
    const foot = el(`<div style="display:flex;align-items:center;gap:10px" title="${esc(fmtTime(m.created))}"><div class="stats">${statsHtml(m.meta)}</div><div class="msg-tools"><button data-a="copy">${icon("copy")}</button><button data-a="regen" class="regen-btn">${icon("refresh")}${t("regenerate")}</button></div></div>`);
    foot.querySelector(".msg-tools").onclick = (e) => {
      const a = e.target.closest("button")?.dataset.a;
      if (a === "copy") { navigator.clipboard.writeText(m.content); toast(t("copied")); }
      if (a === "regen") regenerate();
    };
    n.appendChild(foot);
  }
  return n;
}

const TOOL_LABEL = {
  search_project: "toolSearch", list_files: "toolList", read_file: "toolRead", write_file: "toolWrite", edit_file: "toolEdit", web_search: "toolWeb", fetch_url: "toolFetch",
  grep: "toolGrep", file_outline: "toolOutline", find_symbol: "toolSymbol", compare_files: "toolCompare", git_status: "toolGitStatus", git_diff: "toolGitDiff",
  run_command: "toolRun", report_findings: "toolFindings", audit_code: "toolAudit", calculate: "toolCalc", current_datetime: "toolTime",
};
const TOOL_ICON = {
  search_project: "search", list_files: "list", read_file: "file", write_file: "edit", edit_file: "edit", web_search: "globe", fetch_url: "link",
  grep: "search", file_outline: "code", find_symbol: "code", compare_files: "diff", git_status: "git", git_diff: "git",
  run_command: "terminal", report_findings: "shield", audit_code: "shield", calculate: "calc", current_datetime: "clock",
};
function toolArgSummary(name, argsRaw) {
  let a = {};
  try { a = JSON.parse(argsRaw || "{}"); } catch {}
  if (name === "compare_files") return [a.path_a, a.path_b].filter(Boolean).join(" ↔ ");
  if (name === "git_diff") return [a.base, a.staged ? "--staged" : "", a.path].filter(Boolean).join(" ");
  if (name === "grep") return (a.pattern || "") + (a.glob ? `  (${a.glob})` : "");
  return a.query || a.command || a.expression || a.name || a.path || a.url || "";
}
const SEV_ORDER = ["critical", "high", "medium", "low", "info"];
/** Code-review card from report_findings: counts per severity, then one row per finding. */
function findingsEl(d, m) {
  const list = d.findings || [];
  const counts = SEV_ORDER.filter((s) => d.counts?.[s]).map((s) => `<span class="sev sev-${s}">${d.counts[s]} ${esc(t("sev_" + s))}</span>`).join("");
  const n = el(`<div class="review" data-call="${esc(m.meta?.tool_call_id || "")}"><div class="review-h">${icon("shield")}<span class="name">${esc(t("reviewTitle"))}</span><span class="counts">${counts || `<span class="sev sev-info">${esc(t("reviewClean"))}</span>`}</span><span class="spacer"></span><button data-a="copy" title="${esc(t("copy"))}">${icon("copy")}</button></div>${d.summary ? `<div class="review-sum"></div>` : ""}<div class="review-list"></div></div>`);
  if (d.summary) n.querySelector(".review-sum").textContent = d.summary;
  if (d.audit) {
    const a = d.audit;
    const info = [t(a.stopped ? "auditStopped" : "auditInfo", { f: a.files, b: a.batches }), a.failed ? t("auditFailed", { n: a.failed }) : "", a.skipped ? t("auditSkipped", { n: a.skipped }) : ""].filter(Boolean).join(" · ");
    n.querySelector(".review-h").insertAdjacentElement("afterend", el(`<div class="review-sum">${esc(info)}</div>`));
  }
  const box = n.querySelector(".review-list");
  for (const f of list) {
    const loc = f.path ? `${f.path}${f.line ? ":" + f.line : ""}` : "";
    const row = el(`<div class="finding"><div class="finding-h"><span class="sev sev-${esc(f.severity)}">${esc(t("sev_" + f.severity))}</span><span class="ft"></span>${loc ? `<span class="chip">${icon("file")}${esc(loc)}</span>` : ""}</div>${f.code ? `<pre class="fcode"></pre>` : ""}${f.detail ? `<div class="fd"></div>` : ""}${f.suggestion ? `<div class="fs"><b>${esc(t("reviewFix"))}</b> <span></span></div>` : ""}</div>`);
    row.querySelector(".ft").textContent = f.title;
    if (f.code) row.querySelector(".fcode").textContent = f.code;
    if (f.detail) row.querySelector(".fd").textContent = f.detail;
    if (f.suggestion) row.querySelector(".fs span").textContent = f.suggestion;
    box.appendChild(row);
  }
  n.querySelector('[data-a="copy"]').onclick = () => {
    const md = list.map((f) => `- **[${f.severity}]** ${f.title}${f.path ? ` (\`${f.path}${f.line ? ":" + f.line : ""}\`)` : ""}${f.detail ? `\n  ${f.detail}` : ""}${f.suggestion ? `\n  ${t("reviewFix")} ${f.suggestion}` : ""}`).join("\n");
    navigator.clipboard.writeText((d.summary ? d.summary + "\n\n" : "") + md); toast(t("copied"));
  };
  return n;
}
/** git_diff result: one read-only diff card per file (no Undo: git owns those changes). */
function gitDiffEl(d, m) {
  const files = d.files || [];
  const n = el(`<div class="tool gitdiff open" data-call="${esc(m.meta?.tool_call_id || "")}"><div class="tool-h">${icon("git")}<span class="name">${esc(t("toolGitDiff"))}</span><span class="arg">${esc(t("gitFiles", { n: files.length }))} · <span class="n-add">+${d.added || 0}</span> <span class="n-del">−${d.removed || 0}</span></span><span class="st">${icon("check", "ok-ic")}${icon("chevron")}</span></div><div class="gd-body"></div></div>`);
  const body = n.querySelector(".gd-body");
  if (!files.length) body.appendChild(el(`<div class="muted-note">${esc(t("gitNoChanges"))}</div>`));
  for (const f of files) {
    const card = diffEl({ path: f.path, created: f.created, added: f.added, removed: f.removed, diff: f.diff || "" }, m);
    if (f.clipped || !f.diff) card.querySelector(".diff-body").appendChild(el(`<div class="muted-note">${esc(t("gitDiffClipped"))}</div>`));
    if (files.length > 3) { card.querySelector(".diff-body").classList.add("collapsed"); card.querySelector('[data-a="toggle"] span').textContent = t("expand"); }
    body.appendChild(card);
  }
  if (d.untracked?.length) {
    const u = el(`<div class="hits"></div>`);
    u.innerHTML = `<span class="muted-note">${esc(t("gitUntracked"))}</span>` + d.untracked.slice(0, 30).map((p) => `<span class="chip">${icon("file")}${esc(p)}</span>`).join("");
    body.appendChild(u);
  }
  n.querySelector(".tool-h").onclick = () => n.classList.toggle("open");
  return n;
}
/** The "allow this process?" prompt shown on a running git / run_command tool. */
function confirmEl(p) {
  const n = el(`<div class="confirm" data-confirm="${esc(p.id)}"><div class="confirm-q">${icon(p.scope === "git" ? "git" : "terminal")}<span>${esc(t(p.scope === "git" ? "confirmGit" : "confirmRun"))}</span></div><pre class="confirm-cmd"></pre><div class="confirm-cwd"></div><div class="confirm-b"><button class="btn primary" data-d="allow">${icon("play")}${esc(t("confirmAllow"))}</button><button class="btn" data-d="always">${esc(t("confirmAlways"))}</button><button class="btn" data-d="deny">${icon("x")}${esc(t("confirmDeny"))}</button></div></div>`);
  n.querySelector(".confirm-cmd").textContent = p.command;
  n.querySelector(".confirm-cwd").textContent = t("confirmIn", { dir: p.cwd });
  n.querySelector(".confirm-b").onclick = async (e) => {
    const b = e.target.closest("button"); if (!b) return;
    n.querySelectorAll("button").forEach((x) => (x.disabled = true));
    await call("confirm_tool", { id: p.id, decision: b.dataset.d });
  };
  return n;
}
function showConfirm(p) {
  $(`#thread [data-confirm="${p.id}"]`)?.remove();
  const host = $(`#thread .tool[data-call="${p.call_id}"]`);
  if (host) host.appendChild(confirmEl(p));
  else currentTurn(true).querySelector(".turn-body").appendChild(confirmEl(p));
  scrollBottom(true);
}
function runningToolEl(id, name, args) {
  return el(`<div class="tool" data-call="${esc(id)}"><div class="tool-h">${icon(TOOL_ICON[name] || "bolt")}<span class="name">${esc(t(TOOL_LABEL[name] || name))}</span><span class="arg">${esc(toolArgSummary(name, args))}</span><span class="st"><span class="spinner"></span></span></div></div>`);
}
function toolEl(m) {
  const d = m.meta?.display || {};
  const name = m.meta?.name || "tool";
  if (d.kind === "diff") return diffEl(d, m);
  if (d.kind === "findings") return findingsEl(d, m);
  if (d.kind === "gitdiff") return gitDiffEl(d, m);
  const n = el(`<div class="tool" data-call="${esc(m.meta?.tool_call_id || "")}"><div class="tool-h">${icon(TOOL_ICON[name] || "bolt")}<span class="name">${esc(t(TOOL_LABEL[name] || name))}</span><span class="arg">${esc(toolArgSummary(name, m.meta?.arguments))}${d.lines ? " · " + esc(d.lines) : ""}</span><span class="st">${m.meta?.ok ? icon("check", "ok-ic") : icon("x", "bad-ic")}${icon("chevron")}</span></div><div class="tool-b"></div></div>`);
  const full = m.meta?.full || m.content; // pruned / clipped results keep the original for the UI
  n.querySelector(".tool-b").textContent = full.length > 20000 ? full.slice(0, 20000) + "\n…" : full;
  if (m.meta?.pruned || m.meta?.clipped) n.querySelector(".tool-h .arg").insertAdjacentHTML("afterend", `<span class="res" title="${esc(t(m.meta.pruned ? "prunedHint" : "clippedHint"))}">${icon("compress")}</span>`);
  if (d.kind === "search" && d.hits?.length) {
    const h = el(`<div class="hits"></div>`);
    h.innerHTML = d.hits.map((x) => `<span class="chip">${icon("file")}${esc(x.path)}:${esc(x.lines)}</span>`).join("");
    n.insertBefore(h, n.querySelector(".tool-b"));
  }
  if (d.kind === "web" && d.hits?.length) {
    const h = el(`<div class="hits"></div>`);
    h.innerHTML = d.hits.slice(0, 8).map((x) => `<a class="chip" href="${esc(x.url)}">${icon("globe")}${esc(x.title)}</a>`).join("");
    n.insertBefore(h, n.querySelector(".tool-b"));
  }
  // short inline result next to the tool name
  const res = { calc: () => `= ${d.result}`, datetime: () => d.text, grep: () => t("grepCount", { n: d.matches, f: d.files }),
    symbol: () => t("symbolCount", { d: d.defs, r: d.refs }), outline: () => t("outlineCount", { n: d.count }),
    git_status: () => `${d.branch ? d.branch.split("...")[0] + " · " : ""}${t("gitFiles", { n: d.count })}`,
    command: () => d.timed_out ? t("cmdTimeout") : `${t("cmdExit", { c: d.code ?? "?" })} · ${(d.secs || 0).toFixed(1)} s` }[d.kind];
  if (res) n.querySelector(".tool-h .arg").insertAdjacentHTML("afterend", `<span class="res ${d.kind === "command" && !d.ok ? "bad" : ""}">${esc(res())}</span>`);
  if ((d.kind === "grep" || d.kind === "symbol") && d.hits?.length) {
    const h = el(`<div class="hits"></div>`);
    h.innerHTML = d.hits.slice(0, 12).map((x) => `<span class="chip">${icon(x.def ? "code" : "file")}${esc(x.path)}:${esc(x.lines)}</span>`).join("");
    n.insertBefore(h, n.querySelector(".tool-b"));
  }
  if (d.kind === "git_status" && d.files?.length) {
    const h = el(`<div class="hits"></div>`);
    h.innerHTML = d.files.slice(0, 16).map((x) => `<span class="chip"><b class="gst">${esc(x.status)}</b>${esc(x.path)}</span>`).join("");
    n.insertBefore(h, n.querySelector(".tool-b"));
  }
  if (d.kind === "command") n.classList.add("cmd");
  n.querySelector(".tool-h").onclick = () => n.classList.toggle("open");
  return n;
}
/** Renders a unified diff (from tools.rs) as a two-gutter, colored card with Hide / Undo. */
function diffEl(d, m) {
  const n = el(`<div class="diff" data-call="${esc(m.meta?.tool_call_id || "")}"><div class="diff-h">${icon("file")}<span class="path">${esc(d.path)}</span>${d.created ? `<span class="badge new">${t("created")}</span>` : ""}<span class="n-add">+${d.added}</span><span class="n-del">−${d.removed}</span><span class="spacer"></span><button data-a="toggle">${icon("chevron")}<span>${t("collapse")}</span></button>${d.backup ? `<button data-a="undo">${icon("undo")}${t("undo")}</button>` : ""}</div><div class="diff-body"></div></div>`);
  const body = n.querySelector(".diff-body");
  let oldN = 0, newN = 0, html = "";
  for (const line of (d.diff || "").replace(/\n$/, "").split("\n")) {
    if (line.startsWith("---") || line.startsWith("+++")) continue;
    if (line.startsWith("@@")) {
      const mm = /@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(line);
      if (mm) { oldN = +mm[1]; newN = +mm[2]; }
      html += `<div class="dl hunk"><span class="tx">${esc(line)}</span></div>`;
    } else if (line.startsWith("+")) {
      html += `<div class="dl add"><span class="ln"></span><span class="ln">${newN++}</span><span class="tx">${esc(line.slice(1))}</span></div>`;
    } else if (line.startsWith("-")) {
      html += `<div class="dl del"><span class="ln">${oldN++}</span><span class="ln"></span><span class="tx">${esc(line.slice(1))}</span></div>`;
    } else if (line.length || html) {
      html += `<div class="dl"><span class="ln">${oldN++}</span><span class="ln">${newN++}</span><span class="tx">${esc(line.slice(1))}</span></div>`;
    }
  }
  body.innerHTML = html;
  n.querySelector(".diff-h").onclick = async (e) => {
    const a = e.target.closest("button")?.dataset.a;
    if (a === "toggle") { body.classList.toggle("collapsed"); e.target.closest("button").querySelector("span").textContent = body.classList.contains("collapsed") ? t("expand") : t("collapse"); }
    if (a === "undo") {
      if (!d.backup) return;
      await call("undo_change", { backupId: d.backup });
      toast(t("undone") + ": " + d.path);
      const b = e.target.closest("button"); b.disabled = true; b.innerHTML = icon("check") + t("undone");
    }
  };
  return n;
}

/** Progress cards stored as `event` messages: conversation compression and deep research. */
function eventEl(m) {
  const meta = m.meta || {};
  if (meta.kind === "compress") {
    const done = meta.state === "done";
    const failed = meta.state === "error" || meta.state === "cancelled";
    const pct = done ? 100 : Math.round((meta.progress || 0.03) * 100);
    return el(`<div class="event" data-id="${m.id}"><div class="event-h"><span class="ic">${icon("compress")}</span>${done ? t("compressed") : failed ? t("compressed") + " ✕" : t("compressing") + "…"}<small>${done ? Math.round((1 - meta.after / Math.max(1, meta.before)) * 100) + "% ↓" : pct + "%"}</small></div>
      <div class="prog ${done || failed ? "done" : ""}"><div style="width:${failed ? 0 : pct}%"></div></div>
      ${done ? `<div class="meta">${t("compressedInfo", { n: `<b>${meta.count}</b>`, a: `<b>${fmtK(meta.before)}</b>`, b: `<b>${fmtK(meta.after)}</b>` })}</div>` : ""}
      ${meta.error ? `<div class="err-box">${esc(meta.error)}</div>` : ""}</div>`);
  }
  if (meta.kind === "research") {
    const stopped = meta.state === "cancelled" || meta.state === "error";
    const done = meta.state === "done" || stopped;
    const pct = Math.round((meta.progress || 0) * 100);
    const steps = (meta.steps || []).slice(-8);
    const stepIc = { plan: "list", search: "search", read: "book", think: "brain", write: "edit", done: "check", warn: "warn" };
    const n = el(`<div class="event" data-id="${m.id}"><div class="event-h"><span class="ic" style="background:rgba(34,184,240,.14);color:#aee6ff">${icon("telescope")}</span>${stopped ? t("researching") + " ✕" : done ? t("researchDone") : t("researching") + "…"}<small>${pct}% · ${(meta.sources || []).length} ${t("sources").toLowerCase()}</small></div>
      <div class="prog ${done ? "done" : ""}"><div style="width:${pct}%"></div></div>
      ${meta.objective ? `<div style="font-size:13px;color:var(--text-2)">${esc(meta.objective)}</div>` : ""}
      <div class="steps">${steps.map((s) => `<div>${icon(stepIc[s.icon] || "sparkle")}<span>${esc(s.text)}</span></div>`).join("")}</div>
      ${(meta.sources || []).length ? `<div class="sources">${meta.sources.map((s) => `<a class="chip" href="${esc(s.url)}" title="${esc(s.url)}">${s.n}. ${esc(s.title || s.url)}</a>`).join("")}</div>` : ""}</div>`);
    if (done) n.querySelector(".steps").style.display = "none";
    return n;
  }
  return el(`<div></div>`);
}

// ---------------------------------------------------------------- live streaming
// assistant_start creates a temporary segment; deltas are buffered and re-rendered at most every
// 70 ms (markdown re-parse); assistant_done swaps it for the persisted message element.
function startLive(tempId) {
  const turn = currentTurn(true);
  const seg = el(`<div class="seg live" style="display:flex;flex-direction:column;gap:10px"><div class="md cursor"></div><div class="stats live-stats"></div></div>`);
  turn.querySelector(".turn-body").appendChild(seg);
  S.live = { tempId, seg, content: "", reasoning: "", think: null, thinkStart: null, thinkEnd: null, tokens: 0, tps: 0, dirty: false, start: performance.now() };
  if (!S.liveCtxBase) S.liveCtxBase = S.ctxUsed;
  scrollBottom();
}
function liveDelta(p) {
  const L = S.live;
  if (!L || L.tempId !== p.temp_id) return;
  if (p.reasoning) {
    L.reasoning += p.reasoning;
    if (!L.think) { L.think = thinkEl("", true); L.seg.insertBefore(L.think, L.seg.firstChild); L.thinkStart = performance.now(); }
  }
  if (p.content) {
    L.content += p.content;
    if (L.think && !L.thinkEnd) { L.thinkEnd = performance.now(); }
  }
  L.dirty = true;
  scheduleLive();
}
let liveTimer = null;
function scheduleLive() {
  if (liveTimer) return;
  liveTimer = setTimeout(() => { liveTimer = null; flushLive(); }, 70);
}
function flushLive() {
  const L = S.live;
  if (!L || !L.dirty) return;
  L.dirty = false;
  const atBottom = isAtBottom();
  if (L.think) {
    const b = L.think.querySelector(".think-b");
    b.textContent = L.reasoning;
    if (L.think.classList.contains("open")) b.scrollTop = b.scrollHeight;
    if (L.thinkEnd) {
      L.think.classList.remove("live");
      L.think.querySelector(".lbl").textContent = t("thoughtFor", { s: Math.max(1, Math.round((L.thinkEnd - L.thinkStart) / 1000)) });
    }
  }
  const md = L.seg.querySelector(".md");
  md.innerHTML = renderMd(L.content);
  md.classList.toggle("cursor", true);
  if (atBottom) scrollBottom();
}
function liveProgress(p) {
  const L = S.live;
  if (!L || L.tempId !== p.temp_id) return;
  L.tokens = p.tokens; L.tps = p.tps;
  L.seg.querySelector(".live-stats").innerHTML = `<span>${icon("bolt")}${p.tps.toFixed(1)} ${t("tps")}</span><span>${icon("layers")}${p.tokens} tok</span>`;
  updateCtxMeter();
  renderStatus();
}
function finishLive(p) {
  const L = S.live;
  if (L && L.thinkStart) p.message.meta.think_s = Math.max(1, Math.round(((L.thinkEnd || performance.now()) - L.thinkStart) / 1000));
  if (L) L.seg.remove();
  S.live = null;
  addMessage(p.message);
  S.ctxUsed = p.ctx_used || S.ctxUsed; if (p.ctx_budget) S.budget = p.ctx_budget;
  S.liveCtxBase = S.ctxUsed;
  if (p.message.meta?.tps) S.lastTps = p.message.meta.tps;
  if (p.message.meta?.prompt_tps && p.message.meta.prompt_evaluated > 1) S.lastPromptTps = p.message.meta.prompt_tps;
  updateCtxMeter(); renderStatus();
}
/** Appends a persisted message (deduplicated by id); a tool result replaces its "running" card. */
function addMessage(m) {
  if (S.messages.some((x) => x.id === m.id)) return;
  const wasEmpty = !S.messages.length;
  S.messages.push(m);
  if (wasEmpty) return renderChat();
  const th = $("#thread");
  const atBottom = isAtBottom();
  if (m.role === "tool") {
    const running = $(`#thread .tool[data-call="${CSS.escape(m.meta?.tool_call_id || "")}"]`);
    const n = toolEl(m);
    if (running) running.replaceWith(n); else currentTurn(true).querySelector(".turn-body").appendChild(n);
  } else if (m.role === "assistant") {
    const n = assistantEl(m);
    if (n) currentTurn(true).querySelector(".turn-body").appendChild(n);
  } else {
    placeMessage(th, m, null);
  }
  markLastTurn();
  if (atBottom || m.role === "user") scrollBottom(m.role === "user");
}
function isAtBottom() { const s = $("#scroller"); return s.scrollHeight - s.scrollTop - s.clientHeight < 120; }
function scrollBottom(force) { const s = $("#scroller"); if (force || isAtBottom()) requestAnimationFrame(() => { s.scrollTop = s.scrollHeight; updateJumpBtn(); }); }
/** "Jump to latest" appears once the user scrolls up away from the end of the thread. */
function updateJumpBtn() { $("#jumpBtn").classList.toggle("on", !isAtBottom()); }
$("#scroller").addEventListener("scroll", updateJumpBtn, { passive: true });
$("#jumpBtn").addEventListener("click", () => { const s = $("#scroller"); s.scrollTo({ top: s.scrollHeight, behavior: "smooth" }); });

// ---------------------------------------------------------------- composer
function setToggle(k, v) {
  S.toggles[k] = v;
  if (k === "research" && v) S.toggles.web = false;
  if (k === "web" && v) S.toggles.research = false;
  renderComposer();
  updatePlaceholder();
}
function updatePlaceholder() {
  $("#input").placeholder = S.toggles.research ? t("placeholderResearch") : S.chat?.project_id ? t("placeholderProject") : t("placeholder");
}
function renderComposer() {
  $("#tThink").innerHTML = icon("brain") + t("think");
  $("#tWeb").innerHTML = icon("globe") + t("web");
  $("#tResearch").innerHTML = icon("telescope") + t("research");
  $("#tThink").classList.toggle("on", S.toggles.think);
  $("#tWeb").classList.toggle("on", S.toggles.web);
  $("#tResearch").classList.toggle("on", S.toggles.research);
  const busy = S.busy[S.chatId];
  const b = $("#sendBtn");
  b.classList.toggle("stop", !!busy);
  b.innerHTML = busy ? icon("stop") : icon("send");
  b.title = busy ? t("stop") : t("send");
}
$("#tThink").onclick = () => setToggle("think", !S.toggles.think);
$("#tWeb").onclick = () => setToggle("web", !S.toggles.web);
$("#tResearch").onclick = () => setToggle("research", !S.toggles.research);
const input = $("#input");
function autosize() { input.style.height = "auto"; input.style.height = Math.min(260, input.scrollHeight) + "px"; }
input.addEventListener("input", autosize);
input.addEventListener("keydown", (e) => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); } });
$("#sendBtn").onclick = () => (S.busy[S.chatId] ? call("stop_generation", { chatId: S.chatId }) : send());

async function send() {
  const text = input.value.trim();
  if (!text || S.busy[S.chatId]) return;
  if (S.info.state !== "ready") {
    // unloaded (idle or by hand): bring the last model back, then send
    const last = S.settings.last_model;
    if (S.info.state === "stopped" && last && S.models.some((m) => m.file === last)) {
      toast(t("reloading", { x: shortModel(last) }));
      try { await call("load_model", { file: last }); } catch { return; }
    } else {
      toast(S.info.state === "starting" ? t("loading") : t("noModel"), true);
      if (!S.models.length) showView("models");
      return;
    }
  }
  if (!S.chatId) {
    const c = await call("create_chat", { projectId: null });
    await refreshChats();
    await openChat(c.id);
  }
  input.value = ""; autosize();
  delete S.drafts[S.chatId]; delete S.drafts[""];
  S.busy[S.chatId] = true; renderComposer(); renderSidebar();
  S.liveCtxBase = S.ctxUsed + estTokens(text);
  const research = S.toggles.research;
  await call("send_message", { chatId: S.chatId, text, think: S.toggles.think, web: S.toggles.web, research });
  if (research) setToggle("research", false);
}
/** Drops the last user turn locally (the backend truncates and re-sends the same text). */
function regenerate() {
  if (S.busy[S.chatId]) return;
  const idx = S.messages.map((m) => m.role).lastIndexOf("user");
  if (idx < 0) return;
  S.liveCtxBase = S.ctxUsed;
  S.messages = S.messages.slice(0, idx);
  S.busy[S.chatId] = true;
  renderChat();
  call("regenerate", { chatId: S.chatId, think: S.toggles.think, web: S.toggles.web });
}

// ---------------------------------------------------------------- models page
function progressHtml(p) {
  if (!p) return "";
  if (p.error) return `<div class="err-box">${esc(p.error)}</div>`;
  const pct = p.total ? (p.done / p.total) * 100 : 0;
  const eta = p.speed > 0 && p.total ? Math.max(0, (p.total - p.done) / p.speed) : 0;
  const etaS = eta ? (eta > 3600 ? (eta / 3600).toFixed(1) + " h" : eta > 60 ? Math.round(eta / 60) + " min" : Math.round(eta) + " s") : "";
  return `<div class="mini-prog"><div class="prog"><div style="width:${pct.toFixed(1)}%"></div></div><small><span>${fmtBytes(p.done)} / ${fmtBytes(p.total)} · ${pct.toFixed(0)}%</span><span>${p.speed ? fmtBytes(p.speed) + "/s" : ""} ${etaS ? "· " + etaS : ""}</span></small></div>`;
}
function fitTag(sizeGb) {
  const vram = (S.info.vram_total || 0) / 1073741824;
  if (!vram) return "";
  return sizeGb + 1.6 < vram ? `<span class="tag ok">${t("fits")}</span>` : `<span class="tag warn">${t("partial")}</span>`;
}
async function refreshModels() { S.models = await call("list_models"); if (S.view === "models") renderModels(); if (S.view === "chat" && !S.messages.length) renderChat(); }
/** Downloads a GGUF; when it finishes and nothing is loaded yet, loads it automatically. */
function startDownload(repo, path) {
  const name = path.split("/").pop();
  S.downloads["model:" + name] = { done: 0, total: 0, speed: 0 };
  updateDownloadUI("model:" + name);
  if (S.view === "models") renderModels();
  invoke("download_model", { repo, path }).then(async (file) => {
    delete S.downloads["model:" + name];
    toast(`${name} ✓`);
    await refreshModels();
    if (S.info.state !== "ready" && S.info.state !== "starting") loadModel(file);
  }).catch((e) => {
    const msg = String(e);
    if (msg.includes("cancelled")) delete S.downloads["model:" + name]; else S.downloads["model:" + name] = { error: msg };
    if (S.view === "models") renderModels(); else updateDownloadUI("model:" + name);
  });
}
function updateDownloadUI(id) {
  $$(`[data-dl="${CSS.escape(id)}"]`).forEach((n) => (n.innerHTML = progressHtml(S.downloads[id])));
}

let hfState = { query: "", results: null, repo: null, files: null, loading: false };
function renderModels() {
  const pg = $("#modelsPage");
  const inst = new Set(S.models.map((m) => m.file));
  const gpu = S.info.gpu_name ? `${esc(S.info.gpu_name)} · ${fmtBytes(S.info.vram_total)} VRAM` : "";
  let html = `<h2>${t("models")}</h2><p class="lead">${gpu}</p>`;
  html += `<h3>${icon("cube")}${t("installed")}</h3>`;
  if (!S.models.length) html += `<div class="empty" style="text-align:left;padding-left:0">${t("noInstalled")}</div>`;
  else {
    html += `<div class="grid">`;
    for (const m of S.models) {
      const active = m.active && S.info.state === "ready";
      const loading = m.active && S.info.state === "starting";
      html += `<div class="card ${active ? "active" : ""}"><div class="nm">${icon("cube")}${esc(shortModel(m.file))}</div>
        <div class="row"><span class="tag">${fmtBytes(m.size)}</span>${fitTag(m.size / 1073741824)}${active ? `<span class="tag ok">● ${t("active")} · ${fmtK(S.info.n_ctx)} ctx</span>` : ""}${loading ? `<span class="tag warn">${t("loading")}</span>` : ""}</div>
        <div class="row">${active ? `<button class="btn" data-unload>${icon("power")}${t("unload")}</button>` : `<button class="btn primary" data-load="${esc(m.file)}" ${loading ? "disabled" : ""}>${icon("play")}${t("load")}</button>`}
        <button class="btn danger ghost" data-del="${esc(m.file)}">${icon("trash")}${t("del")}</button></div></div>`;
    }
    html += `</div>`;
  }
  html += `<h3>${icon("sparkle")}${t("recommended")}</h3><div class="grid">`;
  const lang = S.settings.ui_lang === "ro" ? "ro" : "en";
  for (const c of S.catalog) {
    const id = "model:" + c.file;
    const dl = S.downloads[id];
    const have = inst.has(c.file);
    html += `<div class="card ${c.recommended ? "rec" : ""}"><div class="nm">${esc(c.name)}</div>
      <div class="row"><span class="tag acc">${esc(c.tag)}</span><span class="tag">${c.size_gb.toFixed(1)} GB</span>${fitTag(c.size_gb)}</div>
      <div class="ds">${esc(lang === "ro" ? c.desc_ro : c.desc_en)}</div>
      <div data-dl="${esc(id)}">${dl ? progressHtml(dl) : ""}</div>
      <div class="row">${have ? `<span class="tag ok">${icon("check")} ${t("installed")}</span>` : dl && !dl.error ? `<button class="btn" data-cancel="${esc(id)}">${icon("x")}${t("cancel")}</button>` : `<button class="btn primary" data-dlrepo="${esc(c.repo)}" data-dlpath="${esc(c.file)}">${icon("download")}${t("download")}</button>`}
      <button class="btn ghost" data-url="https://huggingface.co/${esc(c.repo)}">${icon("link")}Hugging Face</button></div></div>`;
  }
  html += `</div>`;
  html += `<h3>${icon("globe")}${t("browseHF")}</h3><div class="hf-row"><input class="inp" id="hfInp" placeholder="${esc(t("hfPlaceholder"))}" value="${esc(hfState.query)}"/><button class="btn primary" id="hfGo">${hfState.loading ? '<span class="spinner"></span>' : icon("search")}</button></div>`;
  if (hfState.files) {
    html += `<div class="list-box"><div class="li"><b class="grow">${esc(hfState.repo)}</b><button class="btn ghost" data-url="https://huggingface.co/${esc(hfState.repo)}">${icon("link")}</button><button class="btn ghost" id="hfBack">${icon("x")}</button></div>`;
    for (const f of hfState.files) {
      const name = f.path.split("/").pop();
      const id = "model:" + name;
      const dl = S.downloads[id];
      html += `<div class="li"><span class="grow">${esc(f.path)}</span><small>${fmtBytes(f.size)}</small>${fitTag((f.size || 0) / 1073741824)}
        ${inst.has(name) ? `<span class="tag ok">${t("installed")}</span>` : dl && !dl.error ? `<div style="width:220px" data-dl="${esc(id)}">${progressHtml(dl)}</div><button class="btn ghost" data-cancel="${esc(id)}">${icon("x")}</button>` : `<button class="btn" data-dlrepo="${esc(hfState.repo)}" data-dlpath="${esc(f.path)}">${icon("download")}</button>`}</div>`;
    }
    if (!hfState.files.length) html += `<div class="empty">${t("noGguf")}</div>`;
    html += `</div>`;
  } else if (hfState.results) {
    html += `<div class="list-box">`;
    for (const r of hfState.results) html += `<div class="li" data-repo="${esc(r.id)}" style="cursor:pointer">${icon("cube")}<span class="grow">${esc(r.id)}</span><small>↓ ${fmtK(r.downloads || 0)} · ♥ ${fmtK(r.likes || 0)}</small></div>`;
    if (!hfState.results.length) html += `<div class="empty">—</div>`;
    html += `</div>`;
  }
  pg.innerHTML = html;
  const inp = $("#hfInp");
  inp.onkeydown = (e) => { if (e.key === "Enter") hfSearch(); };
  inp.oninput = () => (hfState.query = inp.value);
  $("#hfGo").onclick = hfSearch;
  $("#hfBack") && ($("#hfBack").onclick = () => { hfState.files = null; renderModels(); });
}
async function hfSearch() {
  const q = hfState.query.trim();
  if (!q) return;
  hfState.loading = true; renderModels();
  try {
    if (q.includes("/")) { const r = await call("list_repo", { input: q }); hfState.repo = r.repo; hfState.files = r.files; }
    else { hfState.results = await call("search_hf", { query: q }); hfState.files = null; }
  } finally { hfState.loading = false; renderModels(); }
}
$("#modelsPage").addEventListener("click", async (e) => {
  const b = e.target.closest("button, .li[data-repo]");
  if (!b) return;
  if (b.dataset.load) return loadModel(b.dataset.load);
  if (b.hasAttribute("data-unload")) return call("unload_model");
  if (b.dataset.del) {
    if (!(await confirmBox(t("confirmDelete", { x: b.dataset.del }), t("del")))) return;
    await call("delete_model", { file: b.dataset.del }); return refreshModels();
  }
  if (b.dataset.dlrepo) return startDownload(b.dataset.dlrepo, b.dataset.dlpath);
  if (b.dataset.cancel) return invoke("cancel_download", { id: b.dataset.cancel });
  if (b.dataset.url) return invoke("open_url", { url: b.dataset.url });
  if (b.dataset.repo) {
    hfState.loading = true;
    try { const r = await call("list_repo", { input: b.dataset.repo }); hfState.repo = r.repo; hfState.files = r.files; } finally { hfState.loading = false; renderModels(); }
  }
});

// ---------------------------------------------------------------- settings page
function renderSettings() {
  const s = S.settings;
  const sel = (key, opts) => `<select class="inp" data-k="${key}">${opts.map(([v, l]) => `<option value="${v}" ${String(s[key]) === String(v) ? "selected" : ""}>${esc(l)}</option>`).join("")}</select>`;
  const sw = (key) => `<label class="switch"><input type="checkbox" data-k="${key}" ${s[key] ? "checked" : ""}/><span></span></label>`;
  const f = (label, hint, ctrl) => `<div class="field"><div><label>${label}</label>${hint ? `<small>${hint}</small>` : ""}</div>${ctrl}</div>`;
  let html = `<h2>${t("settings")}</h2>`;
  if (S.needsReload && S.info.model) html += `<div class="setup-card" style="margin-top:12px;flex-direction:row;align-items:center"><span style="flex:1">${icon("refresh")} ${t("reloadHint")}</span><button class="btn primary" id="reloadBtn">${t("reload")}</button></div>`;
  html += `<h3>${icon("globe")}Language / Limbă</h3>`;
  html += f(t("uiLang"), "", sel("ui_lang", [["en", "English"], ["ro", "Română"]]));
  html += f(t("replyLang"), t("replyLangHint"), sel("reply_lang", [["auto", t("auto")], ["en", "English"], ["ro", "Română"]]));
  html += `<h3>${icon("chip")}${t("engine")}</h3>`;
  // (macOS has one llama.cpp build, Metal: runtime.rs `effective_backend`)
  const backends = S.os === "macos" ? [["metal", t("metalRec")], ["cpu", t("cpuOnly")]] : [["vulkan", t("vulkanRec")], ["rocm", "ROCm / HIP"], ["cpu", t("cpuOnly")]];
  html += f(t("backend"), S.os === "macos" ? t("backendHintMac") : t("backendHint"), sel("backend", backends));
  html += f(t("runtimeVer"), esc(S.runtime || "—"), `<button class="btn" id="updRt">${icon("download")}${t("update")}</button>`);
  html += f(t("ctxSize"), t("ctxSizeHint"), sel("ctx_size", [[0, t("auto")], [8192, "8k"], [16384, "16k"], [32768, "32k"], [65536, "64k"], [131072, "128k"], [262144, "256k"]]));
  html += f(t("kv"), t("kvHint"), sel("kv_cache", [["q8_0", "q8_0"], ["f16", "f16"], ["q4_0", "q4_0"]]));
  html += f(t("idleUnload"), t("idleUnloadHint"), sel("idle_unload_min", [[0, t("never")], [5, "5 min"], [15, "15 min"], [30, "30 min"], [60, "1 h"], [120, "2 h"]]));
  html += f(t("moe"), t("moeHint"), `<input class="inp" style="min-width:90px;width:90px" type="number" min="0" max="99" data-k="n_cpu_moe" value="${s.n_cpu_moe}"/>`);
  html += `<h3>${icon("sparkle")}${t("chatSection")}</h3>`;
  html += `<div class="field col"><div><label>${t("customInstr")}</label><small>${t("customInstrHint")}</small></div><textarea class="inp" data-k="custom_instructions" placeholder="${esc(t("customInstrPh"))}">${esc(s.custom_instructions || "")}</textarea></div>`;
  html += f(t("temp"), `<span id="tempV">${(+s.temperature).toFixed(2)}</span>`, `<input type="range" min="0" max="1.5" step="0.05" data-k="temperature" value="${s.temperature}"/>`);
  html += f(t("compressAt"), `<span id="compV">${Math.round(s.compress_at * 100)}%</span> · ${t("compressHint")}`, `<input type="range" min="0.4" max="0.9" step="0.02" data-k="compress_at" value="${s.compress_at}"/>`);
  html += f(t("thinkDefault"), "", sw("thinking"));
  html += f(t("semantic"), t("semanticHint"), sw("semantic_search"));
  html += `<h3>${icon("globe")}${t("webSearch")}</h3>`;
  html += f(t("searchProvider"), t("searchProviderHint"), sel("search_provider", [["duckduckgo", t("ddgNoKey")], ["wikipedia", t("wikiNoKey")], ["brave", "Brave Search API"], ["tavily", "Tavily"], ["searxng", t("searxSelf")]]));
  if (s.search_provider === "brave" || s.search_provider === "tavily") {
    const link = s.search_provider === "brave" ? "https://brave.com/search/api/" : "https://tavily.com/";
    html += f(t("apiKey"), `<a href="${link}">${link}</a>`, `<input class="inp" type="password" data-k="search_key" value="${esc(s.search_key)}" placeholder="key"/>`);
  }
  if (s.search_provider === "searxng") html += f("SearXNG URL", t("searxHint"), `<input class="inp" data-k="searxng_url" value="${esc(s.searxng_url)}" placeholder="http://localhost:8080"/>`);
  html += `<h3>${icon("gear")}${t("appSection")}</h3>`;
  html += f(t("tray"), t("trayHint"), sw("close_to_tray"));
  html += f(t("autoload"), "", sw("autoload_model"));
  html += f(t("dataFolder"), esc(S.root), `<button class="btn" id="openData">${icon("folder")}${t("open")}</button>`);
  html += f(t("quit"), "", `<button class="btn danger" id="quitBtn">${icon("power")}${t("quit")}</button>`);
  html += `<h3>${icon("bolt")}${t("shortcuts")}</h3><div class="shortcuts">${SHORTCUTS.map(([k, d]) => `<div><kbd>${k}</kbd><span>${t(d)}</span></div>`).join("")}</div>`;
  html += `<h3>${icon("sparkle")}${t("about")}</h3><div class="about"><img src="logo.png" alt=""/><div><div class="ttl">Lumen v${esc(S.version)}</div><small>${t("madeBy")} Akulav · Tauri + Rust + llama.cpp ${esc(S.runtime || "")}</small></div></div>`;
  const u = S.updates || {};
  // both buttons: check any time, update when GitHub has a newer release (never for a copy built from source)
  const upd = `<div class="btn-row">${u.app && !u.self_built ? `<button class="btn primary" id="installUpd" ${u.installing ? "disabled" : ""}>${icon("download")}${t("updateApp", { v: esc(u.app.version) })}</button>` : ""}<button class="btn" id="checkUpd" ${u.checking ? "disabled" : ""}>${u.checking ? '<span class="spinner"></span>' : icon("refresh")}${t("checkUpdates")}</button></div>`;
  const status = (u.app ? t("updateAvailable", { v: esc(u.app.version) }) + (u.app.url ? ` · <a href="${esc(u.app.url)}">${t("releaseNotes")}</a>` : "")
    : u.checked_at ? t("lastChecked", { x: esc(fmtTime(u.checked_at)) }) : t("notChecked")) + (u.self_built ? `<br>${t("selfBuilt")}` : "");
  html += f(t("updates"), status + (u.error ? `<br><span style="color:var(--bad)">${esc(u.error)}</span>` : ""), upd);
  if (u.runtime) html += f(t("runtimeVer"), t("runtimeAvailable", { a: esc(u.runtime_current || "—"), b: esc(u.runtime) }), `<button class="btn primary" id="rtUpd">${icon("download")}${t("update")}</button>`);
  html += f(t("autoCheck"), t("autoCheckHint"), sw("check_updates"));
  html += `<h3>${icon("list")}${t("logs")} <button class="btn ghost" id="logRef" style="margin-left:auto">${icon("refresh")}${t("refresh")}</button></h3><div class="logs" id="logs"></div>`;
  $("#settingsPage").innerHTML = html;
  loadLogs();
}
async function loadLogs() { const l = await invoke("get_logs"); const n = $("#logs"); if (n) { n.textContent = l.slice(-250).join("\n") || "—"; n.scrollTop = n.scrollHeight; } }
$("#settingsPage").addEventListener("change", async (e) => {
  const k = e.target.dataset.k;
  if (!k) return;
  let v = e.target.type === "checkbox" ? e.target.checked : e.target.value;
  if (["ctx_size", "n_cpu_moe", "idle_unload_min"].includes(k)) v = parseInt(v || "0", 10);
  if (["temperature", "compress_at"].includes(k)) v = parseFloat(v);
  S.settings[k] = v;
  await call("set_settings", { settings: S.settings });
  if (k === "custom_instructions") return toast(t("saved")); // keep focus/scroll; nothing else depends on it
  if (["backend", "ctx_size", "kv_cache", "n_cpu_moe"].includes(k)) S.needsReload = true;
  if (k === "thinking") S.toggles.think = v;
  if (k === "ui_lang") { renderSidebar(); renderTopbar(); renderComposer(); }
  if (k === "backend") { const st = await invoke("get_state"); S.runtime = st.runtime; }
  renderSettings();
});
$("#settingsPage").addEventListener("input", (e) => {
  if (e.target.dataset.k === "temperature") $("#tempV").textContent = (+e.target.value).toFixed(2);
  if (e.target.dataset.k === "compress_at") $("#compV").textContent = Math.round(e.target.value * 100) + "%";
});
$("#settingsPage").addEventListener("click", async (e) => {
  const id = e.target.closest("button")?.id;
  if (id === "reloadBtn") loadModel(S.info.model);
  if (id === "updRt") { toast(t("downloading") + " llama.cpp…"); S.runtime = await call("update_runtime"); renderSettings(); toast("llama.cpp " + S.runtime); }
  if (id === "openData") invoke("open_path", { path: S.root });
  if (id === "quitBtn") invoke("quit_app");
  if (id === "logRef") loadLogs();
  if (id === "checkUpd") checkUpdatesNow();
  if (id === "installUpd") promptAppUpdate();
  if (id === "rtUpd") promptRuntimeUpdate();
});

// ---------------------------------------------------------------- projects
function newProjectModal() {
  const m = el(`<div class="modal-bg"><div class="modal"><h4>${icon("folder")} ${t("newProject")}</h4><div style="color:var(--text-2);font-size:13px">${t("projectHint")}</div>
    <input class="inp" id="npName" placeholder="${t("projName")}"/>
    <div class="hf-row"><input class="inp" id="npFolder" placeholder="${t("folder")}" readonly/><button class="btn" id="npPick">${icon("folder")}${t("pickFolder")}</button></div>
    <div class="acts"><button class="btn ghost" data-a="c">${t("cancel")}</button><button class="btn primary" data-a="ok">${t("create")}</button></div></div></div>`);
  document.body.appendChild(m);
  const pick = async () => {
    const f = await invoke("pick_folder");
    if (f) { $("#npFolder").value = f; if (!$("#npName").value) $("#npName").value = f.split(/[\\/]/).filter(Boolean).pop(); }
  };
  $("#npPick").onclick = pick;
  $("#npFolder").onclick = pick;
  m.addEventListener("click", async (e) => {
    const a = e.target.closest("[data-a]")?.dataset.a;
    if (a === "c" || e.target === m) m.remove();
    if (a === "ok") {
      const folder = $("#npFolder").value;
      if (!folder) return pick();
      const p = await call("create_project", { name: $("#npName").value, folder });
      m.remove();
      await refreshProjects();
      openProject(p.id);
    }
  });
}
let projState = { chats: [], info: null, progress: null };
async function openProject(id) {
  S.projectId = id; S.tab = "projects";
  projState = { chats: [], info: null, progress: projState.progress?.project_id === id ? projState.progress : null };
  showView("project");
  await refreshProjectChats();
}
async function refreshProjectChats() {
  if (!S.projectId) return;
  const [info, chats] = await Promise.all([call("get_project", { id: S.projectId }), call("list_chats", { projectId: S.projectId })]);
  projState.info = info; projState.chats = chats;
  if (S.view === "project") renderProject();
}
function projProgressHtml() {
  const p = projState.progress;
  if (!p || p.phase === "done" || p.phase === "error") return p?.phase === "error" ? `<div class="err-box">${esc(p.extra?.error)}</div>` : "";
  const pct = p.total ? (p.done / p.total) * 100 : 5;
  const label = p.phase === "embed" ? t("embedded") : p.phase === "scan" ? t("indexing") + "…" : `${t("indexing")} · ${esc(p.extra?.file || "")}`;
  return `<div class="mini-prog" style="margin-top:14px"><div class="prog"><div style="width:${pct.toFixed(1)}%"></div></div><small><span>${label}</span><span>${p.done} / ${p.total}</span></small></div>`;
}
function renderProject() {
  const info = projState.info;
  if (!info) return;
  const p = info.project, st = info.stats;
  let html = `<div class="proj-head"><div class="proj-ic">${icon("folder")}</div><div style="flex:1;min-width:0"><h2>${esc(p.name)}</h2><div class="lead" style="cursor:pointer" id="openFolder">${esc(p.folder)}</div></div>
    <button class="btn" id="reidx">${icon("refresh")}${t("reindex")}</button><button class="btn primary" id="projNew">${icon("plus")}${t("newChat")}</button></div>
    <div class="kpis"><div class="kpi"><small>${t("indexed")}</small><b>${st.files}</b></div><div class="kpi"><small>${t("chunks")}</small><b>${st.chunks}</b></div><div class="kpi"><small>${t("embedded")}</small><b>${st.embedded}</b></div></div>
    <div id="projProg">${projProgressHtml()}</div>
    <h3>${icon("chat")}${t("projChats")}</h3><div class="list-box">`;
  for (const c of projState.chats) {
    html += `<div class="li pchat" data-pchat="${c.id}" style="cursor:pointer">${icon("chat")}<span class="grow">${esc(c.title)}</span><small>${new Date(c.updated).toLocaleString()}</small>
      <span class="li-acts"><button data-pdel="${c.id}" title="${t("del")}">${icon("trash")}</button></span></div>`;
  }
  if (!projState.chats.length) html += `<div class="empty">${t("noChats")}</div>`;
  html += `</div><h3>${icon("book")}${t("instructions")}</h3><small style="color:var(--text-3)">${t("instructionsHint")}</small>
    <textarea class="inp" id="projInstr" style="margin-top:8px"></textarea><div style="margin-top:8px;display:flex;justify-content:flex-end"><button class="btn" id="saveInstr">${icon("check")}${t("saveShort")}</button></div>`;
  $("#projectPage").innerHTML = html;
  $("#projInstr").value = p.instructions || "";
  renderTopbar();
}
$("#projectPage").addEventListener("click", async (e) => {
  const del = e.target.closest("[data-pdel]");
  if (del) return deleteChat(del.dataset.pdel);
  const c = e.target.closest("[data-pchat]");
  if (c) return openChat(c.dataset.pchat);
  const id = e.target.closest("button, #openFolder")?.id;
  if (id === "projNew") return newChat(S.projectId);
  if (id === "reidx") { projState.progress = { phase: "scan", done: 0, total: 0 }; $("#projProg").innerHTML = projProgressHtml(); return call("reindex_project", { id: S.projectId }); }
  if (id === "openFolder") return invoke("open_path", { path: projState.info.project.folder });
  if (id === "saveInstr") { await call("update_project", { id: S.projectId, name: projState.info.project.name, instructions: $("#projInstr").value }); toast(t("saved")); }
});

// ---------------------------------------------------------------- status bar
/** Bottom bar: server state, downloads, live/avg tok/s, VRAM and RAM of llama-server. */
function renderStatus() {
  const i = S.info, m = S.metrics;
  const live = S.live && S.live.tps > 0;
  const tps = live ? S.live.tps : S.lastTps;
  const vt = m.vram_total || i.vram_total || 0;
  const vp = vt ? Math.min(100, (m.vram / vt) * 100) : 0;
  const dls = Object.entries(S.downloads).filter(([, p]) => !p.error && p.total);
  const stateLbl = { ready: t("ready"), starting: t("loading"), runtime: t("runtime"), error: t("error"), stopped: t("stopped") }[i.state] || i.state;
  const msg = i.state === "stopped" && i.idle_unloaded_min ? t("idleUnloaded", { n: i.idle_unloaded_min }) : i.state === "stopped" && i.crash ? t("serverCrashed", { x: i.crash }) : i.message || "";
  $("#statusbar").innerHTML = `
    <div class="seg state"><span class="dot ${i.state}"></span><b>${esc(stateLbl)}</b>${i.state === "ready" ? `<span class="msg">${esc(shortModel(i.model))} · ${fmtK(i.n_ctx)} ctx</span>` : `<span class="msg" title="${esc(msg)}">${esc(msg)}</span>`}</div>
    <button class="seg engine-btn ${S.engineOpen ? "on" : ""}" id="engineBtn" title="${t("engineStats")}">${icon("chip")}</button>
    <div class="spacer"></div>
    ${dls.length ? `<div class="seg">${icon("download")}${dls.map(([id, p]) => `${esc(p.label || id.replace("model:", ""))} ${p.total ? Math.round((p.done / p.total) * 100) : 0}%`).join(" · ")}</div>` : ""}
    <div class="seg tps ${live ? "live" : ""}" title="tokens/s">${icon("bolt")}<b>${tps ? tps.toFixed(1) : "–"}</b> ${t("tps")}</div>
    <div class="seg opt" title="${t("avgHint")}">${t("avg")} <b>${m.avg_tps ? m.avg_tps.toFixed(1) : "–"}</b></div>
    <div class="seg" title="${t("vram")}: ${fmtBytes(m.vram)} / ${fmtBytes(vt)}${m.shared ? ` · ${t("shared")} ${fmtBytes(m.shared)}` : ""}">${t("vram")} <div class="meter"><div style="width:${vp}%"></div></div><b>${fmtBytes(m.vram || 0)}</b>${vt ? " / " + fmtBytes(vt) : ""}</div>
    <div class="seg opt" title="${t("ram")} (llama-server)">${t("ram")} <b>${fmtBytes(m.ram || 0)}</b></div>
    ${i.pid ? `<div class="seg" title="${t("gpuLoad")}">${t("gpu")} <b>${Math.round(m.gpu || 0)}%</b></div><div class="seg opt" title="${t("cpuLoad")}">${t("cpu")} <b>${Math.round(m.cpu || 0)}%</b></div>` : ""}`;
  $("#engineBtn").onclick = (e) => { e.stopPropagation(); S.engineOpen = !S.engineOpen; renderStatus(); };
  renderEnginePanel();
}

const fmtDur = (s) => { s = Math.max(0, Math.round(s)); const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60); return h ? `${h} h ${m} min` : m ? `${m} min ${s % 60} s` : `${s} s`; };
const fmtNum = (n) => (n == null ? "–" : Math.round(n).toLocaleString());
/** Live engine telemetry (local only): process load and llama-server's own counters since the model loaded. */
function renderEnginePanel() {
  const p = $("#enginePanel");
  p.classList.toggle("on", S.engineOpen);
  if (!S.engineOpen) return;
  const i = S.info, m = S.metrics, e = m.engine || {};
  const row = (k, v) => `<div class="er"><span>${k}</span><b>${v}</b></div>`;
  const sec = (title, rows) => `<div class="es"><div class="eh">${title}</div>${rows}</div>`;
  const ready = i.state === "ready";
  let html = `<div class="ep-h">${icon("chip")}<b>${t("engineStats")}</b><span class="spacer"></span><small>${t("engineLocal")}</small></div>`;
  html += sec(t("engineProc"),
    row(t("model"), ready ? esc(shortModel(i.model)) : esc(t("noModel"))) +
    row(t("backend"), `${esc(S.settings.backend || "")} · llama.cpp ${esc(S.runtime || "—")}`) +
    (ready ? row(t("uptime"), i.loaded_at ? fmtDur((Date.now() - i.loaded_at) / 1000) : "–") + row("PID", i.pid) + row(t("ctx"), fmtK(i.n_ctx) + " tok") : ""));
  html += sec(t("engineLoad"),
    row(t("gpuLoad"), `${Math.round(m.gpu || 0)}%`) + row(t("cpuLoad"), `${Math.round(m.cpu || 0)}%`) +
    row(t("vram"), `${fmtBytes(m.vram || 0)} / ${fmtBytes(m.vram_total || i.vram_total || 0)}`) +
    row(t("sharedMem"), fmtBytes(m.shared || 0)) + row(t("ram"), fmtBytes(m.ram || 0)));
  html += sec(t("engineSpeed"),
    row(t("genNow"), m.tps ? m.tps.toFixed(1) + " tok/s" : "–") +
    row(t("genLast"), S.lastTps ? S.lastTps.toFixed(1) + " tok/s" : "–") +
    row(t("genAvg"), m.avg_tps ? m.avg_tps.toFixed(1) + " tok/s" : "–") +
    row(t("promptLast"), S.lastPromptTps ? fmtNum(S.lastPromptTps) + " tok/s" : "–") +
    row(t("promptAvg"), e.prompt_tokens_seconds ? fmtNum(e.prompt_tokens_seconds) + " tok/s" : "–"));
  if (ready && m.engine) {
    html += sec(t("engineTotals"),
      row(t("promptTok"), fmtNum(e.prompt_tokens_total)) +
      row(t("cachedTok"), fmtNum(e.prompt_tokens_cached_total)) +
      row(t("genTok"), fmtNum(e.tokens_predicted_total)) +
      row(t("busyTime"), fmtDur((e.prompt_seconds_total || 0) + (e.tokens_predicted_seconds_total || 0))) +
      row(t("requests"), `${fmtNum(e.requests_processing)} / ${fmtNum(e.requests_deferred)}`));
  }
  p.innerHTML = html;
}
document.addEventListener("click", (e) => {
  if (S.engineOpen && !e.target.closest("#enginePanel, #engineBtn")) { S.engineOpen = false; renderStatus(); }
});

// ---------------------------------------------------------------- events
listen("server-status", (e) => {
  const prev = S.info.state;
  S.info = e.payload;
  // the backend remembers the loaded model; keep the UI copy in sync so set_settings never writes back a stale one
  if (S.info.state === "ready" && S.info.model) S.settings.last_model = S.info.model;
  renderStatus(); renderTopbar();
  if (prev !== S.info.state) {
    refreshModels();
    if (S.info.state === "error") toast(S.info.message, true);
    if (S.info.state === "stopped" && S.info.idle_unloaded_min) toast(t("idleUnloaded", { n: S.info.idle_unloaded_min }));
    if (S.info.state === "stopped" && S.info.crash) toast(t("serverCrashed", { x: S.info.crash }), true);
  }
  if (S.view === "models") renderModels();
});
listen("metrics", (e) => { S.metrics = e.payload; renderStatus(); });
listen("update-status", (e) => {
  const prev = S.updates?.app?.version;
  S.updates = e.payload;
  if (S.updates.app && S.updates.app.version !== prev) toast(t("updateAvailable", { v: S.updates.app.version }));
  renderSidebarStatic(); // (the credit row's check button and the Update entries)
  if (S.view === "settings") renderSettings();
});
// tray menu → the window confirms and shows progress
listen("update-prompt", (e) => {
  const k = e.payload.kind;
  if (k === "app_update") promptAppUpdate();
  if (k === "rt_update") promptRuntimeUpdate();
  if (k === "check") checkUpdatesNow();
});
listen("download-progress", (e) => {
  const p = e.payload;
  if (p.finished || p.cancelled) {
    delete S.downloads[p.id];
    if (p.finished && (p.id === "runtime" || p.id === "embed-model")) toast(`${p.label} ✓`);
  } else {
    S.downloads[p.id] = p; // progress or { error }
    updateDownloadUI(p.id);
  }
  renderStatus();
});
listen("index-progress", (e) => {
  const p = e.payload;
  if (p.project_id !== S.projectId) return;
  projState.progress = p;
  if (p.phase === "done") { refreshProjectChats(); return; }
  const n = $("#projProg"); if (n && S.view === "project") n.innerHTML = projProgressHtml();
});
listen("chat-event", (e) => {
  const p = e.payload;
  if (p.kind === "busy") {
    S.busy[p.chat_id] = p.busy;
    if (!p.busy && p.chat_id === S.chatId) { S.live = null; S.liveCtxBase = 0; }
    renderSidebar();
    if (p.chat_id === S.chatId) { renderComposer(); markLastTurn(); }
    return;
  }
  if (p.kind === "title") {
    const c = S.chats.find((x) => x.id === p.chat_id); if (c) c.title = p.title;
    if (S.chat?.id === p.chat_id) { S.chat.title = p.title; renderTopbar(); }
    renderSidebar(); refreshProjectChats();
    return;
  }
  if (p.chat_id !== S.chatId) return;
  switch (p.kind) {
    case "message": addMessage(p.message); break;
    case "message_update": {
      const m = S.messages.find((x) => x.id === p.message.id);
      if (m) Object.assign(m, p.message);
      const old = $(`#thread [data-id="${p.message.id}"]`);
      if (old && m) old.replaceWith(eventEl(m));
      break;
    }
    case "compress_progress": {
      const n = $(`#thread [data-id="${p.id}"]`);
      if (n) { n.querySelector(".prog > div").style.width = Math.round(p.progress * 100) + "%"; n.querySelector(".event-h small").textContent = Math.round(p.progress * 100) + "%"; }
      break;
    }
    case "compressed":
      S.messages.forEach((m) => { if (m.seq < p.upto_seq && m.role !== "event") m.compressed = true; });
      S.ctxUsed = p.ctx_used; S.liveCtxBase = p.ctx_used; if (p.ctx_budget) S.budget = p.ctx_budget;
      if (!S.live) renderChat();
      updateCtxMeter();
      break;
    case "ctx": S.ctxUsed = p.ctx_used; updateCtxMeter(); break;
    case "assistant_start": startLive(p.temp_id); break;
    case "delta": liveDelta(p); break;
    case "progress": liveProgress(p); break;
    case "tool_pending": {
      if (S.live && S.live.tempId === p.temp_id && !S.live.seg.querySelector(".pending-tool")) {
        S.live.seg.appendChild(el(`<div class="tool pending-tool"><div class="tool-h"><span class="spinner"></span><span class="name">${esc(t(TOOL_LABEL[p.name] || p.name))}</span></div></div>`));
      }
      break;
    }
    case "assistant_done": flushLive(); finishLive(p); break;
    case "assistant_failed": if (S.live) { S.live.seg.remove(); S.live = null; } break;
    case "tool_start": currentTurn(true).querySelector(".turn-body").appendChild(runningToolEl(p.id, p.name, p.arguments)); scrollBottom(); break;
    case "tool_confirm": S.confirms[p.id] = p; showConfirm(p); break;
    case "tool_progress": { // audit_code batches
      const n = $(`#thread .tool[data-call="${p.call_id}"]`); if (!n) break;
      let r = n.querySelector(".tool-h .res");
      if (!r) { r = el(`<span class="res"></span>`); n.querySelector(".tool-h .st").before(r); }
      r.textContent = p.total ? `${Math.min(p.done + 1, p.total)}/${p.total} · ${p.text}` : p.text;
      let bar = n.querySelector(".tool-prog");
      if (!bar) { bar = el(`<div class="prog tool-prog"><div></div></div>`); n.appendChild(bar); }
      bar.firstElementChild.style.width = (p.total ? Math.round((p.done / p.total) * 100) : 2) + "%";
      break;
    }
    case "tool_confirmed": {
      delete S.confirms[p.id];
      const n = $(`#thread [data-confirm="${p.id}"]`);
      if (n) n.outerHTML = `<div class="confirm done">${icon(p.decision === "deny" ? "x" : "check")}${esc(t(p.decision === "deny" ? "confirmDenied" : "confirmAllowed"))}</div>`;
      break;
    }
    case "notice": toast(t(p.key)); break;
    case "error": {
      if (S.live) { S.live.seg.remove(); S.live = null; }
      $("#thread").appendChild(el(`<div class="err-box">${icon("warn")} ${esc(p.error)}</div>`));
      scrollBottom(true);
      break;
    }
  }
});

// ---------------------------------------------------------------- keyboard shortcuts
// [keys, i18n key of the description] – also listed under Settings → Keyboard shortcuts
const SHORTCUTS = [
  ["Ctrl+N", "scNew"], ["Ctrl+K", "scSearch"], ["Ctrl+,", "scSettings"], ["Ctrl+Shift+E", "scExport"],
  ["Esc", "scEsc"], ["Enter / Shift+Enter", "scSend"],
];
document.addEventListener("keydown", (e) => {
  const k = e.key.toLowerCase();
  if (e.ctrlKey && !e.shiftKey && k === "n") { e.preventDefault(); newChat(contextProject()?.id || null); return; }
  if (e.ctrlKey && k === "k") { e.preventDefault(); if (S.tab !== "chats") { S.tab = "chats"; renderSidebar(); } $("#searchInp").focus(); $("#searchInp").select(); return; }
  if (e.ctrlKey && k === ",") { e.preventDefault(); showView("settings"); return; }
  if (e.ctrlKey && e.shiftKey && k === "e" && S.chatId) { e.preventDefault(); exportChat(); return; }
  if (e.key === "Escape") {
    // innermost first: popup menu, dialog, engine panel, then a running generation
    if (openMenuEl) return closeMenu();
    if (closeTopModal()) return;
    if (S.engineOpen) { S.engineOpen = false; return renderStatus(); }
    if (S.busy[S.chatId]) call("stop_generation", { chatId: S.chatId });
  }
});

// ---------------------------------------------------------------- boot
(async function boot() {
  const st = await invoke("get_state");
  S.settings = st.settings; S.info = st.info; S.catalog = st.catalog; S.root = st.root; S.runtime = st.runtime; S.version = st.version;
  S.updates = st.updates || {};
  S.os = st.os || "windows";
  S.toggles.think = !!S.settings.thinking;
  await Promise.all([refreshChats(), refreshProjects(), refreshModels()]);
  showView("chat");
  renderStatus();
  input.focus();
})();
