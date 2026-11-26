// Lodestone Beacon for Android, first version. Plain JavaScript, no build step. Everything the screens can do is a
// command in src-tauri/src/lib.rs; nothing here talks to the network directly.
"use strict";
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const app = document.getElementById("app");
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
let camera = null;

function stopCamera() {
  if (camera) { camera.getTracks().forEach((t) => t.stop()); camera = null; }
}

async function home() {
  stopCamera();
  let s;
  try { s = await invoke("status"); } catch (e) { return fatal(e); }
  if (!s.identity) return setup("new");
  const servers = s.servers.length
    ? `<ul class="list">${s.servers.map((v) => `<li><b>${esc(v.url.replace(/^https?:\/\//, ""))}</b><br><span class="muted">signed in as ${esc(v.username || "?")}</span></li>`).join("")}</ul>`
    : `<p class="muted">No servers yet. Add the server your PC uses, so this phone can approve that PC.</p>`;
  app.innerHTML = `<h1>Lodestone Beacon</h1>
    <p class="muted">This phone holds your identity and approves new PCs.</p>
    <div class="card"><b>Your identity</b><span class="big">${esc(s.identity.fingerprint)}</span><p class="muted">Name: ${esc(s.identity.username)}. The key stays on this phone.</p></div>
    <div class="card"><b>Servers</b>${servers}<p><button class="ghost" id="add">Add a server</button></p></div>
    <button class="primary" id="scan" ${s.servers.length ? "" : "disabled"}>Approve a PC (scan its QR code)</button>
    <h2>Backup</h2>
    <button class="ghost" id="phrase">Show my 24 recovery words</button>`;
  document.getElementById("add").onclick = addServer;
  document.getElementById("scan").onclick = scan;
  document.getElementById("phrase").onclick = showPhrase;
}

function fatal(e) { app.innerHTML = `<h1>Something went wrong</h1><p class="err">${esc(e)}</p><button id="again">Try again</button>`; document.getElementById("again").onclick = home; }

function setup(tab, err) {
  app.innerHTML = `<h1>Set up your identity</h1>
    <p class="muted">Your identity is a key that lives on this phone. It's your name on every Beacon server, and it lets you approve new PCs. Nobody else keeps a copy.</p>
    <div class="tabs"><button data-t="new" class="${tab === "new" ? "on" : ""}">New identity</button><button data-t="restore" class="${tab === "restore" ? "on" : ""}">Restore from words</button></div>
    <form id="f" class="stack">
      ${tab === "restore" ? `<label>Your 24 recovery words<textarea name="phrase" required autocomplete="off" spellcheck="false" autocapitalize="none"></textarea></label>` : ""}
      <label>Username<input name="username" required maxlength="32" autocomplete="off" spellcheck="false" autocapitalize="none" placeholder="2 to 32: a-z 0-9 _ . -"></label>
      <p class="err" id="err">${esc(err || "")}</p>
      <button class="primary">${tab === "restore" ? "Restore" : "Create my identity"}</button>
    </form>`;
  app.querySelectorAll("[data-t]").forEach((b) => (b.onclick = () => setup(b.dataset.t)));
  document.getElementById("f").onsubmit = async (e) => {
    e.preventDefault();
    const f = new FormData(e.target);
    try {
      if (tab === "restore") { await invoke("identity_restore", { phrase: String(f.get("phrase")), username: String(f.get("username")) }); home(); }
      else { const r = await invoke("identity_create", { username: String(f.get("username")) }); words(r.phrase, true); }
    } catch (x) { setup(tab, x); }
  };
}

function words(phrase, first) {
  app.innerHTML = `<h1>Your recovery words</h1>
    <p>${first ? "Write these 24 words down and keep them somewhere safe." : "Anyone who has these words is you."} They are the only way to get your identity back on a new phone. We can't recover them for you.</p>
    <ol class="words">${phrase.split(" ").map((w) => `<li>${esc(w)}</li>`).join("")}</ol>
    <button class="primary" id="done">${first ? "I've written them down" : "Done"}</button>`;
  document.getElementById("done").onclick = home;
}

async function showPhrase() {
  try { words((await invoke("identity_phrase")).phrase, false); } catch (e) { fatal(e); }
}

function addServer(err) {
  app.innerHTML = `<h1>Add a server</h1>
    <form id="f" class="stack">
      <label>Server's domain<input name="server" required autocomplete="off" spellcheck="false" autocapitalize="none" placeholder="beacon.yourdomain.com"></label>
      <label>What is this?
        <select name="mode" id="mode"><option value="recover">I already have an account on it (from my PC)</option><option value="register">I'm joining it for the first time</option></select></label>
      <div id="new" class="stack" hidden>
        <label>Username on this server<input name="username" autocomplete="off" spellcheck="false" autocapitalize="none" placeholder="leave blank to use your identity name"></label>
        <label>Invite code or the server's setup code<input name="code" autocomplete="off" spellcheck="false" autocapitalize="none"></label>
      </div>
      <p class="err" id="err">${typeof err === "string" ? esc(err) : ""}</p>
      <div class="row"><button type="button" class="ghost" id="back">Back</button><button class="primary">Add server</button></div>
    </form>`;
  const mode = document.getElementById("mode");
  mode.onchange = () => { document.getElementById("new").hidden = mode.value !== "register"; };
  document.getElementById("back").onclick = home;
  document.getElementById("f").onsubmit = async (e) => {
    e.preventDefault();
    const f = new FormData(e.target);
    const btn = e.target.querySelector("button.primary");
    btn.disabled = true;
    try {
      await invoke("server_add", { server: String(f.get("server")), username: String(f.get("username") || ""), code: String(f.get("code") || ""), mode: String(f.get("mode")) });
      home();
    } catch (x) { addServer(x); }
  };
}

// ---------- approving a PC ----------
async function scan(err) {
  stopCamera();
  app.innerHTML = `<h1>Approve a PC</h1>
    <p class="muted">On the PC choose <b>Sign in with my phone</b>, then point this phone at the QR code.</p>
    <div class="video"><video id="v" playsinline muted></video></div>
    <canvas id="c" hidden></canvas>
    <p class="err" id="err">${esc(err || "")}</p>
    <label>If the camera won't work, paste the text from the PC<input id="paste" autocomplete="off" spellcheck="false" autocapitalize="none" placeholder="beacon-pair:2|…"></label>
    <div class="row"><button class="ghost" id="back">Back</button><button id="use">Use pasted text</button></div>`;
  document.getElementById("back").onclick = home;
  document.getElementById("use").onclick = () => inspect(document.getElementById("paste").value);
  const v = document.getElementById("v");
  const c = document.getElementById("c");
  const ctx = c.getContext("2d", { willReadFrequently: true });
  try {
    camera = await navigator.mediaDevices.getUserMedia({ video: { facingMode: "environment" }, audio: false });
    v.srcObject = camera;
    await v.play();
  } catch {
    document.getElementById("err").textContent = "The camera isn't available (allow camera access for Beacon in Android settings), or paste the text instead.";
    return;
  }
  const tick = () => {
    if (!camera) return;
    if (v.readyState === v.HAVE_ENOUGH_DATA && v.videoWidth) {
      const w = 480, h = Math.round((480 * v.videoHeight) / v.videoWidth);
      c.width = w; c.height = h;
      ctx.drawImage(v, 0, 0, w, h);
      const hit = jsQR(ctx.getImageData(0, 0, w, h).data, w, h);
      if (hit && hit.data.startsWith("beacon-pair:2|")) { return inspect(hit.data); }
    }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

async function inspect(text) {
  stopCamera();
  let r;
  try { r = await invoke("scan_inspect", { text }); } catch (e) { return scan(e); }
  app.innerHTML = `<h1>Approve this PC?</h1>
    <p>A PC is asking to sign in to <b>${esc(r.server.replace(/^https?:\/\//, ""))}</b> as you. Approve it only if the PC shows exactly:</p>
    <span class="big">${esc(r.fingerprint)}</span>
    <p class="muted">Approving adds the PC to your account and gives it your identity, so it can act as you.</p>
    <p class="err" id="err"></p>
    <div class="row"><button class="danger" id="no">Don't approve</button><button class="primary" id="yes">Approve</button></div>`;
  document.getElementById("no").onclick = home;
  document.getElementById("yes").onclick = async (e) => {
    e.target.disabled = true;
    try {
      await invoke("approve", { text });
      app.innerHTML = `<h1 class="ok">PC approved</h1><p>It is signed in now.</p><button class="primary" id="ok">Done</button>`;
      document.getElementById("ok").onclick = home;
    } catch (x) { document.getElementById("err").textContent = x; e.target.disabled = false; }
  };
}

home();
