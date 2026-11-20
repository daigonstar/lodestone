const T = window.__TAURI__;
const invoke = T ? T.core.invoke : async () => { throw new Error("Run inside Lodestone (cargo tauri dev)"); };
const fileSrc = (p) => (T ? T.core.convertFileSrc(p) : p);

const STORES = {
  steam: { label: "Steam", color: "#66c0f4" },
  epic: { label: "Epic Games", color: "#e6e6e6" },
  gog: { label: "GOG", color: "#b26bff" },
  amazon: { label: "Amazon", color: "#ff9900" },
  battlenet: { label: "Battle.net", color: "#148eff" },
  arenanet: { label: "ArenaNet", color: "#d9342b" },
  ea: { label: "EA", color: "#ff4747" },
  ubisoft: { label: "Ubisoft", color: "#2f7bff" },
  rockstar: { label: "Rockstar", color: "#fcaf17" },
  minecraft: { label: "Minecraft", color: "#62b47a" },
  lutris: { label: "Lutris", color: "#ff9c2b" },
  custom: { label: "Custom", color: "#9aa0a6" },
};
const store = (s) => STORES[s] || { label: s, color: "#9aa0a6" };

const $ = (s) => document.querySelector(s);
const ls = {
  get: (k, d) => { try { const v = localStorage.getItem(k); return v === null ? d : JSON.parse(v); } catch { return d; } },
  set: (k, v) => { try { localStorage.setItem(k, JSON.stringify(v)); } catch {} },
};

const state = {
  games: [],
  view: ls.get("view", "all"),
  sort: ls.get("sort", "recent"),
  search: "",
  showUninstalled: ls.get("showUninstalled", false),
  minimize: ls.get("minimize", false),
  selected: ls.get("selected", null),
  running: new Map(),
  genre: "",
  series: "",
  letter: "",
  visible: [],
};

// ---------- helpers ----------
function imgSrc(p, width) {
  if (!p) return null;
  if (!/^https?:/.test(p)) return fileSrc(p);
  // Epic's CDN resizes on request: full key art is several MB, a grid tile needs ~360px.
  if (width && /\.epicgames\.com\//.test(p) && !p.includes("?")) return `${p}?h=${Math.round(width * 1.5)}&resize=1&w=${width}`;
  return p;
}
function fmtTime(sec) {
  if (!sec) return "Never played";
  const h = Math.floor(sec / 3600), m = Math.floor((sec % 3600) / 60);
  return h ? `${h}h ${m}m` : `${Math.max(m, 1)}m`;
}
function fmtAgo(ts) {
  if (!ts) return "—";
  const d = Date.now() / 1000 - ts;
  if (d < 90) return "just now";
  if (d < 3600) return `${Math.round(d / 60)} min ago`;
  if (d < 86400) return `${Math.round(d / 3600)} h ago`;
  if (d < 86400 * 30) return `${Math.round(d / 86400)} days ago`;
  return new Date(ts * 1000).toLocaleDateString();
}
function fmtSize(b) {
  if (!b) return null;
  return b > 1e9 ? `${(b / 1e9).toFixed(1)} GB` : b > 1e6 ? `${Math.round(b / 1e6)} MB` : `${Math.max(1, Math.round(b / 1e3))} KB`;
}
function hue(str) {
  let h = 0;
  for (const c of str) h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
}
function placeholder(g) {
  const h = hue(g.title);
  return `background:linear-gradient(160deg,hsl(${h} 55% 32%),hsl(${(h + 50) % 360} 60% 14%))`;
}
function toast(msg, err = false) {
  const t = document.createElement("div");
  t.className = "toast" + (err ? " err" : "");
  t.textContent = msg;
  $("#toasts").append(t);
  setTimeout(() => t.remove(), err ? 6000 : 3000);
}
const esc = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));

// ---------- data ----------
// ---------- genres / series / letters ----------
function prettySeries(s) {
  // Steam sometimes writes franchises in capitals ("FINAL FANTASY").
  s = s.replace(/[™®©]/g, "").trim();
  return s === s.toUpperCase() ? s.toLowerCase().replace(/\b\w/g, (c) => c.toUpperCase()) : s;
}
const seriesKey = (s) => s.toLowerCase().replace(/^the\s+/, "").replace(/[^a-z0-9]+/g, " ").trim();
/** Series per game, from the store/Wikidata data Lodestone fetched. Only series with 2+ games count. */
function computeSeries() {
  const names = new Map(), members = new Map(), of = new Map();
  for (const g of state.games) {
    if (!g.series) continue;
    const k = seriesKey(g.series);
    if (!k) continue;
    if (!names.has(k)) names.set(k, prettySeries(g.series));
    if (!members.has(k)) members.set(k, new Set());
    members.get(k).add(titleKey(g.title));
    of.set(g.id, k);
  }
  state.seriesOf = new Map([...of].filter(([, k]) => members.get(k).size >= 2));
  state.seriesNames = names;
}

// Genres of a game, including those of its other-store copies (Epic often lacks them).
function genresOf(g) {
  const key = titleKey(g.title);
  const set = new Set(g.genres || []);
  for (const c of (state.copies || new Map()).get(key) || []) for (const x of c.genres || []) set.add(x);
  return set;
}
function seriesOfGame(g) {
  const own = state.seriesOf && state.seriesOf.get(g.id);
  if (own) return own;
  for (const c of (state.copies || new Map()).get(titleKey(g.title)) || []) { const k = state.seriesOf && state.seriesOf.get(c.id); if (k) return k; }
  return null;
}
function letterOf(t) {
  const c = (t.replace(/^[^\p{L}\p{N}]+/u, "")[0] || "#").toUpperCase();
  return /[A-Z]/.test(c) ? c : "#";
}

function renderFilters(preLetter) {
  const show = state.view !== "minecraft";
  $("#filters").classList.toggle("hidden", !show);
  if (!show) return;
  const pool = preLetter || [];
  const gcount = new Map();
  for (const g of pool) for (const x of genresOf(g)) gcount.set(x, (gcount.get(x) || 0) + 1);
  const genres = [...gcount].sort((a, b) => a[0].localeCompare(b[0]));
  if (state.genre && !gcount.has(state.genre)) genres.push([state.genre, 0]);
  $("#f-genre").innerHTML = `<option value="">All genres</option>` + genres.map(([g, n]) => `<option value="${esc(g)}" ${g === state.genre ? "selected" : ""}>${esc(g)} (${n})</option>`).join("");
  const scount = new Map();
  for (const g of pool) { const k = seriesOfGame(g); if (k) scount.set(k, (scount.get(k) || 0) + 1); }
  const series = [...scount].filter(([, n]) => n >= 2 || false).map(([k, n]) => [k, state.seriesNames.get(k) || k, n]).sort((a, b) => a[1].localeCompare(b[1]));
  if (state.series && !scount.has(state.series)) series.push([state.series, state.seriesNames.get(state.series) || state.series, 0]);
  $("#f-series").innerHTML = `<option value="">All series</option>` + series.map(([k, name, n]) => `<option value="${esc(k)}" ${k === state.series ? "selected" : ""}>${esc(name)} (${n})</option>`).join("");
  const lc = new Map();
  for (const g of pool) { const l = letterOf(g.title); lc.set(l, (lc.get(l) || 0) + 1); }
  $("#f-letters").innerHTML = ["", "#", ..."ABCDEFGHIJKLMNOPQRSTUVWXYZ"].map((l) =>
    `<button data-letter="${l}" class="${state.letter === l ? "on" : ""}" ${l && !lc.get(l) ? "disabled" : ""}>${l || "All"}</button>`).join("");
  $("#f-clear").classList.toggle("hidden", !(state.genre || state.series || state.letter));
}

if (T) T.event.listen("series-updated", () => load(true));
async function load(quiet = false) {
  await loadWinapps();
  try {
    state.games = await invoke("list_games");
    keyCache.clear();
    try { state.kilnPlay = (await invoke("get_settings")).kiln_play !== false; } catch { state.kilnPlay = true; }
  } catch (e) {
    toast(String(e), true);
    state.games = [];
  }
  state.copies = copiesByKey();
  computeSeries();
  render();
  // Look up series for games not checked yet (no-op when everything is cached).
  if (state.games.length) invoke("refresh_series").catch(() => {});
  if (!quiet) {
    const n = state.games.filter((g) => g.installed).length;
    toast(`${n} installed games across ${new Set(state.games.map((g) => g.store)).size} stores`);
  }
}

// ---------- one entry per game across stores ----------
const DEFAULT_STORE_ORDER = ["steam", "epic", "gog", "battlenet", "amazon", "arenanet", "ea", "ubisoft", "rockstar", "lutris", "minecraft", "custom"];
function storeOrder() {
  const saved = ls.get("storeOrder", null);
  const order = Array.isArray(saved) ? saved.filter((s) => STORES[s]) : [];
  for (const s of DEFAULT_STORE_ORDER) if (!order.includes(s)) order.push(s);
  return order;
}
const MERGED_VIEWS = new Set(["all", "favorites", "recent"]);
const mergeOn = () => ls.get("mergeDupes", true) && MERGED_VIEWS.has(state.view);

// Same game on different stores: ignore case, accents, ™/®, punctuation and edition words.
const EDITION_WORDS = /\b(game of the year|goty|definitive|complete|deluxe|enhanced|ultimate|anniversary|remastered|special|standard|gold|digital|directors cut|edition|the)\b/g;
const keyCache = new Map();
function titleKey(t) {
  let k = keyCache.get(t);
  if (k === undefined) { k = titleKeyRaw(t); keyCache.set(t, k); }
  return k;
}
function titleKeyRaw(t) {
  return t.toLowerCase().normalize("NFKD").replace(/[\u0300-\u036f]/g, "")
    .replace(/[™®©]/g, "").replace(/&/g, " and ").replace(/['’]/g, "")
    .replace(/[^a-z0-9]+/g, " ").replace(EDITION_WORDS, " ").replace(/\s+/g, " ").trim();
}

// Rank copies: installed first, then your store order.
function bestCopy(copies) {
  const order = storeOrder();
  const rank = (g) => (state.running.has(g.id) ? 0 : 1) * 1000 + (g.installed ? 0 : 100) + (order.indexOf(g.store) + 1 || 99);
  return copies.slice().sort((a, b) => rank(a) - rank(b))[0];
}

// key -> every non-hidden copy (used for "Also in your library").
function copiesByKey() {
  const m = new Map();
  for (const g of state.games) {
    if (g.hidden) continue;
    const k = titleKey(g.title) || g.id;
    if (!m.has(k)) m.set(k, []);
    m.get(k).push(g);
  }
  return m;
}

function mergeList(list) {
  const groups = new Map();
  for (const g of list) {
    const k = titleKey(g.title) || g.id;
    if (!groups.has(k)) groups.set(k, []);
    groups.get(k).push(g);
  }
  return [...groups.values()].map(bestCopy);
}

function filtered() {
  const q = state.search.trim().toLowerCase();
  // Battle.net / ArenaNet with nothing installed yet: show what you own instead of an empty page.
  const anyInstalled = state.games.some((g) => g.store === state.view && g.installed);
  let list = state.games.filter((g) => {
    if (state.view === "hidden") return g.hidden;
    if (g.hidden) return false;
    if (!state.showUninstalled && !g.installed && state.view !== "favorites" && !(WINAPP_FOR[state.view] && !anyInstalled)) return false;
    if (state.view === "favorites" && !g.favorite) return false;
    if (state.view === "recent" && !g.last_played) return false;
    if (state.view === "modded" && !(g.kiln && (g.installed || g.kiln_managed))) return false;
    if (STORES[state.view] && g.store !== state.view) return false;
    if (q && !g.title.toLowerCase().includes(q)) return false;
    return true;
  });
  const by = {
    title: (a, b) => a.title.localeCompare(b.title),
    recent: (a, b) => b.last_played - a.last_played || a.title.localeCompare(b.title),
    playtime: (a, b) => b.playtime - a.playtime || a.title.localeCompare(b.title),
    size: (a, b) => (b.size || 0) - (a.size || 0),
  }[state.view === "recent" ? "recent" : state.sort];
  if (mergeOn()) list = mergeList(list);
  if (state.genre) list = list.filter((g) => genresOf(g).has(state.genre));
  if (state.series) list = list.filter((g) => seriesOfGame(g) === state.series);
  state.preLetter = list;
  if (state.letter) list = list.filter((g) => letterOf(g.title) === state.letter);
  if (state.view === "modded") list.sort((a, b) => (b.kiln_managed - a.kiln_managed) || (b.installed - a.installed) || by(a, b));
  else list.sort((a, b) => (b.installed - a.installed) || by(a, b));
  return list;
}

// ---------- render ----------
function renderNav() {
  const visible = state.games.filter((g) => !g.hidden);
  const inst = (g) => g.installed || state.showUninstalled;
  const lib = [
    ["all", "All games", ls.get("mergeDupes", true) ? mergeList(visible.filter(inst)).length : visible.filter(inst).length],
    ["favorites", "Favorites", visible.filter((g) => g.favorite).length],
    ["recent", "Recently played", visible.filter((g) => g.last_played && inst(g)).length],
  ];
  const kilnN = visible.filter((g) => g.kiln && (g.installed || g.kiln_managed)).length;
  if (kilnN) lib.push(["modded", "Modded (Kiln)", kilnN]);
  const hiddenN = state.games.filter((g) => g.hidden).length;
  if (hiddenN) lib.push(["hidden", "Hidden", hiddenN]);
  $("#nav-library").innerHTML = lib
    .map(([k, l, n]) => `<button data-view="${k}" class="${state.view === k ? "active" : ""}">${l}<span class="count">${n}</span></button>`)
    .join("");

  const counts = {};
  for (const g of visible) if (inst(g)) counts[g.store] = (counts[g.store] || 0) + 1;
  $("#nav-stores").innerHTML = Object.keys(STORES)
    .filter((k) => counts[k] || ["battlenet", "arenanet"].includes(k))
    .map((k) => `<button data-view="${k}" class="${state.view === k ? "active" : ""}"><span class="dot" style="background:${STORES[k].color}"></span>${STORES[k].label}<span class="count">${counts[k] || 0}</span></button>`)
    .join("");
}

function cardHTML(g) {
  const s = store(g.store);
  const others = mergeOn() ? (state.copies.get(titleKey(g.title)) || []).filter((c) => c.id !== g.id) : [];
  const src = imgSrc(g.cover, 360);
  const run = state.running.get(g.id);
  const isSel = g.id === state.selected || (others.length && state.selKey === titleKey(g.title));
  const cls = ["card", g.installed ? "" : "uninstalled", isSel ? "selected" : "", run ? "running" : ""].join(" ");
  const dots = [g, ...others].map((c) => store(c.store)).filter((v, i, a) => a.findIndex((x) => x.label === v.label) === i);
  return `<div class="${cls}" data-id="${esc(g.id)}" data-key="${esc(titleKey(g.title))}">
    <div class="art" style="${placeholder(g)}">
      <div class="ph">${esc(g.title)}</div>
      ${src ? `<img loading="lazy" decoding="async" src="${esc(src)}" alt="" onerror="this.remove()">` : ""}
      <span class="badges">${dots.map((d) => `<span class="badge" style="background:${d.color}" title="${d.label}"></span>`).join("")}</span>
      ${g.favorite ? '<span class="fav">★</span>' : ""}
      ${g.kiln_managed ? '<span class="modded" title="Modded with Kiln">MODDED</span>' : ""}
      ${run ? `<div class="state">${run.state === "launching" ? "Launching…" : "Running"}</div>` : ""}
    </div>
    <div class="title" title="${esc(g.title)}">${esc(g.title)}</div>
    <div class="sub">${g.installed ? (g.playtime ? fmtTime(g.playtime) : s.label) : "Not installed"}${others.length ? ` · +${others.length} store${others.length > 1 ? "s" : ""}` : ""}</div>
  </div>`;
}

const WINAPP_FOR = { battlenet: "battlenet", arenanet: "gw2" };
function renderSetup() {
  const el = $("#setup");
  const app = (state.winapps || []).find((a) => a.id === WINAPP_FOR[state.view]);
  if (!app || (app.installed && app.id === "gw2")) return el.classList.add("hidden");
  el.classList.remove("hidden");
  const busy = state.setupBusy === app.id;
  if (!app.installed) {
    el.innerHTML = app.id === "battlenet"
      ? `<div><h2>Battle.net on Proton</h2><p class="muted">Lodestone downloads Blizzard's official installer and installs Battle.net into its own Proton prefix: no Steam, Lutris or Wine setup needed. Sign in with your Blizzard account in the window that opens, install your games there, then Rescan. The first run also downloads the Proton runtime, so it can take a few minutes.</p></div>
         <button class="play" data-winsetup="battlenet" ${busy ? "disabled" : ""}>${busy ? "Setting up…" : "Set up Battle.net"}</button>`
      : `<div><h2>Guild Wars 2 on Proton</h2><p class="muted">Lodestone downloads ArenaNet's official installer and runs it in its own Proton prefix. Sign in with your ArenaNet account in the game's launcher.</p></div>
         <button class="play" data-winsetup="gw2" ${busy ? "disabled" : ""}>${busy ? "Setting up…" : "Install Guild Wars 2"}</button>`;
  } else {
    el.innerHTML = `<div><h2>Battle.net is set up</h2><p class="muted">Your owned and free Battle.net games are below. <b>Install</b> opens Battle.net on that game's page; after it finishes, Rescan and play it from Lodestone. Tip: in Battle.net's settings, set "When I launch a game" to exit or minimize to tray so its window stays out of the way.</p></div>
      <div class="row-btns"><button class="ghost" data-winopen="battlenet">Open Battle.net</button><button class="play" onclick="document.getElementById('btn-refresh').click()">Rescan</button></div>`;
  }
}

async function loadWinapps() {
  try { state.winapps = await invoke("winapp_status"); } catch { state.winapps = []; }
}

function renderGrid() {
  renderSetup();
  state.copies = copiesByKey();
  const selG = state.games.find((x) => x.id === state.selected);
  state.selKey = selG ? titleKey(selG.title) : null;
  const list = filtered();
  state.visible = list;
  renderFilters(state.preLetter);
  $("#grid").innerHTML = list.map(cardHTML).join("");
  const title = { all: "All games", favorites: "Favorites", recent: "Recently played", hidden: "Hidden", modded: "Modded with Kiln" }[state.view] || store(state.view).label;
  $("#grid-title").textContent = title;
  $("#grid-count").textContent = `${list.length} game${list.length === 1 ? "" : "s"}`;
  const empty = $("#empty");
  if (!list.length && WINAPP_FOR[state.view]) {
    empty.classList.add("hidden");
  } else if (!list.length) {
    empty.classList.remove("hidden");
    empty.textContent = state.search ? `Nothing matches “${state.search}”.` : (state.genre || state.series || state.letter) ? "No games match these filters." : state.view === "favorites" ? "Star a game to pin it here." : "No games here yet.";
  } else empty.classList.add("hidden");
  const selGame = state.games.find((g) => g.id === state.selected);
  const selShown = list.find((g) => g.id === state.selected) || (selGame && mergeOn() && list.find((g) => titleKey(g.title) === titleKey(selGame.title)));
  if (!selShown) {
    const fallback = list.find((g) => g.installed) || list[0];
    state.selected = fallback ? fallback.id : null;
    const el = fallback && $(`.card[data-id="${CSS.escape(fallback.id)}"]`);
    el && el.classList.add("selected");
  }
}

function renderHero() {
  const g = state.games.find((x) => x.id === state.selected);
  const hero = $("#hero");
  if (!g) return hero.classList.add("empty");
  hero.classList.remove("empty");
  const s = store(g.store);
  const bg = imgSrc(g.hero, 1600) || imgSrc(g.cover, 1600);
  hero.querySelector(".hero-bg").style.backgroundImage = bg ? `url("${bg}")` : "";
  hero.querySelector(".hero-bg").style.cssText += bg ? "" : placeholder(g);
  const cover = hero.querySelector(".hero-cover");
  const csrc = imgSrc(g.cover, 320);
  cover.style.cssText = placeholder(g);
  if (csrc) { cover.src = csrc; cover.style.visibility = ""; } else { cover.removeAttribute("src"); cover.style.visibility = "hidden"; }
  cover.onerror = () => { cover.style.visibility = "hidden"; };
  hero.querySelector(".hero-store").innerHTML = `<span class="dot" style="background:${s.color}"></span>${s.label} · via ${esc(g.via)}`;
  hero.querySelector(".hero-title").textContent = g.title;
  const meta = [`Played <b>${fmtTime(g.playtime)}</b>`, `Last played <b>${fmtAgo(g.last_played)}</b>`];
  const size = fmtSize(g.size);
  if (size) meta.push(`<b>${size}</b>`);
  hero.querySelector(".hero-meta").innerHTML = meta.map((m) => `<span>${m}</span>`).join("");
  const tags = [...genresOf(g)].map((x) => `<button class="tag" data-pick-genre="${esc(x)}">${esc(x)}</button>`);
  const sk = seriesOfGame(g);
  if (sk) tags.push(`<button class="tag series" data-pick-series="${esc(sk)}">${esc(state.seriesNames.get(sk) || sk)} series</button>`);
  hero.querySelector(".hero-tags").innerHTML = tags.join("");
  renderKiln(g);
  const copies = ((state.copies || copiesByKey()).get(titleKey(g.title)) || []);
  const alsoEl = hero.querySelector(".hero-also");
  if (copies.length > 1) {
    alsoEl.classList.remove("hidden");
    alsoEl.innerHTML = `<span class="muted">In your library on</span>` + copies
      .slice().sort((a, b) => storeOrder().indexOf(a.store) - storeOrder().indexOf(b.store))
      .map((c) => `<button class="copy ${c.id === g.id ? "on" : ""}" data-copy="${esc(c.id)}"><span class="dot" style="background:${store(c.store).color}"></span>${store(c.store).label}<small>${c.installed ? "installed" : "not installed"}</small></button>`).join("");
  } else alsoEl.classList.add("hidden");

  const run = state.running.get(g.id);
  const play = $("#btn-play");
  play.classList.toggle("running", !!run);
  play.disabled = (!g.installed && !g.can_install) || !!run;
  play.textContent = !g.installed ? (g.can_install ? "⤓ Install" : "Not installed") : run ? (run.state === "launching" ? "Launching…" : "● Running") : g.kiln_managed && state.kilnPlay ? "▶ Play modded" : "▶ Play";
  play.title = g.kiln_managed && state.kilnPlay ? "Launches through Kiln, which checks your mods first" : "";
  $("#btn-play-menu").classList.toggle("hidden", !(g.kiln_managed && g.installed && !run));
  play.classList.toggle("split", !!(g.kiln_managed && g.installed && !run));
  $("#btn-workshop").classList.toggle("hidden", !g.workshop);
  $('#menu [data-act="uninstall"]').classList.toggle("hidden", !(g.installed && g.can_uninstall));
  $('#menu [data-act="runtimes"]').classList.toggle("hidden", !(g.installed && g.prefix));
  const fav = $("#btn-fav");
  fav.textContent = g.favorite ? "★" : "☆";
  fav.classList.toggle("on", g.favorite);
  $('#menu [data-act="remove"]').classList.toggle("hidden", !g.id.startsWith("custom:"));
  $('#menu [data-act="hide"]').textContent = g.hidden ? "Unhide" : "Hide from library";
  $('#menu [data-act="client"]').textContent = g.installed ? `Open ${s.label === "Custom" ? "Lutris" : g.via.split(" ")[0]}` : `Install with ${g.via.split(" ")[0]}`;
  renderMini(g);
}

// ---------- in-app store browser ----------
let shopOpen = false, shopSuspended = false;
function shopRect() {
  const r = $("#shop-frame").getBoundingClientRect();
  return { x: r.left, y: r.top, w: r.width, h: r.height };
}
function shopShow() {
  if (!state.view.startsWith("shop:") || shopSuspended) return;
  document.querySelector("main").scrollTop = 0;
  const store = state.view.slice(5);
  invoke("shop_show", { store, ...shopRect() }).then(() => (shopOpen = true)).catch((e) => toast(String(e), true));
}
function shopHide() {
  if (shopOpen) invoke("shop_hide").catch(() => {});
  shopOpen = false;
}
// Native webviews draw above the page, so step aside while a dialog or the setup guide is up.
function shopSuspend(on) {
  shopSuspended = on;
  if (on) shopHide();
  else if (state.view.startsWith("shop:")) shopShow();
}
const _showModal = HTMLDialogElement.prototype.showModal;
HTMLDialogElement.prototype.showModal = function () {
  shopSuspend(true);
  this.addEventListener("close", () => shopSuspend(false), { once: true });
  return _showModal.call(this);
};
new ResizeObserver(() => { if (shopOpen) invoke("shop_bounds", shopRect()).catch(() => {}); }).observe($("#shop-frame"));
window.addEventListener("resize", () => { if (shopOpen) invoke("shop_bounds", shopRect()).catch(() => {}); });
document.querySelector(".shop-bar").addEventListener("click", (e) => {
  const b = e.target.closest("[data-shop-nav]");
  if (b) invoke("shop_nav", { store: state.view.slice(5), action: b.dataset.shopNav }).catch((err) => toast(String(err), true));
});
$("#shop-external").onclick = () => invoke("shop_open_external").catch((e) => toast(String(e), true));
if (T) T.event.listen("shop-page", ({ payload }) => {
  try { const u = new URL(payload.url); $("#shop-url").textContent = u.host + (u.pathname.length > 1 ? u.pathname : ""); } catch { $("#shop-url").textContent = payload.url; }
});

// ---------- Kiln (mod manager) ----------
const kilnCache = new Map(); // kiln game id -> {at, data}
async function renderKiln(g) {
  const el = $("#hero .hero-kiln");
  if (!g.kiln) return el.classList.add("hidden");
  el.classList.remove("hidden");
  if (!g.kiln_managed) {
    el.innerHTML = `<span class="kiln-logo">Kiln</span><span class="muted">${g.installed ? "Kiln can mod this game: Nexus mods, load order, profiles and crash checks." : "Kiln can mod this game once it's installed."}</span>
      ${g.installed ? `<button class="ghost small-btn" data-kiln-open="${esc(g.kiln)}">Set up mods in Kiln</button>` : ""}`;
    return;
  }
  const cached = kilnCache.get(g.kiln);
  const show = (d) => {
    if (state.selected !== g.id) return;
    if (!d) { el.innerHTML = `<span class="kiln-logo">Kiln</span><span class="muted">Modded with Kiln</span><button class="ghost small-btn" data-kiln-open="${esc(g.kiln)}">Manage mods</button>`; return; }
    const issues = d.issues || [];
    el.innerHTML = `<span class="kiln-logo">Kiln</span>
      <span><b>${esc(d.profile || "Default")}</b> profile · <b>${d.enabled}</b> of ${d.mods} mods on${d.plugins ? ` · ${d.plugins} plugins` : ""} · ${d.vanilla ? '<span class="muted">mods taken out for an unmodded game; Play modded puts them back</span>' : d.deployed ? '<span class="ok-text">deployed</span>' : '<span class="warn-text">not deployed</span>'}</span>
      ${issues.length ? `<span class="warn-text" title="${esc(issues.map((i) => i.title).join("\n"))}">⚠ ${issues.length} to fix before playing</span>` : ""}
      <button class="ghost small-btn" data-kiln-open="${esc(g.kiln)}">Manage mods</button>`;
  };
  show(cached && cached.data);
  if (cached && Date.now() - cached.at < 60000) return;
  try {
    const d = await invoke("kiln_status", { game: g.kiln });
    kilnCache.set(g.kiln, { at: Date.now(), data: d });
    show(d);
  } catch { show(null); }
}

// Compact bar pinned under the search bar once the big header scrolls away.
function renderMini(g) {
  const m = $("#mini");
  if (!g) return m.classList.add("hidden");
  const src = imgSrc(g.cover, 120);
  m.querySelector(".mini-art").style.cssText = src ? `background-image:url("${src}")` : placeholder(g);
  m.querySelector(".mini-title").textContent = g.title;
  m.querySelector(".mini-store").innerHTML = `<span class="dot" style="background:${store(g.store).color}"></span>${store(g.store).label}`;
  const p = $("#btn-play"), mp = $("#mini-play");
  mp.textContent = p.textContent;
  mp.disabled = p.disabled;
  mp.className = p.className;
  $("#mini-fav").textContent = g.favorite ? "★" : "☆";
  $("#mini-fav").classList.toggle("on", g.favorite);
  updateMiniVisibility();
}
let heroVisible = true;
function updateMiniVisibility() {
  const show = !heroVisible && state.view !== "minecraft" && !$("#hero").classList.contains("empty") && !$("#hero").classList.contains("hidden");
  $("#mini").classList.toggle("hidden", !show);
}
new IntersectionObserver(([e]) => { heroVisible = e.isIntersecting; updateMiniVisibility(); }, { root: document.querySelector("main"), rootMargin: "-80px 0px 0px 0px", threshold: 0.15 }).observe($("#hero"));
$("#mini-play").onclick = () => $("#btn-play").click();
$("#mini-fav").onclick = () => $("#btn-fav").click();
$("#mini-more").onclick = () => { $("#hero").scrollIntoView({ behavior: "smooth", block: "start" }); };

function renderRunning() {
  $("#running").innerHTML = [...state.running.values()]
    .map((r) => `<div class="run-pill ${r.state}"><span class="pulse"></span><span>${esc(r.title)}</span>
      <small>${r.state === "launching" ? "starting…" : fmtTime(r.seconds)}</small>
      <button title="Stop tracking (doesn't close the game)" data-forget="${esc(r.id)}">✕</button></div>`)
    .join("");
}

const SHOPS = [
  ["steam", "Steam Store"],
  ["epic", "Epic Games Store"],
  ["gog", "GOG"],
  ["battlenet", "Battle.net Shop"],
];
function render() {
  renderNav();
  $("#nav-shop").innerHTML = SHOPS.map(([k, l]) => `<button data-view="shop:${k}" class="${state.view === "shop:" + k ? "active" : ""}"><span class="dot" style="background:${store(k).color}"></span>${l}</button>`).join("");
  const shopView = state.view.startsWith("shop:");
  const mc = state.view === "minecraft" || shopView;
  for (const el of ["#hero", ".grid-head", "#grid", "#setup", "#filters"]) $(el).classList.toggle("hidden", mc);
  $("#mc").classList.toggle("hidden", state.view !== "minecraft");
  $("#shop").classList.toggle("hidden", !shopView);
  document.querySelector("main").classList.toggle("no-scroll", shopView);
  if (shopView) {
    $("#empty").classList.add("hidden");
    $("#mini").classList.add("hidden");
    requestAnimationFrame(shopShow);
    renderRunning();
    return;
  }
  shopHide();
  if (mc) {
    $("#empty").classList.add("hidden");
    window.mcShow && window.mcShow();
  } else {
    renderGrid();
    renderHero();
  }
  renderRunning();
}

async function winSetup(id) {
  state.setupBusy = id;
  renderSetup();
  toast("Downloading the official installer (and Proton tools on first use)…");
  try {
    await invoke("winapp_setup", { id });
    toast("Installer is starting. Follow it in its window; this can take a minute on first run.");
    // Poll until the client shows up, then refresh the banner.
    for (let i = 0; i < 120; i++) {
      await new Promise((r) => setTimeout(r, 5000));
      await loadWinapps();
      if (state.winapps.find((a) => a.id === id)?.installed) { toast("Setup finished"); await load(true); break; }
    }
  } catch (e) {
    toast(String(e), true);
  } finally {
    state.setupBusy = null;
    renderSetup();
  }
}

// ---------- accounts ----------
const ACCT = [
  { id: "epic", name: "Epic Games", color: "#e6e6e6", login: true },
  { id: "gog", name: "GOG", color: "#b26bff", login: true },
  { id: "steam", name: "Steam", color: "#66c0f4" },
  { id: "battlenet", name: "Battle.net", color: "#148eff" },
];
const acctBusy = new Set();
async function renderAccounts() {
  let a;
  try { a = await invoke("accounts"); } catch (e) { return toast(String(e), true); }
  const html = ACCT.map((s) => {
    const info = a[s.id] || {};
    const busy = acctBusy.has(s.id);
    let status, actions = "";
    if (s.login) {
      status = info.account ? `Signed in as <b>${esc(info.account)}</b>` : "Not signed in";
      actions = info.account
        ? `<button class="ghost" data-acct-out="${s.id}">Sign out</button>`
        : `${info.heroic ? `<button class="ghost" data-acct-heroic="${s.id}" title="Moves Heroic's sign-in to Lodestone; Heroic will ask you to sign in again">Use Heroic's sign-in</button>` : ""}
           <button class="play small" data-acct-in="${s.id}" ${busy ? "disabled" : ""}>${busy ? "Waiting…" : "Sign in"}</button>`;
    } else if (s.id === "steam") {
      status = info.account ? `Signed in as <b>${esc(info.account)}</b> in the Steam client (kept in the tray)` : "Sign in once in the Steam client";
      actions = `<button class="ghost" data-acct-steam>Open Steam</button>`;
    } else {
      status = info.installed ? "Sign in inside Battle.net" : "Not set up yet";
      actions = info.installed ? `<button class="ghost" data-winopen="battlenet">Open Battle.net</button>` : `<button class="ghost" data-winsetup="battlenet">Set up Battle.net</button>`;
    }
    const paste = s.login && !info.account
      ? `<details class="paste"><summary>Sign-in window not working?</summary>
          <p class="hint">Sign in at the store in your normal browser, then paste the code it shows.
          ${s.id === "epic" ? `After signing in, <a href="#" data-href="https://www.epicgames.com/id/api/redirect?clientId=34a02cf8f4414e29b15921876da36f9a&responseType=code">open this page</a> and copy the <code>authorizationCode</code> value.` : `<a href="#" data-href="https://auth.gog.com/auth?client_id=46899977096215655&redirect_uri=https%3A%2F%2Fembed.gog.com%2Fon_login_success%3Forigin%3Dclient&response_type=code&layout=client2">Sign in here</a>; when the page goes blank, copy the <code>code=</code> value from its address.`}</p>
          <div class="row-line"><input placeholder="Paste code" data-code-for="${s.id}" /><button class="ghost" data-acct-code="${s.id}">Use code</button></div></details>`
      : "";
    return `<div class="acct"><span class="dot" style="background:${s.color}"></span>
      <div class="grow"><div class="t">${s.name}</div><small class="muted">${status}</small>${paste}</div>
      <div class="row-btns">${actions}</div></div>`;
  }).join("");
  document.querySelectorAll(".acct-list").forEach((el) => (el.innerHTML = html));
}
$("#btn-accounts").onclick = () => { $("#dlg-accounts").showModal(); renderAccounts(); };
async function onAccountClick(e) {
  const t = e.target.closest("button");
  if (!t) return;
  try {
    if (t.dataset.acctIn) {
      acctBusy.add(t.dataset.acctIn); renderAccounts();
      await invoke("store_login", { store: t.dataset.acctIn });
    } else if (t.dataset.acctOut) {
      if (!confirm("Sign out? Installed games stay on disk.")) return;
      await invoke("store_logout", { store: t.dataset.acctOut });
      toast("Signed out"); renderAccounts(); load(true);
    } else if (t.dataset.acctHeroic) {
      if (!confirm("Move Heroic's sign-in to Lodestone? Heroic will ask you to sign in again next time you open it.")) return;
      t.disabled = true; t.textContent = "Importing…";
      const name = await invoke("store_import_heroic", { store: t.dataset.acctHeroic });
      toast(`Signed in${name ? " as " + name : ""}`); renderAccounts(); load(true);
    } else if (t.dataset.acctCode) {
      const code = t.closest(".acct").querySelector(`[data-code-for="${t.dataset.acctCode}"]`).value;
      acctBusy.add(t.dataset.acctCode); renderAccounts();
      await invoke("store_login_code", { store: t.dataset.acctCode, code });
    } else if (t.hasAttribute("data-acct-steam")) {
      await invoke("open_client", { store: "steam" });
    }
  } catch (err) {
    toast(String(err), true);
    acctBusy.clear(); renderAccounts();
  }
}
document.querySelectorAll(".acct-list").forEach((el) => el.addEventListener("click", onAccountClick));
if (T) T.event.listen("account", ({ payload }) => {
  acctBusy.delete(payload.store);
  if (payload.ok) { toast(`Signed in to ${payload.store === "epic" ? "Epic Games" : "GOG"} as ${payload.account}`); load(true); }
  else toast(`Sign-in failed: ${payload.error}`, true);
  if ($("#dlg-accounts").open || !$("#onboard").classList.contains("hidden")) renderAccounts();
});

// ---------- downloads ----------
let dlTimer = null;
const dlSeen = new Map();
async function pollDownloads() {
  clearTimeout(dlTimer);
  let list = [];
  try { list = (await invoke("downloads_status")) || []; } catch {}
  for (const d of list) {
    const prev = dlSeen.get(d.id);
    if (prev === "downloading" && d.state === "done") { toast(`${d.title} is installed`); load(true); }
    if (prev === "downloading" && d.state === "failed") toast(`${d.title} failed: ${d.detail}`, true);
    dlSeen.set(d.id, d.state);
  }
  $("#downloads").innerHTML = list.map((d) => `<div class="dl ${d.state}">
      <div class="dl-top"><span class="grow">${esc(d.title)}</span>
        <small class="muted">${d.state === "downloading" ? `${d.percent.toFixed(1)}%` : d.state === "done" ? "Installed" : "Failed"}</small>
        <button title="${d.state === "downloading" ? "Cancel" : "Dismiss"}" data-dl="${esc(d.id)}" data-dl-state="${d.state}">✕</button></div>
      <div class="bar"><span style="width:${Math.max(2, d.percent)}%"></span></div>
      ${d.state === "failed" ? `<small class="err-text">${esc(d.detail)}</small>` : ""}
    </div>`).join("");
  if (list.some((d) => d.state === "downloading")) dlTimer = setTimeout(pollDownloads, 1500);
}
$("#downloads").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-dl]");
  if (!b) return;
  if (b.dataset.dlState === "downloading") {
    if (!confirm("Cancel this download? You can resume it later by installing again.")) return;
    await invoke("download_cancel", { id: b.dataset.dl });
  } else await invoke("download_dismiss", { id: b.dataset.dl });
  pollDownloads();
});
pollDownloads();

// ---------- Steam Workshop ----------
let wsApp = null;
async function openWorkshop(g) {
  wsApp = g.workshop;
  $("#ws-title").textContent = `${g.title}: Workshop`;
  $("#ws-list").innerHTML = '<div class="muted pad">Loading installed items…</div>';
  $("#dlg-workshop").showModal();
  try {
    const items = await invoke("workshop_items", { appid: g.workshop });
    $("#ws-list").innerHTML = items.length
      ? `<div class="muted pad">${items.length} subscribed item${items.length === 1 ? "" : "s"} installed</div>` +
        items.map((i) => `<div class="ws-item" data-ws="${esc(i.id)}" title="Open item page">
          <div class="ws-thumb" style="${i.preview ? `background-image:url('${esc(i.preview)}')` : ""}"></div>
          <div class="grow"><div>${esc(i.title)}</div><small class="muted">${fmtSize(i.size) || ""}${i.updated ? " · updated " + new Date(i.updated * 1000).toLocaleDateString() : ""}</small></div>
          <span class="muted">›</span></div>`).join("")
      : '<div class="muted pad">No Workshop items installed for this game yet.</div>';
  } catch (e) {
    $("#ws-list").innerHTML = `<div class="pad err-text">${esc(String(e))}</div>`;
  }
}
$("#btn-workshop").onclick = () => {
  const g = state.games.find((x) => x.id === state.selected);
  if (g && g.workshop) openWorkshop(g);
};
$("#ws-browse").onclick = () => invoke("open_workshop", { appid: wsApp, item: null, inSteam: false }).catch((e) => toast(String(e), true));
$("#ws-steam").onclick = () => invoke("open_workshop", { appid: wsApp, item: null, inSteam: true }).catch((e) => toast(String(e), true));
$("#ws-list").onclick = (e) => {
  const it = e.target.closest("[data-ws]");
  if (it) invoke("open_workshop", { appid: wsApp, item: it.dataset.ws, inSteam: false }).catch((err) => toast(String(err), true));
};

// ---------- settings ----------
async function renderProton() {
  const p = await invoke("proton_info");
  document.querySelectorAll(".umu-status").forEach((el) => (el.textContent = p.umu ? `Proton runner ready (umu-launcher ${p.umu})` : "Proton runner not set up yet"));
  document.querySelectorAll(".umu-setup").forEach((el) => { el.textContent = p.umu ? "Update" : "Set up Proton"; el.disabled = false; });
  const opts = [["", `Automatic (${p.effective === p.auto ? "download latest GE-Proton" : p.effective.split("/").pop()})`]]
    .concat(p.protons.map((x) => [x.path, x.name]))
    .concat([[p.auto, "Latest GE-Proton (download automatically)"]]);
  const html = opts.map(([v, l]) => `<option value="${esc(v)}" ${v === p.selected ? "selected" : ""}>${esc(l)}</option>`).join("");
  document.querySelectorAll(".proton-select").forEach((el) => (el.innerHTML = html));
  return p;
}
document.querySelectorAll(".umu-setup").forEach((btn) => (btn.onclick = async () => {
  document.querySelectorAll(".umu-status").forEach((el) => (el.textContent = "Downloading umu-launcher…"));
  btn.disabled = true;
  try { const v = await invoke("setup_proton"); toast(`Proton runner ready (umu ${v})`); } catch (e) { toast(String(e), true); }
  renderProton();
}));
function renderStoreOrder() {
  const order = storeOrder();
  $("#merge-dupes").checked = ls.get("mergeDupes", true);
  const html = order.map((k, i) => `<div class="so"><span class="n">${i + 1}</span><span class="dot" style="background:${STORES[k].color}"></span><span class="grow">${STORES[k].label}</span>
    <button type="button" data-mv="${k}" data-d="-1" ${i === 0 ? "disabled" : ""} title="Prefer this store">↑</button>
    <button type="button" data-mv="${k}" data-d="1" ${i === order.length - 1 ? "disabled" : ""} title="Prefer it less">↓</button></div>`).join("");
  document.querySelectorAll(".store-order").forEach((el) => (el.innerHTML = html));
}
const onStoreOrderClick = (e) => {
  const b = e.target.closest("[data-mv]");
  if (!b) return;
  const order = storeOrder();
  const i = order.indexOf(b.dataset.mv), j = i + Number(b.dataset.d);
  [order[i], order[j]] = [order[j], order[i]];
  ls.set("storeOrder", order);
  renderStoreOrder();
  render();
};
document.querySelectorAll(".store-order").forEach((el) => (el.onclick = onStoreOrderClick));
$("#kiln-play").onchange = async (e) => { await invoke("set_kiln_play", { on: e.target.checked }); state.kilnPlay = e.target.checked; renderHero(); };
$("#merge-dupes").onchange = (e) => { ls.set("mergeDupes", e.target.checked); render(); };
$("#btn-settings").onclick = async () => {
  renderStoreOrder();
  const s = await invoke("get_settings");
  $("#kiln-play").checked = s.kiln_play !== false;
  $("#form-settings").cf.value = s.curseforge_api_key || "";
  $("#form-settings").gamesdir.value = s.games_dir || "";
  $("#form-settings").steamkey.value = s.steam_api_key || "";
  renderProton().catch((e) => toast(String(e), true));
  $("#dlg-settings").showModal();
};
$("#form-settings").onsubmit = async (e) => {
  e.preventDefault();
  try {
    const keyChanged = e.target.steamkey.value.trim() !== ((await invoke("get_settings")).steam_api_key || "");
    await invoke("save_settings", { curseforgeApiKey: e.target.cf.value, gamesDir: e.target.gamesdir.value, steamApiKey: e.target.steamkey.value });
    await invoke("set_proton", { path: e.target.proton.value });
    $("#dlg-settings").close();
    toast("Settings saved");
    if (keyChanged && e.target.steamkey.value.trim()) {
      toast("Checking your Steam library…");
      try { await invoke("store_refresh"); toast("Steam library updated from your account"); } catch (err) { toast(String(err), true); }
      load(true);
    }
    window.mcSettingsChanged && window.mcSettingsChanged();
  } catch (err) {
    toast(String(err), true);
  }
};
document.addEventListener("click", (e) => {
  const a = e.target.closest("[data-href]");
  if (a) {
    e.preventDefault();
    invoke("open_url", { url: a.dataset.href }).catch((err) => toast(String(err), true));
  }
});

function select(id, scroll = false) {
  state.selected = id;
  ls.set("selected", id);
  document.querySelectorAll(".card.selected").forEach((c) => c.classList.remove("selected"));
  const g = state.games.find((x) => x.id === id);
  const el = $(`.card[data-id="${CSS.escape(id)}"]`) || (g && $(`.card[data-key="${CSS.escape(titleKey(g.title))}"]`));
  if (el) {
    el.classList.add("selected");
    if (scroll) el.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }
  renderHero();
}

// ---------- actions ----------
async function play(id, mode = null) {
  const g = state.games.find((x) => x.id === id);
  if (!g || !g.installed) return;
  try {
    await invoke("launch_game", { id, mode });
    toast(mode === "vanilla" ? `Starting ${g.title} without mods (Kiln takes them out; Play modded puts them back)…` : `Starting ${g.title}…`);
    if (g.kiln) kilnCache.delete(g.kiln);
    g.last_played = Date.now() / 1000;
    if (state.minimize && T) setTimeout(() => T.window.getCurrentWindow().minimize(), 600);
  } catch (e) {
    toast(String(e), true);
  }
}

async function install(g) {
  try {
    const how = await invoke("install_game", { id: g.id });
    if (how === "download") {
      toast(`Downloading ${g.title}…`);
      pollDownloads();
    } else {
      toast(g.store === "steam" ? `Steam's install dialog is opening for ${g.title}. Rescan when it's done.` : `Opening ${g.via.split(" ·")[0]} to install ${g.title}…`);
    }
  } catch (e) {
    toast(String(e), true);
  }
}

async function setFlag(id, flag, value) {
  const g = state.games.find((x) => x.id === id);
  await invoke("set_flag", { id, flag, value });
  g[flag] = value;
  render();
}

async function menuAction(act) {
  $("#menu").classList.add("hidden");
  const g = state.games.find((x) => x.id === state.selected);
  if (!g) return;
  try {
    if (act === "hide") await setFlag(g.id, "hidden", !g.hidden);
    if (act === "log") {
      $("#log-text").textContent = await invoke("read_log", { id: g.id });
      $("#dlg-log").showModal();
    }
    if (act === "client") {
      if (!g.installed && g.can_install) await install(g);
      else await invoke("open_client", { store: g.store });
    }
    if (act === "runtimes") {
      await invoke("install_runtimes", { id: g.id });
      toast(`Installing DirectX and Visual C++ runtimes for ${g.title}. Takes a minute or two the first time.`);
      pollDownloads();
    }
    if (act === "uninstall") {
      const own = g.via.startsWith("Lodestone");
      if (own && !confirm(g.store === "gog" ? `Uninstall ${g.title}? Its folder goes to the trash.` : `Uninstall ${g.title}? Its files are deleted.`)) return;
      const how = await invoke("uninstall_game", { id: g.id });
      if (how === "done") { toast(`Uninstalled ${g.title}`); await load(true); }
      else toast(`${g.via.split(" ·")[0]} will ask to confirm uninstalling ${g.title}.`);
    }
    if (act === "remove" && confirm(`Remove “${g.title}” from Lodestone? (Doesn't delete any files.)`)) {
      await invoke("remove_custom_game", { id: g.id });
      await load(true);
    }
  } catch (e) {
    toast(String(e), true);
  }
}

// ---------- events ----------
document.addEventListener("click", (e) => {
  const nav = e.target.closest("[data-view]");
  if (nav) {
    state.view = nav.dataset.view;
    ls.set("view", state.view);
    render();
    return;
  }
  const pg = e.target.closest("[data-pick-genre]");
  if (pg) { state.genre = pg.dataset.pickGenre; state.letter = ""; return renderGrid(); }
  const ps = e.target.closest("[data-pick-series]");
  if (ps) { state.series = ps.dataset.pickSeries; state.letter = ""; return renderGrid(); }
  const lb = e.target.closest("[data-letter]");
  if (lb) { state.letter = lb.dataset.letter; return renderGrid(); }
  const ko = e.target.closest("[data-kiln-open]");
  if (ko) return invoke("kiln_open", { game: ko.dataset.kilnOpen }).then(() => toast("Opening Kiln…")).catch((err) => toast(String(err), true));
  const copy = e.target.closest("[data-copy]");
  if (copy) return select(copy.dataset.copy);
  const ws = e.target.closest("[data-winsetup]");
  if (ws) return winSetup(ws.dataset.winsetup);
  const wo = e.target.closest("[data-winopen]");
  if (wo) return invoke("winapp_open", { id: wo.dataset.winopen }).then(() => toast("Opening Battle.net…")).catch((err) => toast(String(err), true));
  const forget = e.target.closest("[data-forget]");
  if (forget) return invoke("forget_session", { id: forget.dataset.forget });
  const act = e.target.closest("[data-act]");
  if (act) return menuAction(act.dataset.act);
  if (!e.target.closest(".menu-wrap")) $("#menu").classList.add("hidden");
  const card = e.target.closest(".card");
  if (card) {
    select(card.dataset.id);
  }
});
$("#btn-play").onclick = () => {
  const g = state.games.find((x) => x.id === state.selected);
  if (g && !g.installed && g.can_install) return install(g);
  play(state.selected);
};
$("#btn-play-menu").onclick = (e) => { e.stopPropagation(); $("#play-menu").classList.toggle("hidden"); };
$("#play-menu").onclick = (e) => {
  const b = e.target.closest("[data-play-mode]");
  if (!b) return;
  $("#play-menu").classList.add("hidden");
  play(state.selected, b.dataset.playMode);
};
document.addEventListener("click", (e) => { if (!e.target.closest(".play-wrap")) $("#play-menu").classList.add("hidden"); });
$("#btn-fav").onclick = () => {
  const g = state.games.find((x) => x.id === state.selected);
  if (g) setFlag(g.id, "favorite", !g.favorite);
};
$("#btn-more").onclick = () => $("#menu").classList.toggle("hidden");
$("#btn-refresh").onclick = async () => {
  toast("Rescanning libraries…");
  try { await invoke("store_refresh"); } catch (e) { toast(String(e), true); }
  load();
};
$("#f-genre").onchange = (e) => { state.genre = e.target.value; state.letter = ""; renderGrid(); };
$("#f-series").onchange = (e) => { state.series = e.target.value; state.letter = ""; renderGrid(); };
$("#f-clear").onclick = () => { state.genre = state.series = state.letter = ""; renderGrid(); };
let searchTimer;
$("#search").oninput = (e) => {
  state.search = e.target.value;
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => { renderGrid(); renderHero(); }, 120);
};
$("#sort").value = state.sort;
$("#sort").onchange = (e) => { state.sort = e.target.value; ls.set("sort", state.sort); render(); };
$("#show-uninstalled").checked = state.showUninstalled;
$("#show-uninstalled").onchange = (e) => { state.showUninstalled = e.target.checked; ls.set("showUninstalled", state.showUninstalled); render(); };
$("#minimize").checked = state.minimize;
$("#minimize").onchange = (e) => { state.minimize = e.target.checked; ls.set("minimize", state.minimize); };

// Add game dialog
$("#btn-add").onclick = () => $("#dlg-add").showModal();
$("#add-cancel").onclick = () => $("#dlg-add").close();
$("#form-add").proton.onchange = (e) => {
  $("#cmd-label").textContent = e.target.checked ? "Path to the game's .exe" : "Command";
  $("#form-add").command.placeholder = e.target.checked ? "/home/you/Games/MyGame/Game.exe" : "/home/you/Games/mygame/start.sh";
};
$("#form-add").onsubmit = async (e) => {
  e.preventDefault();
  const f = new FormData(e.target);
  try {
    await invoke("add_custom_game", { title: f.get("title"), command: f.get("command"), store: f.get("store"), cover: f.get("cover") || null, proton: f.get("proton") === "on" });
    $("#dlg-add").close();
    e.target.reset();
    await load(true);
    toast("Game added");
  } catch (err) {
    toast(String(err), true);
  }
};

// Keyboard: arrows move, Enter plays, / searches, F favorites
document.addEventListener("keydown", (e) => {
  if (document.querySelector("dialog[open]") || !$("#onboard").classList.contains("hidden")) return;
  if (state.view === "minecraft" || state.view.startsWith("shop:")) return;
  const inSearch = document.activeElement === $("#search");
  if (e.key === "/" && !inSearch) { e.preventDefault(); return $("#search").focus(); }
  if (e.key === "Escape" && inSearch) { $("#search").value = ""; state.search = ""; $("#search").blur(); return render(); }
  if ((e.ctrlKey && e.key === "r") || e.key === "F5") { e.preventDefault(); return load(); }
  if (e.key === "Enter") { e.preventDefault(); return play(state.selected); }
  if (inSearch && !["ArrowDown", "ArrowUp"].includes(e.key)) return;
  if (!inSearch && (e.key === "f" || e.key === "F")) return $("#btn-fav").click();
  const list = state.visible;
  if (!list.length) return;
  const cols = Math.max(1, getComputedStyle($("#grid")).gridTemplateColumns.split(" ").length);
  const i = Math.max(0, list.findIndex((g) => g.id === state.selected));
  const step = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: cols, ArrowUp: -cols }[e.key];
  if (step === undefined) return;
  e.preventDefault();
  if (inSearch) $("#search").blur();
  const next = Math.min(list.length - 1, Math.max(0, i + step));
  select(list[next].id, true);
});

// Backend events
if (T) {
  T.event.listen("running", ({ payload }) => {
    const before = [...state.running.keys()].sort().join();
    const prevStates = new Map([...state.running].map(([k, v]) => [k, v.state]));
    state.running = new Map(payload.map((r) => [r.id, r]));
    const changed = before !== [...state.running.keys()].sort().join() || payload.some((r) => prevStates.get(r.id) !== r.state);
    renderRunning();
    if (changed && state.view !== "minecraft") { renderGrid(); renderHero(); }
    if (changed && state.view === "minecraft") window.mcShow && window.mcShow(true);
  });
  T.event.listen("stopped", ({ payload }) => {
    const g = state.games.find((x) => x.id === payload.id);
    if (g) { g.playtime += payload.played; g.last_played = Date.now() / 1000; }
    if (g && g.prefix && payload.played < 20) toast(`${payload.title} closed right away. Many Windows games need Microsoft's runtimes: try ⋯ → Install DirectX & VC++ runtimes, then play again.`, true);
    else if (payload.played > 0) toast(`${payload.title}: played ${fmtTime(payload.played)}`);
    else toast(`${payload.title} didn't start. Check ⋯ → View launch log`, true);
    render();
  });
  invoke("running").then((r) => { state.running = new Map(r.map((x) => [x.id, x])); render(); });
}

// ---------- first-run setup guide ----------
const OB_STEPS = 6;
let obStep = 0;
async function obRenderFound() {
  const o = await invoke("setup_overview");
  const n = (store) => state.games.filter((g) => g.store === store && g.installed).length;
  const row = (ok, name, detail) => `<div class="ob-found-row ${ok ? "ok" : ""}"><span class="mark">${ok ? "✓" : "–"}</span><div><b>${name}</b><br><small class="muted">${detail}</small></div></div>`;
  $("#ob-found").innerHTML = [
    row(o.steam.found, "Steam", o.steam.found ? `${o.steam.account ? `Signed in as ${esc(o.steam.account)}. ` : ""}${n("steam")} installed games. Steam stays in the tray; Lodestone launches its games.` : "Not found. Install Steam if you have a Steam library."),
    row(o.heroic, "Heroic", o.heroic ? "Found. Its installed Epic/GOG games show up here. Lodestone can also take over its sign-ins in the next steps, so Heroic isn't needed anymore." : "Not needed: Lodestone handles Epic and GOG itself."),
    row(o.lutris, "Lutris", o.lutris ? "Found. Games installed in Lutris appear in your library." : "Optional. Games installed with Lutris show up automatically."),
    row(o.atlauncher || o.prism, "Minecraft", o.atlauncher || o.prism ? `${[o.atlauncher && "ATLauncher", o.prism && "Prism Launcher"].filter(Boolean).join(" and ")} found. Your instances get the Minecraft mod manager.` : "Install ATLauncher or Prism Launcher to play and mod Minecraft from here."),
    row(!!o.kiln, "Kiln (mod manager)", o.kiln ? (o.kiln.managed.length ? `Found. ${o.kiln.managed.length} modded game${o.kiln.managed.length > 1 ? "s" : ""} (${esc(o.kiln.managed.slice(0, 4).join(", "))}${o.kiln.managed.length > 4 ? "…" : ""}) launch through Kiln so their mods always load. Look for the <b>Modded</b> view.` : `Found. It can mod ${o.kiln.supported} of your installed games; use "Set up mods in Kiln" on any of them.`) : "Not installed. Kiln adds Nexus mods, load order and crash checks for Skyrim, Fallout, Cyberpunk, Stardew and 30+ more."),
    row(o.protons.length > 0, "Proton builds", o.protons.length ? `${o.protons.length} found (${esc(o.protons.slice(0, 3).join(", "))}${o.protons.length > 3 ? "…" : ""}).` : "None yet. Lodestone can download GE-Proton when a game needs it."),
  ].join("");
  return o;
}
async function obShow(step) {
  obStep = Math.max(0, Math.min(OB_STEPS - 1, step));
  document.querySelectorAll(".ob-step").forEach((s) => s.classList.toggle("on", Number(s.dataset.step) === obStep));
  $("#ob-dots").innerHTML = Array.from({ length: OB_STEPS }, (_, i) => `<span class="${i === obStep ? "on" : i < obStep ? "done" : ""}"></span>`).join("");
  $("#ob-back").classList.toggle("hidden", obStep === 0);
  $("#ob-skip").classList.toggle("hidden", obStep === OB_STEPS - 1);
  $("#ob-next").textContent = obStep === 0 ? "Let's set it up" : obStep === OB_STEPS - 1 ? "Open my library" : "Next";
  try {
    if (obStep === 1) await obRenderFound();
    if (obStep === 2) await renderProton();
    if (obStep === 3) {
      renderAccounts();
      $("#ob-steamkey").value = (await invoke("get_settings")).steam_api_key || "";
      const gw2 = (state.winapps || []).find((a) => a.id === "gw2");
      $("#ob-gw2").textContent = gw2 && gw2.installed ? "Installed ✓" : "Install";
      $("#ob-gw2").disabled = !!(gw2 && gw2.installed);
    }
    if (obStep === 4) {
      renderStoreOrder();
      $("#ob-gamesdir").value = (await invoke("get_settings")).games_dir || "";
      $("#ob-minimize").checked = state.minimize;
    }
    if (obStep === 5) {
      const a = await invoke("accounts");
      const stores = new Set(state.games.map((g) => g.store)).size;
      const linked = ["epic", "gog"].filter((s) => a[s] && a[s].account).map((s) => (s === "epic" ? "Epic Games" : "GOG"));
      $("#ob-summary").textContent = `${state.games.length.toLocaleString()} games from ${stores} sources${linked.length ? `, with ${linked.join(" and ")} connected` : ""}.`;
    }
  } catch (e) {
    toast(String(e), true);
  }
}
async function obSaveStep() {
  if (obStep === 2) await invoke("set_proton", { path: $("#ob-proton").value });
  if (obStep === 4) {
    const s = await invoke("get_settings");
    await invoke("save_settings", { curseforgeApiKey: s.curseforge_api_key || "", gamesDir: $("#ob-gamesdir").value, steamApiKey: null });
    state.minimize = $("#ob-minimize").checked;
    ls.set("minimize", state.minimize);
    $("#minimize").checked = state.minimize;
  }
}
function obOpen() {
  shopSuspend(true);
  $("#onboard").classList.remove("hidden");
  obShow(0);
}
async function obClose(finished) {
  await invoke("set_onboarded", { done: true }).catch(() => {});
  $("#onboard").classList.add("hidden");
  shopSuspend(false);
  if (finished) {
    toast("Refreshing your libraries…");
    try { await invoke("store_refresh"); } catch (e) { toast(String(e), true); }
    load(true);
  }
}
$("#ob-next").onclick = async () => {
  try { await obSaveStep(); } catch (e) { return toast(String(e), true); }
  if (obStep === OB_STEPS - 1) return obClose(true);
  obShow(obStep + 1);
};
$("#ob-back").onclick = () => obShow(obStep - 1);
$("#ob-skip").onclick = () => { if (confirm("Skip setup? You can run it again from Settings.")) obClose(false); };
$("#ob-steamkey-save").onclick = async () => {
  const key = $("#ob-steamkey").value.trim();
  const s = await invoke("get_settings");
  await invoke("save_settings", { curseforgeApiKey: s.curseforge_api_key || "", gamesDir: null, steamApiKey: key });
  if (!key) return toast("Steam key removed");
  toast("Checking your Steam library…");
  try { await invoke("store_refresh"); toast("Steam library updated from your account"); load(true); } catch (e) { toast(String(e), true); }
};
$("#btn-rerun-setup").onclick = () => { $("#dlg-settings").close(); obOpen(); };

invoke("startup_view")
  .then((v) => { if (v) { state.view = v; ls.set("view", v); } })
  .catch(() => {})
  .finally(async () => {
    await load(true);
    try { if (!(await invoke("setup_overview")).done) obOpen(); } catch {}
  });

window.L = { invoke, state, toast, esc, fmtSize, fmtTime, imgSrc, play, $ };
