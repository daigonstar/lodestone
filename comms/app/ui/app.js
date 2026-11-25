// Lodestone Comms interface. Talks only to its own plug-in on 127.0.0.1, with the per-launch secret from
// the address (#t=...). Everything shown from servers and other people is escaped or set as text.
"use strict";

const TOKEN = new URLSearchParams(location.hash.slice(1)).get("t") || "";
history.replaceState(null, "", location.pathname); // don't leave the secret in the address bar

const $ = (s) => document.querySelector(s);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const state = { hosts: [], bridge: false, sel: { host: "", space: "", channel: "" }, me: "" };

async function api(path, body) {
  const r = await fetch(path, {
    method: body ? "POST" : "GET",
    headers: { "x-comms-token": TOKEN, ...(body ? { "content-type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(j.error || `Error ${r.status}`);
  return j;
}

function toast(text, bad) {
  const t = $("#toast");
  t.textContent = text;
  t.classList.toggle("bad", !!bad);
  t.classList.remove("hidden");
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => t.classList.add("hidden"), 5000);
}

function applyTheme(vars) {
  for (const [k, v] of Object.entries(vars || {})) if (k.startsWith("--") && typeof v === "string") document.documentElement.style.setProperty(k, v);
}

// ---------- data ----------
function spaces() {
  return state.hosts.flatMap((h) => (h.spaces || []).map((s) => ({ ...s, host: h.url, me: h.username })));
}
function current() {
  const sp = spaces().find((s) => s.host === state.sel.host && s.id === state.sel.space);
  const ch = sp && sp.channels.find((c) => c.id === state.sel.channel);
  return { sp, ch };
}

async function loadState() {
  try {
    const s = await api("/api/state");
    state.hosts = s.hosts;
    state.bridge = s.bridge;
    applyTheme(s.theme);
    for (const h of s.hosts) if (h.error) toast(`${h.url}: ${h.error}`, true);
  } catch (e) { toast(e.message, true); }
  const all = spaces();
  if (!all.some((s) => s.host === state.sel.host && s.id === state.sel.space)) {
    const first = all[0];
    state.sel = first ? { host: first.host, space: first.id, channel: "" } : { host: "", space: "", channel: "" };
  }
  const { sp } = current();
  if (sp && !sp.channels.some((c) => c.id === state.sel.channel)) state.sel.channel = (sp.channels.find((c) => c.ready) || sp.channels[0] || {}).id || "";
  render();
}

// ---------- rendering ----------
function render() {
  const all = spaces();
  $("#servers").innerHTML = all.map((s) => `<button class="srv ${s.host === state.sel.host && s.id === state.sel.space ? "on" : ""}" data-space="${esc(s.id)}" data-host="${esc(s.host)}" title="${esc(s.name)}">${iconHTML(s)}</button>`).join("")
    + `<button class="srv add" id="add-server" title="Join or create a server">+</button>`;
  const { sp, ch } = current();
  if (!sp) {
    $("#channels").innerHTML = "";
    $("#chat").innerHTML = `<div class="empty"><h2>Lodestone Comms</h2>
      <p class="muted">End-to-end encrypted chat on servers you or your friends host. Messages are encrypted on your PC; the host only passes them on.</p>
      <div class="row"><button class="primary" data-act="join">Join with an invite</button><button class="ghost" data-act="setup">Set up a new host</button></div></div>`;
    return;
  }
  const admin = sp.your_role === "owner" || sp.your_role === "admin";
  $("#channels").innerHTML = `<h2><span>${esc(sp.name)}</span><button class="gear" data-act="settings" title="Server settings">⚙</button></h2><div class="host" title="${esc(sp.host)}">${esc(sp.me)} · ${esc(sp.host.replace(/^https?:\/\//, ""))}</div>
    <div class="chlist">${sp.channels.map((c) => `<button class="ch ${c.id === state.sel.channel ? "on" : ""}" data-channel="${esc(c.id)}"># ${esc(c.name)}${c.ready ? "" : ' <span class="wait">(waiting for keys)</span>'}</button>`).join("")}</div>
    <div class="side-actions"><button data-act="invite">Invite people</button>${admin ? '<button class="ghost" data-act="channel">New channel</button>' : ""}</div>`;
  if (!ch) { $("#chat").innerHTML = ""; return; }
  $("#chat").innerHTML = `<header><h3># ${esc(ch.name)}${ch.topic ? `<span class="topic">${esc(ch.topic)}</span>` : ""}</h3><span class="lock" title="Only members' devices can read this channel; the host stores ciphertext.">🔒 end-to-end encrypted</span></header>
    <div class="msgs" id="msgs"></div>
    <div class="composer">${ch.ready
      ? '<textarea id="text" placeholder="Message (Enter to send, Shift+Enter for a new line)" maxlength="8000"></textarea>'
      : '<p class="muted">You\'re a member, but no one in this channel has been online to give your device its keys yet. It happens automatically.</p>'}</div>`;
  loadMessages();
  const t = $("#text");
  if (t) {
    t.focus();
    t.addEventListener("keydown", async (e) => {
      if (e.key !== "Enter" || e.shiftKey) return;
      e.preventDefault();
      const text = t.value.trim();
      if (!text) return;
      t.disabled = true;
      try { await api("/api/send", { host: state.sel.host, channel: state.sel.channel, text }); t.value = ""; }
      catch (err) { toast(err.message, true); }
      t.disabled = false;
      t.focus();
    });
  }
}

function initials(name) {
  return name.split(/\s+/).filter(Boolean).slice(0, 2).map((w) => [...w][0]).join("").toUpperCase() || "?";
}

async function loadMessages() {
  const { ch } = current();
  if (!ch || !ch.ready) return;
  const box = $("#msgs");
  try {
    const r = await api(`/api/messages?host=${encodeURIComponent(state.sel.host)}&channel=${encodeURIComponent(ch.id)}`);
    if (state.sel.channel !== ch.id || !box.isConnected) return;
    const atBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 40;
    box.replaceChildren(...r.messages.map((m) => {
      const row = document.createElement("div");
      row.className = "msg";
      const time = document.createElement("time");
      time.textContent = new Date(m.sent_at * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      const who = document.createElement("span");
      who.className = "who" + (m.sender === r.me ? " me" : "") + (m.sender.startsWith("⚠") ? " warn" : "");
      who.textContent = m.sender;
      if (m.sender.startsWith("⚠")) who.title = "This sender's encryption key doesn't match what the host lists for them";
      row.append(time, who, document.createTextNode(m.body));
      return row;
    }));
    if (!r.messages.length) box.innerHTML = '<p class="muted">No messages yet. Messages sent before you joined can\'t be read on this device, by design.</p>';
    if (atBottom || !box.dataset.seen) box.scrollTop = box.scrollHeight;
    box.dataset.seen = "1";
  } catch (e) { toast(e.message, true); }
}

// ---------- server icons ----------
// Icons come from the host through the plug-in (the page can't send its secret with a plain <img>),
// so they're fetched once and shown from a local blob.
const icons = new Map();
function iconHTML(s) {
  const key = s.host + "|" + s.id;
  const url = icons.get(key);
  if (url) return `<img src="${esc(url)}" alt="">`;
  if (s.has_icon && !icons.has(key)) {
    icons.set(key, "");
    fetch(`/api/icon?host=${encodeURIComponent(s.host)}&space=${encodeURIComponent(s.id)}`, { headers: { "x-comms-token": TOKEN } })
      .then((r) => (r.ok ? r.blob() : null))
      .then((b) => { if (b) { icons.set(key, URL.createObjectURL(b)); render(); } });
  }
  return esc(initials(s.name));
}
function forgetIcon(s) {
  const key = s.host + "|" + s.id;
  const url = icons.get(key);
  if (url) URL.revokeObjectURL(url);
  icons.delete(key);
}

// ---------- server settings ----------
/** A confirmation using Comms's own dialog (native confirm() isn't reliable inside Lodestone's frame).
 *  With `typed`, the person has to type that text to confirm. */
function confirmBox(text, okLabel, typed) {
  return new Promise((resolve) => {
    let done = false;
    const finish = (v) => { if (!done) { done = true; resolve(v); } };
    ask("Are you sure?", `<p>${esc(text)}</p>${typed ? `<label>Type <b>${esc(typed)}</b> to confirm<input name="typed" autocomplete="off"></label>` : ""}`, okLabel, (f) => {
      if (typed && f.get("typed") !== typed) throw new Error("That doesn't match");
      finish(true);
    });
    $("#dlg").addEventListener("close", () => finish(false), { once: true });
  });
}

const manage = (method, path, body) => api("/api/manage", { host: state.sel.host, method, path, body });
const fmtDate = (t) => new Date(t * 1000).toLocaleString([], { dateStyle: "medium", timeStyle: "short" });

function openSettings(tab) {
  const { sp } = current();
  if (!sp) return;
  const admin = sp.your_role === "owner" || sp.your_role === "admin";
  const tabs = [["overview", "Overview"], ...(admin ? [["channels", "Channels"]] : []), ["members", "Members"], ...(admin ? [["bans", "Bans"]] : []), ["invites", "Invites"]];
  $("#set-title").textContent = `${sp.name}: settings`;
  $("#set-tabs").innerHTML = tabs.map(([k, l]) => `<button type="button" data-tab="${k}" class="${k === tab ? "on" : ""}">${l}</button>`).join("");
  if (!$("#settings").open) $("#settings").showModal();
  ({ overview: tabOverview, channels: tabChannels, members: tabMembers, bans: tabBans, invites: tabInvites })[tab](sp, admin);
}
$("#set-close").onclick = () => $("#settings").close();
$("#set-tabs").onclick = (e) => { const b = e.target.closest("[data-tab]"); if (b) openSettings(b.dataset.tab); };
async function after(tab, fn) {
  try { await fn(); await loadState(); if (current().sp) openSettings(tab); else $("#settings").close(); }
  catch (e) { toast(e.message, true); }
}

function tabOverview(sp, admin) {
  const body = $("#set-body");
  const owner = sp.your_role === "owner";
  body.innerHTML = `<div class="row" style="align-items:flex-start;gap:16px">
      <div class="icon-preview">${iconHTML(sp)}</div>
      <div style="flex:1">
        <label>Name<input id="s-name" maxlength="64" value="${esc(sp.name)}" ${admin ? "" : "disabled"}></label>
        <label>Description<textarea id="s-desc" class="desc" maxlength="500" ${admin ? "" : "disabled"}>${esc(sp.description || "")}</textarea></label>
      </div></div>
    ${admin ? `<div class="row end"><button type="button" class="ghost" id="s-icon">Change icon…</button>${sp.has_icon ? '<button type="button" class="ghost" id="s-icon-rm">Remove icon</button>' : ""}<button type="button" class="primary" id="s-save">Save</button></div>` : ""}
    <p class="muted" style="margin-top:18px">Server names, descriptions, icons, channel names and topics are visible to the host. Messages are not.</p>
    <div class="row end" style="margin-top:18px">
      ${owner ? '<button type="button" class="danger" id="s-delete">Delete server…</button>' : '<button type="button" class="danger" id="s-leave">Leave server</button>'}
    </div>`;
  if (admin) {
    $("#s-save").onclick = () => after("overview", () => manage("PATCH", `/v1/spaces/${sp.id}`, { name: $("#s-name").value, description: $("#s-desc").value }));
    $("#s-icon").onclick = () => $("#icon-file").click();
    $("#icon-file").onchange = async (e) => {
      const f = e.target.files[0];
      e.target.value = "";
      if (!f) return;
      if (f.size > 256 * 1024) return toast("Icons can be at most 256 KiB", true);
      const data = await new Promise((res) => { const r = new FileReader(); r.onload = () => res(String(r.result).split(",")[1]); r.readAsDataURL(f); });
      forgetIcon(sp);
      after("overview", () => api("/api/icon", { host: sp.host, space: sp.id, data }));
    };
    const rm = $("#s-icon-rm");
    if (rm) rm.onclick = () => { forgetIcon(sp); after("overview", () => manage("DELETE", `/v1/spaces/${sp.id}/icon`)); };
  }
  const del = $("#s-delete");
  if (del) del.onclick = async () => {
    if (!(await confirmBox(`This deletes "${sp.name}", its channels and all their messages for everyone.`, "Delete server", sp.name))) return;
    after("overview", () => manage("DELETE", `/v1/spaces/${sp.id}`));
  };
  const leave = $("#s-leave");
  if (leave) leave.onclick = async () => {
    if (!(await confirmBox(`Leave "${sp.name}"? You'll need a new invite to come back.`, "Leave"))) return;
    after("overview", () => manage("DELETE", `/v1/spaces/${sp.id}/members/${encodeURIComponent(sp.me)}`));
  };
}

function tabChannels(sp) {
  const chans = sp.channels;
  $("#set-body").innerHTML = chans.map((c, i) => `<div class="set-row" data-ch="${esc(c.id)}">
      <div class="grow"><input data-f="name" maxlength="64" value="${esc(c.name)}"><input data-f="topic" maxlength="300" placeholder="Topic (optional)" value="${esc(c.topic || "")}" style="margin-top:4px"></div>
      <button type="button" data-ch-act="up" ${i ? "" : "disabled"} title="Move up">↑</button>
      <button type="button" data-ch-act="down" ${i < chans.length - 1 ? "" : "disabled"} title="Move down">↓</button>
      <button type="button" data-ch-act="save">Save</button>
      <button type="button" class="danger" data-ch-act="delete" ${chans.length > 1 ? "" : "disabled"}>Delete</button></div>`).join("")
    + `<div class="row end"><button type="button" class="primary" data-act="channel">New channel</button></div>`;
  $("#set-body").onclick = async (e) => {
    const b = e.target.closest("[data-ch-act]");
    if (!b) return;
    const row = b.closest("[data-ch]");
    const id = row.dataset.ch;
    const act = b.dataset.chAct;
    if (act === "save") return after("channels", () => manage("PATCH", `/v1/channels/${id}`, { name: row.querySelector('[data-f="name"]').value, topic: row.querySelector('[data-f="topic"]').value }));
    if (act === "delete") {
      if (!(await confirmBox("Delete this channel and all its messages for everyone?", "Delete channel"))) return;
      return after("channels", () => manage("DELETE", `/v1/channels/${id}`));
    }
    const order = chans.map((c) => c.id);
    const i = order.indexOf(id), j = act === "up" ? i - 1 : i + 1;
    [order[i], order[j]] = [order[j], order[i]];
    after("channels", () => manage("PUT", `/v1/spaces/${sp.id}/channel-order`, { channels: order }));
  };
}

function tabMembers(sp, admin) {
  const owner = sp.your_role === "owner";
  const rank = { owner: 0, admin: 1, member: 2 };
  const list = [...sp.members].sort((a, b) => rank[a.role] - rank[b.role] || a.username.localeCompare(b.username));
  $("#set-body").innerHTML = list.map((m) => {
    const me = m.username === sp.me;
    const canRemove = !me && (owner || (admin && m.role === "member"));
    return `<div class="set-row" data-user="${esc(m.username)}"><div class="grow"><b>${esc(m.username)}</b> ${me ? "<small>(you)</small>" : ""}<br><small>${esc(m.role)} · ${m.devices.length} device${m.devices.length === 1 ? "" : "s"}</small></div>
      ${owner && !me && m.role !== "owner" ? `<select data-m-act="role"><option value="member" ${m.role === "member" ? "selected" : ""}>Member</option><option value="admin" ${m.role === "admin" ? "selected" : ""}>Admin</option></select><button type="button" data-m-act="transfer">Make owner…</button>` : ""}
      ${canRemove ? '<button type="button" data-m-act="kick">Remove</button><button type="button" class="danger" data-m-act="ban">Ban</button>' : ""}</div>`;
  }).join("");
  $("#set-body").onchange = (e) => {
    const sel = e.target.closest('[data-m-act="role"]');
    if (!sel) return;
    const u = sel.closest("[data-user]").dataset.user;
    after("members", () => manage("PUT", `/v1/spaces/${sp.id}/members/${encodeURIComponent(u)}/role`, { role: sel.value }));
  };
  $("#set-body").onclick = async (e) => {
    const b = e.target.closest("[data-m-act]");
    if (!b || b.tagName === "SELECT") return;
    const u = b.closest("[data-user]").dataset.user;
    const path = `/v1/spaces/${sp.id}/members/${encodeURIComponent(u)}`;
    const act = b.dataset.mAct;
    if (act === "kick" && (await confirmBox(`Remove ${u} from the server? They can rejoin with an invite.`, "Remove"))) after("members", () => manage("DELETE", path));
    if (act === "ban" && (await confirmBox(`Ban ${u}? They're removed and can't rejoin until unbanned.`, "Ban"))) after("members", () => manage("DELETE", path + "?ban=true"));
    if (act === "transfer" && (await confirmBox(`Make ${u} the owner? You'll become an admin.`, "Make owner", u))) after("members", () => manage("POST", `/v1/spaces/${sp.id}/transfer`, { username: u }));
  };
}

async function tabBans(sp) {
  const body = $("#set-body");
  body.innerHTML = '<p class="muted">Loading…</p>';
  try {
    const { bans } = await manage("GET", `/v1/spaces/${sp.id}/bans`);
    body.innerHTML = bans.length ? bans.map((u) => `<div class="set-row" data-user="${esc(u)}"><div class="grow">${esc(u)}</div><button type="button" data-unban>Unban</button></div>`).join("") : '<p class="muted">Nobody is banned.</p>';
    body.onclick = (e) => {
      const b = e.target.closest("[data-unban]");
      if (b) after("bans", () => manage("DELETE", `/v1/spaces/${sp.id}/bans/${encodeURIComponent(b.closest("[data-user]").dataset.user)}`));
    };
  } catch (e) { body.innerHTML = `<p class="err">${esc(e.message)}</p>`; }
}

async function tabInvites(sp) {
  const body = $("#set-body");
  body.innerHTML = '<p class="muted">Loading…</p>';
  try {
    const { invites } = await manage("GET", `/v1/spaces/${sp.id}/invites`);
    body.innerHTML = `<p class="muted">The host keeps only a scrambled copy of each code, so links can't be shown again. Revoke ones you no longer want.</p>`
      + (invites.length ? invites.map((i) => `<div class="set-row" data-inv="${esc(i.id)}"><div class="grow">Invite <code>${esc(i.id.slice(0, 6))}</code> by ${esc(i.created_by)}<br><small>${i.uses} of ${i.max_uses} uses · expires ${esc(fmtDate(i.expires))}</small></div><button type="button" class="danger" data-revoke>Revoke</button></div>`).join("") : '<p class="muted">No active invites.</p>')
      + `<div class="row end"><button type="button" class="primary" data-act="invite">New invite</button></div>`;
    body.onclick = (e) => {
      const b = e.target.closest("[data-revoke]");
      if (b) after("invites", () => manage("DELETE", `/v1/spaces/${sp.id}/invites/${b.closest("[data-inv]").dataset.inv}`));
    };
  } catch (e) { body.innerHTML = `<p class="err">${esc(e.message)}</p>`; }
}

// ---------- dialogs ----------
function ask(title, body, okText, onOk) {
  $("#dlg-title").textContent = title;
  $("#dlg-body").innerHTML = body;
  $("#dlg-err").textContent = "";
  $("#dlg-ok").textContent = okText || "OK";
  $("#dlg-ok").classList.toggle("hidden", !onOk);
  $("#dlg-cancel").textContent = onOk ? "Cancel" : "Close";
  $("#dlg-form").onsubmit = async (e) => {
    e.preventDefault();
    if (!onOk) return $("#dlg").close();
    $("#dlg-ok").disabled = true;
    try { if ((await onOk(new FormData($("#dlg-form")))) !== false) $("#dlg").close(); }
    catch (err) { $("#dlg-err").textContent = err.message; }
    $("#dlg-ok").disabled = false;
  };
  $("#dlg").showModal();
  const first = $("#dlg-body input");
  if (first) first.focus();
}
$("#dlg-cancel").onclick = () => $("#dlg").close();

function joinDialog() {
  ask("Join a server", `<label>Invite link<input name="link" placeholder="comms.yourdomain.com/join/…" autocomplete="off" spellcheck="false" required></label>
    <label>Your username on that host <span class="muted">(only the first time)</span><input name="username" autocomplete="off" spellcheck="false" placeholder="2 to 32: a-z 0-9 _ . -"></label>`,
    "Join", async (f) => {
      const r = await api("/api/join", { link: f.get("link"), username: f.get("username") || null });
      state.sel = { host: r.host, space: r.space, channel: "" };
      await loadState();
    });
}

function setupDialog() {
  ask("Set up a new host", `<p class="muted">After starting a Comms host (see the README), its log shows a one-time setup code. The first account becomes the host's admin.</p>
    <label>Host's domain<input name="host" placeholder="comms.yourdomain.com" autocomplete="off" spellcheck="false" required></label>
    <label>Setup code<input name="code" autocomplete="off" required></label>
    <label>Your username<input name="username" autocomplete="off" spellcheck="false" required></label>`,
    "Set up", async (f) => {
      const r = await api("/api/setup", { host: f.get("host"), code: f.get("code"), username: f.get("username") });
      await loadState();
      newServerDialog(r.host);
    });
}

function newServerDialog(host) {
  ask("Create a server", `<label>Name<input name="name" maxlength="64" required placeholder="Friday raids"></label>`, "Create", async (f) => {
    const r = await api("/api/spaces", { host, name: f.get("name") });
    state.sel = { host, space: r.space, channel: "" };
    await loadState();
  });
}

function addDialog() {
  const hosts = state.hosts.map((h) => h.url);
  ask("Servers", `<div class="choices">
      <button type="button" class="primary" data-act="join">Join a server with an invite</button>
      ${hosts.map((h) => `<button type="button" class="ghost" data-act="create" data-host="${esc(h)}">Create a server on <b>${esc(h.replace(/^https?:\/\//, ""))}</b></button>`).join("")}
      <button type="button" class="ghost" data-act="setup">Set up a new host…</button></div>`, null, null);
}

async function inviteDialog() {
  const { sp } = current();
  let link = "";
  try { link = (await api("/api/invites", { host: sp.host, space: sp.id, max_uses: 5 })).link; } catch (e) { return toast(e.message, true); }
  ask(`Invite people to ${sp.name}`, `<label>Invite link <span class="muted">(5 uses, 7 days)</span>
      <div class="row"><input id="inv-link" readonly value="${esc(link)}"><button type="button" id="inv-copy">Copy</button></div></label>
      <label>Or invite a friend from Lodestone</label><div class="friends" id="friends"><p class="muted">Loading your friends…</p></div>`, null, null);
  $("#inv-copy").onclick = () => { const i = $("#inv-link"); i.select(); navigator.clipboard?.writeText(i.value).then(() => toast("Link copied"), () => toast("Press Ctrl+C to copy")); };
  const box = $("#friends");
  try {
    const { friends } = await api("/api/friends");
    box.innerHTML = friends.length ? friends.map((f) => `<div class="friend">${f.avatar ? `<img src="${esc(f.avatar)}" alt="">` : ""}<span title="${esc(f.store)}">${esc(f.name)} <small class="muted">${esc(f.store)}</small></span>
      <button type="button" data-friend="${esc(f.id)}" data-store="${esc(f.store)}">Invite</button></div>`).join("") : '<p class="muted">No friends found in Lodestone.</p>';
  } catch (e) { box.innerHTML = `<p class="muted">${esc(e.message)}</p>`; }
  box.onclick = async (e) => {
    const b = e.target.closest("[data-friend]");
    if (!b) return;
    b.disabled = true;
    try {
      await api("/api/invite-friend", { host: sp.host, space: sp.id, store: b.dataset.store, id: b.dataset.friend });
      b.textContent = "Sent to chat";
      toast(b.dataset.store === "steam" ? "Their Steam chat is open and the invite is on your clipboard: paste it there." : "The invite is on your clipboard: send it to them.");
    } catch (err) { b.disabled = false; toast(err.message, true); }
  };
}

function channelDialog() {
  const { sp } = current();
  ask("New channel", `<label>Name<input name="name" maxlength="64" required placeholder="raids"></label>`, "Create", async (f) => {
    const r = await api("/api/channels", { host: sp.host, space: sp.id, name: f.get("name") });
    state.sel.channel = r.channel;
    await loadState();
  });
}

document.addEventListener("click", (e) => {
  const s = e.target.closest("[data-space]");
  if (s) { state.sel = { host: s.dataset.host, space: s.dataset.space, channel: "" }; return loadState(); }
  const c = e.target.closest("[data-channel]");
  if (c) { state.sel.channel = c.dataset.channel; return render(); }
  if (e.target.closest("#add-server")) return addDialog();
  const a = e.target.closest("[data-act]");
  if (!a) return;
  ({ join: joinDialog, setup: setupDialog, invite: inviteDialog, channel: channelDialog, settings: () => openSettings("overview"), create: () => newServerDialog(a.dataset.host) })[a.dataset.act]?.();
});

// ---------- live updates ----------
async function listen() {
  for (;;) {
    try {
      const r = await fetch("/api/events", { headers: { "x-comms-token": TOKEN } });
      const reader = r.body.getReader();
      const dec = new TextDecoder();
      let buf = "";
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buf += dec.decode(value, { stream: true });
        let i;
        while ((i = buf.indexOf("\n")) >= 0) {
          const line = buf.slice(0, i);
          buf = buf.slice(i + 1);
          let ev;
          try { ev = JSON.parse(line); } catch { continue; }
          if (ev.type === "state") loadState();
          else if (ev.type === "theme") applyTheme(ev.data);
          else if (ev.type === "messages" && ev.host === state.sel.host && ev.channels.includes(state.sel.channel)) loadMessages();
        }
      }
    } catch {}
    await new Promise((r) => setTimeout(r, 2000));
  }
}

loadState();
listen();
// Other people's changes (names, channels, roles) show up within a few seconds.
setInterval(() => { if (!document.querySelector("dialog[open]")) loadState(); }, 15000);
