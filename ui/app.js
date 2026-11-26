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
  emulation: { label: "Emulation", color: "#e0598b" },
};
const store = (s) => STORES[s] || { label: s, color: "#9aa0a6" };
// Windows: games run natively and launchers come through winget (WebView2 reports Windows).
const WIN = /Windows/.test(navigator.userAgent);
// Lodestone's data folder on this system (~/.local/share/lodestone, or %APPDATA%\lodestone on Windows).
invoke("data_folder").then((d) => { state.dataDir = d; document.querySelectorAll(".data-dir").forEach((el) => (el.textContent = d)); }).catch(() => {});
// Problems in this page go to logs/ui.log on this PC (for bug reports; never sent anywhere).
window.addEventListener("error", (e) => { try { window.__TAURI__.core.invoke("ui_error", { message: `${e.message} at ${e.filename}:${e.lineno}:${e.colno}` }); } catch {} });
window.addEventListener("unhandledrejection", (e) => { try { window.__TAURI__.core.invoke("ui_error", { message: `unhandled: ${e.reason && (e.reason.stack || e.reason)}` }); } catch {} });
if (WIN) document.body.classList.add("win");


const $ = (s) => document.querySelector(s);
const ls = {
  get: (k, d) => { try { const v = localStorage.getItem(k); return v === null ? d : JSON.parse(v); } catch { return d; } },
  set: (k, v) => { try { localStorage.setItem(k, JSON.stringify(v)); } catch {} },
};

const state = {
  games: [],
  view: ls.get("view", "home"),
  layout: ls.get("layout", "grid"),
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
  // A modal dialog sits above the page, so show the message inside the open one.
  const dlg = [...document.querySelectorAll("dialog[open]")].pop();
  let host = $("#toasts");
  if (dlg) {
    host = dlg.querySelector(":scope > .toasts") || dlg.appendChild(Object.assign(document.createElement("div"), { className: "toasts" }));
  }
  host.append(t);
  setTimeout(() => t.remove(), err ? 6000 : 3000);
}
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
// For url('...') inside a style attribute: HTML escaping is undone before CSS reads it, so quotes,
// brackets and backslashes are percent-encoded too (they can't then end the url() early).
const cssUrl = (u) => esc(String(u).replace(/['"()\\\s]/g, (c) => "%" + c.charCodeAt(0).toString(16).padStart(2, "0").toUpperCase()));
// Inline event handlers aren't allowed (the content security policy only runs app.js and minecraft.js),
// so images that fail and buttons that close dialogs are handled here instead.
document.addEventListener("error", (e) => {
  const t = e.target;
  if (!(t instanceof HTMLImageElement) || !t.dataset.err) return;
  if (t.dataset.err === "remove") t.remove();
  else if (t.dataset.err === "parent") t.parentElement && t.parentElement.remove();
  else if (t.dataset.err === "hide") t.style.visibility = "hidden";
  else if (t.dataset.err === "mcico") t.replaceWith(Object.assign(document.createElement("div"), { className: "mc-ico ph", textContent: t.dataset.letter || "?" }));
}, true);
document.addEventListener("click", (e) => {
  const c = e.target.closest("[data-close-dialog]");
  if (c) return c.closest("dialog")?.close();
  const k = e.target.closest("[data-click]");
  if (k) document.getElementById(k.dataset.click)?.click();
});

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
  const cc = COMPAT_FILTERS.map(([k, label]) => [k, label, pool.filter((g) => compatMatch(g, k)).length]);
  // System libraries: ROM filters instead of the store ones (ProtonDB, series; genres only if known).
  const sys = state.view.startsWith("sys:");
  $("#f-compat").classList.toggle("hidden", sys);
  $("#f-series").classList.toggle("hidden", sys && !series.length);
  $("#f-genre").classList.toggle("hidden", sys && !genres.length);
  if (sys) state.compatF = "";
  else state.achF = state.regionF = state.playedF = "";
  renderRomFilters(sys, pool);
  $("#f-compat").innerHTML = `<option value="">All ratings</option>` + cc.map(([k, label, n]) => `<option value="${k}" ${k === state.compatF ? "selected" : ""}>${label} (${n})</option>`).join("");
  const lc = new Map();
  for (const g of pool) { const l = letterOf(g.title); lc.set(l, (lc.get(l) || 0) + 1); }
  $("#f-letters").innerHTML = ["", "#", ..."ABCDEFGHIJKLMNOPQRSTUVWXYZ"].map((l) =>
    `<button data-letter="${l}" class="${state.letter === l ? "on" : ""}" ${l && !lc.get(l) ? "disabled" : ""}>${l || "All"}</button>`).join("");
  $("#f-clear").classList.toggle("hidden", !(state.genre || state.series || state.compatF || state.letter || state.achF || state.regionF || state.playedF));
}

// ---------- ROM filters (system libraries) ----------
const REGION_TAGS = { usa: "USA", u: "USA", us: "USA", europe: "Europe", e: "Europe", eu: "Europe", japan: "Japan", j: "Japan", jp: "Japan", world: "World", w: "World", "usa, europe": "USA", "japan, usa": "USA", "usa, japan": "USA", "europe, australia": "Europe", "japan, europe": "Europe" };
/** A ROM's region from the tags in its file name ("(USA)", "(E)", "(Japan, USA)"), "" if none. */
function regionOf(g) {
  if (!g.system) return "";
  const name = g.id.split(":").slice(2).join(":").split("/").pop();
  for (const m of name.matchAll(/\(([^)]+)\)/g)) {
    const t = m[1].trim().toLowerCase();
    if (REGION_TAGS[t]) return REGION_TAGS[t];
    const first = t.split(",")[0].trim();
    if (["usa", "europe", "japan", "world"].includes(first)) return REGION_TAGS[first];
    if (["germany", "france", "spain", "italy", "korea", "brazil", "australia", "china", "netherlands", "sweden", "asia"].includes(first)) return first[0].toUpperCase() + first.slice(1);
  }
  return "";
}
const played = (g) => !!(g.playtime || g.last_played);
function romFilter(g) {
  if (state.achF === "yes" && !g.ra_id) return false;
  if (state.achF === "no" && g.ra_id) return false;
  if (state.regionF && regionOf(g) !== state.regionF) return false;
  if (state.playedF === "yes" && !played(g)) return false;
  if (state.playedF === "no" && played(g)) return false;
  return true;
}
function renderRomFilters(sys, pool) {
  for (const id of ["#f-ach", "#f-region", "#f-played"]) $(id).classList.toggle("hidden", !sys);
  if (!sys) return;
  const n = (f) => pool.filter(f).length;
  const opt = (v, label, cur) => `<option value="${v}" ${v === cur ? "selected" : ""}>${label}</option>`;
  $("#f-ach").innerHTML = opt("", "All games", state.achF) + opt("yes", `With achievements (${n((g) => g.ra_id)})`, state.achF) + opt("no", `Without (${n((g) => !g.ra_id)})`, state.achF);
  const rc = new Map();
  for (const g of pool) { const r = regionOf(g) || ""; rc.set(r, (rc.get(r) || 0) + 1); }
  const regions = [...rc].filter(([r]) => r).sort((a, b) => b[1] - a[1]);
  $("#f-region").classList.toggle("hidden", regions.length < 2 && !state.regionF);
  $("#f-region").innerHTML = opt("", "All regions", state.regionF) + regions.map(([r, c]) => opt(esc(r), `${esc(r)} (${c})`, state.regionF)).join("");
  $("#f-played").innerHTML = opt("", "Played or not", state.playedF) + opt("yes", `Played (${n(played)})`, state.playedF) + opt("no", `Not played yet (${n((g) => !played(g))})`, state.playedF);
}

// ProtonDB filter: "or better" options include Native games (they need no Proton at all).
const COMPAT_RANK = { native: 0, platinum: 1, gold: 2, silver: 3, bronze: 4, borked: 5 };
const COMPAT_FILTERS = [["native", "Native Linux"], ["platinum", "Platinum or Native"], ["gold", "Gold or better"], ["silver", "Silver or better"], ["bronze", "Bronze or better"], ["borked", "Borked"], ["none", "No rating"]];
function compatMatch(g, f) {
  const t = compatOf(g);
  if (f === "none") return !(t in COMPAT_RANK);
  if (!(t in COMPAT_RANK)) return false;
  if (f === "native" || f === "borked") return t === f;
  return COMPAT_RANK[t] <= COMPAT_RANK[f];
}

if (T) T.event.listen("series-updated", () => load(true));
// RetroAchievements matched more ROMs or fetched box art: reload (and carry on with the next batch).
if (T) T.event.listen("ra-updated", () => load(true));
async function load(quiet = false) {
  await loadWinapps();
  try {
    state.games = await invoke("list_games");
    keyCache.clear();
    try { state.systems = await invoke("emu_systems"); } catch { state.systems = []; }
    try { state.plugins = await invoke("plugins_list"); } catch { state.plugins = []; }
    try { const st = await invoke("get_settings"); state.kilnPlay = st.kiln_play !== false; state.steamBg = st.steam_background !== false; state.mangohudAll = !!st.mangohud; state.emuMenu = !!st.emu_menu_entry; } catch { state.kilnPlay = true; state.steamBg = true; }
  } catch (e) {
    toast(String(e), true);
    state.games = [];
  }
  state.copies = copiesByKey();
  computeSeries();
  render();
  announceUpdates();
  loadCompat();
  // Look up series for games not checked yet (no-op when everything is cached).
  if (state.games.length) invoke("refresh_series").catch(() => {});
  // Once per session: the sync keeps going in the background and sends "ra-updated" as it goes.
  if (!state.raSynced && state.games.some((g) => g.system)) {
    state.raSynced = true;
    invoke("ra_sync").catch((e) => console.warn("RetroAchievements:", e));
    invoke("thumbs_sync").catch((e) => console.warn("libretro thumbnails:", e));
  }
  if (!quiet) {
    const n = state.games.filter((g) => g.installed).length;
    toast(`${n} installed games across ${new Set(state.games.map((g) => g.store)).size} stores`);
  }
}

// ---------- one entry per game across stores ----------
const DEFAULT_STORE_ORDER = ["steam", "epic", "gog", "battlenet", "amazon", "arenanet", "ea", "ubisoft", "rockstar", "lutris", "minecraft", "custom", "emulation"];
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
    if (state.view === "missing" && !g.art_missing) return false;
    if (!state.showUninstalled && !g.installed && state.view !== "favorites" && !(WINAPP_FOR[state.view] && !anyInstalled)) return false;
    if (state.view === "favorites" && !g.favorite) return false;
    if (state.view === "recent" && !g.last_played) return false;
    if (state.view === "modded" && !(g.kiln && (g.installed || g.kiln_managed))) return false;
    if (state.view === "updates" && !(g.installed && g.update)) return false;
    // Emulated games live in their system's library, not in All games.
    if (state.view.startsWith("sys:")) { if (g.system !== state.view.slice(4)) return false; }
    else if (state.view === "all" && g.system) return false;
    if (STORES[state.view] && g.store !== state.view) return false;
    if (q && !g.title.toLowerCase().includes(q)) return false;
    return true;
  });
  const by = SORTS[state.view === "recent" ? "recent" : currentSort()] || SORTS.title;
  if (mergeOn()) list = mergeList(list);
  if (state.genre) list = list.filter((g) => genresOf(g).has(state.genre));
  if (state.series) list = list.filter((g) => seriesOfGame(g) === state.series);
  if (state.compatF) list = list.filter((g) => compatMatch(g, state.compatF));
  if (state.view.startsWith("sys:")) list = list.filter(romFilter);
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
    ["home", "Home", ""],
    ["downloads", "Downloads", (state.dl || []).filter((d) => d.state === "downloading").length || ""],
    ["all", "All games", ls.get("mergeDupes", true) ? mergeList(visible.filter((g) => inst(g) && !g.system)).length : visible.filter((g) => inst(g) && !g.system).length],
    ["favorites", "Favorites", visible.filter((g) => g.favorite).length],
    ["recent", "Recently played", visible.filter((g) => g.last_played && inst(g)).length],
  ];
  const updN = visible.filter((g) => g.installed && g.update).length;
  if (updN) lib.splice(1, 0, ["updates", "Updates", updN]);
  const kilnN = visible.filter((g) => g.kiln && (g.installed || g.kiln_managed)).length;
  if (kilnN) lib.push(["modded", "Modded (Kiln)", kilnN]);
  const missN = visible.filter((g) => g.art_missing && inst(g)).length;
  if (missN) lib.push(["missing", "Missing artwork", missN]);
  const hiddenN = state.games.filter((g) => g.hidden).length;
  if (hiddenN) lib.push(["hidden", "Hidden", hiddenN]);
  $("#nav-library").innerHTML = lib
    .map(([k, l, n]) => `<button data-view="${k}" class="${state.view === k ? "active" : ""}">${l}<span class="count">${n}</span></button>`)
    .join("");

  const counts = {};
  for (const g of visible) if (inst(g)) counts[g.store] = (counts[g.store] || 0) + 1;
  $("#nav-stores").innerHTML = Object.keys(STORES)
    // Stores with a setup banner are always listed (on Windows: Steam, Epic and GOG too, installed via winget).
    .filter((k) => k !== "emulation" && (counts[k] || ["battlenet", "ea", "ubisoft", "arenanet", ...(WIN ? ["steam", "epic", "gog"] : [])].includes(k)))
    .map((k) => `<button data-view="${k}" class="${state.view === k ? "active" : ""}"><span class="dot" style="background:${STORES[k].color}"></span>${STORES[k].label}<span class="count">${counts[k] || 0}</span></button>`)
    .join("");
  // Each emulated system is its own library.
  const sysCount = {};
  for (const g of visible) if (g.system) sysCount[g.system] = (sysCount[g.system] || 0) + 1;
  const systems = (state.systems || []).filter((s) => sysCount[s.id]);
  $("#nav-systems-label").classList.toggle("hidden", !systems.length);
  $("#nav-systems").innerHTML = systems
    .map((s) => `<button data-view="sys:${esc(s.id)}" class="${state.view === "sys:" + s.id ? "active" : ""}" title="${esc(s.emulators.length ? "Plays with " + s.emulators.join(", ") : "No emulator installed for this system yet")}"><span class="dot" style="background:${s.emulators.length ? STORES.emulation.color : "#5b5f6a"}"></span>${esc(s.name)}<span class="count">${sysCount[s.id]}</span></button>`)
    .join("");
  // Plug-ins: the installed ones, then the management page.
  const PLUGIN_VIEW = { beacon: "beacon" };
  $("#nav-plugins").innerHTML = (state.plugins || []).filter((p) => p.installed).map((p) =>
    `<button data-view="${PLUGIN_VIEW[p.id]}" class="${state.view === PLUGIN_VIEW[p.id] ? "active" : ""}"><span class="dot" style="background:${p.running ? "#3ddc97" : "#5b5f6a"}"></span>${esc(p.name.replace(/^Lodestone /, ""))}</button>`).join("")
    + `<button data-view="plugins" class="${state.view === "plugins" ? "active" : ""}"><span class="dot" style="background:transparent;border:1px solid #5b5f6a"></span>Manage plug-ins</button>`;
}

function cardHTML(g) {
  if (state.layout === "list") return rowHTML(g);
  const s = store(g.store);
  const others = mergeOn() ? (state.copies.get(titleKey(g.title)) || []).filter((c) => c.id !== g.id) : [];
  const wide = state.layout === "banner";
  const fb = !wide && g.art_missing ? fallbackArt(g) : null;
  // ROM box art comes in every shape (square GBA boxes, wide SNES ones, title screens), so show it whole
  // over a blurred copy instead of cropping it to the 2:3 card.
  const fit = !wide && !fb && g.system && g.cover ? imgSrc(g.cover, state.layout === "compact" ? 220 : 360) : null;
  const src = fb || fit ? null : wide ? imgSrc(g.hero || g.cover, 640) : imgSrc(g.cover, state.layout === "compact" ? 220 : 360);
  const run = state.running.get(g.id);
  const isSel = g.id === state.selected || (others.length && state.selKey === titleKey(g.title));
  const cls = ["card", g.installed ? "" : "uninstalled", isSel ? "selected" : "", run ? "running" : ""].join(" ");
  const dots = [g, ...others].map((c) => store(c.store)).filter((v, i, a) => a.findIndex((x) => x.label === v.label) === i);
  return `<div class="${cls}" data-id="${esc(g.id)}" data-key="${esc(titleKey(g.title))}">
    <div class="art" style="${placeholder(g)}">
      <div class="ph">${esc(g.title)}</div>
      ${src ? `<img loading="lazy" decoding="async" src="${esc(src)}" alt="" data-err="remove">` : ""}
      ${fit ? `<div class="fb-art fit"><div class="fb-blur" style="background-image:url('${cssUrl(fit)}')"></div><img loading="lazy" decoding="async" src="${esc(fit)}" alt="" data-err="parent"></div>` : ""}
      ${fb ? `<div class="fb-art"><div class="fb-blur" style="background-image:url('${cssUrl(fb)}')"></div><img loading="lazy" src="${esc(fb)}" alt="" data-err="parent"><div class="fb-title">${esc(g.title)}</div></div>` : ""}
      <span class="badges">${dots.map((d) => `<span class="badge" style="background:${d.color}" title="${d.label}"></span>`).join("")}</span>
      ${g.favorite ? '<span class="fav">★</span>' : ""}
      <span class="corner-br">${g.kiln_managed ? '<span class="modded" title="Modded with Kiln">MODDED</span>' : ""}${compatBadge(g)}</span>
      ${g.installed && g.update ? `<span class="upd-badge" title="${g.update === "updating" ? "Updating" : "Update available"}">${g.update === "updating" ? "UPDATING" : "UPDATE"}</span>` : ""}
      ${run ? `<div class="state">${run.state === "launching" ? "Launching…" : "Running"}</div>` : ""}
    </div>
    <div class="title" title="${esc(g.title)}">${esc(g.title)}</div>
    <div class="sub">${g.installed ? (g.playtime ? fmtTime(g.playtime) : s.label) : "Not installed"}${others.length ? ` · +${others.length} store${others.length > 1 ? "s" : ""}` : ""}</div>
  </div>`;
}

/** A wide image to build a cover from when a game has no portrait art (old Steam apps often don't). */
function fallbackArt(g) {
  const m = g.id.match(/^steam:(\d+)$/);
  if (m) return `https://shared.steamstatic.com/store_item_assets/steam/apps/${m[1]}/header.jpg`;
  return g.hero ? imgSrc(g.hero, 640) : null;
}

function rowHTML(g) {
  const others = mergeOn() ? (state.copies.get(titleKey(g.title)) || []).filter((c) => c.id !== g.id) : [];
  const src = imgSrc(g.cover, 120);
  const run = state.running.get(g.id);
  const isSel = g.id === state.selected || (others.length && state.selKey === titleKey(g.title));
  const cls = ["card", "row", g.installed ? "" : "uninstalled", isSel ? "selected" : "", run ? "running" : ""].join(" ");
  const stores = [g, ...others].map((c) => store(c.store)).filter((v, i, a) => a.findIndex((x) => x.label === v.label) === i);
  const status = run ? `<span class="ok-text">${run.state === "launching" ? "Launching…" : "Running"}</span>`
    : g.installed && g.update ? `<span class="upd-text">${g.update === "updating" ? "Updating" : "Update"}</span>`
    : g.kiln_managed ? `<span class="mod-text">Modded</span>` : !g.installed ? `<span class="muted">Not installed</span>` : "";
  return `<div class="${cls}" data-id="${esc(g.id)}" data-key="${esc(titleKey(g.title))}">
    <div class="row-art" style="${placeholder(g)}">${src ? `<img loading="lazy" decoding="async" src="${esc(src)}" alt="" data-err="remove">` : ""}</div>
    <div class="row-main"><div class="title">${g.favorite ? '<span class="fav-s">★</span> ' : ""}${esc(g.title)}</div>
      <div class="sub">${stores.map((d) => `<span class="dot" style="background:${d.color}"></span>${d.label}`).join(" ")}</div></div>
    <div class="row-genres muted">${esc([...genresOf(g)].slice(0, 3).join(", "))}</div>
    <div class="row-num" title="Time played">${g.playtime ? fmtTime(g.playtime) : "—"}</div>
    <div class="row-num" title="Last played">${g.last_played ? fmtAgo(g.last_played) : "—"}</div>
    <div class="row-num" title="Size on disk">${fmtSize(g.size) || "—"}</div>
    <div class="row-pdb">${compatBadge(g) || '<span class="muted">—</span>'}</div>
    <div class="row-status">${status}</div>
  </div>`;
}

const WINAPP_FOR = WIN
  ? { battlenet: "battlenet", ea: "ea", ubisoft: "ubisoft", arenanet: "gw2", steam: "steam", epic: "epic", gog: "gog" }
  : { battlenet: "battlenet", ea: "ea", ubisoft: "ubisoft", arenanet: "gw2" };
// Store clients Lodestone installs into their own Proton prefix.
const CLIENTS = {
  steam: { name: "Steam", account: "Steam",
    owned: "Your installed Steam games are below and start from Lodestone through Steam.", tip: "" },
  epic: { name: "Epic Games Launcher", account: "Epic",
    owned: "Games you install in the Epic Games Launcher show up below after a Rescan and start from Lodestone through the launcher.", tip: "" },
  gog: { name: "GOG GALAXY", account: "GOG",
    owned: "Games you install with GOG GALAXY show up below after a Rescan and start from Lodestone through GALAXY.", tip: "" },
  battlenet: { name: "Battle.net", account: "Blizzard",
    owned: "Your owned and free Battle.net games are below. <b>Install</b> opens Battle.net on that game's page; after it finishes, Rescan and play it from Lodestone.",
    tip: 'Tip: in Battle.net\'s settings, set "When I launch a game" to exit or minimize to tray so its window stays out of the way.' },
  ea: { name: "EA app", account: "EA",
    owned: "Games you install in the EA app show up below after a Rescan and start from Lodestone. EA games open the EA app themselves to check your sign-in. Sign in to EA under 👤 Accounts to also see the games you own but haven't installed; Install opens the EA app and starts the download.",
    tip: "Tip: in the EA app's settings, turn off \"Open EA app at startup\" and the in-game overlay if a game has trouble starting." },
  ubisoft: { name: "Ubisoft Connect", account: "Ubisoft",
    owned: "Games you install in Ubisoft Connect show up below after a Rescan. Play starts them through Ubisoft Connect, which signs you in and keeps them updated. Sign in to Ubisoft under 👤 Accounts to also see the games you own but haven't installed; Install opens Ubisoft Connect on the game.",
    tip: "Tip: in Ubisoft Connect's settings, turn off the in-game overlay if a game has trouble starting." },
};
// A system's library: which emulator plays it, and one-click installs for the ones that aren't there.
function renderSystemPanel(el) {
  const sys = (state.systems || []).find((s) => s.id === state.view.slice(4));
  if (!sys || !sys.options.length) return el.classList.add("hidden");
  el.classList.remove("hidden");
  const cur = sys.options.find((o) => o.index === sys.chosen) || sys.options[0];
  const busy = state.emuBusy === sys.id;
  const status = cur.installed
    ? `Your ${sys.games} ${esc(sys.name)} games play with <b>${esc(cur.label)}</b>.`
    : cur.can_install
      ? `<b>${esc(cur.label)}</b> isn't installed yet. Install sets it up ${WIN ? "with winget, from its official download (installing accepts its licence)" : "from Flathub"}${cur.core ? ", with its core from libretro" : ""}, in the background.`
      : `<b>${esc(cur.label)}</b> isn't installed. Install ${esc(cur.emulator)} yourself, or pick another emulator.`;
  el.innerHTML = `<div><h2>${esc(sys.name)}</h2><p class="muted">${status}</p></div>
    <div class="row-btns emu-pick">
      <label class="emu-label">Plays with
        <select id="emu-choice">${sys.options.map((o) => `<option value="${o.index}" ${o.index === cur.index ? "selected" : ""}>${esc(o.label)}${o.installed ? "" : " (not installed)"}</option>`).join("")}</select>
      </label>
      ${!cur.installed && cur.can_install ? `<button class="play" id="emu-install" ${busy ? "disabled" : ""}>${busy ? "Installing…" : "Install"}</button>` : ""}
      ${updateFor(cur.emulator) ? `<button class="ghost" data-prog-update="${esc(updateFor(cur.emulator).id)}">Update ${esc(cur.emulator)}${updateFor(cur.emulator).latest ? " to " + esc(updateFor(cur.emulator).latest) : ""}</button>` : ""}
      ${!WIN && !cur.installed && cur.can_install && cur.in_menu == null ? `<label class="emu-menu" title="Flatpak adds installed apps to your desktop's app menu. Leave this off to only start ${esc(cur.emulator)} from Lodestone.">
        <input type="checkbox" id="emu-menu" ${state.emuMenu ? "checked" : ""}> Add to my app menu</label>` : ""}
      ${cur.in_menu != null ? `<label class="emu-menu" title="Hiding adds a copy of its menu entry marked NoDisplay to ~/.local/share/applications; showing removes it.">
        <input type="checkbox" id="emu-menu-toggle" ${cur.in_menu ? "checked" : ""}> Show ${esc(cur.emulator)} in my app menu</label>` : ""}
    </div>`;
  const mt = $("#emu-menu-toggle");
  if (mt) mt.onchange = async (e) => {
    try { await invoke("emu_menu_entry", { system: sys.id, index: cur.index, show: e.target.checked }); await load(true); }
    catch (err) { e.target.checked = !e.target.checked; toast(String(err), true); }
  };
  $("#emu-choice").onchange = async (e) => {
    try { await invoke("emu_set_choice", { system: sys.id, index: Number(e.target.value) }); await load(true); } catch (err) { toast(String(err), true); }
  };
  const ib = $("#emu-install");
  if (ib) ib.onclick = async () => {
    state.emuBusy = sys.id;
    renderSystemPanel(el);
    try {
      const menu = !!($("#emu-menu") && $("#emu-menu").checked);
      state.emuMenu = menu;
      const how = await invoke("emu_install", { system: sys.id, index: cur.index, menu });
      if (how === "installing") { toast(`Installing ${cur.emulator} in the background. Follow it in Downloads.`); pollDownloads(); }
      else { toast(how === "done" ? `${cur.label} is ready` : `${cur.label} is already installed`); await load(true); }
    } catch (err) { toast(String(err), true); }
    state.emuBusy = null;
    renderSystemPanel(el);
  };
}

function renderSetup() {
  const el = $("#setup");
  if (state.view.startsWith("sys:")) return renderSystemPanel(el);
  const app = (state.winapps || []).find((a) => a.id === WINAPP_FOR[state.view]);
  if (!app || (app.installed && app.id === "gw2")) return el.classList.add("hidden");
  el.classList.remove("hidden");
  const busy = state.setupBusy === app.id;
  const c = CLIENTS[app.id];
  if (!app.installed) {
    el.innerHTML = WIN && c
      ? `<div><h2>Install ${c.name}</h2><p class="muted">Lodestone installs ${c.name} with winget, Windows' own package manager, from ${c.account}'s official download (winget checks it before installing). Installing accepts ${c.name}'s licence, as installing it yourself would, and Windows may ask for permission. Then sign in with your ${c.account} account, install your games there, and Rescan.</p></div>
         <button class="play" data-winsetup="${app.id}" ${busy ? "disabled" : ""}>${busy ? "Installing…" : `Install ${c.name}`}</button>`
      : c
      ? `<div><h2>${c.name} on Proton</h2><p class="muted">Lodestone downloads ${c.account}'s official installer and installs ${c.name} into its own Proton prefix: no Steam, Lutris or Wine setup needed. Sign in with your ${c.account} account in the window that opens, install your games there, then Rescan. The first run also downloads the Proton runtime, so it can take a few minutes.</p></div>
         <button class="play" data-winsetup="${app.id}" ${busy ? "disabled" : ""}>${busy ? "Setting up…" : `Set up ${c.name}`}</button>`
      : `<div><h2>Guild Wars 2 on Proton</h2><p class="muted">Lodestone downloads ArenaNet's official installer and runs it in its own Proton prefix. Sign in with your ArenaNet account in the game's launcher.</p></div>
         <button class="play" data-winsetup="gw2" ${busy ? "disabled" : ""}>${busy ? "Setting up…" : "Install Guild Wars 2"}</button>`;
  } else {
    const from = app.found_in ? ` <b>Using the ${c.name} you installed with ${app.found_in}</b>, and the games installed there.` : "";
    el.innerHTML = `<div><h2>${c.name} is set up</h2><p class="muted">${c.owned}${from} ${c.tip}</p></div>
      <div class="row-btns">${updateFor(app.name) ? `<button class="ghost" data-prog-update="${esc(updateFor(app.name).id)}">Update${updateFor(app.name).latest ? " to " + esc(updateFor(app.name).latest) : ""}</button>` : ""}<button class="ghost" data-winopen="${app.id}">Open ${c.name}</button><button class="play" data-click="btn-refresh">Rescan</button></div>`;
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
  $("#grid").className = `grid layout-${state.layout}`;
  document.querySelectorAll("#layout [data-layout]").forEach((b) => b.classList.toggle("on", b.dataset.layout === state.layout));
  parkDetail();
  $("#grid").innerHTML = (state.layout === "list" && list.length ? `<div class="row-head"><span></span><span>Game</span><span>Genres</span><span>Played</span><span>Last played</span><span>Size</span><span>ProtonDB</span><span></span></div>` : "") + list.map(cardHTML).join("");
  const sysView = state.view.startsWith("sys:") && (state.systems || []).find((s) => s.id === state.view.slice(4));
  const title = sysView ? sysView.name : { missing: "Missing artwork", all: "All games", favorites: "Favorites", recent: "Recently played", hidden: "Hidden", modded: "Modded with Kiln", updates: "Updates available" }[state.view] || store(state.view).label;
  $("#grid-title").textContent = title;
  $("#grid-count").textContent = `${list.length} game${list.length === 1 ? "" : "s"}`;
  const empty = $("#empty");
  if (!list.length && WINAPP_FOR[state.view]) {
    empty.classList.add("hidden");
  } else if (!list.length) {
    empty.classList.remove("hidden");
    empty.textContent = state.search ? `Nothing matches “${state.search}”.` : (state.genre || state.series || state.compatF || state.letter || state.achF || state.regionF || state.playedF) ? "No games match these filters." : state.view === "favorites" ? "Star a game to pin it here." : "No games here yet.";
  } else empty.classList.add("hidden");
  const selGame = state.games.find((g) => g.id === state.selected);
  const selShown = list.find((g) => g.id === state.selected) || (selGame && mergeOn() && list.find((g) => titleKey(g.title) === titleKey(selGame.title)));
  if (!selShown) {
    const fallback = list.find((g) => g.installed) || list[0];
    state.selected = fallback ? fallback.id : null;
    const el = fallback && $(`.card[data-id="${CSS.escape(fallback.id)}"]`);
    el && el.classList.add("selected");
  }
  placeDetail();
}

// ---------- details open inline, under the row of the game you clicked ----------
state.open = false;
function parkDetail() {
  const d = $("#detail");
  if (d.parentElement !== $("#detail-home").parentElement || d.previousElementSibling !== $("#detail-home")) $("#detail-home").after(d);
}
function detailCard() {
  const g = state.games.find((x) => x.id === state.selected);
  return $(`#grid .card[data-id="${CSS.escape(state.selected || "")}"]`) || (g && $(`#grid .card[data-key="${CSS.escape(titleKey(g.title))}"]`));
}
function placeDetail() {
  const d = $("#detail");
  const gridView = !["home", "minecraft", "downloads", "beacon", "plugins"].includes(state.view) && !state.view.startsWith("shop:");
  const card = state.open && gridView ? detailCard() : null;
  if (!card) {
    if (state.open && gridView && state.selected) state.open = false;
    parkDetail();
    d.classList.add("hidden");
    updateMiniVisibility();
    return;
  }
  // Last card on the clicked card's row (cards on one row share a top edge).
  let last = card;
  if (state.layout !== "list") {
    for (let n = card.nextElementSibling; n && n.classList.contains("card") && n.offsetTop === card.offsetTop; n = n.nextElementSibling) last = n;
  }
  if (last.nextElementSibling !== d) last.after(d);
  d.classList.remove("hidden");
  // Point the little notch at the clicked card.
  d.style.setProperty("--notch-x", `${card.offsetLeft - d.offsetLeft + card.offsetWidth / 2}px`);
  updateMiniVisibility();
}
function openDetail(id, scroll = true) {
  if (state.open && state.selected === id) return closeDetail();
  state.open = true;
  select(id);
  placeDetail();
  if (scroll) requestAnimationFrame(() => {
    const card = detailCard();
    const main = document.querySelector("main");
    const top = (card || $("#detail")).getBoundingClientRect().top - main.getBoundingClientRect().top;
    const bottom = $("#detail").getBoundingClientRect().bottom - main.getBoundingClientRect().top;
    // Keep the card and as much of the details as fit; only scroll as far as needed.
    const pad = 80, h = main.clientHeight;
    if (top < pad) main.scrollBy({ top: top - pad, behavior: "smooth" });
    else if (bottom > h) main.scrollBy({ top: Math.min(bottom - h + 16, top - pad), behavior: "smooth" });
  });
}
function closeDetail() {
  const card = detailCard();
  state.open = false;
  placeDetail();
  if (card) card.scrollIntoView({ block: "nearest" });
}
$("#detail-close").onclick = closeDetail;
let detailResize;
window.addEventListener("resize", () => { clearTimeout(detailResize); detailResize = setTimeout(() => state.open && (parkDetail(), placeDetail()), 120); });

function renderHero() {
  const g = state.games.find((x) => x.id === state.selected);
  const hero = $("#hero");
  renderAbout(g);
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
  if (g.installed && g.update) meta.push(`<span class="upd-text">${g.update === "updating" ? "Updating in Steam…" : `Update available${g.update_size ? ` · ${fmtSize(g.update_size)}` : ""}`}</span>`);
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
  play.disabled = (!g.installed && !g.can_install) || !!run || !!g.needs_emulator;
  play.textContent = g.needs_emulator ? "No emulator yet" : !g.installed ? (g.can_install ? "⤓ Install" : "Not installed") : run ? (run.state === "launching" ? "Launching…" : "● Running") : g.kiln_managed && state.kilnPlay ? "▶ Play modded" : "▶ Play";
  play.title = g.needs_emulator ? "None of the emulators this system uses is installed yet" : g.kiln_managed && state.kilnPlay ? "Launches through Kiln, which checks your mods first" : "";
  const options = playOptions(g);
  $("#play-menu").innerHTML = options.map(([mode, label, hint]) => `<button data-play-mode="${esc(mode)}">${label}${hint ? ` <small class="muted">${hint}</small>` : ""}</button>`).join("");
  const split = !!(options.length && g.installed && !run);
  $("#btn-play-menu").classList.toggle("hidden", !split);
  play.classList.toggle("split", split);
  $("#btn-workshop").classList.toggle("hidden", !g.workshop);
  const ub = $("#btn-update");
  ub.classList.toggle("hidden", !(g.installed && g.update === "available"));
  ub.textContent = g.update_via === "steam" ? "⟳ Update in Steam" : "⟳ Update";
  $('#menu [data-act="uninstall"]').classList.toggle("hidden", !(g.installed && g.can_uninstall));
  $('#menu [data-act="runtimes"]').classList.toggle("hidden", !(g.installed && g.prefix));
  const fav = $("#btn-fav");
  fav.textContent = g.favorite ? "★" : "☆";
  fav.classList.toggle("on", g.favorite);
  $('#menu [data-act="remove"]').classList.toggle("hidden", !g.id.startsWith("custom:"));
  $('#menu [data-act="hide"]').textContent = g.hidden ? "Unhide" : "Hide from library";
  const alt = $('#menu [data-act="play-alt"]');
  alt.textContent = g.alt_label ? `▶ ${g.alt_label}` : "";
  alt.classList.toggle("hidden", !(g.alt_label && g.installed && !run));
  const hud = $('#menu [data-act="mangohud"]');
  hud.textContent = `MangoHud overlay: ${hudOn(g) ? "on" : "off"}`;
  // ROMs play in an emulator, not through a store: no store app, MangoHud, runtimes or uninstall.
  const rom = !!g.system;
  hud.classList.toggle("hidden", rom || !g.installed || g.store === "steam");
  const client = $('#menu [data-act="client"]');
  client.classList.toggle("hidden", rom);
  client.textContent = g.installed ? `Open ${s.label === "Custom" ? "Lutris" : g.via.split(" ")[0]}` : `Install with ${g.via.split(" ")[0]}`;
  if (rom) {
    $('#menu [data-act="runtimes"]').classList.add("hidden");
    $('#menu [data-act="uninstall"]').classList.add("hidden");
  }
  // via is "<system> · <emulator>"
  const emuName = (rom && g.via.split(" · ")[1]) || "the emulator";
  $('#menu [data-act="emu-open"]').textContent = `Open ${emuName} (settings, controllers)`;
  $('#menu [data-act="emu-open"]').classList.toggle("hidden", !rom || g.needs_emulator);
  $('#menu [data-act="emu-system"]').classList.toggle("hidden", !rom);
  $('#menu [data-act="rom-reveal"]').classList.toggle("hidden", !rom);
  $('#menu [data-act="ra-page"]').classList.toggle("hidden", !g.ra_id);
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
  const show = state.open && !heroVisible && state.view !== "minecraft" && !$("#hero").classList.contains("empty") && !$("#hero").classList.contains("hidden");
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
  ["ea", "EA"],
  ["ubisoft", "Ubisoft Store"],
];
// ---------- sorting (remembered per library) ----------
const az = (a, b) => a.title.localeCompare(b.title);
const TIER_RANK = { native: 0, platinum: 1, gold: 2, silver: 3, bronze: 4, borked: 5 };
const SORTS = {
  recent: (a, b) => (b.last_played || 0) - (a.last_played || 0) || az(a, b),
  title: az,
  title_desc: (a, b) => b.title.localeCompare(a.title),
  playtime: (a, b) => (b.playtime || 0) - (a.playtime || 0) || az(a, b),
  size: (a, b) => (b.size || 0) - (a.size || 0) || az(a, b),
  protondb: (a, b) => (TIER_RANK[compatOf(a)] ?? 9) - (TIER_RANK[compatOf(b)] ?? 9) || az(a, b),
  store: (a, b) => storeOrder().indexOf(a.store) - storeOrder().indexOf(b.store) || az(a, b),
  favorites: (a, b) => (b.favorite ? 1 : 0) - (a.favorite ? 1 : 0) || az(a, b),
  ra: (a, b) => (b.ra_id ? 1 : 0) - (a.ra_id ? 1 : 0) || az(a, b),
};
const SORT_LABELS = [
  ["recent", "Recently played"], ["title", "A–Z"], ["title_desc", "Z–A"], ["playtime", "Most played"],
  ["favorites", "Favorites first"], ["size", "Size on disk"], ["protondb", "ProtonDB rating"], ["store", "Store"],
  ["ra", "RetroAchievements first"],
];
/** Which sorts make sense in this library. */
function sortChoices() {
  const sys = state.view.startsWith("sys:");
  return SORT_LABELS.filter(([k]) => {
    if (k === "ra") return sys;
    if (k === "protondb") return !WIN && !sys && state.view !== "minecraft";
    if (k === "store") return !sys && !STORES[state.view];
    return true;
  });
}
function currentSort() {
  const fallback = state.view.startsWith("sys:") ? "title" : ls.get("sort", "recent");
  const k = ls.get("sort:" + state.view, fallback);
  return sortChoices().some(([c]) => c === k) ? k : "title";
}
function syncSort() {
  const el = $("#sort");
  const cur = currentSort();
  el.innerHTML = sortChoices().map(([k, l]) => `<option value="${k}" ${k === cur ? "selected" : ""}>${l}</option>`).join("");
  el.title = "Sort this library (remembered for each library)";
}

function render() {
  renderNav();
  syncSort();
  $("#nav-shop").innerHTML = SHOPS.map(([k, l]) => `<button data-view="shop:${k}" class="${state.view === "shop:" + k ? "active" : ""}"><span class="dot" style="background:${store(k).color}"></span>${l}</button>`).join("");
  const shopView = state.view.startsWith("shop:");
  const home = state.view === "home";
  const dlv = state.view === "downloads";
  const beaconView = state.view === "beacon";
  const pluginsView = state.view === "plugins";
  const mc = state.view === "minecraft" || shopView || home || dlv || beaconView || pluginsView;
  for (const el of ["#hero", ".grid-head", "#grid", "#setup", "#filters"]) $(el).classList.toggle("hidden", mc);
  $("#home").classList.toggle("hidden", !home);
  $("#dlv").classList.toggle("hidden", !dlv);
  $("#beacon").classList.toggle("hidden", !beaconView);
  $("#plugins").classList.toggle("hidden", !pluginsView);
  // Opening Plug-ins looks up the latest Beacon release (only then: never in the background).
  if (pluginsView && render.prevView !== "plugins") checkBeaconRelease();
  render.prevView = state.view;
  if (pluginsView) {
    for (const el of ["#empty", "#mini", "#mc", "#shop", "#downloads"]) $(el).classList.add("hidden");
    shopHide();
    renderPluginsPage();
    return;
  }
  document.querySelector("main").classList.toggle("beacon-open", beaconView);
  if (beaconView) {
    for (const el of ["#empty", "#mini", "#mc", "#shop", "#downloads"]) $(el).classList.add("hidden");
    shopHide();
    openBeacon();
    return;
  }
  $("#downloads").classList.toggle("hidden", dlv);
  if (mc) $("#about").classList.add("hidden");
  if (dlv) {
    $("#empty").classList.add("hidden");
    $("#mini").classList.add("hidden");
    $("#mc").classList.add("hidden");
    $("#shop").classList.add("hidden");
    shopHide();
    renderDlView();
    renderRunning();
    return;
  }
  if (home) {
    $("#empty").classList.add("hidden");
    $("#mini").classList.add("hidden");
    shopHide();
    renderHome();
    renderRunning();
    return;
  }
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
  toast(WIN ? "Installing with winget, from the official download…" : "Downloading the official installer (and Proton tools on first use)…");
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
  { id: "ea", name: "EA", color: "#ff4747", client: "EA app" },
  { id: "ubisoft", name: "Ubisoft", color: "#2f7bff", client: "Ubisoft Connect" },
];
const acctBusy = new Set();
async function renderAccounts() {
  let a;
  try { a = await invoke("accounts"); } catch (e) { return toast(String(e), true); }
  const html = ACCT.map((s) => {
    const info = a[s.id] || {};
    const busy = acctBusy.has(s.id);
    let status, actions = "";
    if (s.client) {
      // A Windows launcher (games install and run through it) plus a web sign-in for the owned-games list.
      const open = info.installed ? `<button class="ghost" data-winopen="${s.id}">Open ${s.client}</button>` : `<button class="ghost" data-winsetup="${s.id}">Set up ${s.client}</button>`;
      status = info.account ? `Signed in as <b>${esc(info.account)}</b>: games you own show in the library` : `Sign in to list the games you own${info.installed ? "" : `, and set up ${s.client} to install and play them`}`;
      actions = open + (info.account
        ? `<button class="ghost" data-acct-out="${s.id}">Sign out</button>`
        : `<button class="play small" data-acct-in="${s.id}" ${busy ? "disabled" : ""}>${busy ? "Waiting…" : "Sign in"}</button>`);
    } else if (s.login) {
      status = info.account ? `Signed in as <b>${esc(info.account)}</b>` : "Not signed in";
      actions = info.account
        ? `<button class="ghost" data-acct-out="${s.id}">Sign out</button>`
        : `${info.heroic ? `<button class="ghost" data-acct-heroic="${s.id}" title="Moves Heroic's sign-in to Lodestone; Heroic will ask you to sign in again">Use Heroic's sign-in</button>` : ""}
           <button class="play small" data-acct-in="${s.id}" ${busy ? "disabled" : ""}>${busy ? "Waiting…" : "Sign in"}</button>`;
    } else if (s.id === "steam") {
      status = info.account ? `Signed in as <b>${esc(info.account)}</b> in the Steam client (kept in the tray)` : "Sign in once in the Steam client";
      actions = `<button class="ghost" data-acct-steam>Open Steam</button>`;
    } else {
      status = info.installed ? `Sign in inside ${s.name}` : "Not set up yet";
      actions = info.installed ? `<button class="ghost" data-winopen="${s.id}">Open ${s.name}</button>` : `<button class="ghost" data-winsetup="${s.id}">Set up ${s.name}</button>`;
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
  if (payload.ok) { toast(`Signed in to ${(ACCT.find((a) => a.id === payload.store) || { name: payload.store }).name} as ${payload.account}`); load(true); }
  else toast(`Sign-in failed: ${payload.error}`, true);
  if ($("#dlg-accounts").open || !$("#onboard").classList.contains("hidden")) renderAccounts();
});

// ---------- downloads ----------
let dlTimer = null;
const dlSeen = new Map();
// ---------- downloads: overlay cards and the Downloads tab ----------
const dlSamples = new Map();
const fmtRate = (b) => (b ? `${b >= 1e9 ? (b / 1e9).toFixed(2) + " GB" : b >= 1e6 ? (b / 1e6).toFixed(1) + " MB" : Math.max(1, Math.round(b / 1e3)) + " KB"}/s` : null);
function fmtLeft(s) {
  if (s == null || s <= 0) return null;
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
  return h ? `${h} h ${m} min left` : m ? `${m} min left` : `${s} s left`;
}
function fmtDur(s) {
  if (!s) return "—";
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = s % 60;
  return h ? `${h} h ${m} min` : m ? `${m} min ${sec} s` : `${sec} s`;
}
const dlGame = (d) => state.games.find((g) => g.id === d.game_id);
const dlDone = (d) => ({ update: "Up to date", uninstall: "Uninstalled", runtimes: "Runtimes installed" }[d.kind] || "Installed");
const dlKind = (k) => ({ update: "Update", uninstall: "Uninstall", runtimes: "Runtimes" }[k] || "Install");
function dlArt(d, kind) {
  const g = dlGame(d);
  const p = g && (kind === "cover" ? g.cover || g.hero : g.hero || g.cover);
  return p ? imgSrc(p, kind === "cover" ? 120 : 640) : null;
}
// One line: "45% · 1.2 GB of 5.0 GB · 12.3 MB/s · 4 min left"
function dlLine(d) {
  if (d.state === "done") return dlDone(d) + (d.total ? ` · ${fmtSize(d.total)}` : "") + ` · took ${fmtDur(d.seconds)}`;
  if (d.state === "failed") return "Failed";
  const parts = [];
  if (d.percent > 0) parts.push(`${d.percent.toFixed(1)}%`);
  if (d.total) parts.push(`${fmtSize(d.downloaded || 0) || "0 MB"} of ${fmtSize(d.total)}`);
  const r = fmtRate(d.speed);
  if (r) parts.push(r);
  const left = fmtLeft(d.eta);
  if (left) parts.push(left);
  if (!parts.length || (d.percent <= 0 && !d.total)) parts.push(esc(d.detail || "Starting…"));
  return parts.join(" · ");
}
function dlCard(d, big) {
  // Downloads found in Battle.net's records only know their install id: show the game's name.
  const g = dlGame(d);
  if (g && /^[a-z0-9_]+$/.test(d.title)) d = { ...d, title: d.kind === "update" ? `Updating ${g.title}` : g.title };
  const art = dlArt(d, "hero");
  const s = store(d.store);
  const btn = d.state === "downloading" && d.kind !== "install" ? "" : `<button class="dl-x" title="${d.state === "downloading" ? "Cancel" : "Dismiss"}" data-dl="${esc(d.id)}" data-dl-state="${d.state}">✕</button>`;
  if (!big) {
    return `<div class="dl ${d.state}">
      <div class="dl-banner" ${art ? `style="background-image:url('${cssUrl(art)}')"` : `style="${placeholder({ title: d.title })}"`}></div>
      <div class="dl-body">
        <div class="dl-top"><span class="grow dl-title">${esc(d.title)}</span>${btn}</div>
        <div class="dl-meta"><span class="dot" style="background:${s.color}"></span>${esc(s.label)} · ${dlKind(d.kind)}</div>
        <div class="bar"><span style="width:${Math.max(2, d.percent)}%"></span></div>
        <div class="dl-line">${dlLine(d)}</div>
        ${d.state === "failed" ? `<small class="err-text">${esc(d.detail)}</small>` : ""}
      </div>
    </div>`;
  }
  const stat = (k, v) => (v ? `<div><small>${k}</small><span>${v}</span></div>` : "");
  const samples = dlSamples.get(d.id) || [];
  return `<div class="dlv-card ${d.state}">
    <div class="dlv-banner" ${art ? `style="background-image:url('${cssUrl(art)}')"` : `style="${placeholder({ title: d.title })}"`}>
      <div class="dlv-over">
        <div class="dl-meta"><span class="dot" style="background:${s.color}"></span>${esc(s.label)} · ${dlKind(d.kind)}</div>
        <h3>${esc(d.title)}</h3>
      </div>
      ${btn.replace('class="dl-x"', 'class="dl-x big"')}
    </div>
    <div class="dlv-main">
      <div class="dlv-progress"><div class="bar big"><span style="width:${Math.max(2, d.percent)}%"></span></div><b>${d.state === "downloading" ? `${d.percent.toFixed(1)}%` : d.state === "done" ? dlDone(d) : "Failed"}</b></div>
      ${d.state === "downloading" && samples.some((x) => x > 0) ? sparkline(samples) : ""}
      <div class="dlv-stats">
        ${d.state === "downloading" ? stat("Downloaded", d.total ? `${fmtSize(d.downloaded || 0) || "0 MB"} of ${fmtSize(d.total)}` : fmtSize(d.downloaded)) : stat("Size", fmtSize(d.total || d.downloaded))}
        ${stat("Speed", d.state === "downloading" ? fmtRate(d.speed) : null)}
        ${stat("Disk writes", d.state === "downloading" ? fmtRate(d.disk) : null)}
        ${stat("Time left", d.state === "downloading" ? fmtLeft(d.eta)?.replace(" left", "") : null)}
        ${stat(d.state === "downloading" ? "Running for" : "Took", fmtDur(d.seconds))}
        ${stat("Size installed", fmtSize(d.install_size))}
        ${stat("Status", d.state === "downloading" ? esc(d.detail || "") : null)}
      </div>
      ${d.path ? `<div class="dlv-path"><small>Install folder</small><code>${esc(d.path)}</code></div>` : ""}
      ${d.state === "failed" ? `<div class="err-text">${esc(d.detail)}</div>` : ""}
    </div>
  </div>`;
}
function sparkline(v) {
  const w = 300, h = 44, max = Math.max(...v, 1);
  const pts = v.map((x, i) => `${(i / Math.max(1, v.length - 1)) * w},${h - (x / max) * (h - 4) - 2}`).join(" ");
  return `<div class="dlv-spark" title="Download speed (peak ${fmtRate(max)})"><svg viewBox="0 0 ${w} ${h}" preserveAspectRatio="none"><polyline points="0,${h} ${pts} ${w},${h}" class="area"/><polyline points="${pts}" class="line"/></svg><small>Speed · peak ${fmtRate(max)}</small></div>`;
}
let dlHistory = null;
async function renderDlView() {
  const list = state.dl || [];
  const active = list.filter((d) => d.state === "downloading");
  const speed = active.reduce((a, d) => a + (d.speed || 0), 0);
  const left = active.reduce((a, d) => a + (d.total ? Math.max(0, d.total - (d.downloaded || 0)) : 0), 0);
  $("#dlv-summary").innerHTML = active.length
    ? `<span><b>${active.length}</b> downloading</span>${speed ? `<span><b>${fmtRate(speed)}</b> total</span>` : ""}${left ? `<span><b>${fmtSize(left)}</b> to go</span>` : ""}`
    : `<span class="muted">Nothing downloading right now.</span>`;
  $("#dlv-active").innerHTML = list.length ? list.map((d) => dlCard(d, true)).join("") : `<div class="dlv-empty muted">Installs, updates and uninstalls from every store show up here while they run: Steam, Epic, GOG and Battle.net.</div>`;
  const upd = state.games.filter((g) => g.installed && g.update === "available" && !g.hidden && !list.some((d) => d.game_id === g.id && d.state === "downloading"));
  $("#dlv-updates-wrap").classList.toggle("hidden", !upd.length);
  $("#dlv-updates").innerHTML = upd.map((g) => `<div class="dlv-row">
      <img src="${esc(imgSrc(g.cover || g.hero, 80) || "")}" alt="" data-err="hide">
      <div class="grow"><b>${esc(g.title)}</b><small class="muted">${esc(store(g.store).label)}${g.update_size ? ` · ${fmtSize(g.update_size)}` : ""}</small></div>
      <button class="ghost small" data-dlv-update="${esc(g.id)}">⟳ Update</button></div>`).join("");
  if (dlHistory === null) { try { dlHistory = await invoke("downloads_history"); } catch { dlHistory = []; } }
  const hist = dlHistory.filter((p) => !list.some((d) => d.id === p.id && d.state !== "downloading"));
  $("#dlv-history").innerHTML = hist.length ? hist.map((p) => {
    const g = state.games.find((x) => x.id === p.game_id);
    const art = g && (g.cover || g.hero);
    return `<div class="dlv-row ${p.state}">
      <img src="${esc(art ? imgSrc(art, 80) : "")}" alt="" data-err="hide">
      <div class="grow"><b>${esc(p.title)}</b><small class="muted">${esc(store(p.store).label)} · ${dlKind(p.kind)}${p.total ? ` · ${fmtSize(p.total)}` : ""} · took ${fmtDur(p.seconds)}</small>${p.state === "failed" && p.detail ? `<small class="err-text">${esc(p.detail)}</small>` : ""}</div>
      <span class="dlv-res ${p.state}">${p.state === "done" ? "✓ " + dlDone(p) : "✕ Failed"}</span>
      <small class="muted dlv-when" title="${new Date(p.finished * 1000).toLocaleString()}">${fmtAgo(p.finished)}</small></div>`;
  }).join("") : `<div class="muted">Finished downloads are listed here.</div>`;
}
$("#dlv").addEventListener("click", async (e) => {
  const u = e.target.closest("[data-dlv-update]");
  if (u) { u.disabled = true; return homeUpdate(u.dataset.dlvUpdate); }
  if (e.target.id === "dlv-clear") {
    await invoke("downloads_history_clear");
    dlHistory = [];
    return renderDlView();
  }
  const b = e.target.closest("[data-dl]");
  if (b) dlAction(b);
});
async function dlAction(b) {
  if (b.dataset.dlState === "downloading") {
    if (!confirm("Cancel this download? You can resume it later by installing again.")) return;
    try { await invoke("download_cancel", { id: b.dataset.dl }); } catch (err) { toast(String(err), true); }
  } else await invoke("download_dismiss", { id: b.dataset.dl });
  dlHistory = null;
  pollDownloads();
}

async function pollDownloads() {
  clearTimeout(dlTimer);
  let list = [];
  try { list = (await invoke("downloads_status")) || []; } catch {}
  for (const d of list) {
    const prev = dlSeen.get(d.id);
    if (prev === "downloading" && d.state === "done") {
      toast(d.kind === "update" ? `${d.title.replace(/^Updating /, "")} is up to date` : d.kind === "uninstall" ? `${d.title.replace(/^Uninstalling /, "")} is uninstalled` : `${d.title} is installed`);
      load(true);
    }
    if (prev === "downloading" && d.state === "failed") toast(`${d.title} failed: ${d.detail}`, true);
    if (prev === "downloading" && d.state !== "downloading") dlHistory = null;
    dlSeen.set(d.id, d.state);
  }
  const before = (state.dl || []).filter((d) => d.state === "downloading").length;
  state.dl = list;
  for (const d of list) {
    if (d.state !== "downloading") continue;
    const s = dlSamples.get(d.id) || [];
    s.push(d.speed || 0);
    if (s.length > 120) s.shift();
    dlSamples.set(d.id, s);
  }
  $("#downloads").innerHTML = list.map((d) => dlCard(d, false)).join("") + (list.length ? `<button class="dl-open ghost small" data-view="downloads">Open Downloads</button>` : "");
  if (state.view === "downloads") renderDlView();
  if (before !== list.filter((d) => d.state === "downloading").length) renderNav();
  // Fast while something's downloading; otherwise every few seconds, so downloads Steam starts by itself
  // (or restarts after pausing them) show up quickly.
  dlTimer = setTimeout(pollDownloads, list.some((d) => d.state === "downloading") ? 1500 : 5000);
}
if (T) T.event.listen("downloads-changed", ({ payload }) => { toast(`Installing ${payload.title} in the background…`); pollDownloads(); });
if (T) T.event.listen("shop-error", ({ payload }) => toast(String(payload), true));
$("#downloads").addEventListener("click", (e) => {
  const b = e.target.closest("[data-dl]");
  if (b) dlAction(b);
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
          <div class="ws-thumb" style="${i.preview ? `background-image:url('${cssUrl(i.preview)}')` : ""}"></div>
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
  if (WIN) return; // Windows games run natively: no Proton
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
$("#steam-bg").onchange = async (e) => { await invoke("set_steam_background", { on: e.target.checked }); state.steamBg = e.target.checked; };
async function renderMangohud() {
  if (WIN) return; // MangoHud is for Linux
  const m = await invoke("mangohud_info");
  $("#mh-status").textContent = m.installed === "system" ? "MangoHud is installed on your system" : m.installed ? `MangoHud ${m.installed} is set up` : "MangoHud isn't set up yet";
  $("#mh-setup").textContent = m.installed === "system" ? "Installed" : m.installed ? "Update" : "Set up MangoHud";
  $("#mh-setup").disabled = m.installed === "system";
  $("#mh-all").checked = m.all_games;
  return m;
}
// ---------- MangoHud settings editor ----------
// Checkboxes write key=1 / key=0 (MangoHud shows some things by default, so off has to be explicit).
const MH_SHOW = [
  ["fps", "FPS", 1], ["frametime", "Frame time (ms)", 1], ["frame_timing", "Frame time graph", 1], ["fps_metrics", "1% and 0.1% lows", 0],
  ["cpu_stats", "CPU load", 1], ["cpu_temp", "CPU temperature", 0], ["cpu_power", "CPU power", 0], ["cpu_mhz", "CPU clock", 0],
  ["gpu_stats", "GPU load", 1], ["gpu_temp", "GPU temperature", 0], ["gpu_power", "GPU power", 0], ["gpu_core_clock", "GPU clock", 0], ["gpu_name", "GPU name", 0],
  ["vram", "VRAM used", 0], ["ram", "RAM used", 0], ["swap", "Swap used", 0],
  ["resolution", "Resolution", 0], ["vulkan_driver", "Driver", 0], ["wine", "Wine/Proton version", 0], ["engine_version", "Graphics API (DXVK, VKD3D…)", 0],
  ["gamemode", "GameMode status", 0], ["battery", "Battery", 0], ["time", "Clock", 0], ["show_fps_limit", "FPS limit", 0],
];
const MH_VALUES = ["position", "font_size", "background_alpha", "round_corners", "fps_limit", "toggle_hud"];
const MH_PRESETS = {
  fps: { on: ["fps"], set: { position: "top-left" } },
  basic: { on: ["fps", "frametime", "frame_timing", "cpu_stats", "gpu_stats"], set: { position: "top-left" } },
  full: { on: MH_SHOW.map((x) => x[0]), set: {} },
  bar: { on: ["fps", "frametime", "cpu_stats", "gpu_stats", "cpu_temp", "gpu_temp", "ram", "vram"], set: { position: "top-center", horizontal: true } },
};
$("#mh-checks").innerHTML = MH_SHOW.map(([k, l]) => `<label class="check"><input type="checkbox" data-mh="${k}" /> ${l}</label>`).join("");
// The file as key -> value ("" for a bare key like "fps"); comments and order are kept in mhLines.
function mhParse(text) {
  const m = new Map();
  for (const line of text.split("\n")) {
    const t = line.trim();
    if (!t || t.startsWith("#")) continue;
    const i = t.indexOf("=");
    m.set((i < 0 ? t : t.slice(0, i)).trim(), i < 0 ? "" : t.slice(i + 1).trim());
  }
  return m;
}
function mhOn(m, k, def) {
  if (!m.has(k)) return !!def && !m.has("full");
  const v = m.get(k);
  return v === "" || v === "1" || v === "true";
}
function mhToForm() {
  const m = mhParse($("#mh-text").value);
  const f = $("#form-mh");
  // An empty file means MangoHud's own default set.
  const empty = m.size === 0;
  for (const [k, , def] of MH_SHOW) f.querySelector(`[data-mh="${k}"]`).checked = m.has("full") ? !(m.get(k) === "0") : mhOn(m, k, empty || def);
  f.position.value = m.get("position") || "top-left";
  f.horizontal.checked = mhOn(m, "horizontal", 0);
  f.no_display.checked = mhOn(m, "no_display", 0);
  for (const k of ["font_size", "background_alpha", "round_corners", "fps_limit", "toggle_hud"]) f[k].value = m.get(k) || "";
}
// Write the form into the file text, changing only the lines for these settings.
function mhFromForm() {
  const f = $("#form-mh");
  const want = new Map();
  for (const [k] of MH_SHOW) want.set(k, f.querySelector(`[data-mh="${k}"]`).checked ? "1" : "0");
  want.set("full", null);
  want.set("position", f.position.value);
  want.set("horizontal", f.horizontal.checked ? "1" : null);
  want.set("no_display", f.no_display.checked ? "1" : null);
  for (const k of ["font_size", "background_alpha", "round_corners", "fps_limit", "toggle_hud"]) want.set(k, f[k].value.trim() || null);
  const lines = $("#mh-text").value.split("\n");
  const done = new Set();
  const out = [];
  for (const line of lines) {
    const t = line.trim();
    const key = t && !t.startsWith("#") ? t.split("=")[0].trim() : null;
    if (key && want.has(key)) {
      if (done.has(key)) continue;
      done.add(key);
      const v = want.get(key);
      if (v !== null) out.push(`${key}=${v}`);
    } else out.push(line);
  }
  while (out.length && out[out.length - 1] === "") out.pop();
  // New lines only where it differs from MangoHud's default (lines already in the file are kept explicit).
  const defaults = new Map(MH_SHOW.map(([k, , d]) => [k, d ? "1" : "0"]));
  const added = [...want].filter(([k, v]) => !done.has(k) && v !== null && defaults.get(k) !== v);
  if (added.length && !out.some((l) => l.includes("Set by Lodestone"))) out.push("", "# Set by Lodestone");
  for (const [k, v] of added) out.push(`${k}=${v}`);
  $("#mh-text").value = out.join("\n") + "\n";
}
$("#form-mh").addEventListener("change", (e) => { if (e.target.id !== "mh-text") mhFromForm(); });
$("#mh-text").addEventListener("input", mhToForm);
$("#form-mh").addEventListener("click", (e) => {
  const p = e.target.closest("[data-mh-preset]");
  if (!p) return;
  const pre = MH_PRESETS[p.dataset.mhPreset], f = $("#form-mh");
  for (const [k] of MH_SHOW) f.querySelector(`[data-mh="${k}"]`).checked = pre.on.includes(k);
  f.horizontal.checked = !!pre.set.horizontal;
  if (pre.set.position) f.position.value = pre.set.position;
  mhFromForm();
});
$("#mh-config").onclick = async () => {
  try {
    const [path, text] = await invoke("mangohud_config_get");
    $("#mh-path").textContent = path;
    $("#mh-text").value = text;
    mhToForm();
    $("#dlg-mh").showModal();
  } catch (e) { toast(String(e), true); }
};
$("#mh-cancel").onclick = () => $("#dlg-mh").close();
$("#mh-preview").onclick = async () => {
  try {
    await invoke("mangohud_config_save", { text: $("#mh-text").value });
    await invoke("mangohud_preview");
    toast("Preview window opening. MangoHud picks up saved changes while it runs.");
  } catch (e) { toast(String(e), true); }
};
$("#form-mh").onsubmit = async (e) => {
  e.preventDefault();
  try {
    await invoke("mangohud_config_save", { text: $("#mh-text").value });
    $("#dlg-mh").close();
    toast("MangoHud settings saved. Games pick them up the next time they start (running ones within a second or two).");
  } catch (err) { toast(String(err), true); }
};

$("#mh-setup").onclick = async () => {
  $("#mh-status").textContent = "Downloading MangoHud…";
  $("#mh-setup").disabled = true;
  try { const v = await invoke("setup_mangohud"); toast(`MangoHud ${v} is set up`); } catch (e) { toast(String(e), true); }
  renderMangohud();
};
$("#mh-all").onchange = async (e) => {
  const on = e.target.checked;
  try {
    if (on && !(await invoke("mangohud_info")).installed) { $("#mh-status").textContent = "Downloading MangoHud…"; await invoke("setup_mangohud"); }
    await invoke("set_mangohud", { on });
    state.mangohudAll = on;
    renderHero();
  } catch (err) { e.target.checked = !on; toast(String(err), true); }
  renderMangohud();
};
$("#kiln-play").onchange = async (e) => { await invoke("set_kiln_play", { on: e.target.checked }); state.kilnPlay = e.target.checked; renderHero(); };
$("#merge-dupes").onchange = (e) => { ls.set("mergeDupes", e.target.checked); render(); };
/** Settings → Emulation: which ROMs folders Lodestone found, and whether the chosen one has ROM folders. */
async function renderEmuFolders() {
  const el = $("#emu-found");
  try {
    const f = await invoke("emu_folders");
    const list = f.found.map((p) => `<code>${esc(p)}</code>`).join(", ");
    el.innerHTML = (f.chosen && !f.chosen_ok ? `<span class="err-text">${esc(f.chosen)} has no system folders yet. Create folders, or choose another folder.</span><br>` : "")
      + (f.found.length ? `Found: ${list}` : "No ROMs folder found yet.");
  } catch (e) { el.textContent = String(e); }
}
$("#emu-browse").onclick = async () => {
  const input = $("#form-settings").emudir;
  try { const p = await invoke("emu_pick_folder", { start: input.value }); if (p) input.value = p; } catch (e) { toast(String(e), true); }
};
$("#emu-create").onclick = async () => {
  const input = $("#form-settings").emudir;
  try {
    const [roms, made] = await invoke("emu_create_folders", { path: input.value });
    if (!input.value.trim()) input.value = "~/Emulation";
    toast(made ? `Made ${made} system folders in ${roms}. Put each system's ROMs in its folder, then Rescan.` : `${roms} already has every system folder.`);
    renderEmuFolders();
    load(true);
  } catch (e) { toast(String(e), true); }
};
$("#btn-settings").onclick = async () => {
  renderStoreOrder();
  const s = await invoke("get_settings");
  $("#kiln-play").checked = s.kiln_play !== false;
  $("#steam-bg").checked = s.steam_background !== false;
  $("#form-settings").cf.value = s.curseforge_api_key || "";
  $("#form-settings").gamesdir.value = s.games_dir || "";
  $("#form-settings").steamkey.value = s.steam_api_key || "";
  $("#form-settings").sgdb.value = s.steamgriddb_key || "";
  $("#form-settings").rauser.value = s.ra_user || "";
  $("#form-settings").rakey.value = s.ra_key || "";
  $("#form-settings").emudir.value = s.emulation_dir || "";
  renderEmuFolders();
  renderProton().catch((e) => toast(String(e), true));
  renderMangohud().catch((e) => toast(String(e), true));
  $("#dlg-settings").showModal();
};
$("#form-settings").onsubmit = async (e) => {
  e.preventDefault();
  try {
    const keyChanged = e.target.steamkey.value.trim() !== ((await invoke("get_settings")).steam_api_key || "");
    const before = await invoke("get_settings");
    await invoke("save_settings", { curseforgeApiKey: e.target.cf.value, gamesDir: e.target.gamesdir.value, steamApiKey: e.target.steamkey.value, steamgriddbKey: e.target.sgdb.value, raUser: e.target.rauser.value, raKey: e.target.rakey.value });
    if ((e.target.rauser.value.trim() && e.target.rakey.value.trim()) && (before.ra_user !== e.target.rauser.value.trim() || before.ra_key !== e.target.rakey.value.trim())) {
      toast("Matching your ROMs with RetroAchievements in the background…");
      invoke("ra_sync").catch((err) => toast(String(err), true));
    }
    const emuChanged = e.target.emudir.value.trim() !== (before.emulation_dir || "");
    if (emuChanged) await invoke("emu_set_folder", { path: e.target.emudir.value });
    await invoke("set_proton", { path: e.target.proton.value });
    $("#dlg-settings").close();
    toast("Settings saved");
    if (keyChanged && e.target.steamkey.value.trim()) {
      toast("Checking your Steam library…");
      try { await invoke("store_refresh"); toast("Steam library updated from your account"); } catch (err) { toast(String(err), true); }
      load(true);
    }
    if (emuChanged) load(true);
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
  if (state.open) { parkDetail(); placeDetail(); }
}

// ---------- actions ----------
// The ▾ menu next to Play: [mode, label, hint].
function playOptions(g) {
  const o = [];
  if (g.kiln_managed) {
    o.push(["modded", "▶ Play modded", "through Kiln's checks"], ["vanilla", "▶ Play without mods", "mods go back next time"]);
  }
  if (g.pick_realm) o.push(["realm", "▶ Play and choose a realm", "shows the realm list"]);
  if (g.alt_label) o.push(["alt", `▶ ${esc(g.alt_label)}`, ""]);
  return o;
}
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
    } else if (how === "client-fallback") {
      toast(`Couldn't install ${g.title} in the background, so Battle.net is opening on its page. Click Install there.`, true);
    } else if (how === "client-busy") {
      toast(`A Steam game is running, so Steam's own install dialog opened for ${g.title}.`);
    } else {
      toast(g.store === "steam" ? `Steam's install dialog is opening for ${g.title}. Rescan when it's done.`
        : g.store === "ubisoft" ? `Opening Ubisoft Connect: find ${g.title} under Games and click Download. Rescan when it's done.`
        : `Opening ${g.via.split(" ·")[0]} to install ${g.title}…`);
    }
  } catch (e) {
    toast(String(e), true);
  }
}

const hudOn = (g) => (g.mangohud ?? state.mangohudAll) === true;
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
    if (act === "play-alt") return play(g.id, "alt");
    if (act === "mangohud") {
      const on = !hudOn(g);
      if (on && !(await invoke("mangohud_info")).installed) {
        toast("Setting up MangoHud…");
        await invoke("setup_mangohud");
      }
      await setFlag(g.id, "mangohud", on);
      toast(`MangoHud ${on ? "on" : "off"} for ${g.title}${on ? " from its next launch" : ""}`);
    }
    if (act === "edit") await openMeta(g);
    if (act === "log") {
      $("#log-text").textContent = await invoke("read_log", { id: g.id });
      $("#dlg-log").showModal();
    }
    if (act === "emu-open") { await invoke("emu_open", { id: g.id }); toast("Opening the emulator…"); }
    if (act === "emu-system") { state.view = "sys:" + g.system; ls.set("view", state.view); render(); }
    if (act === "rom-reveal") await invoke("rom_reveal", { id: g.id });
    if (act === "ra-page") await invoke("open_page", { url: `https://retroachievements.org/game/${g.ra_id}`, title: g.title });
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
      const own = g.via.startsWith("Lodestone") || (g.store === "steam" && state.steamBg);
      const msg = g.store === "gog" ? `Uninstall ${g.title}? Its folder goes to the trash.`
        : g.store === "steam" ? `Uninstall ${g.title}? Its game files are deleted. Saves in Steam Cloud${WIN ? "" : " and its Proton prefix"} are kept.`
        : `Uninstall ${g.title}? Its files are deleted.`;
      if (own && !confirm(msg)) return;
      const how = await invoke("uninstall_game", { id: g.id });
      if (how === "done") { toast(`Uninstalled ${g.title}`); await load(true); }
      else if (how === "background") { toast(`Uninstalling ${g.title}…`); pollDownloads(); }
      else if (how === "client-busy") toast(`A Steam game is running, so Steam will ask to confirm uninstalling ${g.title}.`);
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

// ---------- Lodestone Beacon (a plug-in: its own program, shown in a frame) ----------
// The frame shows a page Beacon serves on 127.0.0.1. It can't use Lodestone's commands; Beacon only reaches
// Lodestone through the bridge, for the permissions you approve here.
async function openBeacon() {
  const msg = $("#beacon-msg"), frame = $("#beacon-frame");
  if (frame.src && !frame.classList.contains("hidden")) return;
  msg.innerHTML = '<p class="muted">Starting Lodestone Beacon…</p>';
  try {
    let st = await invoke("beacon_status");
    if (!st.installed) {
      msg.innerHTML = `<div class="beacon-intro"><h2>Lodestone Beacon</h2>
        <p>Text chat with your friends on servers you host yourselves, end-to-end encrypted. It's a separate program with its own sandbox: it can't see your store sign-ins, and it only gets what you allow below.</p>
        <p class="muted">Beacon isn't installed yet. It's in early development.</p>
        <p><button class="play" id="beacon-install">Install Beacon</button></p>
        <p class="muted small">Downloaded from Lodestone's official releases on GitHub and checked against GitHub's published checksum before it's installed.</p></div>`;
      $("#beacon-install").onclick = async (e) => {
        const b = e.currentTarget;
        b.disabled = true;
        b.textContent = "Downloading and checking…";
        try { const v = await invoke("beacon_install"); toast(`Beacon ${v} is installed.`); openBeacon(); refreshPlugins(); }
        catch (err) { b.disabled = false; b.textContent = "Install Beacon"; toast(String(err), true); }
      };
      return;
    }
    st = await invoke("beacon_start");
    if (st.needs_approval) return askBeaconPermissions(st);
    showBeacon(st);
  } catch (e) { msg.innerHTML = `<p class="err-text">${esc(String(e))}</p>`; }
}
function askBeaconPermissions(st) {
  $("#beacon-msg").innerHTML = `<div class="beacon-intro"><h2>Allow Lodestone Beacon?</h2>
    <p>Beacon ${esc(st.version || "")} runs as its own program and can't see your store sign-ins, keys or files. It asks Lodestone for:</p>
    <ul>${st.requested.map(([, d]) => `<li>${esc(d)}</li>`).join("")}</ul>
    <p class="muted">You can take this back in Settings → Plug-ins.</p>
    <div class="row-btns"><button class="play" id="beacon-allow">Allow</button><button class="ghost" data-view="home">Not now</button></div></div>`;
  $("#beacon-allow").onclick = async () => {
    try { showBeacon(await invoke("beacon_approve", { permissions: st.requested.map(([p]) => p) })); } catch (e) { toast(String(e), true); }
  };
}
function showBeacon(st) {
  if (!st.ui) return ($("#beacon-msg").innerHTML = '<p class="err-text">Lodestone Beacon didn\'t start. Its log is lodestone-beacon.log in the logs folder of ' + esc(state.dataDir || "Lodestone's data folder") + '.</p>');
  $("#beacon-msg").innerHTML = "";
  const frame = $("#beacon-frame");
  frame.classList.remove("hidden");
  if (frame.src !== st.ui) frame.src = st.ui;
  // Lodestone's colours, so Beacon matches.
  const cs = getComputedStyle(document.documentElement);
  const vars = Object.fromEntries(["--bg", "--bg-2", "--bg-3", "--line", "--text", "--muted", "--accent", "--danger"].map((k) => [k, cs.getPropertyValue(k).trim()]));
  invoke("beacon_theme", { vars }).catch(() => {});
}
if (T) {
  T.event.listen("beacon-ready", () => refreshPlugins());
  T.event.listen("beacon-stopped", () => {
    refreshPlugins(); const f = $("#beacon-frame"); f.removeAttribute("src"); f.classList.add("hidden"); if (state.view === "beacon") openBeacon(); });
  // Beacon asked to invite a friend: copy the invite (Lodestone opened their Steam chat) and say so.
  T.event.listen("beacon-open-chat", async (e) => {
    const { store, id, text } = e.payload || {};
    try { await navigator.clipboard.writeText(text); } catch { return toast("Couldn't copy the invite to the clipboard", true); }
    toast(store === "steam" ? "Invite copied: paste it into the Steam chat that just opened." : "Invite copied: send it to your friend.");
  });
}

// ---------- Plug-ins page ----------
async function refreshPlugins() {
  try { state.plugins = await invoke("plugins_list"); } catch (e) { toast(String(e), true); }
  renderNav();
  if (state.view === "plugins") renderPluginsPage();
}
function renderPluginsPage() {
  const list = state.plugins || [];
  $("#plugins-list").innerHTML = list.map((p) => {
    const status = !p.installed ? "Not installed" : p.running ? `Running${p.version ? " · version " + esc(p.version) : ""}` : "Installed · not running";
    const perms = p.permissions.map(([id, words, granted]) => `<li><span class="perm ${granted ? "on" : ""}">${granted ? "Allowed" : "Not allowed"}</span> ${esc(words)}</li>`).join("");
    const anyGranted = p.permissions.some(([, , g]) => g);
    return `<div class="plugin-card" data-plugin="${esc(p.id)}">
      <div class="plugin-head"><div><h2>${esc(p.name)}</h2><p class="muted">${esc(p.description)}</p></div><span class="plugin-status ${p.running ? "on" : ""}">${status}</span></div>
      ${p.installed ? `<h4>Permissions</h4><ul class="perm-list">${perms}</ul>` : `<p class="muted">${esc(p.install_hint)}</p>`}
      ${p.id === "beacon" ? beaconReleaseHTML(p) : ""}
      <div class="row-btns">
        ${p.installed ? `<button class="play" data-view="${p.id === "beacon" ? "beacon" : ""}">Open</button>` : ""}
        ${p.id === "beacon" && state.beaconRelease?.latest && (!p.installed || state.beaconRelease.update) ? `<button class="${p.installed ? "ghost" : "play"}" data-plugin-act="install">${p.installed ? "Update to" : "Install Beacon"} ${esc(state.beaconRelease.latest)}</button>` : ""}
        ${p.running ? '<button class="ghost" data-plugin-act="stop">Stop</button>' : ""}
        ${anyGranted ? '<button class="ghost" data-plugin-act="revoke">Revoke permissions</button>' : ""}
      </div></div>`;
  }).join("") || '<p class="muted">No plug-ins yet.</p>';
}
$("#plugins-list").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-plugin-act]");
  if (!b) return;
  const id = b.closest("[data-plugin]").dataset.plugin;
  try {
    if (b.dataset.pluginAct === "stop") await invoke("plugin_stop", { id });
    if (b.dataset.pluginAct === "install") {
      b.disabled = true;
      b.textContent = "Downloading and checking…";
      const v = await invoke("beacon_install");
      state.beaconRelease = { ...state.beaconRelease, installed: v, update: false };
      const f = $("#beacon-frame"); f.removeAttribute("src"); f.classList.add("hidden");
      toast(`Beacon ${v} is installed.`);
    }
    if (b.dataset.pluginAct === "revoke") {
      await invoke("beacon_approve", { permissions: [] });
      const f = $("#beacon-frame"); f.removeAttribute("src"); f.classList.add("hidden");
      toast("Permissions removed. The plug-in will ask again next time you open it.");
    }
  } catch (err) { toast(String(err), true); }
  refreshPlugins();
});
$("#open-plugins").onclick = () => { $("#dlg-settings").close(); state.view = "plugins"; ls.set("view", "plugins"); render(); refreshPlugins(); };

// Beacon releases: looked up when you open Plug-ins (or Beacon while it isn't installed), never in the
// background. Installing or updating only happens when you press the button.
async function checkBeaconRelease() {
  try { state.beaconRelease = await invoke("beacon_check_update"); } catch (e) { state.beaconRelease = { error: String(e) }; }
  if (state.view === "plugins") renderPluginsPage();
}
function beaconReleaseHTML(p) {
  const r = state.beaconRelease;
  if (!r) return '<p class="muted small">Checking for the latest Beacon…</p>';
  if (r.error) return `<p class="muted small">${esc(r.error)}</p>`;
  if (r.update) return `<p class="small">Beacon ${esc(r.latest)} is out (you have ${esc(r.installed)}). Updating keeps your communities and messages.</p>`;
  if (!p.installed) return `<p class="small">Latest: Beacon ${esc(r.latest)}. It's downloaded from Lodestone's official GitHub releases and checked before it's installed.</p>`;
  return "";
}

// ---------- collapsible sidebar sections (remembered) ----------
// A collapsed section still shows the library you're in, so you never lose your place.
function applySections() {
  const closed = ls.get("navCollapsed", []);
  document.querySelectorAll(".nav-label[data-section]").forEach((l) => {
    const shut = closed.includes(l.dataset.section);
    l.setAttribute("aria-expanded", String(!shut));
    l.title = shut ? "Show this section" : "Hide this section";
    $("#" + l.dataset.section).classList.toggle("collapsed", shut);
  });
}
document.querySelectorAll(".nav-label[data-section]").forEach((l) => {
  l.onclick = () => {
    const id = l.dataset.section;
    const closed = ls.get("navCollapsed", []);
    ls.set("navCollapsed", closed.includes(id) ? closed.filter((x) => x !== id) : [...closed, id]);
    applySections();
  };
});
applySections();

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
  if (wo) return invoke("winapp_open", { id: wo.dataset.winopen }).then(() => toast(`Opening ${(CLIENTS[wo.dataset.winopen] || { name: "it" }).name}…`)).catch((err) => toast(String(err), true));
  const forget = e.target.closest("[data-forget]");
  if (forget) return invoke("forget_session", { id: forget.dataset.forget });
  const act = e.target.closest("[data-act]");
  if (act) return menuAction(act.dataset.act);
  if (!e.target.closest(".menu-wrap")) $("#menu").classList.add("hidden");
  const card = e.target.closest("#grid .card");
  if (card && !e.target.closest("#detail")) {
    openDetail(card.dataset.id);
  }
});
$("#btn-play").onclick = () => {
  const g = state.games.find((x) => x.id === state.selected);
  if (g && !g.installed && g.can_install) return install(g);
  play(state.selected);
};
$("#btn-play-menu").onclick = (e) => { e.stopPropagation(); $("#play-menu").classList.toggle("hidden"); placeMenu($("#play-menu"), $("#btn-play")); };
$("#play-menu").onclick = (e) => {
  const b = e.target.closest("[data-play-mode]");
  if (!b) return;
  $("#play-menu").classList.add("hidden");
  play(state.selected, b.dataset.playMode);
};
document.addEventListener("click", (e) => { if (!e.target.closest(".play-wrap")) $("#play-menu").classList.add("hidden"); });
$("#btn-update").onclick = async () => {
  const g = state.games.find((x) => x.id === state.selected);
  if (!g) return;
  try {
    const how = await invoke("update_game", { id: g.id });
    if (how === "download") { toast(`Updating ${g.title}…`); pollDownloads(); }
    else toast(`A Steam game is running, so Steam's Downloads page opened. Steam updates ${g.title} there (or right before you play).`);
  } catch (e) { toast(String(e), true); }
};
$("#btn-fav").onclick = () => {
  const g = state.games.find((x) => x.id === state.selected);
  if (g) setFlag(g.id, "favorite", !g.favorite);
};
// Menus sit in the game banner, which clips what sticks out: pin them to the window instead,
// opening upward when there's room above the button, else downward.
function placeMenu(menu, btn) {
  if (menu.classList.contains("hidden")) return;
  const b = btn.getBoundingClientRect();
  menu.style.position = "fixed";
  menu.style.bottom = "auto";
  menu.style.left = `${Math.max(8, Math.min(b.left, innerWidth - menu.offsetWidth - 8))}px`;
  const h = menu.offsetHeight;
  const top = b.top - h - 6 >= 8 ? b.top - h - 6 : Math.min(b.bottom + 6, innerHeight - h - 8);
  menu.style.top = `${Math.max(8, top)}px`;
}
$("#btn-more").onclick = () => { $("#menu").classList.toggle("hidden"); placeMenu($("#menu"), $("#btn-more")); };
// Keep a pinned menu next to its button while the page scrolls or the window resizes.
const replaceMenus = () => { placeMenu($("#menu"), $("#btn-more")); placeMenu($("#play-menu"), $("#btn-play")); };
document.addEventListener("scroll", replaceMenus, true);
addEventListener("resize", replaceMenus);
$("#btn-refresh").onclick = async () => {
  toast("Rescanning libraries…");
  try { await invoke("store_refresh"); } catch (e) { toast(String(e), true); }
  load();
};
$("#f-genre").onchange = (e) => { state.genre = e.target.value; state.letter = ""; renderGrid(); };
$("#f-series").onchange = (e) => { state.series = e.target.value; state.letter = ""; renderGrid(); };
$("#f-compat").onchange = (e) => { state.compatF = e.target.value; state.letter = ""; renderGrid(); };
$("#f-ach").onchange = (e) => { state.achF = e.target.value; state.letter = ""; renderGrid(); };
$("#f-region").onchange = (e) => { state.regionF = e.target.value; state.letter = ""; renderGrid(); };
$("#f-played").onchange = (e) => { state.playedF = e.target.value; state.letter = ""; renderGrid(); };
$("#f-clear").onclick = () => { state.genre = state.series = state.compatF = state.letter = state.achF = state.regionF = state.playedF = ""; renderGrid(); };
let searchTimer;
// Close the search: clear it, and go back to where you searched from (Beacon, say).
function closeSearch() {
  $("#search").value = "";
  state.search = "";
  $("#search").blur();
  $("#search-close").classList.add("hidden");
  if (state.searchFrom) { state.view = state.searchFrom; ls.set("view", state.view); state.searchFrom = null; }
  render();
}
$("#search-close").onclick = closeSearch;
$("#search").oninput = (e) => {
  state.search = e.target.value;
  $("#search-close").classList.toggle("hidden", !state.search && !state.searchFrom);
  // Searching from a page without the game grid (Beacon, Plug-ins…) shows the results in the library;
  // closing the search comes back here.
  if (state.search && ["beacon", "plugins", "downloads", "home"].includes(state.view)) {
    state.searchFrom = state.view;
    state.view = "all"; ls.set("view", "all"); render();
  }
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => { renderGrid(); renderHero(); }, 120);
};
$("#sort").onchange = (e) => { ls.set("sort:" + state.view, e.target.value); render(); };
$("#show-uninstalled").checked = state.showUninstalled;
$("#show-uninstalled").onchange = (e) => { state.showUninstalled = e.target.checked; ls.set("showUninstalled", state.showUninstalled); render(); };
$("#minimize").checked = state.minimize;
$("#minimize").onchange = (e) => { state.minimize = e.target.checked; ls.set("minimize", state.minimize); };

// Add game dialog
$("#btn-add").onclick = () => { $("#add-err").hidden = true; $("#dlg-add").showModal(); };
$("#add-cancel").onclick = () => $("#dlg-add").close();
$("#form-add").proton.onchange = (e) => {
  $("#cmd-label").textContent = e.target.checked ? "Path to the game's .exe" : "Command";
  $("#form-add").command.placeholder = e.target.checked ? "/home/you/Games/MyGame/Game.exe" : "/home/you/Games/mygame/start.sh";
};
$("#form-add").onsubmit = async (e) => {
  e.preventDefault();
  const f = new FormData(e.target);
  const err = $("#add-err");
  err.hidden = true;
  const fail = (msg) => { err.textContent = msg; err.hidden = false; };
  const cmd = String(f.get("command") || "").trim().replace(/^"|"$/g, "");
  if (f.get("proton") === "on" && !/\.exe$/i.test(cmd)) return fail("Point this at the game's .exe file, e.g. ~/Games/MyGame/Game.exe");
  try {
    await invoke("add_custom_game", { title: f.get("title"), command: f.get("command"), store: f.get("store"), cover: f.get("cover") || null, proton: f.get("proton") === "on" });
    $("#dlg-add").close();
    e.target.reset();
    await load(true);
    toast("Game added");
  } catch (e2) {
    fail(String(e2));
  }
};

// Keyboard: arrows move, Enter plays, / searches, F favorites
document.addEventListener("keydown", (e) => {
  if (document.querySelector("dialog[open]") || !$("#onboard").classList.contains("hidden")) return;
  if (state.view === "minecraft" || state.view.startsWith("shop:")) return;
  const inSearch = document.activeElement === $("#search");
  if (e.key === "/" && !inSearch) {
    e.preventDefault();
    return $("#search").focus();
  }
  if (e.key === "Escape" && (inSearch || state.searchFrom)) return closeSearch();
  if (e.key === "Escape" && state.open && !friends.open) { e.preventDefault(); return closeDetail(); }
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
  if (WIN) {
    // Windows: the store launchers Lodestone can install with winget.
    await loadWinapps();
    const la = (id) => (state.winapps || []).find((a) => a.id === id);
    $("#ob-found").innerHTML = ["steam", "epic", "gog", "battlenet", "ea", "ubisoft"].map((id) => {
      const a = la(id);
      const c = CLIENTS[id] || { name: a ? a.name : id };
      return row(a && a.installed, c.name, a && a.installed ? `Found. ${n(a.store)} installed games.` : "Not installed. You can install it in the next step.");
    }).join("");
    return o;
  }
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
// Windows guide, step 2: each store's launcher, installed or with an Install (winget) button.
async function renderObLaunchers() {
  await loadWinapps();
  $("#ob-launchers").innerHTML = ["steam", "epic", "gog", "battlenet", "ea", "ubisoft"].map((id) => {
    const a = (state.winapps || []).find((x) => x.id === id);
    if (!a) return "";
    const c = CLIENTS[id] || { name: a.name };
    const busy = state.setupBusy === id;
    return `<div class="ob-launcher"><span class="dot" style="background:${store(a.store).color}"></span><span class="grow"><b>${esc(c.name)}</b></span>${a.installed
      ? '<span class="muted">Installed ✓</span>'
      : `<button type="button" class="play small" data-winsetup="${id}" ${busy ? "disabled" : ""}>${busy ? "Installing…" : "Install"}</button>`}</div>`;
  }).join("");
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
    if (obStep === 2) await (WIN ? renderObLaunchers() : renderProton());
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
  if (obStep === 2 && !WIN) await invoke("set_proton", { path: $("#ob-proton").value });
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

// ---------- background update checks ----------
let knownUpdates = null;
function announceUpdates() {
  const now = new Set(state.games.filter((g) => g.installed && g.update === "available").map((g) => g.id));
  if (knownUpdates) {
    const fresh = [...now].filter((id) => !knownUpdates.has(id)).map((id) => state.games.find((g) => g.id === id)?.title).filter(Boolean);
    if (fresh.length) toast(`Update${fresh.length > 1 ? "s" : ""} available: ${fresh.slice(0, 3).join(", ")}${fresh.length > 3 ? ` +${fresh.length - 3} more` : ""}`);
  }
  knownUpdates = now;
}
// Steam writes update state to disk itself, so a quiet rescan picks it up; Epic/GOG need a store check.
setInterval(() => load(true), 5 * 60 * 1000);
setInterval(() => { invoke("store_refresh").catch(() => {}).finally(() => load(true)); }, 30 * 60 * 1000);

// Version in the sidebar and Settings; clicking it opens that release's notes on GitHub.
invoke("app_version").then((v) => {
  const rel = `https://github.com/daigonstar/lodestone/releases/tag/${encodeURIComponent(v)}`;
  const a = $("#app-version");
  a.textContent = `v${v}`;
  a.dataset.href = rel;
  $("#settings-version").innerHTML = `Lodestone ${esc(v)} · <a href="#" data-href="${esc(rel)}">release notes</a> · <a href="#" data-href="https://github.com/daigonstar/lodestone">source</a>`;
}).catch(() => {});

invoke("startup_view")
  .then((v) => { if (v) { state.view = v; ls.set("view", v); } })
  .catch(() => {})
  .finally(async () => {
    await load(true);
    try { if (!(await invoke("setup_overview")).done) obOpen(); } catch {}
  });

window.L = { invoke, state, toast, esc, fmtSize, fmtTime, imgSrc, play, $ };


// ---------- layouts ----------
$("#layout").addEventListener("click", (e) => {
  const b = e.target.closest("[data-layout]");
  if (!b) return;
  state.layout = b.dataset.layout;
  ls.set("layout", state.layout);
  renderGrid();
});

// ---------- details panel (below the hero) ----------
const detailsCache = new Map();
const gameNewsCache = new Map();
function fmtDate(ts) { return ts ? new Date(ts * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" }) : ""; }
function newsHTML(n, withGame) {
  const g = withGame && state.games.find((x) => x.id === n.game);
  const src = g && imgSrc(g.cover, 120);
  return `<div class="news" data-page="${esc(n.url)}" data-page-title="${esc(n.title)}" title="Open in Lodestone">
    ${withGame ? `<div class="news-art" style="${g ? placeholder(g) : ""}">${src ? `<img loading="lazy" src="${esc(src)}" alt="" data-err="remove">` : ""}</div>` : ""}
    <div class="news-body">
      <div class="news-meta">${g ? `<b>${esc(g.title)}</b> · ` : ""}${fmtDate(n.date)}${n.patch ? ' <span class="patch">Patch notes</span>' : ""}</div>
      <div class="news-title">${esc(n.title)}</div>
      ${n.summary ? `<div class="news-sum">${esc(n.summary)}</div>` : ""}
    </div></div>`;
}
// Achievements for the game in the details panel: RetroAchievements for ROMs.
const achCache = new Map();
/** A store game's achievements (Steam): icons, your unlocks and how rare each one is. */
async function renderStoreAchievements(g) {
  const wrap = $("#about-ach-wrap"), el = $("#about-ach");
  const id = g.id;
  const show = (info) => {
    if (state.selected !== id) return;
    if (!info || !info.achievements.length) return wrap.classList.add("hidden");
    wrap.classList.remove("hidden");
    const total = info.achievements.length;
    const pct = Math.round((info.earned / total) * 100);
    $("#about-ach-title").innerHTML = `Achievements <small class="muted">· ${esc(info.source)}</small>`;
    // Earned first (newest first), then the rest from most to least common.
    const list = [...info.achievements].sort((a, b) => (b.earned_at - a.earned_at) || ((b.percent ?? 0) - (a.percent ?? 0)));
    const badge = (a) => {
      const got = a.earned_at > 0;
      const secret = a.hidden && !got;
      const rare = a.percent != null ? `\n${a.percent < 1 ? a.percent.toFixed(1) : Math.round(a.percent)}% of players have it` : "";
      const tip = secret ? `Hidden achievement${rare}` : `${a.title}\n${a.description || ""}${got ? `\nEarned ${new Date(a.earned_at * 1000).toLocaleDateString()}` : ""}${rare}`;
      const src = got ? a.icon : (a.icon_locked || a.icon);
      return `<img class="ach-badge ${got ? "got" : ""} ${a.percent != null && a.percent < 5 ? "rare" : ""}" loading="lazy" alt="" title="${esc(tip)}" src="${esc(src)}" data-err="hide">`;
    };
    el.innerHTML = `<div class="ach-sum"><div class="bar"><span style="width:${Math.max(2, pct)}%"></span></div>
        <span><b>${info.earned}</b> of ${total} earned</span>
        <button class="ghost small" data-page="${esc(info.url)}" data-page-title="${esc(g.title)}">On ${esc(info.source)}</button></div>
      <div class="ach-grid">${list.map(badge).join("")}</div>`;
  };
  if (achCache.has(id)) return show(achCache.get(id));
  wrap.classList.remove("hidden");
  $("#about-ach-title").textContent = "Achievements";
  el.innerHTML = '<span class="muted">Loading achievements…</span>';
  try {
    const info = await invoke("store_achievements", { id });
    achCache.set(id, info);
    show(info);
  } catch (e) {
    if (state.selected !== id) return;
    el.innerHTML = String(e) === "need-key"
      ? `<span class="muted">Add your Steam Web API key in <a href="#" data-click="btn-settings">Settings</a> to see this game's achievements.</span>`
      : `<span class="err-text">${esc(String(e))}</span>`;
  }
}

async function renderAchievements(g) {
  const wrap = $("#about-ach-wrap"), el = $("#about-ach");
  if (!g.system && g.store === "steam") return renderStoreAchievements(g);
  if (!g.system) return wrap.classList.add("hidden");
  wrap.classList.remove("hidden");
  const id = g.id;
  const show = (info) => {
    if (state.selected !== id) return;
    if (info === "signin") {
      $("#about-ach-title").textContent = "Achievements";
      el.innerHTML = `<span class="muted">Add your RetroAchievements username and Web API key in <a href="#" data-click="btn-settings">Settings</a> to see this game's achievements.</span>`;
      return;
    }
    if (!info) {
      $("#about-ach-title").textContent = "Achievements";
      el.innerHTML = `<span class="muted">RetroAchievements doesn't know this ROM (or this version of it) yet.</span>`;
      return;
    }
    const total = info.achievements.length;
    $("#about-ach-title").innerHTML = `Achievements <small class="muted">· RetroAchievements</small>`;
    if (!total) { el.innerHTML = `<span class="muted">${esc(info.title)} has no achievements on RetroAchievements yet.</span>`; return; }
    const pct = Math.round((info.earned / total) * 100);
    const badge = (a) => {
      const got = a.earned || a.earned_hardcore;
      const when = a.earned_hardcore || a.earned;
      const tip = `${a.title} (${a.points} pts)\n${a.description}${got ? `\nEarned ${when}${a.earned_hardcore ? " · hardcore" : ""}` : ""}`;
      return `<img class="ach-badge ${got ? "got" : ""} ${a.earned_hardcore ? "hc" : ""}" loading="lazy" alt="" title="${esc(tip)}" src="https://media.retroachievements.org/Badge/${encodeURIComponent(a.badge)}${got ? "" : "_lock"}.png" data-err="hide">`;
    };
    el.innerHTML = `<div class="ach-sum"><div class="bar"><span style="width:${Math.max(2, pct)}%"></span></div>
        <span><b>${info.earned}</b> of ${total} earned · ${info.points_earned} / ${info.points_total} points${info.earned_hardcore ? ` · ${info.earned_hardcore} hardcore` : ""}</span>
        <button class="ghost small" data-page="${esc(info.url)}" data-page-title="${esc(info.title)}">On RetroAchievements</button></div>
      <div class="ach-grid">${info.achievements.map(badge).join("")}</div>`;
  };
  if (achCache.has(id)) return show(achCache.get(id));
  el.innerHTML = '<span class="muted">Loading achievements…</span>';
  try {
    const st = await invoke("get_settings");
    if (!(st.ra_user && st.ra_key)) return show("signin");
    const info = await invoke("ra_game", { id });
    achCache.set(id, info);
    show(info);
  } catch (e) {
    if (state.selected === id) el.innerHTML = `<span class="err-text">${esc(String(e))}</span>`;
  }
}

async function renderAbout(g) {
  const el = $("#about");
  if (!g || ["home", "minecraft", "downloads", "beacon", "plugins"].includes(state.view) || state.view.startsWith("shop:")) return el.classList.add("hidden");
  el.classList.remove("hidden");
  const id = g.id;
  const facts = (d) => {
    const rows = [];
    if (d && d.developers && d.developers.length) rows.push(["Developer", d.developers.join(", ")]);
    if (d && d.publishers && d.publishers.length) rows.push(["Publisher", d.publishers.join(", ")]);
    if (d && d.release) rows.push(["Released", d.release]);
    if (d && d.metacritic) rows.push(["Metacritic", `<span class="mc-score">${d.metacritic}</span>`]);
    rows.push(["Played", fmtTime(g.playtime)]);
    if (g.size) rows.push(["Size", fmtSize(g.size)]);
    const copies = (state.copies.get(titleKey(g.title)) || [g]).map((c) => store(c.store).label);
    rows.push(["Owned on", [...new Set(copies)].join(", ")]);
    return rows.map(([k, v]) => `<div><small>${k}</small><span>${k === "Metacritic" ? v : esc(String(v))}</span></div>`).join("");
  };
  const fill = (d) => {
    if (state.selected !== id) return;
    $("#about-desc").textContent = d && d.description ? d.description : "No description found for this game.";
    $("#about-facts").innerHTML = facts(d);
    $("#about-features").innerHTML = ((d && d.features) || []).map((f) => `<span class="chip">${esc(f)}</span>`).join("");
    $("#about-shots").innerHTML = ((d && d.screenshots) || []).map(([t, f]) => `<button class="shot" data-shot="${esc(f)}"><img loading="lazy" src="${esc(t)}" alt=""></button>`).join("");
    const links = [];
    if (d && d.store_url) links.push(`<button class="ghost small" data-page="${esc(d.store_url)}" data-page-title="${esc(g.title)}">${d.source === "Wikipedia" ? "Wikipedia" : "Store page"}</button>`);
    if (d && d.website) links.push(`<button class="ghost small" data-href="${esc(d.website)}">Website ↗</button>`);
    links.push(`<button class="ghost small" data-edit-meta="${esc(g.id)}">✎ Edit details</button>`);
    $("#about-links").innerHTML = links.join("");
    if (d && !d.description) $("#about-desc").innerHTML = `No details found. <a href="#" data-edit-meta="${esc(g.id)}">Edit details</a> to link it to the matching Steam game.`;
    else if (d && d.source && d.source !== "Steam") $("#about-desc").insertAdjacentHTML("beforeend", ` <small class="muted">(from ${esc(d.source)})</small>`);
  };
  const cached = detailsCache.get(id);
  fill(cached || null);
  renderCompat(g, cached);
  if (!cached) {
    $("#about-desc").innerHTML = '<span class="muted">Loading details…</span>';
    invoke("game_details", { id }).then((d) => { detailsCache.set(id, d); fill(d); renderCompat(g, d); }).catch(() => fill({}));
  }
  renderOwners(g);
  renderAchievements(g);
  const nc = gameNewsCache.get(id);
  const showNews = (list) => { if (state.selected === id) $("#about-news").innerHTML = list.length ? list.slice(0, 3).map((n) => newsHTML(n, false)).join("") : '<span class="muted">No recent news.</span>'; };
  if (nc) showNews(nc);
  else {
    $("#about-news").innerHTML = '<span class="muted">Loading…</span>';
    invoke("game_news", { ids: [id] }).then((n) => { gameNewsCache.set(id, n); showNews(n); }).catch(() => showNews([]));
  }
}
// ProtonDB rating (how well it runs through Proton), plus a Native badge for Linux versions.
const protonCache = new Map();
const TIERS = { platinum: "Platinum", gold: "Gold", silver: "Silver", bronze: "Bronze", borked: "Borked", pending: "Pending" };
// ProtonDB ratings for the whole library: id -> "native" | tier.
state.compat = new Map();
let compatBusy = false;
async function loadCompat() {
  // ProtonDB is about running Windows games on Linux: not asked on Windows.
  if (compatBusy || WIN) return;
  const ids = state.games.filter((g) => !["minecraft", "custom"].includes(g.store) && !state.compat.has(g.id)).map((g) => g.id);
  if (!ids.length) return;
  compatBusy = true;
  try {
    for (const c of await invoke("games_compat", { ids })) {
      state.compat.set(c.id, c.tier);
      if (c.protondb && !protonCache.has(c.id)) protonCache.set(c.id, c.protondb);
    }
    // Remember checked games without a rating so they aren't asked for again this session.
    ids.forEach((id) => { if (!state.compat.has(id)) state.compat.set(id, ""); });
    render();
  } catch {} finally { compatBusy = false; }
}
function compatOf(g) {
  const copies = mergeOn() ? (state.copies.get(titleKey(g.title)) || []) : [];
  for (const c of [g, ...copies]) { const t = state.compat.get(c.id); if (t) return t; }
  return "";
}
function compatBadge(g, cls = "") {
  if (WIN) return "";
  const t = compatOf(g);
  if (!t || t === "pending") return "";
  const label = t === "native" ? "Native" : TIERS[t] || t;
  const tip = t === "native" ? "Has a Linux version" : `ProtonDB: ${label}`;
  return `<span class="pdb ${cls} tier-${esc(t)}" title="${esc(tip)}">${esc(label)}</span>`;
}

function renderCompat(g, d) {
  const el = $("#about-compat");
  if (WIN) { el.innerHTML = ""; return; }
  const id = g.id;
  const draw = (p) => {
    if (state.selected !== id) return;
    const parts = [];
    if (d && d.linux) parts.push('<span class="compat-badge tier-native" title="This game has a Linux version">Native Linux</span>');
    if (p && p.tier) {
      const t = TIERS[p.tier] || p.tier;
      const trend = p.trending && p.trending !== p.tier ? ` · trending ${TIERS[p.trending] || p.trending}` : "";
      parts.push(`<button class="compat-badge tier-${esc(p.tier)}" data-page="${esc(p.url)}" data-page-title="ProtonDB · ${esc(g.title)}" title="ProtonDB community rating for running it through Proton. Click for reports and tweaks.">
        <span class="compat-src">ProtonDB</span><b>${esc(t)}</b></button><small class="muted">${p.reports} report${p.reports === 1 ? "" : "s"}${esc(trend)}</small>`);
    } else if (p === null && !(d && d.linux) && d && d.steam_appid) {
      parts.push('<small class="muted">No ProtonDB reports yet</small>');
    }
    el.innerHTML = parts.join("");
  };
  if (protonCache.has(id)) return draw(protonCache.get(id));
  el.innerHTML = "";
  invoke("game_protondb", { id }).then((p) => { protonCache.set(id, p); draw(p); }).catch(() => draw(undefined));
}

document.addEventListener("click", (e) => {
  const sh = e.target.closest("[data-shot]");
  if (sh) { $("#shot-img").src = sh.dataset.shot; $("#dlg-shot").showModal(); return; }
  const pg = e.target.closest("[data-page]");
  if (pg) { invoke("open_page", { url: pg.dataset.page, title: pg.dataset.pageTitle || "Lodestone" }).catch((err) => toast(String(err), true)); return; }
  const ed = e.target.closest("[data-edit-meta]");
  if (ed) { e.preventDefault(); const g = state.games.find((x) => x.id === ed.dataset.editMeta); if (g) openMeta(g); return; }
  const op = e.target.closest("[data-open-game]");
  if (op) { state.view = "all"; ls.set("view", "all"); state.selected = op.dataset.openGame; ls.set("selected", state.selected); state.open = false; render(); openDetail(op.dataset.openGame); return; }
});
$("#dlg-shot").addEventListener("click", (e) => { if (e.target.id === "dlg-shot" || e.target.id === "shot-img") $("#dlg-shot").close(); });

// ---------- home dashboard ----------
let homeNews = null, homeNewsAt = 0, newsFilter = "all", artChecked = false;
function homeGames() { return state.games.filter((g) => !g.hidden); }
async function renderHome() {
  const vis = homeGames();
  const inst = vis.filter((g) => g.installed);
  const played = vis.reduce((a, g) => a + (g.playtime || 0), 0);
  const updates = inst.filter((g) => g.update).sort((a, b) => (a.update === "updating" ? -1 : 0) - (b.update === "updating" ? -1 : 0) || a.title.localeCompare(b.title));
  const owned = mergeList(vis).length;
  const stat = (n, l) => `<div class="stat"><b>${n}</b><small>${l}</small></div>`;
  $("#home-stats").innerHTML = stat(inst.length, "installed") + stat(owned, "games owned") + stat(new Set(vis.map((g) => g.store)).size, "stores") + stat(Math.round(played / 3600) + "h", "played in Lodestone") + stat(updates.length, updates.length === 1 ? "update" : "updates");

  const patchFor = (g) => (homeNews || []).find((n) => n.game === g.id && n.patch);
  $("#home-updates").innerHTML = updates.length ? updates.map((g) => {
    const src = imgSrc(g.cover, 120);
    const p = patchFor(g);
    return `<div class="upd-row">
      <div class="row-art" style="${placeholder(g)}">${src ? `<img src="${esc(src)}" alt="" data-err="remove">` : ""}</div>
      <div class="grow"><div class="title" data-open-game="${esc(g.id)}">${esc(g.title)}</div>
        <div class="sub"><span class="dot" style="background:${store(g.store).color}"></span>${store(g.store).label} · ${g.update === "updating" ? "updating now" : `update ready${g.update_size ? ` · ${fmtSize(g.update_size)}` : ""}`}</div>
        ${p ? `<a href="#" class="patch-link" data-page="${esc(p.url)}" data-page-title="${esc(p.title)}">${esc(p.title)}</a>` : ""}</div>
      ${g.update === "available" ? `<button class="ghost small" data-home-update="${esc(g.id)}">Update</button>` : '<span class="spinner"></span>'}
    </div>`;
  }).join("") : '<div class="muted pad-s">Everything is up to date.</div>';
  $("#home-update-all").classList.toggle("hidden", updates.filter((g) => g.update === "available").length < 2);

  const recent = vis.filter((g) => g.last_played).sort((a, b) => b.last_played - a.last_played).slice(0, 8);
  $("#home-recent").innerHTML = recent.length ? recent.map((g) => {
    const src = imgSrc(g.cover, 220);
    return `<button class="recent" data-open-game="${esc(g.id)}" title="${esc(g.title)}"><div class="art" style="${placeholder(g)}">${src ? `<img src="${esc(src)}" alt="" data-err="remove">` : ""}</div>
      <div class="title">${esc(g.title)}</div><div class="sub">${fmtAgo(g.last_played)}</div></button>`;
  }).join("") : '<div class="muted pad-s">Games you play from Lodestone show up here.</div>';

  const missing = vis.filter((g) => g.art_missing && (g.installed || state.showUninstalled));
  const missingAll = vis.filter((g) => g.art_missing);
  $("#home-art").innerHTML = missingAll.length
    ? `<p>${missingAll.length} game${missingAll.length === 1 ? " has" : "s have"} no cover art${missing.length !== missingAll.length ? ` (${missing.length} installed)` : ""}.</p>
       <div class="row-btns"><button class="play small" id="art-fix">Find artwork</button><button class="ghost small" data-view="missing">Show them</button></div>`
    : `<p class="muted">${artChecked ? "Every game has cover art." : "Checking covers…"}</p>`;

  renderHomeFriends();
  if (!artChecked) {
    artChecked = true;
    invoke("art_check").then(() => load(true)).catch(() => {});
  }
  if (!homeNews || Date.now() - homeNewsAt > 30 * 60e3) {
    homeNewsAt = Date.now();
    // Updates and recent games first, then favorites and everything else installed (one copy per game).
    const ids = [...new Set([...updates, ...recent, ...vis.filter((g) => g.favorite && g.installed), ...mergeList(inst)].map((g) => g.id))].slice(0, 60);
    invoke("game_news", { ids }).then((n) => { homeNews = n; if (state.view === "home") renderHome(); }).catch(() => { homeNews = []; renderNews(); });
  }
  renderNews();
}
function renderNews() {
  const el = $("#home-news");
  if (!homeNews) return (el.innerHTML = '<span class="muted">Loading news from your games…</span>');
  // At most two posts per game so one busy game doesn't fill the feed.
  const per = new Map();
  const list = homeNews.filter((n) => newsFilter === "all" || n.patch).filter((n) => { const c = (per.get(n.game) || 0) + 1; per.set(n.game, c); return c <= 2; }).slice(0, 30);
  el.innerHTML = list.length ? list.map((n) => newsHTML(n, true)).join("") : '<span class="muted">No news from your games yet.</span>';
}
$("#news-filter").addEventListener("click", (e) => {
  const b = e.target.closest("[data-news]");
  if (!b) return;
  newsFilter = b.dataset.news;
  document.querySelectorAll("#news-filter button").forEach((x) => x.classList.toggle("on", x === b));
  renderNews();
});
async function homeUpdate(id) {
  const g = state.games.find((x) => x.id === id);
  try {
    const how = await invoke("update_game", { id });
    if (how === "download") { toast(`Updating ${g.title}…`); pollDownloads(); }
    else toast(`Steam's Downloads page opened for ${g.title}.`);
  } catch (e) { toast(String(e), true); }
}
$("#home").addEventListener("click", async (e) => {
  const u = e.target.closest("[data-home-update]");
  if (u) { u.disabled = true; return homeUpdate(u.dataset.homeUpdate); }
  if (e.target.id === "art-fix") {
    e.target.disabled = true;
    e.target.textContent = "Searching…";
    try {
      const [missing, fixed] = await invoke("art_fix");
      toast(fixed ? `Found covers for ${fixed} of ${missing} games` : `No covers found automatically. ${missing ? "Edit a game's details to search by hand" : ""}`);
      await load(true);
    } catch (err) { toast(String(err), true); }
  }
});
$("#home-update-all").onclick = async () => {
  for (const g of homeGames().filter((g) => g.installed && g.update === "available")) await homeUpdate(g.id);
};

// ---------- edit details & artwork ----------
let metaGame = null, metaInit = null, artKind = "cover", artHits = [];
function metaPreview() {
  const f = $("#form-meta");
  const c = f.cover.value.trim() || (metaGame && metaGame.cover) || "";
  const h = f.hero.value.trim() || (metaGame && metaGame.hero) || "";
  $("#mp-cover").style.backgroundImage = c ? `url("${imgSrc(c, 320)}")` : "";
  $("#mp-hero").style.backgroundImage = h ? `url("${imgSrc(h, 640)}")` : "";
}
let metaPt = null;
$("#pt-auto").onclick = () => {
  metaPt.auto = true;
  $("#pt-auto").classList.add("hidden");
  $("#pt-hint").textContent = "Saving goes back to counting automatically.";
};
async function openMeta(g) {
  metaGame = g;
  let o = {};
  try { o = await invoke("meta_get", { id: g.id }); } catch {}
  const f = $("#form-meta");
  f.title.value = o.title || g.title;
  f.genres.value = (o.genres || g.genres || []).join(", ");
  f.series.value = o.series != null ? o.series : (g.series || "");
  f.steam.value = o.steam_appid || "";
  f.cover.value = o.auto ? "" : (o.cover || "");
  f.hero.value = o.hero || "";
  const pt = g.playtime || 0;
  f.pt_h.value = Math.floor(pt / 3600);
  f.pt_m.value = Math.floor((pt % 3600) / 60);
  metaPt = { h: f.pt_h.value, m: f.pt_m.value, auto: false };
  $("#pt-auto").classList.toggle("hidden", !g.playtime_edited);
  $("#pt-hint").textContent = g.playtime_edited
    ? "You set this. Lodestone adds the time you play from now on. Count automatically goes back to the store's record and Lodestone's own count."
    : "Set it if the store's record is missing or wrong (Battle.net, EA and Ubisoft keep none). Lodestone adds the time you play from then on.";
  f.installdir.value = o.install_dir || "";
  f.exe.value = o.exe || "";
  $("#loc-hint").textContent = g.store === "steam"
    ? "Only if Lodestone didn't find the game. For Steam games it's better to add the drive as a library folder in Steam (Settings → Storage)."
    : g.store === "epic" || g.store === "gog"
      ? `Only if Lodestone didn't find the game. The folder is handed to ${g.store === "epic" ? "Epic" : "GOG"}'s records, so Play, updates and uninstall work as usual.`
      : "Only if Lodestone didn't find the game. It then counts as installed there; Play still goes through the store app unless you pick a program to run.";
  metaInit = { o, title: f.title.value, genres: f.genres.value, series: f.series.value, steam: f.steam.value, cover: f.cover.value, hero: f.hero.value };
  document.querySelectorAll("#form-meta .store-only").forEach((el) => el.classList.toggle("hidden", !!g.system));
  $("#meta-hint").textContent = g.system
    ? `${g.via.split(" · ")[0]} ROM. Changes stay in Lodestone; your ROM file isn't touched.`
    : `${store(g.store).label} · ${g.id}. Changes stay in Lodestone; the store's own data isn't touched.`;
  $("#art-q").value = g.title;
  metaPreview();
  $("#dlg-meta").showModal();
  artSearch();
}
async function artSearch() {
  const q = $("#art-q").value.trim();
  $("#art-results").innerHTML = '<span class="muted">Searching…</span>';
  try {
    artHits = await invoke("art_search", { query: q });
    const st = await invoke("get_settings");
    $("#art-source").textContent = st.steamgriddb_key ? "From SteamGridDB and Steam." : "From Steam's store. Add a free SteamGridDB key in Settings → Artwork for many more.";
  } catch (e) { artHits = []; toast(String(e), true); }
  renderArt();
}
function renderArt() {
  const f = $("#form-meta");
  const cur = artKind === "cover" ? f.cover.value : f.hero.value;
  const list = artHits.filter((a) => a.kind === artKind);
  $("#art-results").className = `art-results ${artKind}`;
  $("#art-results").innerHTML = list.length ? list.map((a) => `<button type="button" class="art-hit ${a.url === cur ? "on" : ""}" data-art="${esc(a.url)}" title="${esc(a.source)} · ${esc(a.name)}">
      <img loading="lazy" src="${esc(a.thumb)}" alt="" data-err="parent"><small>${esc(a.source)}</small></button>`).join("")
    : '<span class="muted">Nothing found. Try a shorter name.</span>';
}
$("#art-go").onclick = artSearch;
$("#art-q").addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); artSearch(); } });
$("#art-kind").addEventListener("click", (e) => {
  const b = e.target.closest("[data-kind]");
  if (!b) return;
  artKind = b.dataset.kind;
  document.querySelectorAll("#art-kind button").forEach((x) => x.classList.toggle("on", x === b));
  renderArt();
});
$("#art-results").addEventListener("click", (e) => {
  const b = e.target.closest("[data-art]");
  if (!b) return;
  $("#form-meta")[artKind === "cover" ? "cover" : "hero"].value = b.dataset.art;
  metaPreview();
  renderArt();
});
["cover", "hero"].forEach((n) => $("#form-meta")[n].addEventListener("input", metaPreview));
$("#meta-cancel").onclick = () => $("#dlg-meta").close();
$("#meta-reset").onclick = async () => {
  if (!confirm(`Go back to ${store(metaGame.store).label}'s own title, artwork and genres for ${metaGame.title}?`)) return;
  await invoke("meta_reset", { id: metaGame.id });
  detailsCache.delete(metaGame.id);
  $("#dlg-meta").close();
  toast("Back to the store's details");
  load(true);
};
$("#form-meta").onsubmit = async (e) => {
  e.preventDefault();
  const f = e.target, i = metaInit, o = i.o || {};
  const changed = (k) => f[k].value.trim() !== String(i[k]).trim();
  const steamNum = (f.steam.value.match(/app\/(\d+)/) || f.steam.value.match(/^\s*(\d+)\s*$/) || [])[1];
  const edit = {
    title: changed("title") || o.title ? f.title.value.trim() || null : null,
    genres: changed("genres") || o.genres ? f.genres.value.split(",").map((x) => x.trim()).filter(Boolean) : null,
    series: changed("series") || o.series != null ? f.series.value.trim() : null,
    steam_appid: steamNum ? Number(steamNum) : null,
    cover: f.cover.value.trim() || (o.auto ? o.cover : null) || null,
    hero: f.hero.value.trim() || null,
    install_dir: f.installdir.value.trim() || null,
    exe: f.exe.value.trim() || null,
  };
  const btn = f.querySelector('button[type="submit"]');
  btn.disabled = true;
  btn.textContent = "Saving…";
  try {
    const ptChanged = metaPt && (f.pt_h.value !== metaPt.h || f.pt_m.value !== metaPt.m);
    if (metaPt && metaPt.auto && !ptChanged) await invoke("set_playtime", { id: metaGame.id, seconds: null });
    else if (ptChanged) {
      const h = Math.max(0, parseInt(f.pt_h.value || "0", 10) || 0), m = Math.min(59, Math.max(0, parseInt(f.pt_m.value || "0", 10) || 0));
      await invoke("set_playtime", { id: metaGame.id, seconds: h * 3600 + m * 60 });
    }
    await invoke("meta_save", { id: metaGame.id, edit });
    detailsCache.delete(metaGame.id);
    gameNewsCache.delete(metaGame.id);
    $("#dlg-meta").close();
    toast(`Saved ${edit.title || metaGame.title}`);
    await load(true);
  } catch (err) { toast(String(err), true); }
  btn.disabled = false;
  btn.textContent = "Save";
};


// ---------- friends ----------
const friends = { list: [], notes: [], at: 0, open: false, owners: new Map() };
const STATUS_TEXT = { ingame: "In game", online: "Online", away: "Away", offline: "Offline", unknown: "" };
function avatarHTML(f, size = 34) {
  const h = hue(f.name);
  return f.avatar
    ? `<img class="av" style="width:${size}px;height:${size}px" src="${esc(f.avatar)}" alt="" data-err="hide">`
    : `<span class="av ph" style="width:${size}px;height:${size}px;background:hsl(${h} 45% 30%)">${esc((f.name[0] || "?").toUpperCase())}</span>`;
}
function inLibrary(gid) { return gid && state.games.some((g) => g.id === gid); }
function friendLine(f) {
  if (f.status === "ingame" && f.game) {
    return inLibrary(f.game_id) ? `Playing <a href="#" data-open-game="${esc(f.game_id)}">${esc(f.game)}</a>` : `Playing ${esc(f.game)}`;
  }
  if (f.status === "offline" && f.last_online) return `Last online ${fmtAgo(f.last_online)}`;
  return f.store === "epic" ? "Epic Games friend" : STATUS_TEXT[f.status];
}
function friendRow(f) {
  return `<div class="friend ${f.status}">
    <div class="av-wrap">${avatarHTML(f)}<span class="pres"></span></div>
    <div class="grow"><div class="fname">${esc(f.name)} <span class="dot" style="background:${store(f.store).color}" title="${store(f.store).label}"></span></div>
      <div class="fline">${friendLine(f)}</div></div>
    ${f.profile ? `<button class="icon sm" title="Profile" data-page="${esc(f.profile)}" data-page-title="${esc(f.name)}">↗</button>` : ""}
  </div>`;
}
async function loadFriends(force = false) {
  try {
    const r = await invoke("friends_list", { force });
    friends.list = r.friends || [];
    friends.notes = r.notes || [];
    friends.at = Date.now();
  } catch (e) { friends.notes = [String(e)]; }
  const on = friends.list.filter((f) => ["ingame", "online", "away"].includes(f.status)).length;
  const c = $("#friends-count");
  c.textContent = on;
  c.classList.toggle("hidden", !on);
  if (friends.open) renderFriends();
  if (state.view === "home") renderHomeFriends();
}
function renderFriends() {
  const q = $("#friends-q").value.trim().toLowerCase();
  const list = friends.list.filter((f) => !q || f.name.toLowerCase().includes(q));
  const groups = [
    ["In game", list.filter((f) => f.status === "ingame")],
    ["Online", list.filter((f) => f.status === "online" || f.status === "away")],
    ["Offline", list.filter((f) => f.status === "offline")],
    ["Epic Games", list.filter((f) => f.store === "epic")],
  ].filter(([, l]) => l.length);
  $("#friends-notes").innerHTML = friends.notes.map((n) => `<div class="err-text">${esc(n)}</div>`).join("")
    + (!friends.list.length && !friends.notes.length ? `<p class="muted">No friends to show. Add a Steam Web API key in Settings for Steam friends, and sign in to Epic in Accounts for Epic friends.</p>` : "");
  $("#friends-list").innerHTML = groups.map(([name, l]) => `<div class="fgroup"><div class="fgroup-h">${name} <span>${l.length}</span></div>${l.map(friendRow).join("")}</div>`).join("");
}
function toggleFriends(on = !friends.open) {
  friends.open = on;
  $("#friends").classList.toggle("hidden", !on);
  shopSuspend(on);
  if (on) {
    renderFriends();
    if (Date.now() - friends.at > 30e3) loadFriends();
    $("#friends-q").focus();
  }
}
$("#btn-friends").onclick = () => toggleFriends();
$("#friends-close").onclick = () => toggleFriends(false);
$("#friends-refresh").onclick = () => loadFriends(true);
$("#friends-q").addEventListener("input", renderFriends);
$("#home-friends-all").onclick = () => toggleFriends(true);
document.addEventListener("keydown", (e) => { if (e.key === "Escape" && friends.open && !document.querySelector("dialog[open]")) toggleFriends(false); });
// Opening a friend's game from the panel: close it so the details are visible.
$("#friends").addEventListener("click", (e) => { if (e.target.closest("[data-open-game]")) toggleFriends(false); });

function renderHomeFriends() {
  const el = $("#home-friends");
  if (!el) return;
  if (!friends.at) return (el.innerHTML = '<span class="muted">Loading…</span>');
  const playing = friends.list.filter((f) => f.status === "ingame");
  const online = friends.list.filter((f) => f.status === "online" || f.status === "away");
  if (!friends.list.length) {
    el.innerHTML = `<p class="muted">${friends.notes.length ? esc(friends.notes[0]) : "Add a Steam Web API key in Settings to see your Steam friends here."}</p>`;
    return;
  }
  el.innerHTML = (playing.length ? playing.slice(0, 6).map(friendRow).join("") : "")
    + (online.length ? `<div class="avatars">${online.slice(0, 12).map((f) => `<span title="${esc(f.name)} · ${STATUS_TEXT[f.status]}">${avatarHTML(f, 30)}</span>`).join("")}</div>` : "")
    + `<p class="muted small-p">${playing.length} playing · ${online.length} online · ${friends.list.length} friends</p>`;
}

async function renderOwners(g) {
  const wrap = $("#about-friends-wrap");
  const id = g.id;
  const show = (list) => {
    if (state.selected !== id) return;
    wrap.classList.toggle("hidden", !list.length);
    if (!list.length) return;
    const now = list.filter((o) => o.friend.game_id === id || (o.friend.status === "ingame" && o.friend.game && titleKey(o.friend.game) === titleKey(g.title)));
    $("#about-friends-title").textContent = now.length ? `Friends playing now: ${now.length}` : `Friends who own this: ${list.length}`;
    $("#about-friends").innerHTML = list.slice(0, 8).map((o) => `<div class="owner ${o.friend.status}" title="${esc(o.friend.name)}">
        <div class="av-wrap">${avatarHTML(o.friend, 28)}<span class="pres"></span></div>
        <span class="grow">${esc(o.friend.name)}</span><small class="muted">${o.hours >= 1 ? `${Math.round(o.hours)} h` : o.hours > 0 ? "<1 h" : "not played"}</small></div>`).join("")
      + (list.length > 8 ? `<small class="muted">+${list.length - 8} more</small>` : "");
  };
  const c = friends.owners.get(id);
  if (c) return show(c);
  wrap.classList.add("hidden");
  try {
    const list = await invoke("friends_owning", { id });
    friends.owners.set(id, list);
    show(list);
  } catch { show([]); }
}

loadFriends();
setInterval(() => loadFriends(), 90e3);

// Clicking outside these dialogs (on the dimmed backdrop) closes them, like Cancel/Close.
// Add game and Edit details stay open so a stray click doesn't lose what you typed.
for (const id of ["dlg-settings", "dlg-accounts", "dlg-log", "dlg-shot", "dlg-workshop"]) {
  const d = document.getElementById(id);
  if (!d) continue;
  let downOutside = false;
  const outside = (e) => {
    const r = d.getBoundingClientRect();
    return e.target === d && (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom);
  };
  // Both press and release outside: dragging a text selection out of the dialog doesn't close it.
  d.addEventListener("mousedown", (e) => { downOutside = outside(e); });
  d.addEventListener("click", (e) => { if (downOutside && outside(e)) d.close(); downOutside = false; });
}

// The sidebar folds away for more room (remembered).
function setSidebar(collapsed) {
  document.body.classList.toggle("side-collapsed", collapsed);
  ls.set("sideCollapsed", collapsed);
}
$("#side-collapse").onclick = () => setSidebar(true);
$("#side-expand").onclick = () => setSidebar(false);
setSidebar(!!ls.get("sideCollapsed", false));

// ---------- updates for the programs Lodestone installs ----------
// Only when you press "Check for updates" (Settings → Programs). The result also shows as Update
// buttons on store and emulator pages until Lodestone closes.
state.updates = [];
function updateFor(name) {
  return (state.updates || []).find((u) => u.name === name);
}
function renderUpdates() {
  const el = $("#prog-list");
  if (!el) return;
  const list = state.updates || [];
  el.innerHTML = state.updatesChecked
    ? list.length
      ? list.map((u) => `<div class="prog-row"><span class="grow"><b>${esc(u.name)}</b> <small class="muted">${u.have ? esc(u.have) + " → " : ""}${esc(u.latest || "newer version")}</small></span><button type="button" class="ghost small" data-prog-update="${esc(u.id)}">Update</button></div>`).join("")
        + (list.length > 1 ? '<div class="prog-row"><span class="grow"></span><button type="button" class="play small" id="prog-all">Update all</button></div>' : "")
      : '<p class="muted small">Everything Lodestone installed is up to date.</p>'
    : "";
}
async function checkUpdates() {
  const b = $("#prog-check");
  b.disabled = true;
  b.textContent = "Checking…";
  try { state.updates = await invoke("program_updates"); state.updatesChecked = true; }
  catch (e) { toast(String(e), true); }
  b.disabled = false;
  b.textContent = "Check for updates";
  renderUpdates();
  render();
}
async function updateProgram(id) {
  const u = (state.updates || []).find((x) => x.id === id);
  try {
    await invoke("program_update", { id });
    state.updates = state.updates.filter((x) => x.id !== id);
    toast(`Updating ${u ? u.name : id} in the background (see Downloads).`);
  } catch (e) { toast(String(e), true); }
  renderUpdates();
  render();
}
$("#prog-check").onclick = checkUpdates;
document.addEventListener("click", (e) => {
  const b = e.target.closest("[data-prog-update]");
  if (b) { b.disabled = true; return updateProgram(b.dataset.progUpdate); }
  if (e.target.closest("#prog-all")) for (const u of [...(state.updates || [])]) updateProgram(u.id);
});
