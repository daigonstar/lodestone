"""Per-game setup: shows the loaders and frameworks a game needs before modding
and installs the ones you tick. Also the first-run welcome listing your games."""
from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtCore import QSize, Qt
from PySide6.QtGui import QBrush, QColor, QFont
from PySide6.QtWidgets import (QDialog, QFrame, QHBoxLayout, QHeaderView, QLabel, QPushButton,
                               QScrollArea, QTreeWidget, QTreeWidgetItem, QVBoxLayout, QWidget)

from . import essentials

if TYPE_CHECKING:
    from .gui import MainWindow
    from .instance import Instance

STATUS = {"installed": ("✔ installed", "#2f9e44"), "disabled": ("installed but disabled", "#f08c00"),
          "elsewhere": ("downloaded: will reuse", "#1c7ed6"),
          "mismatch": ("for another game version: swap", "#e03131"),
          "missing": ("missing", "#e03131")}


class SetupDialog(QDialog):
    def __init__(self, host: "MainWindow", inst: "Instance"):
        super().__init__(host)
        self.host, self.inst = host, inst
        g = inst.game
        self.setWindowTitle(f"Set up {g.name} for modding")
        self.resize(1080, 540)
        lay = QVBoxLayout(self)
        top = QHBoxLayout()
        art = host.game_art.get(g.id)
        pic = QLabel()
        if art:
            pm = art.pixmap("capsule")
            if not pm.isNull():
                pic.setPixmap(pm.scaled(QSize(96, 144), Qt.KeepAspectRatio, Qt.SmoothTransformation))
        top.addWidget(pic)
        self.head = QLabel()
        self.head.setWordWrap(True)
        top.addWidget(self.head, 1)
        lay.addLayout(top)

        self.tree = QTreeWidget()
        self.tree.setHeaderLabels(["Essential", "What it's for", "File", "Source", "Status"])
        self.tree.setRootIsDecorated(False)
        self.tree.setWordWrap(True)
        self.tree.header().setSectionResizeMode(1, QHeaderView.Stretch)
        self.tree.header().setSectionResizeMode(2, QHeaderView.ResizeToContents)
        lay.addWidget(self.tree, 1)
        self.note = QLabel()
        self.note.setWordWrap(True)
        self.note.setStyleSheet("color: gray;")
        lay.addWidget(self.note)
        row = QHBoxLayout()
        row.addStretch(1)
        self.skip = QPushButton("Not now")
        self.skip.clicked.connect(self.close)
        self.go = QPushButton("Install selected")
        self.go.setDefault(True)
        self.go.clicked.connect(self.install)
        row.addWidget(self.skip)
        row.addWidget(self.go)
        lay.addLayout(row)
        self.items: dict[str, QTreeWidgetItem] = {}
        self.files: dict[str, object] = {}
        self.fill()

    # ------------------------------------------------------------------ list
    def fill(self):
        from . import nexus
        from .gui import Task
        inst, g = self.inst, self.inst.game
        self.tree.clear()
        self.items.clear()
        ess = essentials.for_game(g.id)
        miss = [e for e in ess if essentials.status(inst, e) != "installed"]
        req_miss = [e for e in miss if e.required]
        if not ess:
            self.head.setText(f"<h3>{g.name} is ready to mod</h3>"
                              + essentials.NOTES.get(g.id, "No loader or framework is needed."))
            self.go.hide()
            self.skip.setText("Close")
            return
        self.head.setText(
            f"<h3>{'Get ' + g.name + ' ready for mods' if miss else g.name + ' is ready to mod'}</h3>"
            + ("Most mods for this game build on these. Kiln installs the ones you tick, sets "
               "Steam's launch options so they load, and checks them every time you press Play."
               if miss else "Everything essential is installed.")
            + (f"<br><b style='color:#e03131'>{len(req_miss)} required item(s) missing.</b>"
               if req_miss else ""))
        bold = QFont()
        bold.setWeight(QFont.DemiBold)
        for e in ess:
            st = essentials.status(inst, e)
            txt, col = STATUS[st]
            it = QTreeWidgetItem([e.name, ("Required. " if e.required else "Recommended. ") + e.why,
                                  "…" if e.kind == "nexus" else "latest release",
                                  "Nexus Mods" if e.kind == "nexus" else "GitHub (official)", txt])
            it.setData(0, Qt.UserRole, e)
            it.setForeground(4, QBrush(QColor(col)))
            if e.required:
                it.setFont(0, bold)
            if st != "installed":
                it.setFlags(it.flags() | Qt.ItemIsUserCheckable)
                it.setCheckState(0, Qt.Checked)
            self.tree.addTopLevelItem(it)
            self.items[e.key] = it
            if e.kind == "nexus" and st == "elsewhere":
                m = essentials.downloaded(inst, e)
                it.setText(2, f"{m.file_name or m.name} {m.version}".strip())
                it.setToolTip(4, "Already downloaded for another profile: Kiln adds it to this "
                                 "profile instead of downloading it again.")
            elif e.kind == "nexus" and st != "installed":
                def got(d, e=e, it=it):
                    f = essentials.pick_file(d.files, e, essentials.game_runtime(inst),
                                             essentials.downloaded(inst, e))
                    self.files[e.key] = (d, f)
                    try:
                        it.setText(2, f"{f.name} {f.version}" if f else "no main file")
                        it.setToolTip(2, f.description if f else "")
                        it.setToolTip(1, e.why)
                    except RuntimeError:
                        pass
                Task(nexus.mod_details, g.nexus_domain, g.nexus_game_id, e.nexus_id).start(
                    got, lambda err, it=it: it.setText(2, f"couldn't load: {err}"))
            elif e.kind == "smapi" and st != "installed":
                from . import launchguard
                Task(launchguard.smapi_release).start(
                    lambda r, it=it: it.setText(2, f"SMAPI {r[0]} ({r[2] / 1048576:.0f} MB)"),
                    lambda err, it=it: it.setText(2, f"couldn't check: {err}"))
        for c in (0, 3, 4):
            self.tree.resizeColumnToContents(c)
        self.tree.setColumnWidth(4, max(self.tree.columnWidth(4), 150))
        premium = bool(self.host.user and self.host.user.is_premium)
        self.note.setText(
            ("Premium account: everything downloads automatically. " if premium else
             "Free account: Kiln opens each download page in its own browser with “Slow download” "
             "highlighted; click it and Kiln moves to the next one. ")
            + "Files come from each mod's official page. Items you already downloaded (for "
              "this or another profile) are reused, not downloaded again.")
        self.go.setVisible(bool(miss))
        self.skip.setText("Not now" if miss else "Close")

    def selected(self):
        out = []
        for e in essentials.for_game(self.inst.game.id):
            it = self.items.get(e.key)
            if it and (it.flags() & Qt.ItemIsUserCheckable) and it.checkState(0) == Qt.Checked:
                out.append(e)
        return out

    def install(self):
        sel = self.selected()
        if not sel:
            return
        for e in sel:
            if e.kind == "nexus" and e.key not in self.files and \
                    essentials.status(self.inst, e) not in ("elsewhere", "disabled"):
                self.note.setText("Still looking up files on Nexus. Try again in a moment.")
                return
        self.go.setEnabled(False)
        self.skip.setText("Close")
        self.host.install_essentials(self, sel)


class WelcomeDialog(QDialog):
    """First run: your supported games, what each needs, one click to set up."""

    def __init__(self, host: "MainWindow"):
        super().__init__(host)
        self.host = host
        self.setWindowTitle("Welcome to Kiln")
        self.resize(760, 560)
        lay = QVBoxLayout(self)
        t = QLabel("<h2>Welcome to Kiln</h2>These are the games Kiln supports. For the ones you "
                   "have installed, Kiln can set up the mod loaders and frameworks they need "
                   "before you start, so mods work the first time.")
        t.setWordWrap(True)
        lay.addWidget(t)
        area = QScrollArea()
        area.setWidgetResizable(True)
        area.setFrameShape(QFrame.NoFrame)
        body = QWidget()
        bl = QVBoxLayout(body)
        from .instance import Instance
        for i, g in enumerate(host.games):
            row = QHBoxLayout()
            art = host.game_art[g.id]
            pic = QLabel()
            pm = art.pixmap("header")
            if pm.isNull():
                pm = art.pixmap("hero")
            if not pm.isNull():
                pic.setPixmap(pm.scaled(QSize(184, 86), Qt.KeepAspectRatio, Qt.SmoothTransformation))
            pic.setFixedSize(184, 86)
            row.addWidget(pic)
            path = host._detect(g)
            ess = essentials.for_game(g.id)
            if path is None:
                status, btn = "<span style='color:gray'>Not installed (or not found)</span>", None
            else:
                inst = host.instances.get(g.id) or Instance(
                    g, path, host.settings.game(g.id).get("staging") or Instance.default_staging(g, path))
                host.instances.setdefault(g.id, inst)
                miss = [e for e in ess if essentials.status(inst, e) != "installed"]
                req = [e for e in miss if e.required]
                if not ess:
                    status = "<span style='color:#2f9e44'>Ready to mod</span> · " + essentials.NOTES.get(g.id, "")
                elif not miss:
                    status = "<span style='color:#2f9e44'>✔ Essentials installed</span>"
                else:
                    status = (f"<span style='color:{'#e03131' if req else '#f08c00'}'>"
                              + (f"{len(req)} required" if req else "")
                              + (" + " if req and len(miss) > len(req) else "")
                              + (f"{len(miss) - len(req)} recommended" if len(miss) > len(req) else "")
                              + " missing</span>: " + ", ".join(e.name for e in miss[:3])
                              + ("…" if len(miss) > 3 else ""))
                btn = QPushButton("Set up…" if miss else "Open")
                btn.clicked.connect(lambda _=False, i=i: self._open(i))
            lab = QLabel(f"<b>{g.name}</b><br>{status}")
            lab.setWordWrap(True)
            row.addWidget(lab, 1)
            if btn:
                row.addWidget(btn)
            bl.addLayout(row)
        bl.addStretch(1)
        area.setWidget(body)
        lay.addWidget(area, 1)
        r = QHBoxLayout()
        r.addStretch(1)
        done = QPushButton("Get started")
        done.clicked.connect(self.accept)
        r.addWidget(done)
        lay.addLayout(r)

    def _open(self, i: int):
        self.accept()
        self.host.game_combo.setCurrentIndex(i)
        self.host.open_setup()


class DepsDialog(QDialog):
    """Mods your Stardew mods need but you don't have, matched to Nexus, one click to install."""

    def __init__(self, host: "MainWindow", inst: "Instance"):
        super().__init__(host)
        self.host, self.inst = host, inst
        self.setWindowTitle("Missing required mods")
        self.resize(1080, 460)
        lay = QVBoxLayout(self)
        self.head = QLabel("<h3>Checking what your mods need…</h3>")
        self.head.setWordWrap(True)
        lay.addWidget(self.head)
        self.tree = QTreeWidget()
        self.tree.setHeaderLabels(["Needed mod", "Required by", "Nexus file", "Matched using", "Status"])
        self.tree.setRootIsDecorated(False)
        self.tree.setWordWrap(True)
        self.tree.header().setSectionResizeMode(1, QHeaderView.Stretch)
        lay.addWidget(self.tree, 1)
        self.note = QLabel()
        self.note.setWordWrap(True)
        self.note.setStyleSheet("color: gray;")
        lay.addWidget(self.note)
        row = QHBoxLayout()
        row.addStretch(1)
        self.close_btn = QPushButton("Not now")
        self.close_btn.clicked.connect(self.close)
        self.go = QPushButton("Install selected")
        self.go.setDefault(True)
        self.go.setEnabled(False)
        self.go.clicked.connect(self.install)
        row.addWidget(self.close_btn)
        row.addWidget(self.go)
        lay.addLayout(row)
        self.rows: list[dict] = []
        self.recheck()

    def recheck(self):
        from . import smapideps
        from .gui import Task
        self.tree.clear()
        self.rows = []
        self.go.setEnabled(False)
        missing = smapideps.check(self.inst)
        if not missing:
            self.head.setText("<h3>✔ Every mod has what it needs</h3>No required mods are missing.")
            self.go.hide()
            self.close_btn.setText("Close")
            return
        self.head.setText(f"<h3>{len(missing)} required mod(s) are missing</h3>SMAPI skips any mod "
                          "whose requirements aren't installed. Kiln found these on Nexus:")
        self.note.setText("Looking them up…")

        def work():
            from . import nexus
            smapideps.resolve(missing, self.inst.game.nexus_domain)
            coll = smapideps.collection_files(self.inst)
            g = self.inst.game
            details = {}
            for m in missing:
                if m.nexus_id and m.nexus_id not in details:
                    try:
                        details[m.nexus_id] = nexus.mod_details(g.nexus_domain, g.nexus_game_id, m.nexus_id)
                    except Exception:  # noqa: BLE001
                        details[m.nexus_id] = None
            return missing, coll, details

        Task(work).start(self._resolved, lambda e: self.note.setText(f"Couldn't look them up: {e}"))

    def _resolved(self, res):
        from . import essentials
        missing, coll, details = res
        by_nexus: dict[int, dict] = {}
        for m in missing:
            if m.nexus_id:
                r = by_nexus.get(m.nexus_id)
                if r:
                    r["deps"].append(m)
                    continue
                d = details.get(m.nexus_id)
                f = None
                src = ""
                if d:
                    fid = coll.get(m.nexus_id)
                    f = next((x for x in d.files if x.file_id == fid), None) if fid else None
                    src = "version from your collection" if f else ""
                    f = f or essentials.pick_file(d.files, essentials.Essential("", "", "", False))
                by_nexus[m.nexus_id] = {"deps": [m], "details": d, "file": f, "src": src}
        for m in missing:
            if m.via:
                main = next((r for r in by_nexus.values() if any(x.unique_id == m.via for x in r["deps"])), None)
                if main:
                    main["deps"].append(m)
                    continue
        for nid, r in by_nexus.items():
            d, f = r["details"], r["file"]
            needed = sorted({n for x in r["deps"] for n in x.needed_by})
            how = "; ".join(sorted({x.how for x in r["deps"]}))
            ok = bool(d and f)
            it = QTreeWidgetItem([d.name if d else r["deps"][0].name,
                                  ", ".join(needed),
                                  (f"{f.name} {f.version}" + (f" ({r['src']})" if r["src"] else "")) if ok else "no file found",
                                  how, "missing"])
            it.setToolTip(0, "Provides: " + ", ".join(x.unique_id for x in r["deps"]))
            it.setForeground(4, QBrush(QColor("#e03131")))
            if ok:
                it.setFlags(it.flags() | Qt.ItemIsUserCheckable)
                it.setCheckState(0, Qt.Checked)
            self.tree.addTopLevelItem(it)
            self.rows.append({"item": it, "details": d, "file": f, "nexus_id": nid})
        for m in missing:
            if not m.nexus_id and not any(m in r["deps"] for r in by_nexus.values()):
                it = QTreeWidgetItem([m.unique_id, ", ".join(m.needed_by), "not found on Nexus",
                                      "", "install it yourself"])
                it.setForeground(4, QBrush(QColor("#f08c00")))
                self.tree.addTopLevelItem(it)
        for c in (0, 2, 3, 4):
            self.tree.resizeColumnToContents(c)
        premium = bool(self.host.user and self.host.user.is_premium)
        self.note.setText(("Premium: downloads run automatically. " if premium else
                           "Free account: click “Slow download” on each page Kiln opens. ")
                          + "Hover a mod to see which IDs it provides.")
        self.go.setEnabled(any(r["file"] for r in self.rows))

    def install(self):
        from . import collections as nc
        entries = []
        for r in self.rows:
            it = r["item"]
            if not (it.flags() & Qt.ItemIsUserCheckable) or it.checkState(0) != Qt.Checked:
                continue
            d, f = r["details"], r["file"]
            cm = nc.CollectionMod(r["nexus_id"], f.file_id, d.name, f.name, f.version, False,
                                  f.size // 1024, f.category)
            it.setText(4, "queued")
            entries.append({"kind": "nexus", "mod": cm, "item": it, "status": "queued"})
        if not entries:
            return
        self.go.setEnabled(False)
        self.close_btn.setText("Close")
        self.host.start_nexus_queue(self.inst, entries, "Missing mods",
                                    on_finish=lambda: (self.recheck(), self.host.refresh_mods()))
