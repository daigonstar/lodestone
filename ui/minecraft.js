// Minecraft hub: per-instance mods / resource packs / shaders, installed + browse (Modrinth, CurseForge).
(() => {
  const { invoke, toast, esc, fmtSize, $ } = window.L;
  const ls = {
    get: (k, d) => { try { const v = localStorage.getItem(k); return v === null ? d : JSON.parse(v); } catch { return d; } },
    set: (k, v) => { try { localStorage.setItem(k, JSON.stringify(v)); } catch {} },
  };
  const LOADERS = { neoforge: "NeoForge", forge: "Forge", fabric: "Fabric", quilt: "Quilt", vanilla: "Vanilla" };
  const KIND_LABEL = { mod: "mods", resourcepack: "resource packs", shader: "shaders" };

  const mc = {
    instances: null,
    current: ls.get("mc.instance", null),
    kind: "mod",
    tab: "installed",
    source: ls.get("mc.source", "modrinth"),
    query: "",
    filter: "",
    items: [],
    itemsKey: null,
    checked: false,
    loading: false,
    results: [],
    offset: 0,
    busy: new Set(),
    hasCfKey: null,
  };

  const inst = () => (mc.instances || []).find((i) => i.id === mc.current);
  const fmtNum = (n) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : String(n));
  const iconHTML = (src, title) =>
    src
      ? `<img class="mc-ico" loading="lazy" src="${esc(src)}" alt="" data-err="mcico" data-letter="${esc((title || "?")[0])}">`
      : `<div class="mc-ico ph">${esc((title || "?")[0])}</div>`;

  // ---------- header ----------
  function renderHead() {
    const i = inst();
    if (!i) {
      $("#mc-name").textContent = "No Minecraft instances found";
      $("#mc-chips").innerHTML = '<span class="muted">Create an instance in ATLauncher or Prism Launcher, then Rescan.</span>';
      return;
    }
    const sel = $("#mc-instance");
    sel.classList.toggle("hidden", mc.instances.length < 2);
    sel.innerHTML = mc.instances.map((x) => `<option value="${esc(x.id)}" ${x.id === i.id ? "selected" : ""}>${esc(x.name)}</option>`).join("");
    $("#mc-name").textContent = i.name;
    $("#mc-chips").innerHTML = [
      `Minecraft ${esc(i.mc_version || "?")}`,
      `${LOADERS[i.loader] || esc(i.loader)} ${esc(i.loader_version)}`,
      esc(i.launcher),
    ].map((c) => `<span class="chip">${c}</span>`).join("");
    const game = window.L.state.games.find((g) => g.id === i.id);
    const icon = game && window.L.imgSrc(game.cover);
    $(".mc-icon").style.backgroundImage = icon ? `url("${icon}")` : "";
    const run = window.L.state.running.get(i.id);
    const btn = $("#mc-play");
    btn.disabled = !!run;
    btn.classList.toggle("running", !!run);
    btn.textContent = run ? (run.state === "launching" ? "Launching…" : "● Running") : "▶ Play";
    $("#mc-query").placeholder = `Search ${mc.source === "modrinth" ? "Modrinth" : "CurseForge"} ${KIND_LABEL[mc.kind]} for ${i.mc_version} ${mc.kind === "mod" ? LOADERS[i.loader] || "" : ""}…`;
  }

  function syncSeg() {
    for (const [sel, key, val] of [["#mc-kind", "kind", mc.kind], ["#mc-tab", "tab", mc.tab], ["#mc-source", "source", mc.source]]) {
      document.querySelectorAll(`${sel} button`).forEach((b) => b.classList.toggle("on", b.dataset[key] === val));
    }
    $("#mc-installed").classList.toggle("hidden", mc.tab !== "installed");
    $("#mc-browse").classList.toggle("hidden", mc.tab !== "browse");
  }

  // ---------- installed ----------
  async function loadInstalled(updates = false) {
    const i = inst();
    if (!i) return;
    const key = `${i.id}|${mc.kind}`;
    mc.loading = true;
    if (mc.itemsKey !== key) { mc.items = []; mc.checked = false; }
    renderInstalled();
    try {
      const r = await invoke("mc_installed", { instance: i.id, kind: mc.kind, updates });
      if (`${inst()?.id}|${mc.kind}` !== key) return; // switched away meanwhile
      mc.items = r.items;
      mc.itemsKey = key;
      mc.checked = updates;
      if (r.update_error) toast(`Couldn't check updates: ${r.update_error}`, true);
    } catch (e) {
      toast(String(e), true);
    } finally {
      mc.loading = false;
      renderInstalled();
      if (mc.tab === "browse") renderResults();
    }
  }

  function renderInstalled() {
    const f = mc.filter.toLowerCase();
    const list = mc.items.filter((it) => !f || it.title.toLowerCase().includes(f) || it.file.toLowerCase().includes(f));
    const ups = mc.items.filter((it) => it.update);
    const off = mc.items.filter((it) => !it.enabled).length;
    $("#mc-summary").textContent = mc.loading && !mc.items.length
      ? "Reading files…"
      : `${mc.items.length} ${KIND_LABEL[mc.kind]}${off ? ` · ${off} disabled` : ""}${mc.checked ? ` · ${ups.length ? `${ups.length} update${ups.length === 1 ? "" : "s"}` : "all up to date"}` : ""}`;
    const all = $("#mc-update-all");
    all.classList.toggle("hidden", !ups.length);
    if (!mc.busy.has("all")) all.textContent = `Update all (${ups.length})`;
    $("#mc-check").textContent = mc.loading && mc.items.length ? "Checking…" : "Check for updates";
    $("#mc-list").innerHTML = list.length
      ? list.map((it) => {
          const busy = mc.busy.has(it.file);
          return `<div class="mc-row ${it.enabled ? "" : "off"}">
            ${iconHTML(it.icon, it.title)}
            <div class="grow">
              <div class="t">${esc(it.title)} ${it.url ? `<a href="#" data-href="${esc(it.url)}" class="ext" title="Open page">↗</a>` : ""}</div>
              <small class="muted">${it.version ? esc(it.version) + " · " : ""}${esc(it.file)} · ${fmtSize(it.size) || "<1 MB"}</small>
            </div>
            ${it.update ? `<button class="ghost upd" data-update="${esc(it.file)}" ${busy ? "disabled" : ""}>${busy ? "Updating…" : "↑ " + esc(it.update)}</button>` : ""}
            <label class="switch" title="${it.enabled ? "Disable" : "Enable"}"><input type="checkbox" data-toggle="${esc(it.file)}" ${it.enabled ? "checked" : ""}><span></span></label>
            <button class="icon sm" data-remove="${esc(it.file)}" title="Move to trash">🗑</button>
          </div>`;
        }).join("")
      : `<div class="muted pad">${mc.loading ? "Loading…" : f ? "No matches." : `No ${KIND_LABEL[mc.kind]} installed. Find some under Browse.`}</div>`;
  }

  // ---------- browse ----------
  let searchSeq = 0;
  async function search(append = false) {
    const i = inst();
    if (!i) return;
    if (mc.source === "curseforge" && !mc.hasCfKey) return renderResults();
    const seq = ++searchSeq;
    if (!append) { mc.offset = 0; mc.results = []; $("#mc-results").innerHTML = '<div class="muted pad">Searching…</div>'; }
    try {
      const hits = await invoke("mc_search", { source: mc.source, instance: i.id, kind: mc.kind, query: mc.query, offset: mc.offset });
      if (seq !== searchSeq) return;
      mc.results = append ? mc.results.concat(hits) : hits;
      mc.offset += hits.length;
      $("#mc-more").classList.toggle("hidden", hits.length < 24);
      renderResults();
    } catch (e) {
      if (seq === searchSeq) $("#mc-results").innerHTML = `<div class="pad err-text">${esc(String(e))}</div>`;
    }
  }

  function isInstalled(hit) {
    if (hit.source === "modrinth") return mc.items.some((it) => it.project_id === hit.id);
    const t = hit.title.toLowerCase();
    return mc.items.some((it) => it.title.toLowerCase() === t);
  }

  function renderResults() {
    if (mc.source === "curseforge" && !mc.hasCfKey) {
      $("#mc-more").classList.add("hidden");
      $("#mc-results").innerHTML = `<div class="pad card-note">
        <b>CurseForge needs an API key.</b><br>
        <span class="muted">It's free: sign in at <a href="#" data-href="https://console.curseforge.com/">console.curseforge.com</a>, create a key, and paste it in Settings. Modrinth works without one.</span><br><br>
        <button class="play small" data-click="btn-settings">Open Settings</button></div>`;
      return;
    }
    if (!mc.results.length) {
      $("#mc-results").innerHTML = '<div class="muted pad">No results.</div>';
      return;
    }
    $("#mc-results").innerHTML = mc.results.map((h, idx) => {
      const have = isInstalled(h);
      const busy = mc.busy.has(`${h.source}:${h.id}`);
      return `<div class="mc-row">
        ${iconHTML(h.icon, h.title)}
        <div class="grow">
          <div class="t">${esc(h.title)} ${h.url ? `<a href="#" data-href="${esc(h.url)}" class="ext" title="Open page">↗</a>` : ""}
            <span class="muted small">by ${esc(h.author || "unknown")} · ${fmtNum(h.downloads)} downloads</span></div>
          <small class="muted clamp">${esc(h.description)}</small>
        </div>
        <button class="${have ? "ghost" : "play small"}" data-install="${idx}" ${have || busy ? "disabled" : ""}>${busy ? "Installing…" : have ? "Installed" : "Install"}</button>
      </div>`;
    }).join("");
  }

  // ---------- actions ----------
  async function install(hit) {
    const i = inst();
    const key = `${hit.source}:${hit.id}`;
    mc.busy.add(key);
    renderResults();
    try {
      const added = await invoke("mc_install", { source: hit.source, instance: i.id, kind: mc.kind, project: hit.id });
      const deps = added.length - 1;
      toast(`Installed ${hit.title}${deps > 0 ? ` + ${deps} dependenc${deps === 1 ? "y" : "ies"} (${added.slice(1).join(", ")})` : ""}`);
    } catch (e) {
      toast(String(e), true);
    } finally {
      mc.busy.delete(key);
      await loadInstalled(false);
    }
  }

  async function update(file) {
    const i = inst();
    mc.busy.add(file);
    renderInstalled();
    try {
      const title = await invoke("mc_update", { instance: i.id, kind: mc.kind, file });
      toast(`Updated ${title}`);
      return true;
    } catch (e) {
      toast(String(e), true);
      return false;
    } finally {
      mc.busy.delete(file);
    }
  }

  // ---------- events ----------
  $("#mc-kind").onclick = (e) => {
    const b = e.target.closest("[data-kind]");
    if (!b || b.dataset.kind === mc.kind) return;
    mc.kind = b.dataset.kind;
    syncSeg(); renderHead();
    loadInstalled(false);
    if (mc.tab === "browse") search();
  };
  $("#mc-tab").onclick = (e) => {
    const b = e.target.closest("[data-tab]");
    if (!b) return;
    mc.tab = b.dataset.tab;
    syncSeg();
    if (mc.tab === "browse" && !mc.results.length) search();
  };
  $("#mc-source").onclick = (e) => {
    const b = e.target.closest("[data-source]");
    if (!b || b.dataset.source === mc.source) return;
    mc.source = b.dataset.source;
    ls.set("mc.source", mc.source);
    syncSeg(); renderHead();
    search();
  };
  let t;
  $("#mc-query").oninput = (e) => { mc.query = e.target.value; clearTimeout(t); t = setTimeout(() => search(), 350); };
  $("#mc-filter").oninput = (e) => { mc.filter = e.target.value; renderInstalled(); };
  $("#mc-more").onclick = () => search(true);
  $("#mc-check").onclick = () => loadInstalled(true);
  $("#mc-play").onclick = () => inst() && window.L.play(inst().id);
  $("#mc-folder").onclick = () => invoke("mc_open_folder", { instance: inst().id, kind: mc.kind }).catch((e) => toast(String(e), true));
  $("#mc-instance").onchange = (e) => {
    mc.current = e.target.value;
    ls.set("mc.instance", mc.current);
    mc.results = [];
    renderHead();
    loadInstalled(false);
    if (mc.tab === "browse") search();
  };
  $("#mc-update-all").onclick = async () => {
    const ups = mc.items.filter((it) => it.update);
    if (!ups.length || !confirm(`Update ${ups.length} ${KIND_LABEL[mc.kind]}? Old files go to the trash, so you can restore them if something breaks.`)) return;
    mc.busy.add("all");
    let ok = 0;
    for (const [n, it] of ups.entries()) {
      $("#mc-update-all").textContent = `Updating ${n + 1}/${ups.length}…`;
      if (await update(it.file)) ok++;
    }
    mc.busy.delete("all");
    toast(`Updated ${ok} of ${ups.length}`);
    loadInstalled(true);
  };
  $("#mc-list").addEventListener("change", async (e) => {
    const file = e.target.dataset.toggle;
    if (!file) return;
    try {
      await invoke("mc_toggle", { instance: inst().id, kind: mc.kind, file, enabled: e.target.checked });
    } catch (err) {
      toast(String(err), true);
    }
    loadInstalled(mc.checked);
  });
  $("#mc-list").addEventListener("click", async (e) => {
    const rm = e.target.closest("[data-remove]");
    if (rm) {
      const it = mc.items.find((x) => x.file === rm.dataset.remove);
      if (!confirm(`Move “${it ? it.title : rm.dataset.remove}” to the trash?`)) return;
      try {
        await invoke("mc_remove", { instance: inst().id, kind: mc.kind, file: rm.dataset.remove });
        toast(`Removed ${it ? it.title : rm.dataset.remove}`);
      } catch (err) { toast(String(err), true); }
      return loadInstalled(mc.checked);
    }
    const up = e.target.closest("[data-update]");
    if (up) { if (await update(up.dataset.update)) loadInstalled(true); }
  });
  $("#mc-results").addEventListener("click", (e) => {
    const b = e.target.closest("[data-install]");
    if (b) install(mc.results[+b.dataset.install]);
  });

  // ---------- entry points used by app.js ----------
  window.mcShow = async (soft = false) => {
    if (soft) return renderHead();
    if (mc.hasCfKey === null) {
      try { mc.hasCfKey = !!(await invoke("get_settings")).curseforge_api_key; } catch { mc.hasCfKey = false; }
    }
    if (!mc.instances) {
      try { mc.instances = await invoke("mc_instances"); } catch (e) { toast(String(e), true); mc.instances = []; }
      if (!mc.instances.find((i) => i.id === mc.current)) mc.current = mc.instances[0]?.id || null;
    }
    syncSeg();
    renderHead();
    if (mc.itemsKey !== `${mc.current}|${mc.kind}`) loadInstalled(false);
    else renderInstalled();
  };
  window.mcSettingsChanged = async () => {
    mc.hasCfKey = !!(await invoke("get_settings")).curseforge_api_key;
    if (window.L.state.view === "minecraft" && mc.source === "curseforge") search();
  };
  if (window.L.state.view === "minecraft") window.mcShow();
})();
