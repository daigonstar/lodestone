// Lodestone Beacon interface. Talks only to its own plug-in on 127.0.0.1, with the per-launch secret from
// the address (#t=...). Everything shown from communities and other people is escaped or set as text.
"use strict";

const TOKEN = new URLSearchParams(location.hash.slice(1)).get("t") || "";
history.replaceState(null, "", location.pathname); // don't leave the secret in the address bar

const $ = (s) => document.querySelector(s);
// An icon from the sprite in index.html (angular line icons; the cyan spark comes from --spark in the CSS).
const ic = (name) => `<svg class="i" aria-hidden="true"><use href="#i-${name}"/></svg>`;
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const state = { servers: [], bridge: false, sel: { server: "", community: "", channel: "" }, me: "", showMembers: true, profiles: {}, prefs: { theme: "lodestone", accent: "" } };

async function api(path, body) {
  const r = await fetch(path, {
    method: body ? "POST" : "GET",
    headers: { "x-beacon-token": TOKEN, ...(body ? { "content-type": "application/json" } : {}) },
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
  if (state.prefs.theme !== "lodestone") return; // another theme is chosen
  for (const [k, v] of Object.entries(vars || {})) if (k.startsWith("--") && typeof v === "string") document.documentElement.style.setProperty(k, v);
}

// ---------- data ----------
function communities() {
  return state.servers.flatMap((h) => (h.communities || []).map((s) => ({ ...s, server: h.url, me: h.username })));
}
function current() {
  const sp = communities().find((s) => s.server === state.sel.server && s.id === state.sel.community);
  const ch = sp && sp.channels.find((c) => c.id === state.sel.channel);
  return { sp, ch };
}

async function loadState() {
  try {
    const s = await api("/api/state");
    state.servers = s.servers;
    state.mentions = await api("/api/mentions").catch(() => ({}));
    state.pairing = s.pairing || [];
    state.bridge = s.bridge;
    applyTheme(s.theme);
    for (const h of s.servers) if (h.error) toast(`${h.url}: ${h.error}`, true);
    await Promise.all(s.servers.map((h) => loadProfiles(h.url)));
  } catch (e) { toast(e.message, true); }
  const all = communities();
  if (!all.some((s) => s.server === state.sel.server && s.id === state.sel.community)) {
    const first = all[0];
    state.sel = first ? { server: first.server, community: first.id, channel: "" } : { server: "", community: "", channel: "" };
  }
  const { sp } = current();
  if (sp && !sp.channels.some((c) => c.id === state.sel.channel && c.kind !== "voice")) { const text = sp.channels.filter((c) => c.kind !== "voice"); state.sel.channel = (text.find((c) => c.ready) || text[0] || {}).id || ""; }
  render();
}

// ---------- rendering ----------
function render() {
  const all = communities();
  $("#communities").innerHTML = `<button class="srv home ${state.home || !all.length ? "on" : ""}" id="home-btn" title="Home: join or create communities">${ic("home")}</button><div class="srv-sep"></div>` + all.map((s) => `<button class="srv ${!state.home && s.server === state.sel.server && s.id === state.sel.community ? "on" : ""}" data-community="${esc(s.id)}" data-server="${esc(s.server)}" title="${esc(s.name)}">${iconHTML(s)}</button>`).join("")
    + `<button class="srv add" id="add-community" title="Join or create a community">${ic("plus")}</button>`;
  const { sp, ch } = current();
  if (!sp || state.home) return renderHome();
  const admin = sp.your_role === "owner" || sp.your_role === "admin";
  $("#channels").innerHTML = `<h2><span>${esc(sp.name)}</span><button class="gear" data-act="settings" title="Community settings">${ic("settings")}</button></h2><div class="server" title="${esc(sp.server)}">${esc(sp.me)} · ${esc(sp.server.replace(/^https?:\/\//, ""))}</div>
    <div class="chlist">${sp.channels.filter((c) => c.kind !== "voice").map((c) => `<button class="ch ${c.id === state.sel.channel ? "on" : ""}" data-channel="${esc(c.id)}"># ${esc(c.name)}${c.ready ? "" : ' <span class="wait">(waiting for keys)</span>'}${mentionBadge(sp.server, c.id)}</button>`).join("")}
    ${sp.channels.some((c) => c.kind === "voice") ? '<h4 class="chhead">Voice</h4>' : ""}
    ${sp.channels.filter((c) => c.kind === "voice").map((c) => `<button class="ch vc" data-voice="${esc(c.id)}" title="${c.ready ? "Join the call" : "Waiting for this device's keys"}">${ic("volume")} ${esc(c.name)}${c.ready ? "" : ' <span class="wait">(waiting for keys)</span>'}</button><div class="vc-people" data-vc="${esc(c.id)}"></div>`).join("")}</div>
    <div class="side-actions"><button data-act="invite">Invite people</button>${admin ? '<button class="ghost" data-act="channel">New channel</button>' : ""}</div>
    <div id="callbar"></div>
    ${meBar()}`;
  renderVoice();
  renderMembers(sp);
  if (!ch) { $("#chat").innerHTML = ""; $("#chat").dataset.key = ""; return; }
  // The chat area is only rebuilt when you switch channel or it changes (name, topic, keys arriving).
  // Refreshes (every few seconds, and on live events) used to rebuild it, which jumped the message list
  // and threw away anything you were typing.
  const chatKey = JSON.stringify([state.sel.server, ch.id, ch.name, ch.topic || "", !!ch.ready]);
  if ($("#chat").dataset.key === chatKey && $("#msgs")) return;
  $("#chat").dataset.key = chatKey;
  $("#chat").innerHTML = `<header><h3># ${esc(ch.name)}${ch.topic ? `<span class="topic">${esc(ch.topic)}</span>` : ""}</h3><button class="gear" id="toggle-members" title="Show or hide members">${ic("users")}</button><span class="lock" title="Only members' devices can read this channel; the server stores ciphertext.">${ic("lock")} end-to-end encrypted</span></header>
    <div class="msgs" id="msgs"></div><button type="button" class="new-msgs hidden" id="new-msgs">New messages ↓</button>
    <div class="composer">${ch.ready
      ? '<div id="pending" class="pending hidden"></div><div class="compose-row"><button type="button" class="emoji-btn" id="attach-btn" title="Attach a file or image (or paste / drop one)">${ic("attach")}</button><textarea id="text" placeholder="Message (Enter to send, Shift+Enter for a new line)" maxlength="8000"></textarea><button type="button" class="emoji-btn" id="emoji-btn" title="Emoji">${ic("smile")}</button></div>'
      : '<p class="muted">You\'re a member, but no one in this channel has been online to give your device its keys yet. It happens automatically.</p>'}</div>`;
  // Stick to the newest message only while you're at the bottom; pictures that finish loading keep
  // you there. Scrolled up, you stay where you are.
  const msgs = $("#msgs");
  msgs.onscroll = () => {
    const bottom = msgs.scrollHeight - msgs.scrollTop - msgs.clientHeight < 40;
    msgs.dataset.stick = bottom ? "1" : "";
    if (bottom) $("#new-msgs").classList.add("hidden");
  };
  msgs.addEventListener("load", () => { if (msgs.dataset.stick) msgs.scrollTop = msgs.scrollHeight; }, true);
  $("#new-msgs").onclick = () => { msgs.scrollTop = msgs.scrollHeight; $("#new-msgs").classList.add("hidden"); };
  loadMessages();
  $("#toggle-members").onclick = () => { state.showMembers = !state.showMembers; renderMembers(current().sp); };
  const t = $("#text");
  if (t) {
    $("#emoji-btn").onclick = (e) => openPicker(e.currentTarget, (emo) => insertAtCursor(t, emo), false);
    $("#attach-btn").onclick = () => $("#attach-file").click();
    t.addEventListener("paste", (e) => {
      const f = [...(e.clipboardData?.files || [])][0];
      if (f) { e.preventDefault(); setPending(f); }
    });
    showPending();
    t.focus();
    mentionPicker(t);
    t.addEventListener("keydown", async (e) => {
      if (e.defaultPrevented) return;
      if (e.key !== "Enter" || e.shiftKey) return;
      e.preventDefault();
      const text = t.value.trim();
      if (!text && !state.pending) return;
      t.disabled = true;
      try {
        if (state.pending) {
          const f = state.pending;
          const data = await fileBase64(f);
          await api("/api/upload", { server: state.sel.server, channel: state.sel.channel, name: f.name || "pasted image", data, caption: text });
          setPending(null);
        } else {
          await api("/api/send", { server: state.sel.server, channel: state.sel.channel, text });
        }
        t.value = "";
      } catch (err) { toast(err.message, true); }
      t.disabled = false;
      t.focus();
    });
  }
}

// ---------- home ----------
// The welcome page: what this is, joining and creating communities, your servers. Shown on first run and from
// the ⌂ button.
function renderHome() {
  $("#members").classList.add("hidden");
  const all = communities();
  const servers = state.servers.filter((h) => h.username);
  $("#channels").innerHTML = `<h2><span>Home</span></h2><div class="server">Lodestone Beacon</div>
    <div class="chlist">${all.length ? '<h4 class="chhead">Your communities</h4>' : ""}${all.map((s) => `<button class="ch" data-community="${esc(s.id)}" data-server="${esc(s.server)}">${esc(s.name)}</button>`).join("")}</div>
    <div id="callbar"></div>${servers.length ? meBar() : ""}`;
  renderVoice();
  $("#chat").dataset.key = "";
  $("#chat").innerHTML = `<div class="home-page">
    <div class="test-banner"><b>${ic("beacon")} Test build</b><span>This is an early test version of Lodestone Beacon for testers. Expect rough edges and missing pieces, and things may change or need resetting between builds. Messages, files, calls, cameras and screen shares are end-to-end encrypted, but please don't rely on it for anything important yet. If something breaks or feels wrong, tell whoever invited you.</span></div>
    <h1>Welcome to Beacon</h1>
    <p class="muted">Chat, voice calls, cameras and screen sharing with your friends, in communities on servers you or they run. Everything is encrypted on your PC; the server only passes it on.</p>
    <div class="home-cards">
      <section class="card">
        <h3>Join a community</h3>
        <p class="muted">Paste the invite link a friend sent you.</p>
        <form id="home-join" class="stack">
          <input name="link" placeholder="beacon.example.com/join/…" autocomplete="off" spellcheck="false" required>
          <input name="username" placeholder="Your username (first time on that server)" autocomplete="off" spellcheck="false">
          <button class="primary">Join</button>
        </form>
      </section>
      <section class="card">
        <h3>Create a community</h3>
        ${servers.length ? `<p class="muted">Make your own community with text and voice channels, then invite people.</p>
          <form id="home-create" class="stack">
            <input name="name" maxlength="64" placeholder="Community name, e.g. Friday raids" required>
            ${servers.length > 1 ? `<select name="server">${servers.map((h) => `<option value="${esc(h.url)}">on ${esc(h.url.replace(/^https?:\/\//, ""))}</option>`).join("")}</select>` : `<input type="hidden" name="server" value="${esc(servers[0].url)}"><small class="muted">on ${esc(servers[0].url.replace(/^https?:\/\//, ""))}</small>`}
            <button class="primary">Create</button>
          </form>` : `<p class="muted">Communities live on a server. Join one with an invite first, or set up your own server below.</p>`}
      </section>
    </div>
    <div class="home-more">
      <button type="button" class="ghost" data-act="pair">Sign in with another device</button>
      <button type="button" class="ghost" data-act="setup">Set up your own server</button>
    </div>
    ${(state.pairing || []).map((p) => `<p class="muted">Waiting for approval as <b>${esc(p.username)}</b> on ${esc(p.url.replace(/^https?:\/\//, ""))}… Your other device should show <code>${esc(p.fingerprint)}</code>.</p>`).join("")}
    <h3>Getting around</h3>
    <ul class="tips">
      <li><b>Communities</b> are on the left; <b>right-click</b> one to invite people, open its settings or leave it.</li>
      <li><b>Voice channels</b> (${ic("volume")}) are calls: click one to join, and click it again to see everyone.</li>
      <li>In a call, the bar at the bottom left has <b>mute</b>, <b>deafen</b>, <b>camera</b> ${ic("camera")}, <b>screen share</b> ${ic("screen")} and <b>leave</b>. ${ic("settings")} picks your microphone, speakers and camera.</li>
      <li>Someone sharing their screen shows <span class="live">LIVE</span>: click it to watch.</li>
      <li>Your status, profile, look and devices are under your name at the bottom left.</li>
    </ul>
  </div>`;
  $("#home-join").onsubmit = async (e) => {
    e.preventDefault();
    const f = new FormData(e.target);
    try {
      const r = await api("/api/join", { link: f.get("link"), username: f.get("username") || null });
      state.home = false;
      state.sel = { server: r.server, community: r.community, channel: "" };
      await loadState();
    } catch (err) { toast(err.message, true); }
  };
  const create = $("#home-create");
  if (create) create.onsubmit = async (e) => {
    e.preventDefault();
    const f = new FormData(e.target);
    try {
      const server = f.get("server");
      const r = await api("/api/communities", { server, name: f.get("name") });
      state.home = false;
      state.sel = { server, community: r.community, channel: "" };
      await loadState();
      toast("Community created. Invite people from the left, or right-click its icon.");
    } catch (err) { toast(err.message, true); }
  };
}

function initials(name) {
  return name.split(/\s+/).filter(Boolean).slice(0, 2).map((w) => [...w][0]).join("").toUpperCase() || "?";
}

async function loadMessages() {
  const { ch } = current();
  if (!ch || !ch.ready) return;
  const box = $("#msgs");
  try {
    const r = await api(`/api/messages?server=${encodeURIComponent(state.sel.server)}&channel=${encodeURIComponent(ch.id)}`);
    if (state.sel.channel !== ch.id || !box.isConnected) return;
    const atBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 40;
    const reacts = new Map();
    for (const x of r.reactions || []) { if (!reacts.has(x.seq)) reacts.set(x.seq, []); reacts.get(x.seq).push(x); }
    const makeRow = (m) => {
      const row = document.createElement("div");
      row.className = "msg" + (isOnlyEmoji(m.body) ? " big-emoji" : "");
      row.dataset.seq = m.seq;
      const time = document.createElement("time");
      time.textContent = new Date(m.sent_at * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      const who = document.createElement("span");
      const prof = profileOf(m.sender);
      who.className = "who" + (m.sender === r.me ? " me" : "") + (m.sender.startsWith("⚠") ? " warn" : "");
      who.textContent = prof.name || m.sender;
      if (prof.color && !m.sender.startsWith("⚠")) who.style.color = prof.color;
      who.title = "@" + m.sender;
      row.append(avatarEl(m.sender));
      if (prof.name && prof.name !== m.sender) { const at = document.createElement("small"); at.className = "at"; at.textContent = "@" + m.sender; who.append(at); }
      if (m.sender.startsWith("⚠")) who.title = "This sender's encryption key doesn't match what the server lists for them";
      const body = document.createElement("span");
      body.className = "body";
      body.append(withMentions(m.body, r.me));
      if (r.me && mentionsMe(m.body, r.me) && m.sender !== r.me) row.classList.add("mentions-me");
      const add = document.createElement("button");
      add.className = "react-add";
      add.type = "button";
      add.title = "Add a reaction";
      add.textContent = "+";
      add.dataset.reactAdd = m.seq;
      row.append(time, who, body, add);
      if (m.attachment) row.append(attachmentEl(m));
      const list = reacts.get(m.seq);
      if (list) {
        const chips = document.createElement("div");
        chips.className = "chips";
        for (const x of list) {
          const c = document.createElement("button");
          c.type = "button";
          const mine = x.users.includes(r.me);
          c.className = "chip" + (mine ? " mine" : "");
          c.dataset.react = m.seq;
          c.dataset.emoji = x.emoji;
          c.dataset.on = mine ? "0" : "1";
          c.title = x.users.join(", ");
          c.textContent = `${x.emoji} ${x.users.length}`;
          chips.append(c);
        }
        row.append(chips);
      }
      return row;
    };
    // Rows are kept and updated in place: only new messages are added, and a row is redrawn only when its
    // reactions change. Rebuilding everything made pictures reload (and the list jump while you read).
    box.querySelector(":scope > p.muted")?.remove();
    const existing = new Map([...box.querySelectorAll(":scope > .msg")].map((x) => [x.dataset.seq, x]));
    let prev = null, added = 0;
    for (const m of r.messages) {
      const key = JSON.stringify(reacts.get(m.seq) || []);
      let row = existing.get(String(m.seq));
      if (!row) added++;
      if (!row || row.dataset.key !== key) {
        const fresh = makeRow(m);
        fresh.dataset.key = key;
        if (row) row.replaceWith(fresh);
        row = fresh;
      }
      const want = prev ? prev.nextElementSibling : box.firstElementChild;
      if (want !== row) box.insertBefore(row, want);
      existing.delete(String(m.seq));
      prev = row;
    }
    for (const gone of existing.values()) gone.remove();
    if (!r.messages.length) box.innerHTML = '<p class="muted">No messages yet. Messages sent before you joined can\'t be read on this device, by design.</p>';
    const first = !box.dataset.seen;
    if (atBottom || first) { box.scrollTop = box.scrollHeight; box.dataset.stick = "1"; }
    else if (added) $("#new-msgs")?.classList.remove("hidden");
    box.dataset.seen = "1";
  } catch (e) { toast(e.message, true); }
}

// ---------- members and status ----------
const STATUS_WORDS = { online: "Online", away: "Away", busy: "Busy", invisible: "Invisible", offline: "Offline" };
function myServer() { return state.servers.find((h) => h.url === state.sel.server) || state.servers[0] || {}; }
function meBar() {
  const h = myServer();
  const st = h.status || "online";
  const dot = st === "invisible" ? "offline" : st;
  return `<button class="me-bar" id="me-bar" title="Set your status"><span class="dot ${dot}"></span><span class="grow"><b>${esc(h.username || "")}</b><small>${esc(h.status_text || STATUS_WORDS[st])}</small></span>▾</button>`;
}
function renderMembers(sp) {
  const el = $("#members");
  if (!sp || !state.showMembers) { el.classList.add("hidden"); return; }
  el.classList.remove("hidden");
  const rank = { owner: 0, admin: 1, member: 2 };
  const sorted = [...sp.members].sort((a, b) => rank[a.role] - rank[b.role] || a.username.localeCompare(b.username));
  const on = sorted.filter((m) => m.presence !== "offline"), off = sorted.filter((m) => m.presence === "offline");
  const row = (m) => `<div class="member ${m.presence}" title="${esc(STATUS_WORDS[m.presence] || m.presence)}${m.status_text ? " · " + esc(m.status_text) : ""}">
      <span class="av-wrap" data-avatar-user="${esc(m.username)}">${avatarHTML(m.username)}<span class="dot ${esc(m.presence)}"></span></span><span class="grow"><span class="mline"><span class="mname" ${profileOf(m.username).color ? `style="color:${esc(profileOf(m.username).color)}"` : ""}>${esc(profileOf(m.username).name || m.username)}</span>${m.role === "owner" ? '<span class="badge" title="Owner">' + ic("crown") + '</span>' : m.role === "admin" ? '<span class="badge" title="Admin">' + ic("shield") + '</span>' : ""}</span>
      <small>${profileOf(m.username).name && profileOf(m.username).name !== m.username ? "@" + esc(m.username) + (m.status_text ? " · " : "") : ""}${esc(m.status_text || "")}</small>${profileOf(m.username).playing && m.presence !== "offline" ? `<small class="playing" title="Playing ${esc(profileOf(m.username).playing)}">${ic("gamepad")} ${esc(profileOf(m.username).playing)}</small>` : ""}</span></div>`;
  el.innerHTML = `<h4>Online — ${on.length}</h4>${on.map(row).join("")}<h4>Offline — ${off.length}</h4>${off.map(row).join("")}`;
}
document.addEventListener("click", (e) => {
  const bar = e.target.closest("#me-bar");
  const menu = $("#status-menu");
  if (bar) {
    const r = bar.getBoundingClientRect();
    menu.classList.remove("hidden");
    menu.style.left = r.left + "px";
    menu.style.top = Math.max(8, r.top - menu.offsetHeight - 6) + "px";
    return;
  }
  if (!e.target.closest("#status-menu")) menu.classList.add("hidden");
});
async function setStatus(status, text) {
  try { await api("/api/status", { status, text }); await loadState(); } catch (e) { toast(e.message, true); }
}
$("#status-menu").onclick = (e) => {
  const b = e.target.closest("button");
  if (!b) return;
  $("#status-menu").classList.add("hidden");
  const h = myServer();
  autoAway = false;
  if (b.dataset.status) return setStatus(b.dataset.status, h.status_text || "");
  if (b.hasAttribute("data-profile")) return profileDialog();
  if (b.hasAttribute("data-appearance")) return appearanceDialog();
  if (b.hasAttribute("data-devices")) return devicesDialog();
  if (b.hasAttribute("data-sound")) return soundDialog();
  ask("Status message", `<label>What are you up to? <span class="muted">(up to 80 characters; empty to clear)</span><input name="text" maxlength="80" value="${esc(h.status_text || "")}"></label>`, "Save", (f) => setStatus(h.status || "online", String(f.get("text") || "")));
};
// Away by itself after 15 minutes without using Beacon, and back when you return (only from Online).
let lastActive = Date.now(), autoAway = false;
for (const ev of ["mousemove", "keydown", "mousedown", "wheel", "focus"]) addEventListener(ev, () => {
  lastActive = Date.now();
  if (autoAway) { autoAway = false; setStatus("online", myServer().status_text || ""); }
}, { passive: true });
setInterval(() => {
  if (!autoAway && myServer().status === "online" && Date.now() - lastActive > 15 * 60 * 1000) { autoAway = true; setStatus("away", myServer().status_text || ""); }
}, 30000);

// ---------- profiles ----------
async function loadProfiles(server) {
  try {
    const r = await api(`/api/profiles?server=${encodeURIComponent(server)}`);
    const map = {};
    for (const p of r.profiles) map[p.user] = p;
    if (r.me.user) map[r.me.user] = { ...(map[r.me.user] || {}), user: r.me.user, name: r.me.name, color: r.me.color, has_avatar: r.me.has_avatar, playing: r.me.playing };
    state.profiles[server] = map;
  } catch {}
}
function profileOf(user, server = state.sel.server) {
  return (state.profiles[server] || {})[user] || {};
}
const avatars = new Map();
function avatarKey(user, server = state.sel.server) { return `${server}|${user}`; }
function avatarHTML(user) {
  const p = profileOf(user);
  const url = avatars.get(avatarKey(user));
  if (p.has_avatar && url) return `<img class="av" src="${esc(url)}" alt="">`;
  if (p.has_avatar && !avatars.has(avatarKey(user))) loadAvatar(user);
  const letter = [...(p.name || user || "?")][0].toUpperCase();
  return `<span class="av av-letter" style="${p.color ? `background:${esc(p.color)}` : ""}">${esc(letter)}</span>`;
}
function avatarEl(user) {
  const span = document.createElement("span");
  span.className = "av-wrap";
  span.dataset.avatarUser = user;
  span.innerHTML = avatarHTML(user);
  return span;
}
function loadAvatar(user) {
  const server = state.sel.server, key = avatarKey(user, server);
  avatars.set(key, "");
  fetch(`/api/avatar?server=${encodeURIComponent(server)}&user=${encodeURIComponent(user)}`, { headers: { "x-beacon-token": TOKEN } })
    .then((r) => (r.ok ? r.blob() : null))
    .then((b) => {
      if (!b) return;
      avatars.set(key, URL.createObjectURL(b));
      // Fill in every place this person's picture is shown (keeps any status dot beside it).
      for (const el of document.querySelectorAll(`[data-avatar-user="${CSS.escape(user)}"]`)) {
        const old = el.querySelector(".av");
        if (old) old.outerHTML = avatarHTML(user);
      }
    });
}
function forgetAvatars(server) {
  for (const [k, u] of avatars) if (k.startsWith(server + "|")) { if (u) URL.revokeObjectURL(u); avatars.delete(k); }
}

function profileDialog() {
  const h = myServer();
  const me = profileOf(h.username, h.url);
  let pic; // undefined = keep, "" = remove, base64 = new
  ask("Your profile", `<div class="row" style="gap:14px;align-items:flex-start">
      <div class="icon-preview" id="pf-preview">${avatarHTML(h.username)}</div>
      <div style="flex:1">
        <label>Display name <span class="muted">(up to 32 characters; empty shows @${esc(h.username)})</span><input name="name" maxlength="32" value="${esc(me.name || "")}"></label>
        <label>Name colour</label>
        <div class="row"><input type="color" name="color" value="${esc(me.color || "#7c5cff")}" style="width:60px;padding:2px"><label class="row" style="margin:0"><input type="checkbox" name="nocolor" ${me.color ? "" : "checked"} style="width:auto"> Default colour</label></div>
        <div class="row" style="margin-top:10px"><button type="button" id="pf-pic">Choose picture…</button><button type="button" class="ghost" id="pf-rm">Remove picture</button></div>
      </div></div>
      <p class="muted">Your profile is sent end-to-end encrypted to the people in your communities; the server can't see it. Others always see your @username beside your display name.</p>`,
    "Save", async (f) => {
      const body = { name: String(f.get("name") || ""), color: f.get("nocolor") ? "" : String(f.get("color") || "") };
      if (pic !== undefined) body.avatar = pic;
      await api("/api/profile", body);
      forgetAvatars(h.url);
      await loadProfiles(h.url);
      render();
    });
  $("#pf-pic").onclick = () => $("#avatar-file").click();
  $("#pf-rm").onclick = () => { pic = ""; $("#pf-preview").innerHTML = `<span class="av av-letter">${esc([...(h.username || "?")][0].toUpperCase())}</span>`; };
  $("#avatar-file").onchange = async (e) => {
    const file = e.target.files[0];
    e.target.value = "";
    if (!file) return;
    if (file.size > 256 * 1024) return toast("Profile pictures can be at most 256 KiB", true);
    pic = await fileBase64(file);
    $("#pf-preview").innerHTML = `<img class="av" src="${esc(URL.createObjectURL(file))}" alt="">`;
  };
}

// ---------- devices and pairing ----------
let pairPoll = null;
async function devicesDialog() {
  const h = myServer();
  let d;
  try { d = await api(`/api/devices?server=${encodeURIComponent(h.url)}`); } catch (e) { return toast(e.message, true); }
  const list = d.devices.map((x) => `<div class="set-row"><div class="grow">${x.this ? "<b>This device</b>" : "Another device"}<br><small><code>${esc(x.fingerprint)}</code></small></div>
      ${x.this ? "" : `<button type="button" class="danger" data-rm="${esc(x.key)}">Remove</button>`}</div>`).join("");
  ask("Your devices", `<p class="muted">Each device has its own key. Removing one takes it out of every channel: it can't read anything new.</p>${list}
      <div id="pair-box"><button type="button" class="primary" id="pair-start">Add a device…</button></div>`, null, null);
  $("#dlg-body").onclick = async (e) => {
    const rm = e.target.closest("[data-rm]");
    if (rm) {
      if (!(await confirmBox("Remove this device? It will no longer be able to read new messages.", "Remove device"))) return;
      try { await api("/api/devices/remove", { server: h.url, device_key: rm.dataset.rm }); toast("Device removed"); } catch (err) { toast(err.message, true); }
      return;
    }
    if (e.target.closest("#pair-start")) startPairing(h);
    const dec = e.target.closest("[data-decide]");
    if (dec) {
      try {
        await api("/api/pair/decide", { server: h.url, device_key: dec.dataset.key, approve: dec.dataset.decide === "yes" });
        toast(dec.dataset.decide === "yes" ? "Device added. It joins your channels in a few seconds." : "Device refused");
        clearInterval(pairPoll);
        $("#dlg").close();
      } catch (err) { toast(err.message, true); }
    }
  };
  $("#dlg").addEventListener("close", () => clearInterval(pairPoll), { once: true });
}
async function startPairing(h) {
  let p;
  try { p = await api("/api/pair/start", { server: h.url }); } catch (e) { return toast(e.message, true); }
  const box = $("#pair-box");
  const show = (waiting) => {
    box.innerHTML = `<div class="pair-code"><p>On the new device choose <b>Sign in with another device</b> and enter:</p>
      <p>Server <code>${esc(p.server)}</code> · Code <code class="big">${esc(p.code)}</code></p>
      <p class="muted">Valid for 10 minutes, once.</p>
      ${waiting.length ? waiting.map((w) => `<div class="set-row"><div class="grow">A device is asking to join. Approve only if it shows<br><code class="big">${esc(w.fingerprint)}</code></div>
        <button type="button" class="primary" data-decide="yes" data-key="${esc(w.key)}">Approve</button><button type="button" class="danger" data-decide="no" data-key="${esc(w.key)}">Refuse</button></div>`).join("") : '<p class="muted">Waiting for the new device…</p>'}</div>`;
  };
  show([]);
  clearInterval(pairPoll);
  let shown = "";
  pairPoll = setInterval(async () => {
    try {
      const d = await api(`/api/devices?server=${encodeURIComponent(h.url)}`);
      // Redraw only when the waiting list changes, so the Approve button doesn't move under the pointer.
      const key = d.waiting.map((w) => w.key).join(",");
      if (key !== shown) { shown = key; show(d.waiting); }
    } catch {}
  }, 3000);
}
function pairDialog() {
  ask("Sign in with another device", `<p class="muted">On a device where you're already signed in, open your name → <b>Devices…</b> → <b>Add a device</b>, then enter what it shows.</p>
    <label>Server's domain<input name="server" placeholder="beacon.yourdomain.com" autocomplete="off" spellcheck="false" required></label>
    <label>Code<input name="code" autocomplete="off" spellcheck="false" required></label>`, "Continue", async (f) => {
      const r = await api("/api/pair/claim", { server: String(f.get("server")), code: String(f.get("code")) });
      await loadState();
      waitForApproval(r);
    });
}
function waitForApproval(r) {
  ask("Waiting for approval", `<p>Signing in as <b>${esc(r.username)}</b>. On your other device, approve the request only if it shows:</p><p><code class="big">${esc(r.fingerprint)}</code></p><p class="muted" id="pair-status">Waiting…</p>`, null, null);
  clearInterval(pairPoll);
  pairPoll = setInterval(async () => {
    try {
      const s = await api("/api/pair/check", { server: r.server });
      if (s.status === "approved") { clearInterval(pairPoll); $("#dlg").close(); toast("Signed in. Your channels appear as your other device adds this one."); loadState(); }
      if (s.status === "refused") { clearInterval(pairPoll); $("#pair-status").textContent = "The request was refused or the code expired."; }
    } catch (e) { $("#pair-status").textContent = e.message; }
  }, 3000);
  $("#dlg").addEventListener("close", () => clearInterval(pairPoll), { once: true });
}

// ---------- appearance ----------
const THEMES = [["lodestone", "Lodestone (follows Lodestone's colours)"], ["midnight", "Midnight"], ["light", "Light"], ["contrast", "High contrast"]];
function applyPrefs() {
  document.documentElement.dataset.theme = state.prefs.theme;
  // Lodestone's colours are set inline (they'd beat any theme), so clear them for other themes.
  if (state.prefs.theme !== "lodestone") for (const k of ["--bg", "--bg-2", "--bg-3", "--line", "--text", "--muted", "--danger"]) document.documentElement.style.removeProperty(k);
  document.documentElement.style.removeProperty("--accent");
  if (state.prefs.accent) document.documentElement.style.setProperty("--accent", state.prefs.accent);
}
async function loadPrefs() {
  try { state.prefs = await api("/api/prefs"); } catch {}
  applyPrefs();
}
function appearanceDialog() {
  ask("Appearance", `<label>Theme<select name="theme">${THEMES.map(([k, l]) => `<option value="${k}" ${state.prefs.theme === k ? "selected" : ""}>${esc(l)}</option>`).join("")}</select></label>
    <label>Accent colour</label>
    <div class="row"><input type="color" name="accent" value="${esc(state.prefs.accent || "#7c5cff")}" style="width:60px;padding:2px"><label class="row" style="margin:0"><input type="checkbox" name="noaccent" ${state.prefs.accent ? "" : "checked"} style="width:auto"> Theme's own accent</label></div>
    <p class="muted">Saved on this PC.</p>`, "Save", async (f) => {
      const prefs = { theme: String(f.get("theme")), accent: f.get("noaccent") ? "" : String(f.get("accent") || "") };
      await api("/api/prefs", prefs);
      state.prefs = prefs;
      if (prefs.theme === "lodestone") for (const k of ["--bg", "--bg-2", "--bg-3", "--line", "--text", "--muted"]) document.documentElement.style.removeProperty(k);
      applyPrefs();
      if (prefs.theme === "lodestone") loadState();
    });
}

// ---------- attachments ----------
const MAX_FILE = 10 * 1024 * 1024;
const fmtSize = (n) => (n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(0)} KB` : `${(n / 1048576).toFixed(1)} MB`);
const fileBase64 = (f) => new Promise((res, rej) => { const r = new FileReader(); r.onload = () => res(String(r.result).split(",")[1] || ""); r.onerror = () => rej(new Error("Couldn't read the file")); r.readAsDataURL(f); });

function setPending(f) {
  if (f && f.size > MAX_FILE) return toast(`Files can be at most ${fmtSize(MAX_FILE)}`, true);
  state.pending = f || null;
  showPending();
}
function showPending() {
  const el = $("#pending");
  if (!el) return;
  const f = state.pending;
  el.classList.toggle("hidden", !f);
  if (!f) return (el.innerHTML = "");
  el.innerHTML = `<span>📎 ${esc(f.name || "pasted image")} <small class="muted">${fmtSize(f.size)} · press Enter to send, with your message as its caption</small></span><button type="button" class="ghost" id="pending-x" title="Remove">${ic("close")}</button>`;
  $("#pending-x").onclick = () => setPending(null);
  $("#text")?.focus();
}
$("#attach-file").onchange = (e) => { const f = e.target.files[0]; e.target.value = ""; if (f) setPending(f); };
document.addEventListener("dragover", (e) => { if ($("#text") && e.dataTransfer?.types?.includes("Files")) e.preventDefault(); });
document.addEventListener("drop", (e) => {
  const f = [...(e.dataTransfer?.files || [])][0];
  if (f && $("#text")) { e.preventDefault(); setPending(f); }
});

// Decrypted files come from the plug-in with the page's secret, so they're fetched and shown as blobs.
const fileUrls = new Map();
async function fileUrl(seq) {
  const key = `${state.sel.server}|${state.sel.channel}|${seq}`;
  if (fileUrls.has(key)) return fileUrls.get(key);
  const r = await fetch(`/api/file?server=${encodeURIComponent(state.sel.server)}&channel=${encodeURIComponent(state.sel.channel)}&seq=${seq}`, { headers: { "x-beacon-token": TOKEN } });
  if (!r.ok) throw new Error((await r.json().catch(() => ({}))).error || "Couldn't open the file");
  const url = URL.createObjectURL(await r.blob());
  fileUrls.set(key, url);
  return url;
}
function attachmentEl(m) {
  const a = m.attachment;
  const wrap = document.createElement("div");
  wrap.className = "attach";
  if (String(a.mime).startsWith("image/")) {
    const img = document.createElement("img");
    img.alt = a.name;
    img.className = "attach-img";
    img.title = `${a.name} · ${fmtSize(a.size)} · click to enlarge`;
    fileUrl(m.seq).then((u) => (img.src = u)).catch((e) => { wrap.textContent = e.message; });
    img.onclick = () => { $("#lightbox-img").src = img.src; $("#lightbox").classList.remove("hidden"); };
    wrap.append(img);
  } else {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "attach-file";
    b.textContent = `${a.name} · ${fmtSize(a.size)} · Save`;
    b.onclick = async () => {
      try {
        const link = document.createElement("a");
        link.href = await fileUrl(m.seq);
        link.download = a.name;
        link.click();
      } catch (e) { toast(e.message, true); }
    };
    wrap.append(b);
  }
  return wrap;
}
$("#lightbox").onclick = () => $("#lightbox").classList.add("hidden");

// ---------- emoji ----------
const isOnlyEmoji = (t) => /^(\p{Extended_Pictographic}|\p{Emoji_Component}|\u200d|\ufe0f|\s){1,24}$/u.test(t) && /\p{Extended_Pictographic}/u.test(t) && [...t.replace(/\s/g, "")].length <= 16;

function insertAtCursor(el, text) {
  const [a, b] = [el.selectionStart ?? el.value.length, el.selectionEnd ?? el.value.length];
  el.value = el.value.slice(0, a) + text + el.value.slice(b);
  el.selectionStart = el.selectionEnd = a + text.length;
  el.focus();
}

const QUICK = ["👍", "❤️", "😂", "😮", "😢", "🎉", "🔥", "👀"];
let pick = null;
function openPicker(anchor, onPick, quick) {
  pick = onPick;
  const p = $("#picker");
  $("#picker-search").value = "";
  $("#picker-tabs").innerHTML = (quick ? `<button type="button" data-g="-1" title="Quick">⭐</button>` : "")
    + EMOJI.map((g, i) => `<button type="button" data-g="${i}" title="${esc(g[0])}">${g[1][0][0]}</button>`).join("");
  showGroup(quick ? -1 : 0);
  p.classList.remove("hidden");
  const r = anchor.getBoundingClientRect();
  const w = p.offsetWidth, h = p.offsetHeight;
  p.style.left = Math.max(8, Math.min(r.right - w, innerWidth - w - 8)) + "px";
  p.style.top = (r.top - h - 6 > 8 ? r.top - h - 6 : Math.min(r.bottom + 6, innerHeight - h - 8)) + "px";
  $("#picker-search").focus();
}
function closePicker() { $("#picker").classList.add("hidden"); pick = null; }
function emojiButtons(list) {
  return list.map(([e, n]) => `<button type="button" data-emo="${esc(e)}" title="${esc(n)}">${e}</button>`).join("");
}
function showGroup(i) {
  for (const b of $("#picker-tabs").children) b.classList.toggle("on", b.dataset.g === String(i));
  $("#picker-grid").innerHTML = i < 0 ? emojiButtons(QUICK.map((e) => [e, ""])) : emojiButtons(EMOJI[i][1]);
  $("#picker-grid").scrollTop = 0;
}
$("#picker-tabs").onclick = (e) => { const b = e.target.closest("[data-g]"); if (b) showGroup(Number(b.dataset.g)); };
$("#picker-search").oninput = (e) => {
  const q = e.target.value.trim().toLowerCase();
  if (!q) return showGroup(0);
  for (const b of $("#picker-tabs").children) b.classList.remove("on");
  const hits = EMOJI.flatMap((g) => g[1]).filter(([, n]) => n.includes(q)).slice(0, 200);
  $("#picker-grid").innerHTML = hits.length ? emojiButtons(hits) : '<p class="muted">No emoji found</p>';
};
$("#picker-search").onkeydown = (e) => {
  if (e.key === "Escape") closePicker();
  if (e.key === "Enter") { e.preventDefault(); const b = $("#picker-grid [data-emo]"); if (b) b.click(); }
};
$("#picker-grid").onclick = (e) => {
  const b = e.target.closest("[data-emo]");
  if (!b || !pick) return;
  const f = pick;
  closePicker();
  f(b.dataset.emo);
};
document.addEventListener("mousedown", (e) => {
  if (pick && !e.target.closest("#picker") && !e.target.closest("#emoji-btn") && !e.target.closest("[data-react-add]")) closePicker();
});
document.addEventListener("keydown", (e) => { if (e.key === "Escape" && pick) closePicker(); });

async function setReaction(seq, emoji, on) {
  try { await api("/api/react", { server: state.sel.server, channel: state.sel.channel, seq: Number(seq), emoji, on }); loadMessages(); }
  catch (err) { toast(err.message, true); }
}
document.addEventListener("click", (e) => {
  const add = e.target.closest("[data-react-add]");
  if (add) return openPicker(add, (emo) => setReaction(add.dataset.reactAdd, emo, true), true);
  const chip = e.target.closest("[data-react]");
  if (chip) setReaction(chip.dataset.react, chip.dataset.emoji, chip.dataset.on === "1");
});

// ---------- community icons ----------
// Icons come from the server through the plug-in (the page can't send its secret with a plain <img>),
// so they're fetched once and shown from a local blob.
const icons = new Map();
function iconHTML(s) {
  const key = s.server + "|" + s.id;
  const url = icons.get(key);
  if (url) return `<img src="${esc(url)}" alt="">`;
  if (s.has_icon && !icons.has(key)) {
    icons.set(key, "");
    fetch(`/api/icon?server=${encodeURIComponent(s.server)}&community=${encodeURIComponent(s.id)}`, { headers: { "x-beacon-token": TOKEN } })
      .then((r) => (r.ok ? r.blob() : null))
      .then((b) => { if (b) { icons.set(key, URL.createObjectURL(b)); render(); } });
  }
  return esc(initials(s.name));
}
function forgetIcon(s) {
  const key = s.server + "|" + s.id;
  const url = icons.get(key);
  if (url) URL.revokeObjectURL(url);
  icons.delete(key);
}

// ---------- community settings ----------
/** A confirmation using Beacon's own dialog (native confirm() isn't reliable inside Lodestone's frame).
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

const manage = (method, path, body) => api("/api/manage", { server: state.sel.server, method, path, body });
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
    <p class="muted" style="margin-top:18px">Community names, descriptions, icons, channel names and topics are visible to the server. Messages are not.</p>
    <div class="row end" style="margin-top:18px">
      ${owner ? '<button type="button" class="danger" id="s-delete">Delete community…</button>' : '<button type="button" class="danger" id="s-leave">Leave community</button>'}
    </div>`;
  if (admin) {
    $("#s-save").onclick = () => after("overview", () => manage("PATCH", `/v1/communities/${sp.id}`, { name: $("#s-name").value, description: $("#s-desc").value }));
    $("#s-icon").onclick = () => $("#icon-file").click();
    $("#icon-file").onchange = async (e) => {
      const f = e.target.files[0];
      e.target.value = "";
      if (!f) return;
      if (f.size > 256 * 1024) return toast("Icons can be at most 256 KiB", true);
      const data = await new Promise((res) => { const r = new FileReader(); r.onload = () => res(String(r.result).split(",")[1]); r.readAsDataURL(f); });
      forgetIcon(sp);
      after("overview", () => api("/api/icon", { server: sp.server, community: sp.id, data }));
    };
    const rm = $("#s-icon-rm");
    if (rm) rm.onclick = () => { forgetIcon(sp); after("overview", () => manage("DELETE", `/v1/communities/${sp.id}/icon`)); };
  }
  const del = $("#s-delete");
  if (del) del.onclick = () => deleteServer(sp);
  const leave = $("#s-leave");
  if (leave) leave.onclick = () => leaveServer(sp);
}

// Leaving or deleting a community, from its settings or its right-click menu.
async function leaveServer(sp) {
  if (sp.your_role === "owner") return toast("You own this community: make someone else the owner first (Community settings → Members), or delete it.", true);
  if (!(await confirmBox(`Leave "${sp.name}"? You'll need a new invite to come back.`, "Leave community"))) return;
  await endServer(sp, "DELETE", `/v1/communities/${sp.id}/members/${encodeURIComponent(sp.me)}`, `You left ${sp.name}.`);
}
async function deleteServer(sp) {
  if (!(await confirmBox(`This deletes "${sp.name}", its channels and all their messages for everyone.`, "Delete community", sp.name))) return;
  await endServer(sp, "DELETE", `/v1/communities/${sp.id}`, `${sp.name} was deleted.`);
}
async function endServer(sp, method, path, done) {
  try {
    if (voice.call && voice.call.server === sp.server && sp.channels.some((c) => c.id === voice.call.channel)) await callAction("leave");
    await api("/api/manage", { server: sp.server, method, path });
    if ($("#settings").open) $("#settings").close();
    forgetIcon(sp);
    if (state.sel.community === sp.id) state.sel = { server: "", community: "", channel: "" };
    toast(done);
    await loadState();
  } catch (e) { toast(e.message, true); }
}

// Right-click a community for its menu.
document.addEventListener("contextmenu", (e) => {
  const b = e.target.closest(".srv[data-community]");
  const menu = $("#srv-menu");
  if (!b) { menu.classList.add("hidden"); return; }
  e.preventDefault();
  const sp = communities().find((s) => s.server === b.dataset.server && s.id === b.dataset.community);
  if (!sp) return;
  const owner = sp.your_role === "owner";
  menu.innerHTML = `<div class="menu-title">${esc(sp.name)}<small>${esc(sp.server.replace(/^https?:\/\//, ""))}</small></div>
    <button type="button" data-srv="invite">Invite people</button>
    <button type="button" data-srv="settings">Community settings</button>
    <hr>
    ${owner ? '<button type="button" class="danger-text" data-srv="delete">Delete community…</button><small class="menu-note">You own it: to leave, make someone else the owner first.</small>'
      : '<button type="button" class="danger-text" data-srv="leave">Leave community</button>'}`;
  menu.classList.remove("hidden");
  menu.style.left = Math.min(e.clientX, innerWidth - menu.offsetWidth - 8) + "px";
  menu.style.top = Math.min(e.clientY, innerHeight - menu.offsetHeight - 8) + "px";
  menu.onclick = async (ev) => {
    const act = ev.target.closest("[data-srv]")?.dataset.srv;
    if (!act) return;
    menu.classList.add("hidden");
    if (act === "leave") return leaveServer(sp);
    if (act === "delete") return deleteServer(sp);
    // Invite and settings act on the selected community: select this one first.
    state.sel = { server: sp.server, community: sp.id, channel: "" };
    await loadState();
    if (act === "invite") inviteDialog();
    else openSettings("overview");
  };
});
document.addEventListener("click", (e) => { if (!e.target.closest("#srv-menu")) $("#srv-menu").classList.add("hidden"); });
document.addEventListener("keydown", (e) => { if (e.key === "Escape") $("#srv-menu").classList.add("hidden"); });

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
    after("channels", () => manage("PUT", `/v1/communities/${sp.id}/channel-order`, { channels: order }));
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
    after("members", () => manage("PUT", `/v1/communities/${sp.id}/members/${encodeURIComponent(u)}/role`, { role: sel.value }));
  };
  $("#set-body").onclick = async (e) => {
    const b = e.target.closest("[data-m-act]");
    if (!b || b.tagName === "SELECT") return;
    const u = b.closest("[data-user]").dataset.user;
    const path = `/v1/communities/${sp.id}/members/${encodeURIComponent(u)}`;
    const act = b.dataset.mAct;
    if (act === "kick" && (await confirmBox(`Remove ${u} from the community? They can rejoin with an invite.`, "Remove"))) after("members", () => manage("DELETE", path));
    if (act === "ban" && (await confirmBox(`Ban ${u}? They're removed and can't rejoin until unbanned.`, "Ban"))) after("members", () => manage("DELETE", path + "?ban=true"));
    if (act === "transfer" && (await confirmBox(`Make ${u} the owner? You'll become an admin.`, "Make owner", u))) after("members", () => manage("POST", `/v1/communities/${sp.id}/transfer`, { username: u }));
  };
}

async function tabBans(sp) {
  const body = $("#set-body");
  body.innerHTML = '<p class="muted">Loading…</p>';
  try {
    const { bans } = await manage("GET", `/v1/communities/${sp.id}/bans`);
    body.innerHTML = bans.length ? bans.map((u) => `<div class="set-row" data-user="${esc(u)}"><div class="grow">${esc(u)}</div><button type="button" data-unban>Unban</button></div>`).join("") : '<p class="muted">Nobody is banned.</p>';
    body.onclick = (e) => {
      const b = e.target.closest("[data-unban]");
      if (b) after("bans", () => manage("DELETE", `/v1/communities/${sp.id}/bans/${encodeURIComponent(b.closest("[data-user]").dataset.user)}`));
    };
  } catch (e) { body.innerHTML = `<p class="err">${esc(e.message)}</p>`; }
}

async function tabInvites(sp) {
  const body = $("#set-body");
  body.innerHTML = '<p class="muted">Loading…</p>';
  try {
    const { invites } = await manage("GET", `/v1/communities/${sp.id}/invites`);
    body.innerHTML = `<p class="muted">The server keeps only a scrambled copy of each code, so links can't be shown again. Revoke ones you no longer want.</p>`
      + (invites.length ? invites.map((i) => `<div class="set-row" data-inv="${esc(i.id)}"><div class="grow">Invite <code>${esc(i.id.slice(0, 6))}</code> by ${esc(i.created_by)}<br><small>${i.uses} of ${i.max_uses} uses · expires ${esc(fmtDate(i.expires))}</small></div><button type="button" class="danger" data-revoke>Revoke</button></div>`).join("") : '<p class="muted">No active invites.</p>')
      + `<div class="row end"><button type="button" class="primary" data-act="invite">New invite</button></div>`;
    body.onclick = (e) => {
      const b = e.target.closest("[data-revoke]");
      if (b) after("invites", () => manage("DELETE", `/v1/communities/${sp.id}/invites/${b.closest("[data-inv]").dataset.inv}`));
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
  ask("Join a community", `<label>Invite link<input name="link" placeholder="beacon.yourdomain.com/join/…" autocomplete="off" spellcheck="false" required></label>
    <label>Your username on that server <span class="muted">(only the first time)</span><input name="username" autocomplete="off" spellcheck="false" placeholder="2 to 32: a-z 0-9 _ . -"></label>`,
    "Join", async (f) => {
      const r = await api("/api/join", { link: f.get("link"), username: f.get("username") || null });
      state.sel = { server: r.server, community: r.community, channel: "" };
      await loadState();
    });
}

function setupDialog() {
  ask("Set up a new server", `<p class="muted">After starting a Beacon server (see the README), its log shows a one-time setup code. The first account becomes the server's admin.</p>
    <label>Server's domain<input name="server" placeholder="beacon.yourdomain.com" autocomplete="off" spellcheck="false" required></label>
    <label>Setup code<input name="code" autocomplete="off" required></label>
    <label>Your username<input name="username" autocomplete="off" spellcheck="false" required></label>`,
    "Set up", async (f) => {
      const r = await api("/api/setup", { server: f.get("server"), code: f.get("code"), username: f.get("username") });
      await loadState();
      newServerDialog(r.server);
    });
}

function newServerDialog(server) {
  ask("Create a community", `<label>Name<input name="name" maxlength="64" required placeholder="Friday raids"></label>`, "Create", async (f) => {
    const r = await api("/api/communities", { server, name: f.get("name") });
    state.sel = { server, community: r.community, channel: "" };
    await loadState();
  });
}

function addDialog() {
  const servers = state.servers.map((h) => h.url);
  ask("Communities", `<div class="choices">
      <button type="button" class="primary" data-act="join">Join a community with an invite</button>
      ${servers.map((h) => `<button type="button" class="ghost" data-act="create" data-server="${esc(h)}">Create a community on <b>${esc(h.replace(/^https?:\/\//, ""))}</b></button>`).join("")}
      <button type="button" class="ghost" data-act="pair">Sign in with another device…</button>
      <button type="button" class="ghost" data-act="setup">Set up a new server…</button></div>`, null, null);
}

async function inviteDialog() {
  const { sp } = current();
  let link = "";
  try { link = (await api("/api/invites", { server: sp.server, community: sp.id, max_uses: 5 })).link; } catch (e) { return toast(e.message, true); }
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
  // Step 1: make the invite and show the message. Step 2 (a click): copy it, then open their chat.
  // Stores' chats can't be filled in by other apps, so you paste it there yourself.
  box.onclick = async (e) => {
    const go = e.target.closest("[data-send]");
    if (go) {
      const text = go.dataset.send;
      const copied = copyText(text);
      try { await api("/api/open-chat", { store: go.dataset.store, id: go.dataset.id, text }); } catch (err) { toast(err.message, true); }
      go.textContent = copied ? "Copied: paste it with Ctrl+V" : "Copy it from the box above";
      toast(go.dataset.store === "steam"
        ? (copied ? "Steam's chat is opening: click its message box and press Ctrl+V to paste the invite." : "Steam's chat is opening: copy the invite from Beacon and paste it there.")
        : "The invite is copied: paste it to your friend.", !copied);
      return;
    }
    const b = e.target.closest("[data-friend]");
    if (!b) return;
    b.disabled = true;
    try {
      const { text } = await api("/api/invite-friend", { server: sp.server, community: sp.id });
      const row = b.closest(".friend");
      const where = b.dataset.store === "steam" ? "Steam chat" : "chat";
      row.insertAdjacentHTML("afterend", `<div class="friend-send"><input readonly value="${esc(text)}">
        <button type="button" class="primary" data-send="${esc(text)}" data-store="${esc(b.dataset.store)}" data-id="${esc(b.dataset.friend)}">Copy and open ${where}</button>
        <small class="muted">Their chat can't be filled in by other apps: after it opens, paste with Ctrl+V.</small></div>`);
      b.textContent = "Ready";
    } catch (err) { b.disabled = false; toast(err.message, true); }
  };
}

function channelDialog() {
  const { sp } = current();
  ask("New channel", `<label>Name<input name="name" maxlength="64" required placeholder="raids"></label>
    <label>Kind<select name="kind"><option value="text">Text: messages, files, reactions</option><option value="voice">Voice: calls (end-to-end encrypted)</option></select></label>`, "Create", async (f) => {
    const kind = f.get("kind");
    const r = await api("/api/channels", { server: sp.server, community: sp.id, name: f.get("name"), kind });
    if (kind !== "voice") state.sel.channel = r.channel;
    await loadState();
  });
}

document.addEventListener("click", (e) => {
  const s = e.target.closest("[data-community]");
  if (s) { state.home = false; state.sel = { server: s.dataset.server, community: s.dataset.community, channel: "" }; return loadState(); }
  if (e.target.closest("#home-btn")) { state.home = true; voice.view = false; renderStage(); return render(); }
  const c = e.target.closest("[data-channel]");
  if (c) { state.sel.channel = c.dataset.channel; voice.view = false; clearMentions(state.sel.server, c.dataset.channel); renderStage(); return render(); }
  const v = e.target.closest("[data-voice]");
  if (v) return joinVoice(v.dataset.voice);
  const w = e.target.closest("[data-watch]");
  if (w) return watchShare(Number(w.dataset.watch), !voice.call?.watching.includes(Number(w.dataset.watch)));
  const cb = e.target.closest("[data-call]");
  if (cb) return callAction(cb.dataset.call);
  if (e.target.closest("#add-community")) return addDialog();
  const a = e.target.closest("[data-act]");
  if (!a) return;
  ({ join: joinDialog, pair: pairDialog, setup: setupDialog, invite: inviteDialog, channel: channelDialog, settings: () => openSettings("overview"), create: () => newServerDialog(a.dataset.server) })[a.dataset.act]?.();
});

// ---------- live updates ----------
async function listen() {
  for (;;) {
    try {
      const r = await fetch("/api/events", { headers: { "x-beacon-token": TOKEN } });
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
          if (ev.type === "state") { clearTimeout(listen.reload); listen.reload = setTimeout(loadState, 150); }
          else if (ev.type === "voice") setTimeout(pollVoice, 300);
          else if (ev.type === "game-started") gameStarted(ev.title);
          else if (ev.type === "mention") {
            const here = ev.server === state.sel.server && ev.channel === state.sel.channel && !voice.view && !state.home && document.visibilityState === "visible";
            if (here) clearMentions(ev.server, ev.channel);
            else { state.mentions[`${ev.server}|${ev.channel}`] = (state.mentions[`${ev.server}|${ev.channel}`] || 0) + 1; render(); toast(`${profileOf(ev.from, ev.server).name || ev.from} mentioned you`); }
          }
          else if (ev.type === "theme") applyTheme(ev.data);
          else if (ev.type === "profiles") { forgetAvatars(ev.server); loadProfiles(ev.server).then(render); }
          else if (ev.type === "messages" && ev.server === state.sel.server && ev.channels.includes(state.sel.channel)) loadMessages();
        }
      }
    } catch {}
    await new Promise((r) => setTimeout(r, 2000));
  }
}

// ---------- calls ----------
// One call at a time. The call's sound runs in the Beacon program, not in this page; the page shows who's
// there and who's speaking, and holds push-to-talk while it has focus.
const voice = { call: null, audio: null, timer: null, busy: false, ptt: false, view: false, camsOn: false };
async function pollVoice() {
  try {
    const v = await api("/api/voice");
    voice.call = v.call; voice.audio = v.audio;
  } catch { voice.call = null; }
  renderVoice();
  renderStage();
  soundMeter();
  clearTimeout(voice.timer);
  // Fast while in a call (speaking rings) or while the sound settings are open; otherwise not at all.
  if (voice.call || voice.audio?.testing || $("#sound-dlg")) voice.timer = setTimeout(pollVoice, voice.call ? 250 : 120);
}
function renderVoice() {
  const { sp } = current();
  const call = voice.call;
  for (const el of document.querySelectorAll("[data-vc]")) {
    const id = el.dataset.vc;
    const ch = sp?.channels.find((c) => c.id === id);
    const mine = call && call.channel === id && call.server === state.sel.server;
    const people = mine ? call.participants : (ch?.in_call || []).map((u) => ({ username: u }));
    setHTML(el, people.map((p) => {
      const live = p.sharing ? (mine && !p.me ? `<button type="button" class="live ${call.watching.includes(p.slot) ? "on" : ""}" data-watch="${p.slot}" title="${call.watching.includes(p.slot) ? "Stop watching" : "Watch their screen"}">LIVE</button>` : '<span class="live" title="Sharing their screen">LIVE</span>') : "";
      return `<div class="vc-person ${p.speaking ? "speaking" : ""}">${avatarHTML(p.username)}<span class="grow">${esc(profileOf(p.username).name || p.username)}</span>${live}${p.deafened ? '<span title="Deafened">' + ic("headphones-off") + '</span>' : p.muted ? '<span title="Muted">' + ic("mic-off") + '</span>' : ""}</div>`;
    }).join(""));
    el.previousElementSibling?.classList.toggle("on", !!mine);
  }
  const bar = $("#callbar");
  if (!bar) return;
  if (!call) { setHTML(bar, ""); return; }
  const ch = communities().find((x) => x.server === call.server)?.channels.find((c) => c.id === call.channel);
  const trouble = voice.audio?.error;
  setHTML(bar, `<div class="callbar">
    <div class="call-info"><b class="${call.connected ? "ok" : "wait"}">${call.connected ? "Voice connected" : "Connecting…"}</b><small title="${esc(call.server)}">${ic("volume")} ${esc(ch?.name || "call")}${call.sharing ? ` · sharing ${call.sharing.width}×${call.sharing.height}${call.sharing.sound ? " with sound" : ""}` : ""}${trouble ? ` · <span class="bad">${esc(trouble)}</span>` : ""}</small></div>
    <div class="call-btns">
      <button type="button" data-call="mute" class="${call.muted ? "active" : ""}" title="${call.muted ? "Unmute" : "Mute"}">${ic(call.muted ? "mic-off" : "mic")}</button>
      <button type="button" data-call="deafen" class="${call.deafened ? "active" : ""}" title="${call.deafened ? "Undeafen" : "Deafen (hear no one, and mute)"}">${ic(call.deafened ? "headphones-off" : "headphones")}</button>
      <button type="button" data-call="camera" class="${call.camera ? "active share-on" : ""}" title="${call.camera ? "Turn your camera off" : "Turn your camera on"}">${ic(call.camera ? "camera" : "camera-off")}</button>
      <button type="button" data-call="share" class="${call.sharing ? "active share-on" : ""}" title="${call.sharing ? "Stop sharing your screen" : "Share your screen"}">${ic("screen")}</button>
      <button type="button" data-call="sound" title="Sound and sharing settings">${ic("settings")}</button>
      <button type="button" data-call="leave" class="leave" title="Leave the call">${ic("call-end")}</button>
    </div></div>`);
}
// Redraw only when something changed: replacing a button between press and release loses the click.
function setHTML(el, html) {
  if (el._html === html) return;
  el._html = html;
  el.innerHTML = html;
}
async function joinVoice(id) {
  const { sp } = current();
  const ch = sp?.channels.find((c) => c.id === id);
  if (!ch) return;
  if (!ch.ready) return toast("This device doesn't have this channel's keys yet; it happens when another member is online.", true);
  if (voice.call && voice.call.channel === id && voice.call.server === sp.server) { voice.view = true; return renderStage(); }
  if (voice.busy) return;
  voice.busy = true;
  toast(`Joining ${ch.name}…`);
  try {
    const v = await api("/api/voice/join", { server: sp.server, channel: id });
    voice.call = v.call; voice.audio = v.audio; voice.view = true;
    toast(`In ${ch.name}. Calls are end-to-end encrypted.`);
  } catch (e) { toast(e.message, true); }
  voice.busy = false;
  pollVoice();
}
async function callAction(act) {
  const c = voice.call;
  if (act === "sound") return soundDialog();
  if (!c) return;
  try {
    if (act === "share") {
      if (c.sharing) await api("/api/voice/share", { on: false });
      else {
        // Windows: Beacon lists your screens and windows. Linux: the desktop's own dialog asks.
        const { sources } = await api("/api/voice/screens");
        const source = sources.length ? await pickScreen(sources, voice.preferWindow) : null;
        voice.preferWindow = null;
        if (sources.length && !source) return;
        if (!sources.length) toast("Your desktop will ask which screen or window to share…");
        await api("/api/voice/share", { on: true, source });
        toast("You're sharing. People in the call can choose to watch.");
      }
    }
    if (act === "camera") {
      if (!c.camera) toast("Starting your camera… (your desktop may ask to allow it)");
      await api("/api/voice/camera", { on: !c.camera });
      if (!c.camera) { voice.view = true; toast("Your camera is on. People see it with the call open."); }
    }
    if (act === "leave") {
      // Gone from the screen at once; the call closes in the background.
      voice.call = null; voice.view = false; voice.camsOn = false;
      renderVoice(); renderStage();
      await api("/api/voice/leave", {});
    }
    else if (act === "mute") await api("/api/voice/state", { muted: c.deafened ? false : !c.muted, deafened: false });
    else if (act === "deafen") await api("/api/voice/state", { muted: !c.deafened, deafened: !c.deafened });
  } catch (e) { toast(e.message, true); }
  pollVoice();
}

// A game started (Lodestone told Beacon): offer to share it if you're in a call, or share it at once.
function gameStarted(title) {
  const c = voice.call;
  const mode = voice.audio?.settings?.game_share || "ask";
  if (!c || c.sharing || mode === "off") return;
  voice.preferWindow = title;
  if (mode === "auto") { toast(`Sharing ${title}…`); return callAction("share"); }
  const bar = document.createElement("div");
  bar.className = "game-offer";
  bar.innerHTML = `<span>🎮 You started <b>${esc(title)}</b>. Share it with the call?</span><button type="button" class="primary" data-go>Share it</button><button type="button" class="ghost" data-no>Not now</button>`;
  document.body.appendChild(bar);
  const done = () => bar.remove();
  bar.querySelector("[data-go]").onclick = () => { done(); callAction("share"); };
  bar.querySelector("[data-no]").onclick = () => { done(); voice.preferWindow = null; };
  setTimeout(done, 30000);
}

// Choose a screen or window to share (Windows).
function pickScreen(sources, prefer) {
  return new Promise((done) => {
    let chosen = null;
    const group = (kind, title) => {
      const list = sources.filter((x) => x.kind === kind);
      return list.length ? `<h4 class="chhead">${title}</h4><div class="pick-list">${list.map((x) => `<label class="pick"><input type="radio" name="src" value="${esc(x.id)}">${esc(x.label)}</label>`).join("")}</div>` : "";
    };
    ask("Share your screen", `${group("monitor", "Screens")}${group("window", "Windows")}<p class="muted small">Windows shows a yellow border around what's being shared.</p>`, "Share", (f) => { chosen = f.get("src"); if (!chosen) throw new Error("Pick a screen or window"); });
    // A game you just started: its window is chosen for you if Beacon finds one with its name.
    const match = prefer && sources.find((x) => x.kind === "window" && x.label.toLowerCase().includes(prefer.toLowerCase()));
    const first = match ? $(`#dlg-body input[name=src][value="${CSS.escape(match.id)}"]`) : $("#dlg-body input[name=src]");
    if (first) first.checked = true;
    $("#dlg").addEventListener("close", () => done(chosen), { once: true });
  });
}

// Push-to-talk: hold the key while this page has focus. (A key that works everywhere needs the
// desktop's global-shortcuts portal; that comes later.)
const PTT_KEY = "`";
function setPtt(on) {
  if (voice.ptt === on) return;
  voice.ptt = on;
  api("/api/voice/ptt", { on }).catch(() => {});
}
addEventListener("keydown", (e) => {
  if (e.key !== PTT_KEY || !voice.call || voice.audio?.settings?.mode !== "ptt") return;
  if (e.target.closest?.("textarea, input")) return;
  e.preventDefault();
  setPtt(true);
});
addEventListener("keyup", (e) => { if (e.key === PTT_KEY) setPtt(false); });
addEventListener("blur", () => setPtt(false));

async function soundDialog() {
  try { voice.audio = (await api("/api/voice")).audio; } catch {}
  const s = voice.audio?.settings || { mic: "", speakers: "", mode: "voice", threshold: -45, volume: 100 };
  // The dialog opens at once; the device lists fill in when they arrive (never wait on the sound system).
  const opts = (list, kind, cur) => `<option value="">System default</option>` + list.filter((d) => d.kind === kind).map((d) => `<option value="${esc(d.name)}" ${d.name === cur ? "selected" : ""}>${esc(d.label)}</option>`).join("")
    + (cur && !list.some((d) => d.name === cur) ? `<option value="${esc(cur)}" selected>${list.length ? "Chosen device (not connected?)" : "Chosen device"}</option>` : "");
  ask("Sound, camera and sharing", `<div id="sound-dlg">
    <label>Microphone<select name="mic">${opts([], "microphone", s.mic)}</select></label>
    <label>Speakers<select name="speakers">${opts([], "speakers", s.speakers)}</select></label>
    <label>Speaker volume: <b id="vol-val">${Math.round(s.volume ?? 100)}%</b><input type="range" name="volume" min="0" max="200" step="5" value="${s.volume ?? 100}"></label>
    <label>Send my voice<select name="mode">
      <option value="voice" ${s.mode === "voice" ? "selected" : ""}>When I talk (voice activity)</option>
      <option value="ptt" ${s.mode === "ptt" ? "selected" : ""}>While I hold \` (push to talk, while Beacon has focus)</option>
      <option value="open" ${s.mode === "open" ? "selected" : ""}>All the time</option></select></label>
    <label>Camera<span class="row"><select name="camera" id="cam-select"><option value="${esc(s.camera || "")}">${s.camera ? "Chosen camera" : "First camera"}</option></select><button type="button" class="ghost" id="cam-find" title="Lists your cameras (your desktop may ask to allow camera access)">Find cameras</button></span></label>
    <label>Sound with your screen share<select name="share_sound">${shareSoundOpts([], s.share_sound ?? "default")}</select></label>
    <p class="muted small">Your friends hear what you hear, but never the call itself (their own voices are taken out). Choose an output to share only what plays there, for example a DAW or game bus.</p>
    <label>When I start a game during a call<select name="game_share">
      <option value="ask" ${(s.game_share || "ask") === "ask" ? "selected" : ""}>Offer to share it</option>
      <option value="auto" ${s.game_share === "auto" ? "selected" : ""}>Share it right away</option>
      <option value="off" ${s.game_share === "off" ? "selected" : ""}>Do nothing</option></select></label>
    <label>Screen share quality<select name="share_quality">
      <option value="1080p" ${s.share_quality !== "720p" ? "selected" : ""}>1080p, 30 frames a second (about 5 Mbps per viewer)</option>
      <option value="720p" ${s.share_quality === "720p" ? "selected" : ""}>720p, 30 frames a second (about 2.5 Mbps per viewer)</option></select></label>
    <label>Sensitivity: send when louder than <b id="thr-val">${Math.round(s.threshold)} dB</b><input type="range" name="threshold" min="-80" max="-10" step="1" value="${s.threshold}"></label>
    <div class="meter" title="Your microphone's level after noise suppression"><div class="meter-fill" id="meter-fill"></div><div class="meter-mark" id="meter-mark"></div></div>
    <div class="row"><button type="button" class="ghost" id="mic-test">Test microphone</button><small class="muted">Talk, and set the marker just below your voice.</small></div>
    <p class="muted small">Device changes apply from your next call. Noise suppression and echo cancellation are always on. Nothing is recorded.</p>
  </div>`, "Done", async () => true);
  const form = $("#dlg-form");
  const save = async () => {
    const f = new FormData(form);
    const body = { mic: f.get("mic"), speakers: f.get("speakers"), mode: f.get("mode"), threshold: Number(f.get("threshold")), share_quality: f.get("share_quality"), camera: f.get("camera") || "", volume: Number(f.get("volume")), share_sound: f.get("share_sound") || "default", game_share: f.get("game_share") || "ask", user_volumes: voice.audio?.settings?.user_volumes || {} };
    $("#vol-val").textContent = `${body.volume}%`;
    $("#thr-val").textContent = `${body.threshold} dB`;
    try { await api("/api/voice/settings", body); if (voice.audio) voice.audio.settings = body; } catch (e) { toast(e.message, true); }
    soundMeter();
  };
  form.querySelectorAll("#sound-dlg select").forEach((el) => (el.onchange = save));
  form.querySelector("[name=threshold]").oninput = save;
  form.querySelector("[name=volume]").oninput = save;
  api("/api/voice/devices").then(({ devices }) => {
    if (!$("#sound-dlg")) return;
    const mic = form.querySelector("[name=mic]"), spk = form.querySelector("[name=speakers]");
    mic.innerHTML = opts(devices, "microphone", mic.value);
    spk.innerHTML = opts(devices, "speakers", spk.value);
    const ss = form.querySelector("[name=share_sound]");
    ss.innerHTML = shareSoundOpts(devices, ss.value);
  }).catch((e) => toast(e.message, true));
  $("#cam-find").onclick = async () => {
    try {
      const list = (await api("/api/voice/cameras")).cameras;
      const cur = $("#cam-select").value;
      $("#cam-select").innerHTML = `<option value="">First camera</option>` + list.map((c) => `<option value="${esc(c.id)}" ${c.id === cur ? "selected" : ""}>${esc(c.label)}</option>`).join("");
      if (!list.length) toast("Your desktop didn't offer any cameras.", true);
    } catch (e) { toast(e.message, true); }
  };
  $("#mic-test").onclick = async () => {
    if (voice.call) return toast("You're in a call: the meter already shows your microphone.");
    try { await api("/api/voice/test", { on: !voice.audio?.testing }); } catch (e) { toast(e.message, true); }
    pollVoice();
  };
  $("#dlg").addEventListener("close", () => { if (voice.audio?.testing) api("/api/voice/test", { on: false }).catch(() => {}); setTimeout(pollVoice, 300); }, { once: true });
  pollVoice();
}
// Choices for a screen share's sound: off, everything you hear, or one output (Linux: its monitor;
// Windows: that device's loopback).
function shareSoundOpts(list, cur) {
  const outs = list.filter((d) => d.kind === "monitor").length ? list.filter((d) => d.kind === "monitor") : list.filter((d) => d.kind === "speakers");
  const label = (d) => d.label.replace(/^Monitor of /, "");
  return `<option value="default" ${cur === "default" ? "selected" : ""}>Everything you hear (except the call)</option>`
    + `<option value="off" ${cur === "off" ? "selected" : ""}>No sound</option>`
    + outs.map((d) => `<option value="${esc(d.name)}" ${d.name === cur ? "selected" : ""}>Only what plays on: ${esc(label(d))}</option>`).join("")
    + (cur && cur !== "default" && cur !== "off" && !outs.some((d) => d.name === cur) ? `<option value="${esc(cur)}" selected>Chosen output</option>` : "");
}
function soundMeter() {
  const fill = $("#meter-fill");
  if (!fill) return;
  const pct = (db) => Math.max(0, Math.min(100, ((db + 80) / 80) * 100));
  const live = voice.call || voice.audio?.testing;
  const db = live ? (voice.audio?.mic_db ?? -100) : -100;
  const thr = Number($("#dlg-form [name=threshold]")?.value ?? -45);
  fill.style.width = pct(db) + "%";
  fill.classList.toggle("over", db >= thr);
  $("#meter-mark").style.left = pct(thr) + "%";
  const b = $("#mic-test");
  if (b) b.textContent = voice.audio?.testing ? "Stop test" : "Test microphone";
}

// ---------- screen shares you watch ----------
// Each watched share is a stream of JPEG pictures from the Beacon program (decoded there; this page
// has no video decoder of its own), read with fetch so the per-launch secret stays in a header.
const streams = new Map(); // "s<slot>" (screen) or "c<slot>" (camera) -> { abort, url }
const ending = new Set(); // shares that ended, being unwatched
async function watchShare(slot, on) {
  try { await api("/api/voice/watch", { slot, on }); } catch (e) { return toast(e.message, true); }
  if (!on) stopStream(`s${slot}`);
  pollVoice();
}
function stopStream(key) {
  const s = streams.get(key);
  if (!s) return;
  s.abort.abort();
  if (s.url) URL.revokeObjectURL(s.url);
  streams.delete(key);
}
// Pictures come from the Beacon program as a stream of JPEGs, each preceded by its length; the newest
// one is shown in every <img data-stream="key">.
async function readStream(key) {
  const abort = new AbortController();
  const entry = { abort, url: "" };
  streams.set(key, entry);
  const cam = key[0] === "c", slot = Number(key.slice(1));
  try {
    const r = await fetch(`/api/voice/picture?slot=${slot}&cam=${cam}`, { headers: { "x-beacon-token": TOKEN }, signal: abort.signal });
    if (!r.ok) throw new Error();
    const reader = r.body.getReader();
    let buf = new Uint8Array(0);
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      const next = new Uint8Array(buf.length + value.length);
      next.set(buf); next.set(value, buf.length); buf = next;
      let newest = null;
      while (buf.length >= 4) {
        const len = new DataView(buf.buffer, buf.byteOffset).getUint32(0);
        if (buf.length < 4 + len) break;
        newest = buf.slice(4, 4 + len);
        buf = buf.slice(4 + len);
      }
      if (!newest) continue;
      const url = URL.createObjectURL(new Blob([newest], { type: "image/jpeg" }));
      const old = entry.url;
      entry.url = url;
      for (const img of document.querySelectorAll(`img[data-stream="${key}"]`)) img.src = url;
      if (old) setTimeout(() => URL.revokeObjectURL(old), 1000);
    }
  } catch {}
  if (streams.get(key) === entry) streams.delete(key);
}
// Cameras only flow while the call view is open (they cost the server upload per person seeing them).
async function seeCameras(on) {
  if (voice.camsOn === on || !voice.call) return;
  voice.camsOn = on;
  try { await api("/api/voice/see-cameras", { on }); } catch { voice.camsOn = !on; }
}
function renderStage() {
  const stage = $("#stage");
  const call = voice.call;
  if (call && call.watching.length) voice.view = true;
  const show = !!(call && voice.view);
  if (call) seeCameras(show); else voice.camsOn = false;
  // Screens: shares you watch, and the preview of your own. Cameras: everyone's that is on.
  const screens = show ? [...(call.sharing ? [call.slot] : []), ...call.watching] : [];
  const cams = show ? call.participants.filter((p) => p.me ? call.camera : p.camera && call.cameras.includes(p.slot)).map((p) => p.slot) : [];
  const keys = [...screens.map((x) => `s${x}`), ...cams.map((x) => `c${x}`)];
  for (const key of [...streams.keys()]) if (!keys.includes(key)) stopStream(key);
  if (!show) { stage.classList.add("hidden"); stage.classList.remove("focus"); setHTML(stage, ""); stage.dataset.videos = ""; return; }
  stage.classList.remove("hidden");
  const ch = communities().find((x) => x.server === call.server)?.channels.find((c) => c.id === call.channel);
  const who = (p) => esc(profileOf(p.username).name || p.username);
  const person = (slot) => call.participants.find((x) => x.slot === slot);
  // The screens area is rebuilt only when the set of screens changes (so pictures don't flicker).
  const key = screens.join();
  if (stage.dataset.videos !== key || !stage.querySelector(".stage-people")) {
    stage.dataset.videos = key;
    stage.innerHTML = `<header class="stage-head"><b>${ic("volume")} ${esc(ch?.name || "Call")}</b><span class="lock">${ic("lock")} end-to-end encrypted</span><span class="grow"></span>
        <button type="button" class="ghost" data-stage="close" title="Back to the chat (you stay in the call)">Back to chat</button></header>
      ${screens.length ? `<div class="stage-grid n${Math.min(screens.length, 4)}">${screens.map((slot) => {
        const mine = slot === call.slot;
        const p = person(slot);
        return `<figure data-slot="${slot}">
        <img data-stream="s${slot}" alt="${mine ? "Preview of your screen share" : "Screen share"}">
        <figcaption><span class="live">LIVE</span> <b>${mine ? "Your screen" : p ? who(p) : "Someone"}</b>${mine ? '<small class="muted">preview: what others see, smaller</small>' : ""}<span class="grow"></span>
          <button type="button" class="ghost" data-stage="focus" title="Fill the Beacon window (double-click the picture for full screen)">${ic("expand")}</button>
          ${mine ? '<button type="button" class="ghost" data-call="share" title="Stop sharing">Stop sharing</button>' : `<button type="button" class="ghost" data-watch="${slot}" title="Stop watching">${ic("close")}</button>`}</figcaption></figure>`;
      }).join("")}</div>` : ""}
      <div class="stage-people ${screens.length ? "strip" : ""}"></div>
      <div class="stage-controls"></div>`;
  }
  // Tiles are kept and updated in place, so camera pictures don't flicker when someone talks.
  const people = stage.querySelector(".stage-people");
  const want = call.participants.map((p) => String(p.slot));
  for (const t of [...people.children]) if (!want.includes(t.dataset.tile)) t.remove();
  for (const p of call.participants) {
    let t = people.querySelector(`[data-tile="${p.slot}"]`);
    if (!t) {
      t = document.createElement("div");
      t.className = "tile";
      t.dataset.tile = p.slot;
      t.innerHTML = '<div class="tile-media"></div><div class="tile-foot"><b></b><span class="tile-state"></span></div>';
      if (!p.me) {
        // Their volume, just for you: made once per tile so refreshes never fight your dragging.
        t.insertAdjacentHTML("beforeend", `<label class="pvol" title="${who(p)}'s volume, just for you"><span>${ic("volume")}</span><input type="range" min="0" max="200" step="5" value="${p.volume ?? 100}" data-pvol="${esc(p.username)}"><output>${Math.round(p.volume ?? 100)}%</output></label>`);
      }
      people.appendChild(t);
    }
    const camOn = cams.includes(p.slot);
    t.classList.toggle("speaking", !!p.speaking);
    t.classList.toggle("has-cam", camOn);
    setHTML(t.querySelector(".tile-media"), camOn ? `<img data-stream="c${p.slot}" alt="${who(p)}'s camera">` : avatarHTML(p.username));
    setHTML(t.querySelector("b"), `${who(p)}${p.me ? " (you)" : ""}${profileOf(p.username).playing ? `<small class="playing">${ic("gamepad")} ${esc(profileOf(p.username).playing)}</small>` : ""}`);
    const watching = call.watching.includes(p.slot);
    const live = p.sharing && !p.me ? `<button type="button" class="live ${watching ? "on" : ""}" data-watch="${p.slot}">${watching ? "Watching" : "Watch"}</button>` : p.sharing ? '<span class="live">LIVE</span>' : "";
    setHTML(t.querySelector(".tile-state"), `${p.deafened ? '<span title="Deafened">' + ic("headphones-off") + '</span>' : p.muted ? '<span title="Muted">' + ic("mic-off") + '</span>' : ""}${live}`);
  }
  // The call's controls, under the tiles (same buttons as the sidebar's call bar).
  setHTML(stage.querySelector(".stage-controls"), `
    <button type="button" data-call="mute" class="${call.muted ? "active" : ""}" title="${call.muted ? "Unmute" : "Mute"}">${ic(call.muted ? "mic-off" : "mic")}<small>${call.muted ? "Unmute" : "Mute"}</small></button>
    <button type="button" data-call="deafen" class="${call.deafened ? "active" : ""}" title="${call.deafened ? "Undeafen" : "Deafen"}">${ic(call.deafened ? "headphones-off" : "headphones")}<small>${call.deafened ? "Undeafen" : "Deafen"}</small></button>
    <button type="button" data-call="camera" class="${call.camera ? "active share-on" : ""}">${ic(call.camera ? "camera" : "camera-off")}<small>${call.camera ? "Camera off" : "Camera"}</small></button>
    <button type="button" data-call="share" class="${call.sharing ? "active share-on" : ""}">${ic("screen")}<small>${call.sharing ? "Stop sharing" : "Share"}</small></button>
    <button type="button" data-call="sound">${ic("settings")}<small>Settings</small></button>
    <button type="button" data-call="leave" class="leave">${ic("call-end")}<small>Leave</small></button>`);
  for (const k of keys) {
    if (!streams.has(k)) readStream(k);
    else { const u = streams.get(k).url; if (u) for (const img of document.querySelectorAll(`img[data-stream="${k}"]`)) if (!img.src) img.src = u; }
  }
  // A share that ended: stop watching it.
  for (const slot of call.watching) {
    if (call.participants.some((p) => p.slot === slot && p.sharing) || ending.has(slot)) continue;
    ending.add(slot);
    watchShare(slot, false).finally(() => ending.delete(slot));
  }
}
document.addEventListener("click", (e) => {
  if (!e.target.closest('[data-stage="close"]')) return;
  voice.view = false;
  for (const slot of voice.call?.watching || []) watchShare(slot, false);
  if (voice.call) voice.call.watching = [];
  renderStage();
});
document.addEventListener("click", (e) => {
  if (e.target.closest('[data-stage="focus"]')) $("#stage").classList.toggle("focus");
});
document.addEventListener("dblclick", (e) => {
  const img = e.target.closest("#stage img");
  if (!img) return;
  if (document.fullscreenElement) document.exitFullscreen?.();
  else img.requestFullscreen?.().catch(() => $("#stage").classList.add("focus"));
});

loadPrefs().then(loadState);
pollVoice();
listen();
// Other people's changes (names, channels, roles) show up within a few seconds.
setInterval(() => { if (!document.querySelector("dialog[open]")) loadState(); }, 15000);

// Copy text from a click. The old copy command works in more places (embedded pages, Wayland) than
// the clipboard API; the API is the fallback.
function copyText(text) {
  const t = document.createElement("textarea");
  t.value = text;
  t.setAttribute("readonly", "");
  t.style.position = "fixed";
  t.style.opacity = "0";
  document.body.appendChild(t);
  t.select();
  let ok = false;
  try { ok = document.execCommand("copy"); } catch {}
  t.remove();
  if (!ok) navigator.clipboard?.writeText(text).catch(() => {});
  return ok;
}

// A person's volume slider (in the call view): saved as you drag, applied at once.
let pvolTimer = null;
document.addEventListener("input", (e) => {
  const r = e.target.closest?.("[data-pvol]");
  if (!r) return;
  r.nextElementSibling.textContent = `${r.value}%`;
  clearTimeout(pvolTimer);
  pvolTimer = setTimeout(() => api("/api/voice/user-volume", { server: voice.call?.server, username: r.dataset.pvol, volume: Number(r.value) }).catch((err) => toast(err.message, true)), 150);
});

// ---------- mentions ----------
// @name in messages: highlighted, and a mention of you marks the message and the channel (worked out on
// this PC: the server never learns who was mentioned).
function mentionsMe(text, me) {
  const re = new RegExp(`(^|[^\\w.-])@${me.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?![\\w-])`, "i");
  return re.test(text);
}
function withMentions(text, me) {
  const frag = document.createDocumentFragment();
  const { sp } = current();
  const names = new Set((sp?.members || []).map((m) => m.username.toLowerCase()));
  let last = 0;
  for (const m of text.matchAll(/(^|[^\w.-])@([a-z0-9_.-]{2,32})/gi)) {
    const name = m[2].replace(/[.]+$/, "");
    if (!names.has(name.toLowerCase())) continue;
    const at = m.index + m[1].length;
    frag.append(text.slice(last, at));
    const span = document.createElement("span");
    span.className = "mention" + (me && name.toLowerCase() === me.toLowerCase() ? " me" : "");
    span.textContent = "@" + (profileOf(name.toLowerCase()).name || name);
    span.title = "@" + name;
    frag.append(span);
    last = at + 1 + name.length;
  }
  frag.append(text.slice(last));
  return frag;
}
function mentionBadge(server, channel) {
  const n = (state.mentions || {})[`${server}|${channel}`];
  return n ? `<span class="mention-badge" title="${n} mention${n > 1 ? "s" : ""} of you">@${n > 1 ? n : ""}</span>` : "";
}
function clearMentions(server, channel) {
  const k = `${server}|${channel}`;
  if (!state.mentions?.[k]) return;
  delete state.mentions[k];
  api("/api/mentions/clear", { server, channel }).catch(() => {});
}
// Typing @ shows the community's members: arrow keys or the mouse to pick, Tab or Enter to insert.
function mentionPicker(t) {
  const box = document.createElement("div");
  box.className = "mention-pick hidden";
  t.closest(".composer").appendChild(box);
  let items = [], pick = 0;
  const query = () => {
    const before = t.value.slice(0, t.selectionStart);
    const m = before.match(/(^|\s)@([\w.-]{0,32})$/);
    return m ? m[2].toLowerCase() : null;
  };
  const show = () => {
    const q = query();
    const { sp } = current();
    items = q === null ? [] : (sp?.members || []).filter((x) => x.username.includes(q) || (profileOf(x.username).name || "").toLowerCase().includes(q)).slice(0, 8);
    if (!items.length) { box.classList.add("hidden"); return; }
    pick = Math.min(pick, items.length - 1);
    box.innerHTML = items.map((x, i) => `<button type="button" class="${i === pick ? "on" : ""}" data-pick="${esc(x.username)}">${avatarHTML(x.username)}<b>${esc(profileOf(x.username).name || x.username)}</b><small>@${esc(x.username)}</small></button>`).join("");
    box.classList.remove("hidden");
  };
  const insert = (name) => {
    const pos = t.selectionStart;
    const before = t.value.slice(0, pos).replace(/@[\w.-]*$/, `@${name} `);
    t.value = before + t.value.slice(pos);
    t.selectionStart = t.selectionEnd = before.length;
    box.classList.add("hidden");
    t.focus();
  };
  t.addEventListener("input", () => { pick = 0; show(); });
  t.addEventListener("keydown", (e) => {
    if (box.classList.contains("hidden")) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); pick = (pick + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length; show(); }
    else if (e.key === "Tab" || e.key === "Enter") { e.preventDefault(); insert(items[pick].username); }
    else if (e.key === "Escape") { e.preventDefault(); box.classList.add("hidden"); }
  });
  box.addEventListener("mousedown", (e) => { const b = e.target.closest("[data-pick]"); if (b) { e.preventDefault(); insert(b.dataset.pick); } });
  t.addEventListener("blur", () => setTimeout(() => box.classList.add("hidden"), 150));
}
