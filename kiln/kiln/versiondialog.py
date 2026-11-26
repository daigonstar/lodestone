"""Game version window: see which version is installed, get an older one through
Steam, and switch between versions Kiln keeps."""
from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtCore import Qt, QTimer, QUrl
from PySide6.QtGui import QDesktopServices, QGuiApplication
from PySide6.QtWidgets import (
    QCheckBox, QDialog, QFrame, QGridLayout, QHBoxLayout, QLabel, QMessageBox, QProgressDialog,
    QPushButton, QScrollArea, QVBoxLayout, QWidget,
)

from . import gameversions as gv

if TYPE_CHECKING:
    from .gui import MainWindow
    from .instance import Instance


def _gb(n: int) -> str:
    return f"{n / 1e9:.1f} GB" if n >= 1e8 else f"{n / 1e6:.0f} MB"


class VersionDialog(QDialog):
    def __init__(self, host: "MainWindow", inst: "Instance"):
        super().__init__(host)
        self.host, self.inst = host, inst
        self.setWindowTitle(f"{inst.game.name}: game version")
        self.resize(820, 620)
        self.watches: dict[str, gv.DepotWatch] = {}
        self.rows: dict[str, dict] = {}
        lay = QVBoxLayout(self)
        self.head = QLabel()
        self.head.setWordWrap(True)
        lay.addWidget(self.head)
        self.scroll = QScrollArea()
        self.scroll.setWidgetResizable(True)
        self.scroll.setFrameShape(QFrame.NoFrame)
        lay.addWidget(self.scroll, 1)
        self.protect = QCheckBox("Stop Steam from updating the game (recommended while on an older version)")
        self.protect.setToolTip("Sets the game to update only when launched from Steam and makes its Steam "
                                "manifest read-only, so a game update can't replace your chosen version. "
                                "Steam has to be closed for a moment.")
        self.protect.setChecked(gv.update_protection(inst))
        self.protect.toggled.connect(self._protect_toggled)
        lay.addWidget(self.protect)
        close = QPushButton("Close")
        close.clicked.connect(self.close)
        row = QHBoxLayout()
        row.addStretch(1)
        row.addWidget(close)
        lay.addLayout(row)
        self.timer = QTimer(self, interval=3000)
        self.timer.timeout.connect(self._poll)
        self.fill()
        self.timer.start()

    # ------------------------------------------------------------- view
    def fill(self):
        inst = self.inst
        cur = gv.installed_version(inst)
        kept = gv.stored(inst)
        self.head.setText(
            f"<h3>{inst.game.name} {gv.short(cur) or '(version unknown)'} is installed</h3>"
            "Mods, especially script-extender plugins, are often made for one game version. When "
            "the game updates, they stop working until their authors catch up. Many modders "
            "stay on the version their mods support. Kiln gets older versions from Steam "
            "itself (you own the game, so Steam lets you download its earlier builds) and keeps "
            "the files it replaces so you can switch back.")
        body = QWidget()
        bl = QVBoxLayout(body)
        self.rows.clear()
        for v in kept:
            card, cl = self._card(f"Switch back to {gv.short(v)}",
                                  "Kiln kept this version's files when you switched.")
            b = QPushButton(f"Switch to {gv.short(v)}")
            b.clicked.connect(lambda _=False, v=v: self.switch_stored(v))
            cl.addWidget(b)
            bl.addWidget(card)
        for ver in gv.for_game(inst.game.id):
            if gv.same(ver.version, cur) or any(gv.same(ver.version, k) for k in kept):
                continue
            bl.addWidget(self._download_card(ver))
        if gv.for_game(inst.game.id) or kept:
            m = QPushButton("Match my mods to this game version…")
            m.setToolTip("Find the script extender build and the mod files made for the version "
                         "you have now, and install them.")
            m.clicked.connect(self.host.open_match)
            bl.addWidget(m)
        if bl.count() == 0:
            msg = QLabel("Kiln doesn't know any other versions of this game yet."
                         if not gv.for_game(inst.game.id) else
                         "You're on the version Kiln knows about. Nothing to switch to.")
            msg.setWordWrap(True)
            bl.addWidget(msg)
        bl.addStretch(1)
        self.scroll.setWidget(body)

    def _card(self, title: str, text: str):
        card = QFrame()
        card.setFrameShape(QFrame.StyledPanel)
        cl = QVBoxLayout(card)
        t = QLabel(f"<b style='font-size:14px'>{title}</b>")
        cl.addWidget(t)
        if text:
            d = QLabel(text)
            d.setWordWrap(True)
            cl.addWidget(d)
        return card, cl

    def _download_card(self, ver: gv.GameVersion) -> QFrame:
        inst = self.inst
        card, cl = self._card(ver.label, ver.why + "".join(f"\n• {n}" for n in ver.notes))
        steps = QLabel("<b>1.</b> Open Steam's console. <b>2.</b> Paste each command below into it and "
                       "press Enter (Kiln copies them for you). Steam downloads the files in the "
                       "background; this window shows their progress. <b>3.</b> Switch.")
        steps.setWordWrap(True)
        cl.addWidget(steps)
        open_c = QPushButton("Open Steam's console")
        open_c.clicked.connect(lambda: QDesktopServices.openUrl(QUrl("steam://open/console")))
        cl.addWidget(open_c)
        grid = QGridLayout()
        row = {"ver": ver, "status": {}, "switch": None}
        for i, (depot, cmd) in enumerate(zip(ver.depots, gv.commands(inst, ver))):
            lab = QLabel(f"<code>{cmd}</code>")
            lab.setTextInteractionFlags(Qt.TextSelectableByMouse)
            copy = QPushButton("Copy")
            copy.clicked.connect(lambda _=False, c=cmd, b=copy: self._copy(c, b))
            st = QLabel("waiting")
            grid.addWidget(lab, i, 0)
            grid.addWidget(copy, i, 1)
            grid.addWidget(st, i, 2)
            row["status"][depot] = st
        cl.addLayout(grid)
        sw = QPushButton(f"Switch to {gv.short(ver.version)}")
        sw.setEnabled(False)
        sw.clicked.connect(lambda: self.switch_download(ver))
        cl.addWidget(sw)
        src = QLabel("Commands checked against: " + " · ".join(
            f"<a href='{u}'>{u.split('/')[2]}</a>" for u in ver.sources))
        src.setOpenExternalLinks(True)
        src.setStyleSheet("color: gray;")
        cl.addWidget(src)
        row["switch"] = sw
        self.rows[ver.version] = row
        self.watches.setdefault(ver.version, gv.DepotWatch(inst, ver))
        return card

    def _copy(self, cmd: str, b: QPushButton):
        QGuiApplication.clipboard().setText(cmd)
        b.setText("Copied ✔")
        QTimer.singleShot(2500, lambda: b.setText("Copy"))

    def _poll(self):
        for v, row in list(self.rows.items()):
            try:
                res = self.watches[v].poll()
            except Exception:  # noqa: BLE001
                continue
            for depot, (st, size) in res.items():
                lab = row["status"][depot]
                lab.setText({"waiting": "waiting for Steam",
                             "downloading": f"downloading… {_gb(size)}",
                             "ready": f"✔ ready ({_gb(size)})"}[st])
            try:
                row["switch"].setEnabled(all(st == "ready" for st, _ in res.values()))
            except RuntimeError:
                pass

    # ------------------------------------------------------------- actions
    def _ready_to_change(self) -> bool:
        from . import launchguard
        h = self.host
        if launchguard.game_running(self.inst):
            QMessageBox.information(self, "Game running", "Close the game first.")
            return False
        if h.crun or h.dl_pool.activeThreadCount() or h.jobs.activeThreadCount():
            QMessageBox.information(self, "Please wait", "Kiln is still downloading or installing. "
                                    "Switch versions once it's finished.")
            return False
        return True

    def switch_download(self, ver: gv.GameVersion):
        if not self._ready_to_change():
            return
        cur = gv.installed_version(self.inst)
        if QMessageBox.question(
                self, "Switch game version",
                f"Switch {self.inst.game.name} from {gv.short(cur)} to {gv.short(ver.version)}?\n\n"
                f"Kiln takes your mods out of the game folder, puts in Steam's {gv.short(ver.version)} "
                f"files, keeps the {gv.short(cur)} files so you can switch back, then puts your mods "
                "back.") != QMessageBox.Yes:
            return
        self._run(lambda p: gv.switch_to_download(self.inst, ver, p), ver.version)

    def switch_stored(self, v: str):
        if not self._ready_to_change():
            return
        if QMessageBox.question(self, "Switch game version",
                                f"Switch {self.inst.game.name} to {gv.short(v)}?") != QMessageBox.Yes:
            return
        self._run(lambda p: gv.switch_to_stored(self.inst, v, p), v)

    def _run(self, fn, target: str):
        from . import deploy
        from .gui import Task
        h, inst = self.host, self.inst
        self.timer.stop()
        dlg = QProgressDialog("Switching game version…", None, 0, 0, self)
        dlg.setWindowTitle("Game version")
        dlg.setMinimumDuration(0)
        dlg.show()

        def work(progress, cancelled):
            deploy.purge(inst)
            old = fn(progress)
            return old

        def prog(done, total):
            dlg.setMaximum(total)
            dlg.setValue(done)
            dlg.setLabelText(f"Moving game files… {done} of {total}")

        def done(old):
            dlg.close()
            h.deploy_timer.stop()
            h._deploy_sync(inst)
            h.refresh_all()
            self.fill()
            self.timer.start()
            on_old = bool(gv.state(inst).get("original"))
            if on_old and not self.protect.isChecked():
                self.protect.setChecked(True)          # asks to close Steam, then protects
            elif not on_old and self.protect.isChecked():
                if QMessageBox.question(self, "Steam updates",
                                        "You're back on the version you started with. Let Steam "
                                        "update the game again?") == QMessageBox.Yes:
                    self.protect.setChecked(False)
            QMessageBox.information(
                self, "Game version", f"{inst.game.name} is now {gv.short(gv.installed_version(inst))}. "
                "Next, Kiln looks for the script extender and mod versions made for it.")
            h.go_to(h.doctor_tab)
            from . import essentials
            rt = essentials.game_runtime(inst)       # the match window covers essentials too
            h.__dict__.setdefault("_ess_prompted", set()).add(
                (inst.game.id, ".".join(map(str, rt)) if rt else ""))
            h.open_match()

        def fail(e):
            dlg.close()
            self.timer.start()
            h.error("Couldn't switch versions", e)
            h._deploy_sync(inst)

        Task(work, progress=True).start(done, fail, prog, pool=h.jobs)

    def _protect_toggled(self, on: bool):
        from . import steamcfg
        if on == gv.update_protection(self.inst):
            return
        if on and steamcfg.steam_running():
            r = QMessageBox.question(
                self, "Close Steam", "Steam has to be closed for a moment so it doesn't undo the "
                "change. Close Steam now? (Start it again afterwards.)")
            if r != QMessageBox.Yes or not steamcfg.shutdown_steam():
                self.protect.blockSignals(True)
                self.protect.setChecked(False)
                self.protect.blockSignals(False)
                return
        try:
            gv.set_update_protection(self.inst, on)
        except Exception as e:  # noqa: BLE001
            self.host.error("Steam updates", e)
            self.protect.blockSignals(True)
            self.protect.setChecked(gv.update_protection(self.inst))
            self.protect.blockSignals(False)
            return
        self.host.statusBar().showMessage(
            "Steam won't update the game now." if on else "Steam can update the game again.", 8000)

    def closeEvent(self, e):
        self.timer.stop()
        super().closeEvent(e)


class MatchDialog(QDialog):
    """After a game version change: the script extender build, Address Library and
    the mod files that fit the new version, found on Nexus (older files included)."""

    def __init__(self, host: "MainWindow", inst: "Instance"):
        from PySide6.QtWidgets import QTreeWidget
        super().__init__(host)
        self.host, self.inst = host, inst
        self.needs = []
        self.setWindowTitle("Match mods to your game version")
        self.resize(980, 560)
        lay = QVBoxLayout(self)
        self.head = QLabel("<h3>Looking for the right versions…</h3>Kiln checks which of your SKSE "
                           "plugins won't load on this game version, then reads each mod's Nexus "
                           "page (older files included) for the file made for it.")
        self.head.setWordWrap(True)
        lay.addWidget(self.head)
        self.tree = QTreeWidget()
        self.tree.setHeaderLabels(["Mod", "Why", "Kiln picked", "How sure", "Status"])
        self.tree.setRootIsDecorated(False)
        self.tree.setWordWrap(True)
        lay.addWidget(self.tree, 1)
        self.note = QLabel("Picked files replace your current copy of each mod; its old download stays "
                           "in Downloads. Troubleshoot checks every plugin again afterwards.")
        self.note.setWordWrap(True)
        self.note.setStyleSheet("color: gray;")
        lay.addWidget(self.note)
        row = QHBoxLayout()
        row.addStretch(1)
        self.go = QPushButton("Install selected")
        self.go.setEnabled(False)
        self.go.clicked.connect(self.install)
        self.close_btn = QPushButton("Not now")
        self.close_btn.clicked.connect(self.close)
        row.addWidget(self.go)
        row.addWidget(self.close_btn)
        lay.addLayout(row)
        self.refresh()

    def refresh(self):
        from . import versionmatch
        from .gui import Task
        self.tree.clear()
        Task(versionmatch.plan, self.inst).start(self._fill, lambda e: self.head.setText(
            f"<h3>Couldn't check</h3>{e}"))

    def _fill(self, needs):
        from PySide6.QtGui import QBrush, QColor
        from PySide6.QtWidgets import QTreeWidgetItem
        self.needs = needs
        cur = gv.short(gv.installed_version(self.inst))
        ok = [n for n in needs if n.file or n.reuse]
        if not needs:
            self.head.setText(f"<h3>Everything fits {self.inst.game.name} {cur}</h3>"
                              "Your script extender, Address Library and SKSE plugins all match this version.")
            self.close_btn.setText("Close")
            return
        self.head.setText(f"<h3>{len(needs)} thing(s) don't fit {self.inst.game.name} {cur}</h3>"
                          f"Kiln found a matching file for {len(ok)}. Tick what to install.")
        sure_txt = {"exact": "✔ made for this version", "likely": "covers this version",
                    "guess": "doesn't say; newest file"}
        for n in needs:
            if n.reuse is not None:
                it = QTreeWidgetItem([n.title, n.why, f"{n.reuse.name} {n.reuse.version} (already downloaded)",
                                      "✔ checked: loads on this version", ""])
                it.setFlags(it.flags() | Qt.ItemIsUserCheckable)
                it.setCheckState(0, Qt.Checked)
                it.setData(0, Qt.UserRole, n)
                self.tree.addTopLevelItem(it)
                continue
            f = n.file
            it = QTreeWidgetItem([n.title, n.why,
                                  f"{f.name} {f.version} ({f.category.replace('_', ' ').lower()})" if f else "—",
                                  sure_txt.get(n.sure, "") if f else "", "" if f else n.problem])
            it.setToolTip(2, (f.description or "")[:600] if f else "")
            it.setToolTip(4, n.problem)
            if f:
                it.setFlags(it.flags() | Qt.ItemIsUserCheckable)
                it.setCheckState(0, Qt.Checked if n.sure != "guess" else Qt.Unchecked)
                if n.sure == "guess":
                    it.setForeground(3, QBrush(QColor("#f08c00")))
            else:
                it.setForeground(4, QBrush(QColor("#e03131")))
            it.setData(0, Qt.UserRole, n)
            self.tree.addTopLevelItem(it)
        for c in (0, 2, 3):
            self.tree.resizeColumnToContents(c)
        self.tree.setColumnWidth(1, 260)
        self.go.setEnabled(bool(ok))
        no = [n for n in needs if not n.file and not n.reuse and n.mod]
        if no:
            self.note.setText(self.note.text() + f" {len(no)} mod(s) have no version for this game "
                              "yet: turn them off on the Mods page (or Troubleshoot) if they cause trouble.")

    def install(self):
        from . import collections as nc
        entries, swapped = [], 0
        for i in range(self.tree.topLevelItemCount()):
            it = self.tree.topLevelItem(i)
            n = it.data(0, Qt.UserRole)
            if not (it.flags() & Qt.ItemIsUserCheckable) or it.checkState(0) != Qt.Checked:
                continue
            if n.reuse is not None:            # no download: switch to the copy you have
                from . import doctor
                doctor.apply(self.inst, f"swap:{n.reuse.id}:{n.mod.id}")
                it.setText(4, "✔ switched")
                swapped += 1
                continue
            f = n.file
            if n.mod is not None:
                self.host.replace_on_install(n.nexus_id, f.file_id, n.mod.id)
            cm = nc.CollectionMod(n.nexus_id, f.file_id, n.title, f.name, f.version, False,
                                  f.size // 1024, f.category)
            it.setText(4, "queued")
            entries.append({"kind": "nexus", "mod": cm, "item": it, "status": "queued"})
        if swapped:
            self.host.refresh_mods(keep_panel=True)
            self.host.schedule_deploy(self.inst)
        if not entries:
            self.go.setEnabled(False)
            self.close_btn.setText("Close")
            return
        self.go.setEnabled(False)
        self.close_btn.setText("Close")
        self.host.start_nexus_queue(self.inst, entries, "Matching mods to your game version",
                                    on_finish=lambda: (self.host.refresh_mods(),
                                                       self.host.go_to(self.host.doctor_tab)))
