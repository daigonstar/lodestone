// Lodestone Comms interface. Talks only to its own plug-in on 127.0.0.1, with the per-launch secret from
// the address (#t=...). Everything shown from servers and other people is escaped or set as text.
"use strict";

const TOKEN = new URLSearchParams(location.hash.slice(1)).get("t") || "";
history.replaceState(null, "", location.pathname); // don't leave the secret in the address bar

const $ = (s) => document.querySelector(s);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const state = { hosts: [], bridge: false, sel: { host: "", space: "", channel: "" }, me: "", showMembers: true, profiles: {}, prefs: { theme: "lodestone", accent: "" } };

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
  if (state.prefs.theme !== "lodestone") return; // another theme is chosen
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
    state.pairing = s.pairing || [];
    state.bridge = s.bridge;
    applyTheme(s.theme);
    for (const h of s.hosts) if (h.error) toast(`${h.url}: ${h.error}`, true);
    await Promise.all(s.hosts.map((h) => loadProfiles(h.url)));
  } catch (e) { toast(e.message, true); }
  const all = spaces();
  if (!all.some((s) => s.host === state.sel.host && s.id === state.sel.space)) {
    const first = all[0];
    state.sel = first ? { host: first.host, space: first.id, channel: "" } : { host: "", space: "", channel: "" };
  }
  const { sp } = current();
  if (sp && !sp.channels.some((c) => c.id === state.sel.channel && c.kind !== "voice")) { const text = sp.channels.filter((c) => c.kind !== "voice"); state.sel.channel = (text.find((c) => c.ready) || text[0] || {}).id || ""; }
  render();
}

// ---------- rendering ----------
function render() {
  const all = spaces();
  $("#servers").innerHTML = all.map((s) => `<button class="srv ${s.host === state.sel.host && s.id === state.sel.space ? "on" : ""}" data-space="${esc(s.id)}" data-host="${esc(s.host)}" title="${esc(s.name)}">${iconHTML(s)}</button>`).join("")
    + `<button class="srv add" id="add-server" title="Join or create a server">+</button>`;
  const { sp, ch } = current();
  if (!sp) {
    $("#members").classList.add("hidden");
    $("#channels").innerHTML = "";
    $("#chat").innerHTML = `<div class="empty"><h2>Lodestone Comms</h2>
      <p class="muted">End-to-end encrypted chat on servers you or your friends host. Messages are encrypted on your PC; the host only passes them on.</p>
      <div class="row"><button class="primary" data-act="join">Join with an invite</button><button class="ghost" data-act="pair">Sign in with another device</button><button class="ghost" data-act="setup">Set up a new host</button></div>
      ${(state.pairing || []).map((p) => `<p class="muted">Waiting for approval as <b>${esc(p.username)}</b> on ${esc(p.url.replace(/^https?:\/\//, ""))}… Your other device should show <code>${esc(p.fingerprint)}</code>.</p>`).join("")}</div>`;
    return;
  }
  const admin = sp.your_role === "owner" || sp.your_role === "admin";
  $("#channels").innerHTML = `<h2><span>${esc(sp.name)}</span><button class="gear" data-act="settings" title="Server settings">⚙</button></h2><div class="host" title="${esc(sp.host)}">${esc(sp.me)} · ${esc(sp.host.replace(/^https?:\/\//, ""))}</div>
    <div class="chlist">${sp.channels.filter((c) => c.kind !== "voice").map((c) => `<button class="ch ${c.id === state.sel.channel ? "on" : ""}" data-channel="${esc(c.id)}"># ${esc(c.name)}${c.ready ? "" : ' <span class="wait">(waiting for keys)</span>'}</button>`).join("")}
    ${sp.channels.some((c) => c.kind === "voice") ? '<h4 class="chhead">Voice</h4>' : ""}
    ${sp.channels.filter((c) => c.kind === "voice").map((c) => `<button class="ch vc" data-voice="${esc(c.id)}" title="${c.ready ? "Join the call" : "Waiting for this device's keys"}">🔊 ${esc(c.name)}${c.ready ? "" : ' <span class="wait">(waiting for keys)</span>'}</button><div class="vc-people" data-vc="${esc(c.id)}"></div>`).join("")}</div>
    <div class="side-actions"><button data-act="invite">Invite people</button>${admin ? '<button class="ghost" data-act="channel">New channel</button>' : ""}</div>
    <div id="callbar"></div>
    ${meBar()}`;
  renderVoice();
  renderMembers(sp);
  if (!ch) { $("#chat").innerHTML = ""; return; }
  $("#chat").innerHTML = `<header><h3># ${esc(ch.name)}${ch.topic ? `<span class="topic">${esc(ch.topic)}</span>` : ""}</h3><button class="gear" id="toggle-members" title="Show or hide members">👥</button><span class="lock" title="Only members' devices can read this channel; the host stores ciphertext.">🔒 end-to-end encrypted</span></header>
    <div class="msgs" id="msgs"></div>
    <div class="composer">${ch.ready
      ? '<div id="pending" class="pending hidden"></div><div class="compose-row"><button type="button" class="emoji-btn" id="attach-btn" title="Attach a file or image (or paste / drop one)">📎</button><textarea id="text" placeholder="Message (Enter to send, Shift+Enter for a new line)" maxlength="8000"></textarea><button type="button" class="emoji-btn" id="emoji-btn" title="Emoji">😊</button></div>'
      : '<p class="muted">You\'re a member, but no one in this channel has been online to give your device its keys yet. It happens automatically.</p>'}</div>`;
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
    t.addEventListener("keydown", async (e) => {
      if (e.key !== "Enter" || e.shiftKey) return;
      e.preventDefault();
      const text = t.value.trim();
      if (!text && !state.pending) return;
      t.disabled = true;
      try {
        if (state.pending) {
          const f = state.pending;
          const data = await fileBase64(f);
          await api("/api/upload", { host: state.sel.host, channel: state.sel.channel, name: f.name || "pasted image", data, caption: text });
          setPending(null);
        } else {
          await api("/api/send", { host: state.sel.host, channel: state.sel.channel, text });
        }
        t.value = "";
      } catch (err) { toast(err.message, true); }
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
    const reacts = new Map();
    for (const x of r.reactions || []) { if (!reacts.has(x.seq)) reacts.set(x.seq, []); reacts.get(x.seq).push(x); }
    box.replaceChildren(...r.messages.map((m) => {
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
      if (m.sender.startsWith("⚠")) who.title = "This sender's encryption key doesn't match what the host lists for them";
      const body = document.createElement("span");
      body.className = "body";
      body.textContent = m.body;
      const add = document.createElement("button");
      add.className = "react-add";
      add.type = "button";
      add.title = "Add a reaction";
      add.textContent = "☺+";
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
    }));
    if (!r.messages.length) box.innerHTML = '<p class="muted">No messages yet. Messages sent before you joined can\'t be read on this device, by design.</p>';
    if (atBottom || !box.dataset.seen) box.scrollTop = box.scrollHeight;
    box.dataset.seen = "1";
  } catch (e) { toast(e.message, true); }
}

// ---------- members and status ----------
const STATUS_WORDS = { online: "Online", away: "Away", busy: "Busy", invisible: "Invisible", offline: "Offline" };
function myHost() { return state.hosts.find((h) => h.url === state.sel.host) || state.hosts[0] || {}; }
function meBar() {
  const h = myHost();
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
      <span class="av-wrap" data-avatar-user="${esc(m.username)}">${avatarHTML(m.username)}<span class="dot ${esc(m.presence)}"></span></span><span class="grow"><span class="mline"><span class="mname" ${profileOf(m.username).color ? `style="color:${esc(profileOf(m.username).color)}"` : ""}>${esc(profileOf(m.username).name || m.username)}</span>${m.role === "owner" ? '<span class="badge" title="Owner">👑</span>' : m.role === "admin" ? '<span class="badge" title="Admin">🛡</span>' : ""}</span>
      <small>${profileOf(m.username).name && profileOf(m.username).name !== m.username ? "@" + esc(m.username) + (m.status_text ? " · " : "") : ""}${esc(m.status_text || "")}</small></span></div>`;
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
  const h = myHost();
  autoAway = false;
  if (b.dataset.status) return setStatus(b.dataset.status, h.status_text || "");
  if (b.hasAttribute("data-profile")) return profileDialog();
  if (b.hasAttribute("data-appearance")) return appearanceDialog();
  if (b.hasAttribute("data-devices")) return devicesDialog();
  if (b.hasAttribute("data-sound")) return soundDialog();
  ask("Status message", `<label>What are you up to? <span class="muted">(up to 80 characters; empty to clear)</span><input name="text" maxlength="80" value="${esc(h.status_text || "")}"></label>`, "Save", (f) => setStatus(h.status || "online", String(f.get("text") || "")));
};
// Away by itself after 15 minutes without using Comms, and back when you return (only from Online).
let lastActive = Date.now(), autoAway = false;
for (const ev of ["mousemove", "keydown", "mousedown", "wheel", "focus"]) addEventListener(ev, () => {
  lastActive = Date.now();
  if (autoAway) { autoAway = false; setStatus("online", myHost().status_text || ""); }
}, { passive: true });
setInterval(() => {
  if (!autoAway && myHost().status === "online" && Date.now() - lastActive > 15 * 60 * 1000) { autoAway = true; setStatus("away", myHost().status_text || ""); }
}, 30000);

// ---------- profiles ----------
async function loadProfiles(host) {
  try {
    const r = await api(`/api/profiles?host=${encodeURIComponent(host)}`);
    const map = {};
    for (const p of r.profiles) map[p.user] = p;
    if (r.me.user) map[r.me.user] = { ...(map[r.me.user] || {}), user: r.me.user, name: r.me.name, color: r.me.color, has_avatar: r.me.has_avatar };
    state.profiles[host] = map;
  } catch {}
}
function profileOf(user, host = state.sel.host) {
  return (state.profiles[host] || {})[user] || {};
}
const avatars = new Map();
function avatarKey(user, host = state.sel.host) { return `${host}|${user}`; }
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
  const host = state.sel.host, key = avatarKey(user, host);
  avatars.set(key, "");
  fetch(`/api/avatar?host=${encodeURIComponent(host)}&user=${encodeURIComponent(user)}`, { headers: { "x-comms-token": TOKEN } })
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
function forgetAvatars(host) {
  for (const [k, u] of avatars) if (k.startsWith(host + "|")) { if (u) URL.revokeObjectURL(u); avatars.delete(k); }
}

function profileDialog() {
  const h = myHost();
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
      <p class="muted">Your profile is sent end-to-end encrypted to the people in your servers; the host can't see it. Others always see your @username beside your display name.</p>`,
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
  const h = myHost();
  let d;
  try { d = await api(`/api/devices?host=${encodeURIComponent(h.url)}`); } catch (e) { return toast(e.message, true); }
  const list = d.devices.map((x) => `<div class="set-row"><div class="grow">${x.this ? "<b>This device</b>" : "Another device"}<br><small><code>${esc(x.fingerprint)}</code></small></div>
      ${x.this ? "" : `<button type="button" class="danger" data-rm="${esc(x.key)}">Remove</button>`}</div>`).join("");
  ask("Your devices", `<p class="muted">Each device has its own key. Removing one takes it out of every channel: it can't read anything new.</p>${list}
      <div id="pair-box"><button type="button" class="primary" id="pair-start">Add a device…</button></div>`, null, null);
  $("#dlg-body").onclick = async (e) => {
    const rm = e.target.closest("[data-rm]");
    if (rm) {
      if (!(await confirmBox("Remove this device? It will no longer be able to read new messages.", "Remove device"))) return;
      try { await api("/api/devices/remove", { host: h.url, device_key: rm.dataset.rm }); toast("Device removed"); } catch (err) { toast(err.message, true); }
      return;
    }
    if (e.target.closest("#pair-start")) startPairing(h);
    const dec = e.target.closest("[data-decide]");
    if (dec) {
      try {
        await api("/api/pair/decide", { host: h.url, device_key: dec.dataset.key, approve: dec.dataset.decide === "yes" });
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
  try { p = await api("/api/pair/start", { host: h.url }); } catch (e) { return toast(e.message, true); }
  const box = $("#pair-box");
  const show = (waiting) => {
    box.innerHTML = `<div class="pair-code"><p>On the new device choose <b>Sign in with another device</b> and enter:</p>
      <p>Host <code>${esc(p.host)}</code> · Code <code class="big">${esc(p.code)}</code></p>
      <p class="muted">Valid for 10 minutes, once.</p>
      ${waiting.length ? waiting.map((w) => `<div class="set-row"><div class="grow">A device is asking to join. Approve only if it shows<br><code class="big">${esc(w.fingerprint)}</code></div>
        <button type="button" class="primary" data-decide="yes" data-key="${esc(w.key)}">Approve</button><button type="button" class="danger" data-decide="no" data-key="${esc(w.key)}">Refuse</button></div>`).join("") : '<p class="muted">Waiting for the new device…</p>'}</div>`;
  };
  show([]);
  clearInterval(pairPoll);
  let shown = "";
  pairPoll = setInterval(async () => {
    try {
      const d = await api(`/api/devices?host=${encodeURIComponent(h.url)}`);
      // Redraw only when the waiting list changes, so the Approve button doesn't move under the pointer.
      const key = d.waiting.map((w) => w.key).join(",");
      if (key !== shown) { shown = key; show(d.waiting); }
    } catch {}
  }, 3000);
}
function pairDialog() {
  ask("Sign in with another device", `<p class="muted">On a device where you're already signed in, open your name → <b>Devices…</b> → <b>Add a device</b>, then enter what it shows.</p>
    <label>Host's domain<input name="host" placeholder="comms.yourdomain.com" autocomplete="off" spellcheck="false" required></label>
    <label>Code<input name="code" autocomplete="off" spellcheck="false" required></label>`, "Continue", async (f) => {
      const r = await api("/api/pair/claim", { host: String(f.get("host")), code: String(f.get("code")) });
      await loadState();
      waitForApproval(r);
    });
}
function waitForApproval(r) {
  ask("Waiting for approval", `<p>Signing in as <b>${esc(r.username)}</b>. On your other device, approve the request only if it shows:</p><p><code class="big">${esc(r.fingerprint)}</code></p><p class="muted" id="pair-status">Waiting…</p>`, null, null);
  clearInterval(pairPoll);
  pairPoll = setInterval(async () => {
    try {
      const s = await api("/api/pair/check", { host: r.host });
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
  el.innerHTML = `<span>📎 ${esc(f.name || "pasted image")} <small class="muted">${fmtSize(f.size)} · press Enter to send, with your message as its caption</small></span><button type="button" class="ghost" id="pending-x" title="Remove">✕</button>`;
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
  const key = `${state.sel.host}|${state.sel.channel}|${seq}`;
  if (fileUrls.has(key)) return fileUrls.get(key);
  const r = await fetch(`/api/file?host=${encodeURIComponent(state.sel.host)}&channel=${encodeURIComponent(state.sel.channel)}&seq=${seq}`, { headers: { "x-comms-token": TOKEN } });
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
    fileUrl(m.seq).then((u) => (img.src = u)).catch((e) => { wrap.textContent = `⚠ ${e.message}`; });
    img.onclick = () => { $("#lightbox-img").src = img.src; $("#lightbox").classList.remove("hidden"); };
    wrap.append(img);
  } else {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "attach-file";
    b.textContent = `📄 ${a.name} · ${fmtSize(a.size)} · Save`;
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
  try { await api("/api/react", { host: state.sel.host, channel: state.sel.channel, seq: Number(seq), emoji, on }); loadMessages(); }
  catch (err) { toast(err.message, true); }
}
document.addEventListener("click", (e) => {
  const add = e.target.closest("[data-react-add]");
  if (add) return openPicker(add, (emo) => setReaction(add.dataset.reactAdd, emo, true), true);
  const chip = e.target.closest("[data-react]");
  if (chip) setReaction(chip.dataset.react, chip.dataset.emoji, chip.dataset.on === "1");
});

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
      <button type="button" class="ghost" data-act="pair">Sign in with another device…</button>
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
  ask("New channel", `<label>Name<input name="name" maxlength="64" required placeholder="raids"></label>
    <label>Kind<select name="kind"><option value="text">Text: messages, files, reactions</option><option value="voice">Voice: calls (end-to-end encrypted)</option></select></label>`, "Create", async (f) => {
    const kind = f.get("kind");
    const r = await api("/api/channels", { host: sp.host, space: sp.id, name: f.get("name"), kind });
    if (kind !== "voice") state.sel.channel = r.channel;
    await loadState();
  });
}

document.addEventListener("click", (e) => {
  const s = e.target.closest("[data-space]");
  if (s) { state.sel = { host: s.dataset.host, space: s.dataset.space, channel: "" }; return loadState(); }
  const c = e.target.closest("[data-channel]");
  if (c) { state.sel.channel = c.dataset.channel; voice.view = false; renderStage(); return render(); }
  const v = e.target.closest("[data-voice]");
  if (v) return joinVoice(v.dataset.voice);
  const w = e.target.closest("[data-watch]");
  if (w) return watchShare(Number(w.dataset.watch), !voice.call?.watching.includes(Number(w.dataset.watch)));
  const cb = e.target.closest("[data-call]");
  if (cb) return callAction(cb.dataset.call);
  if (e.target.closest("#add-server")) return addDialog();
  const a = e.target.closest("[data-act]");
  if (!a) return;
  ({ join: joinDialog, pair: pairDialog, setup: setupDialog, invite: inviteDialog, channel: channelDialog, settings: () => openSettings("overview"), create: () => newServerDialog(a.dataset.host) })[a.dataset.act]?.();
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
          else if (ev.type === "profiles") { forgetAvatars(ev.host); loadProfiles(ev.host).then(render); }
          else if (ev.type === "messages" && ev.host === state.sel.host && ev.channels.includes(state.sel.channel)) loadMessages();
        }
      }
    } catch {}
    await new Promise((r) => setTimeout(r, 2000));
  }
}

// ---------- calls ----------
// One call at a time. The call's sound runs in the Comms program, not in this page; the page shows who's
// there and who's speaking, and holds push-to-talk while it has focus.
const voice = { call: null, audio: null, timer: null, busy: false, ptt: false, view: false };
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
    const mine = call && call.channel === id && call.host === state.sel.host;
    const people = mine ? call.participants : (ch?.in_call || []).map((u) => ({ username: u }));
    setHTML(el, people.map((p) => {
      const live = p.sharing ? (mine && !p.me ? `<button type="button" class="live ${call.watching.includes(p.slot) ? "on" : ""}" data-watch="${p.slot}" title="${call.watching.includes(p.slot) ? "Stop watching" : "Watch their screen"}">LIVE</button>` : '<span class="live" title="Sharing their screen">LIVE</span>') : "";
      return `<div class="vc-person ${p.speaking ? "speaking" : ""}">${avatarHTML(p.username)}<span class="grow">${esc(profileOf(p.username).name || p.username)}</span>${live}${p.deafened ? '<span title="Deafened">🔇</span>' : p.muted ? '<span title="Muted" class="muted-mic">🎙</span>' : ""}</div>`;
    }).join(""));
    el.previousElementSibling?.classList.toggle("on", !!mine);
  }
  const bar = $("#callbar");
  if (!bar) return;
  if (!call) { setHTML(bar, ""); return; }
  const ch = spaces().find((x) => x.host === call.host)?.channels.find((c) => c.id === call.channel);
  const trouble = voice.audio?.error;
  setHTML(bar, `<div class="callbar">
    <div class="call-info"><b class="${call.connected ? "ok" : "wait"}">${call.connected ? "Voice connected" : "Connecting…"}</b><small title="${esc(call.host)}">🔊 ${esc(ch?.name || "call")}${call.sharing ? ` · sharing ${call.sharing.width}×${call.sharing.height}` : ""}${trouble ? ` · <span class="bad">${esc(trouble)}</span>` : ""}</small></div>
    <div class="call-btns">
      <button type="button" data-call="mute" class="${call.muted ? "active" : ""}" title="${call.muted ? "Unmute" : "Mute"}"><span class="${call.muted ? "muted-mic" : ""}">🎙</span></button>
      <button type="button" data-call="deafen" class="${call.deafened ? "active" : ""}" title="${call.deafened ? "Undeafen" : "Deafen (hear no one, and mute)"}">${call.deafened ? "🔇" : "🎧"}</button>
      <button type="button" data-call="share" class="${call.sharing ? "active share-on" : ""}" title="${call.sharing ? "Stop sharing your screen" : "Share your screen"}">🖥</button>
      <button type="button" data-call="sound" title="Sound and sharing settings">⚙</button>
      <button type="button" data-call="leave" class="leave" title="Leave the call">✕</button>
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
  if (voice.call && voice.call.channel === id && voice.call.host === sp.host) { voice.view = true; return renderStage(); }
  if (voice.busy) return;
  voice.busy = true;
  toast(`Joining ${ch.name}…`);
  try {
    const v = await api("/api/voice/join", { host: sp.host, channel: id });
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
      if (!c.sharing) toast("Your desktop will ask which screen or window to share…");
      await api("/api/voice/share", { on: !c.sharing });
      if (!c.sharing) toast("You're sharing. People in the call can choose to watch.");
    }
    if (act === "leave") {
      // Gone from the screen at once; the call closes in the background.
      voice.call = null; voice.view = false;
      renderVoice(); renderStage();
      await api("/api/voice/leave", {});
    }
    else if (act === "mute") await api("/api/voice/state", { muted: c.deafened ? false : !c.muted, deafened: false });
    else if (act === "deafen") await api("/api/voice/state", { muted: !c.deafened, deafened: !c.deafened });
  } catch (e) { toast(e.message, true); }
  pollVoice();
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
  let list = [];
  try { list = (await api("/api/voice/devices")).devices; } catch (e) { toast(e.message, true); }
  try { voice.audio = (await api("/api/voice")).audio; } catch {}
  const s = voice.audio?.settings || { mic: "", speakers: "", mode: "voice", threshold: -45 };
  const opts = (kind, cur) => `<option value="">System default</option>` + list.filter((d) => d.kind === kind).map((d) => `<option value="${esc(d.name)}" ${d.name === cur ? "selected" : ""}>${esc(d.label)}</option>`).join("");
  ask("Sound and sharing", `<div id="sound-dlg">
    <label>Microphone<select name="mic">${opts("microphone", s.mic)}</select></label>
    <label>Speakers<select name="speakers">${opts("speakers", s.speakers)}</select></label>
    <label>Send my voice<select name="mode">
      <option value="voice" ${s.mode === "voice" ? "selected" : ""}>When I talk (voice activity)</option>
      <option value="ptt" ${s.mode === "ptt" ? "selected" : ""}>While I hold \` (push to talk, while Comms has focus)</option>
      <option value="open" ${s.mode === "open" ? "selected" : ""}>All the time</option></select></label>
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
    const body = { mic: f.get("mic"), speakers: f.get("speakers"), mode: f.get("mode"), threshold: Number(f.get("threshold")), share_quality: f.get("share_quality") };
    $("#thr-val").textContent = `${body.threshold} dB`;
    try { await api("/api/voice/settings", body); if (voice.audio) voice.audio.settings = body; } catch (e) { toast(e.message, true); }
    soundMeter();
  };
  form.querySelectorAll("#sound-dlg select").forEach((el) => (el.onchange = save));
  form.querySelector("[name=threshold]").oninput = save;
  $("#mic-test").onclick = async () => {
    if (voice.call) return toast("You're in a call: the meter already shows your microphone.");
    try { await api("/api/voice/test", { on: !voice.audio?.testing }); } catch (e) { toast(e.message, true); }
    pollVoice();
  };
  $("#dlg").addEventListener("close", () => { if (voice.audio?.testing) api("/api/voice/test", { on: false }).catch(() => {}); setTimeout(pollVoice, 300); }, { once: true });
  pollVoice();
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
// Each watched share is a stream of JPEG pictures from the Comms program (decoded there; this page
// has no video decoder of its own), read with fetch so the per-launch secret stays in a header.
const streams = new Map(); // slot -> { abort, url }
const ending = new Set(); // shares that ended, being unwatched
async function watchShare(slot, on) {
  try { await api("/api/voice/watch", { slot, on }); } catch (e) { return toast(e.message, true); }
  if (!on) stopStream(slot);
  pollVoice();
}
function stopStream(slot) {
  const s = streams.get(slot);
  if (!s) return;
  s.abort.abort();
  if (s.url) URL.revokeObjectURL(s.url);
  streams.delete(slot);
}
async function readStream(slot) {
  const abort = new AbortController();
  const entry = { abort, url: "" };
  streams.set(slot, entry);
  try {
    const r = await fetch(`/api/voice/picture?slot=${slot}`, { headers: { "x-comms-token": TOKEN }, signal: abort.signal });
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
      if (newest) {
        const img = document.querySelector(`#stage img[data-slot="${slot}"]`);
        if (!img) continue;
        const url = URL.createObjectURL(new Blob([newest], { type: "image/jpeg" }));
        const old = entry.url;
        entry.url = url;
        img.src = url;
        if (old) setTimeout(() => URL.revokeObjectURL(old), 1000);
      }
    }
  } catch {}
  if (streams.get(slot) === entry) streams.delete(slot);
}
function renderStage() {
  const stage = $("#stage");
  const call = voice.call;
  if (call && call.watching.length) voice.view = true;
  const show = !!(call && voice.view);
  // Pictures to stream: shares you watch, and the preview of your own.
  const videos = show ? [...(call.sharing ? [call.slot] : []), ...call.watching] : [];
  for (const slot of [...streams.keys()]) if (!videos.includes(slot)) stopStream(slot);
  if (!show) { stage.classList.add("hidden"); stage.classList.remove("focus"); setHTML(stage, ""); return; }
  stage.classList.remove("hidden");
  const ch = spaces().find((x) => x.host === call.host)?.channels.find((c) => c.id === call.channel);
  const who = (p) => esc(profileOf(p.username).name || p.username);
  const person = (slot) => call.participants.find((x) => x.slot === slot);
  // The video area is rebuilt only when the set of pictures changes (so pictures don't flicker).
  const key = videos.join();
  if (stage.dataset.videos !== key || !stage.querySelector(".stage-people")) {
    stage.dataset.videos = key;
    stage.innerHTML = `<header class="stage-head"><b>🔊 ${esc(ch?.name || "Call")}</b><span class="lock">🔒 end-to-end encrypted</span><span class="grow"></span>
        <button type="button" class="ghost" data-stage="close" title="Back to the chat (you stay in the call)">Back to chat</button></header>
      ${videos.length ? `<div class="stage-grid n${Math.min(videos.length, 4)}">${videos.map((slot) => {
        const mine = slot === call.slot;
        const p = person(slot);
        return `<figure data-slot="${slot}">
        <img data-slot="${slot}" alt="${mine ? "Preview of your screen share" : "Screen share"}">
        <figcaption><span class="live">LIVE</span> <b>${mine ? "Your screen" : p ? who(p) : "Someone"}</b>${mine ? '<small class="muted">preview: what others see, smaller</small>' : ""}<span class="grow"></span>
          <button type="button" class="ghost" data-stage="focus" title="Fill the Comms window (double-click the picture for full screen)">⤢</button>
          ${mine ? '<button type="button" class="ghost" data-call="share" title="Stop sharing">Stop sharing</button>' : `<button type="button" class="ghost" data-watch="${slot}" title="Stop watching">✕</button>`}</figcaption></figure>`;
      }).join("")}</div>` : ""}
      <div class="stage-people ${videos.length ? "strip" : ""}"></div>`;
  }
  setHTML(stage.querySelector(".stage-people"), call.participants.map((p) => {
    const watching = call.watching.includes(p.slot);
    const live = p.sharing && !p.me ? `<button type="button" class="live ${watching ? "on" : ""}" data-watch="${p.slot}">${watching ? "Watching" : "Watch"}</button>` : p.sharing ? '<span class="live">LIVE</span>' : "";
    return `<div class="tile ${p.speaking ? "speaking" : ""}">${avatarHTML(p.username)}<b>${who(p)}${p.me ? " (you)" : ""}</b>
      <span class="tile-state">${p.deafened ? '<span title="Deafened">🔇</span>' : p.muted ? '<span title="Muted" class="muted-mic">🎙</span>' : ""}${live}</span></div>`;
  }).join(""));
  for (const slot of videos) if (!streams.has(slot)) readStream(slot);
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
