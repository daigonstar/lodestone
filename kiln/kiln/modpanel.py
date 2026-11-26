"""Right-hand mod pane: picture, status, actions and tabs (overview, files,
conflicts, plugins, notes & info) for the selected mod."""
from __future__ import annotations

import html
from typing import TYPE_CHECKING

from PySide6.QtCore import Qt, QTimer
from PySide6.QtGui import QBrush, QColor, QFont, QPixmap
from PySide6.QtWidgets import (QComboBox, QFormLayout, QFrame, QGridLayout, QHBoxLayout,
                               QHeaderView, QLabel, QPlainTextEdit, QPushButton, QScrollArea,
                               QTabWidget, QTextBrowser, QTreeWidget, QTreeWidgetItem, QVBoxLayout,
                               QWidget)

from . import nexus

if TYPE_CHECKING:
    from .gui import MainWindow
    from .instance import Mod

esc = lambda s: html.escape(str(s or ""))

BADGE = ("QLabel { border-radius: 9px; padding: 2px 8px; color: white; background: %s;"
         " font-size: 11px; font-weight: 600; }")


def _badge(text: str, color: str) -> QLabel:
    b = QLabel(text)
    b.setStyleSheet(BADGE % color)
    return b


def fmt_size(n: float) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024 or unit == "GB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024
    return ""


class ModPanel(QScrollArea):
    def __init__(self, host: "MainWindow"):
        super().__init__()
        self.host = host
        self.mod: "Mod | None" = None
        self.setWidgetResizable(True)
        self.setMinimumWidth(380)
        self.setFrameShape(QFrame.NoFrame)
        self.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        body = QWidget()
        self.setWidget(body)
        lay = QVBoxLayout(body)
        lay.setContentsMargins(10, 4, 4, 4)

        self.empty = QLabel("<p style='color:gray'>Select a mod to see its details and what you can "
                            "do with it.<br><br>Tip: drop mod archives onto the list to install "
                            "them.</p>")
        self.empty.setWordWrap(True)
        self.empty.setAlignment(Qt.AlignTop | Qt.AlignLeft)
        lay.addWidget(self.empty)

        self.content = QWidget()
        c = QVBoxLayout(self.content)
        c.setContentsMargins(0, 0, 0, 0)
        self.picture = QLabel()
        self.picture.setAlignment(Qt.AlignCenter)
        self.picture.setFixedHeight(190)
        self.picture.setStyleSheet("background: #1d2026; border-radius: 6px; color: #888;")
        c.addWidget(self.picture)
        self.title = QLabel()
        f = self.title.font()
        f.setPointSizeF(f.pointSizeF() * 1.45)
        f.setWeight(QFont.DemiBold)
        self.title.setFont(f)
        self.title.setWordWrap(True)
        c.addWidget(self.title)
        self.subtitle = QLabel()
        self.subtitle.setWordWrap(True)
        self.subtitle.setOpenExternalLinks(True)
        c.addWidget(self.subtitle)
        self.badges = QHBoxLayout()
        self.badges.setSpacing(4)
        c.addLayout(self.badges)

        grid = QGridLayout()
        grid.setSpacing(4)
        self.btn: dict[str, QPushButton] = {}
        buttons = [
            ("toggle", "Disable", lambda: host.set_mods_enabled([self.mod], not self.mod.enabled)),
            ("update", "⬆ Update", lambda: host.update_mod(self.mod)),
            ("reinstall", "Reinstall", lambda: host.reinstall_mod(self.mod)),
            ("remove", "Remove", lambda: host.remove_mods([self.mod])),
            ("nexus", "Open on Nexus", lambda: host.open_mod_page(self.mod)),
            ("folder", "Open folder", lambda: host.open_mod_folder(self.mod)),
            ("endorse", "👍 Endorse", lambda: host.endorse_mod(self.mod)),
            ("rename", "Rename…", lambda: host.rename_mod(self.mod)),
            ("top", "⤒ Load first", lambda: host.move_mod_to(self.mod, 0)),
            ("bottom", "⤓ Load last (wins)", lambda: host.move_mod_to(self.mod, 10 ** 6)),
        ]
        for i, (key, text, fn) in enumerate(buttons):
            b = QPushButton(text)
            b.clicked.connect(lambda _=False, fn=fn: self.mod and fn())
            grid.addWidget(b, i // 2, i % 2)
            self.btn[key] = b
        self.btn["update"].setStyleSheet("QPushButton:enabled { font-weight: 600; color: #2f9e44; }")
        c.addLayout(grid)

        self.tabs = QTabWidget()
        self.tabs.setDocumentMode(True)
        self.overview = QTextBrowser()
        self.overview.setOpenExternalLinks(True)
        self.tabs.addTab(self.overview, "Overview")
        self.files = QTreeWidget()
        self.files.setHeaderLabels(["File", "Size", "Status"])
        self.files.header().setSectionResizeMode(0, QHeaderView.Stretch)
        self.tabs.addTab(self.files, "Files")
        self.conflicts = QWidget()
        cl = QVBoxLayout(self.conflicts)
        cl.setContentsMargins(0, 4, 0, 0)
        self.conf_label = QLabel()
        self.conf_label.setWordWrap(True)
        cl.addWidget(self.conf_label)
        self.conf_tree = QTreeWidget()
        self.conf_tree.setHeaderLabels(["Mod / file", "Winner"])
        self.conf_tree.header().setSectionResizeMode(0, QHeaderView.Stretch)
        cl.addWidget(self.conf_tree, 1)
        r = QHBoxLayout()
        self.win_btn = QPushButton("Make this mod win")
        self.win_btn.setToolTip("Move this mod just below the selected mod")
        self.win_btn.clicked.connect(lambda: self._resolve(True))
        self.lose_btn = QPushButton("Let the other mod win")
        self.lose_btn.setToolTip("Move this mod just above the selected mod")
        self.lose_btn.clicked.connect(lambda: self._resolve(False))
        r.addWidget(self.win_btn)
        r.addWidget(self.lose_btn)
        cl.addLayout(r)
        self.tabs.addTab(self.conflicts, "Conflicts")
        self.plugins = QTreeWidget()
        self.plugins.setHeaderLabels(["Plugin", "Type", "Masters"])
        self.plugins.header().setSectionResizeMode(2, QHeaderView.Stretch)
        self.plugins.setRootIsDecorated(False)
        self.tabs.addTab(self.plugins, "Plugins")

        info = QWidget()
        il = QVBoxLayout(info)
        il.setContentsMargins(0, 4, 0, 0)
        form = QFormLayout()
        self.category = QComboBox()
        self.category.setEditable(True)
        self.category.setInsertPolicy(QComboBox.NoInsert)
        self.category.lineEdit().editingFinished.connect(self._save_category)
        self.category.activated.connect(lambda _: self._save_category())
        form.addRow("Category:", self.category)
        self.notes = QPlainTextEdit()
        self.notes.setPlaceholderText("Your notes about this mod (only stored in Kiln)…")
        self.notes.setMaximumHeight(110)
        self._notes_timer = QTimer(self, singleShot=True, interval=600)
        self._notes_timer.timeout.connect(self._save_notes)
        self.notes.textChanged.connect(lambda: self.mod and self._notes_timer.start())
        form.addRow("Notes:", self.notes)
        il.addLayout(form)
        self.info = QLabel()
        self.info.setWordWrap(True)
        self.info.setTextInteractionFlags(Qt.TextBrowserInteraction)
        self.info.setOpenExternalLinks(False)
        self.info.linkActivated.connect(self._info_link)
        self.info.setAlignment(Qt.AlignTop)
        il.addWidget(self.info, 1)
        self.tabs.addTab(info, "Notes && info")
        self.tabs.setMinimumHeight(330)
        c.addWidget(self.tabs, 1)
        lay.addWidget(self.content, 1)
        self.content.hide()
        self._pic_url = ""
        self._desc_for = None

    # ------------------------------------------------------------------ show
    def show_mod(self, mod: "Mod | None"):
        if self.mod is not None and self._notes_timer.isActive():
            self._notes_timer.stop()
            self._save_notes()
        self.mod = mod
        self.empty.setVisible(mod is None)
        self.content.setVisible(mod is not None)
        if mod is None:
            return
        h, inst = self.host, self.host.inst
        upd = h.updates.get(mod.id)
        self.title.setText(esc(mod.name))
        bits = []
        if mod.version:
            bits.append(f"v{esc(mod.version)}")
        if mod.author:
            bits.append(f"by <b>{esc(mod.author)}</b>")
        if mod.category:
            bits.append(esc(mod.category))
        self.subtitle.setText(" · ".join(bits) or "<i>local mod</i>")

        while self.badges.count():
            w = self.badges.takeAt(0).widget()
            if w:
                w.deleteLater()
        self.badges.addWidget(_badge("Enabled", "#2f9e44") if mod.enabled else _badge("Disabled", "#868e96"))
        if upd:
            self.badges.addWidget(_badge(f"Update: {upd.version or upd.name}", "#e8590c"))
        if mod.collection:
            cname = next((c["name"] for c in inst.collections if c["slug"] == mod.collection),
                         mod.collection)
            self.badges.addWidget(_badge(f"Collection: {cname}", "#7048e8"))
        self.badges.addWidget(_badge("Nexus" if mod.nexus_mod_id else "Local",
                                     "#d9480f" if mod.nexus_mod_id else "#495057"))
        status = h.endorsements.get((inst.game.nexus_domain, mod.nexus_mod_id or -1), "")
        if status == "Endorsed":
            self.badges.addWidget(_badge("Endorsed", "#1971c2"))
        self.badges.addStretch(1)

        self.btn["toggle"].setText("Disable" if mod.enabled else "Enable")
        self.btn["update"].setEnabled(bool(upd))
        self.btn["update"].setToolTip(f"Install {upd.name} {upd.version}" if upd else
                                      "No update known. Use “Check for updates”.")
        self.btn["reinstall"].setEnabled(bool(mod.archive and (inst.downloads_dir / mod.archive).is_file()))
        self.btn["nexus"].setEnabled(bool(mod.nexus_mod_id))
        can_endorse = bool(mod.nexus_mod_id and h.settings.get("nexus_api_key"))
        self.btn["endorse"].setEnabled(can_endorse)
        self.btn["endorse"].setText("👎 Withdraw endorsement" if status == "Endorsed" else "👍 Endorse")
        idx = inst.mods.index(mod)
        self.btn["top"].setEnabled(idx > 0)
        self.btn["bottom"].setEnabled(idx < len(inst.mods) - 1)

        self._fill_picture(mod)
        self._fill_overview(mod)
        self._fill_files(mod)
        self._fill_conflicts(mod)
        self._fill_plugins(mod)
        self._fill_info(mod)

    # ------------------------------------------------------------------ parts
    def _fill_picture(self, mod):
        url = mod.picture
        self._pic_url = url
        art = self.host.game_art.get(self.host.game.id)
        fallback = art.pixmap("header") if art else QPixmap()
        self._set_pic(fallback, dim=True)
        if url:
            self.host.fetch_image(url, lambda pm, u=url: self._pic_url == u and self._set_pic(pm))
        elif mod.nexus_mod_id:
            self.host.mod_info(mod, lambda d, m=mod: self.mod is m and d.picture and (
                setattr(m, "picture", d.picture), self.host.inst.save(), self._fill_picture(m)))

    def _set_pic(self, pm: QPixmap, dim=False):
        if pm.isNull():
            self.picture.setPixmap(QPixmap())
            self.picture.setText("no picture")
            return
        w = max(self.picture.width(), 340)
        pm = pm.scaled(w, 190, Qt.KeepAspectRatioByExpanding, Qt.SmoothTransformation)
        pm = pm.copy((pm.width() - w) // 2, (pm.height() - 190) // 2, w, 190)
        if dim:
            from PySide6.QtGui import QPainter
            p = QPainter(pm)
            p.fillRect(pm.rect(), QColor(0, 0, 0, 150))
            p.end()
        self.picture.setPixmap(pm)

    def _fill_overview(self, mod):
        base = f"<p><i>{esc(mod.summary)}</i></p>" if mod.summary else ""
        if not mod.nexus_mod_id:
            self.overview.setHtml(base or "<p style='color:gray'>Installed from a local file, so "
                                          "there's no Nexus description.</p>")
            return
        self.overview.setHtml(base + "<p style='color:gray'>Loading description…</p>")
        self._desc_for = mod.id

        def got(d: "nexus.ModDetails", m=mod):
            if self._desc_for != m.id:
                return
            if not m.summary and d.summary:
                m.summary = d.summary
            if not m.category and d.category:
                m.category = d.category
                self.host.inst.save()
            stats = (f"<p style='color:gray'>{d.endorsements:,} endorsements · {d.downloads:,} "
                     f"downloads · updated {esc(d.updated)}</p>")
            self.overview.setHtml(f"<p><i>{esc(d.summary)}</i></p>{stats}{d.description_html}")
        self.host.mod_info(mod, got, lambda e: self._desc_for == mod.id and self.overview.setHtml(
            base + f"<p style='color:gray'>Couldn't load the description: {esc(e)}</p>"))

    def _fill_files(self, mod):
        self.files.clear()
        inst = self.host.inst
        root = inst.mod_path(mod)
        desired, conflicts = self.host.deploy_plan()
        folders: dict[str, QTreeWidgetItem] = {}
        total, count = 0, 0
        files = sorted((p for p in root.rglob("*") if p.is_file()), key=lambda p: str(p).lower())
        for p in files[:5000]:
            rel = p.relative_to(root).as_posix()
            parent = None
            parts = rel.split("/")
            for i in range(1, len(parts)):
                key = "/".join(parts[:i])
                if key not in folders:
                    it = QTreeWidgetItem(["(game folder)/" if parts[i - 1] == "__root__"
                                          else parts[i - 1] + "/"])
                    par = folders.get("/".join(parts[:i - 1])) if i > 1 else None
                    (par or self.files.invisibleRootItem()).addChild(it)
                    folders[key] = it
                parent = folders[key]
            size = p.stat().st_size
            total += size
            count += 1
            from .deploy import deploy_rel
            k = deploy_rel(inst, rel).lower()
            win = desired.get(k)
            if win and win[0] != mod.id:
                other = inst.get(win[0])
                st, color = f"overridden by {other.name if other else win[0]}", "#c92a2a"
            elif k in conflicts:
                st, color = "wins conflict", "#2f9e44"
            else:
                st, color = "", None
            it = QTreeWidgetItem([parts[-1], fmt_size(size), st])
            if color:
                it.setForeground(2, QBrush(QColor(color)))
            (parent or self.files.invisibleRootItem()).addChild(it)
        if len(files) > 5000:
            self.files.addTopLevelItem(QTreeWidgetItem([f"… and {len(files) - 5000} more files"]))
        if count < 300:
            self.files.expandAll()
        self.files.resizeColumnToContents(1)
        self.tabs.setTabText(1, f"Files ({count})")
        self._size = (count, total)

    def _fill_conflicts(self, mod):
        self.conf_tree.clear()
        inst = self.host.inst
        desired, conflicts = self.host.deploy_plan()
        per_other: dict[str, list[tuple[str, bool]]] = {}
        for key, ids in conflicts.items():
            if mod.id not in ids:
                continue
            winner = ids[-1]
            rel = desired[key][1]
            for other in dict.fromkeys(ids):
                if other != mod.id:
                    per_other.setdefault(other, []).append((rel, winner))
        wins = loses = 0
        for other, files in sorted(per_other.items(), key=lambda kv: inst.mods.index(inst.get(kv[0]))
                                   if inst.get(kv[0]) else 0):
            om = inst.get(other)
            name = lambda i: "this mod" if i == mod.id else (inst.get(i).name if inst.get(i) else i)
            n_win = sum(1 for _, w in files if w == mod.id)
            winners = {w for _, w in files}
            top = QTreeWidgetItem([f"{om.name if om else other}  ({len(files)} file"
                                   f"{'' if len(files) == 1 else 's'})",
                                   name(next(iter(winners))) if len(winners) == 1 else "mixed"])
            top.setData(0, Qt.UserRole, other)
            top.setForeground(1, QBrush(QColor("#2f9e44" if n_win == len(files) else "#c92a2a")))
            for rel, w in sorted(files)[:500]:
                ch = QTreeWidgetItem([rel, name(w)])
                ch.setForeground(1, QBrush(QColor("#2f9e44" if w == mod.id else "#c92a2a")))
                top.addChild(ch)
            self.conf_tree.addTopLevelItem(top)
            wins += n_win == len(files)
            loses += n_win != len(files)
        self.conf_tree.resizeColumnToContents(1)
        if per_other:
            self.conf_label.setText(f"Shares files with {len(per_other)} mod(s). Mods lower in the "
                                    "list win. Select a mod below to choose which one wins.")
        else:
            self.conf_label.setText("No file conflicts with other mods.")
        self.tabs.setTabText(2, f"Conflicts ({len(per_other)})" if per_other else "Conflicts")
        self.win_btn.setEnabled(bool(per_other))
        self.lose_btn.setEnabled(bool(per_other))

    def _resolve(self, win: bool):
        it = self.conf_tree.currentItem()
        while it and it.parent():
            it = it.parent()
        if not it and self.conf_tree.topLevelItemCount() == 1:
            it = self.conf_tree.topLevelItem(0)
        if not it:
            self.conf_label.setText("Select which mod to compare with first.")
            return
        inst = self.host.inst
        other = inst.get(it.data(0, Qt.UserRole))
        if not other or not self.mod:
            return
        mods = [m for m in inst.mods if m is not self.mod]
        oi = mods.index(other)
        # remember the choice so auto-sort keeps it
        self.mod.rules = [r for r in self.mod.rules if r.get("mod") != other.id]
        other.rules = [r for r in other.rules if r.get("mod") != self.mod.id]
        self.mod.rules.append({"type": "after" if win else "before", "mod": other.id})
        self.host.move_mod_to(self.mod, oi + 1 if win else oi)

    def _fill_plugins(self, mod):
        inst = self.host.inst
        show = inst.game.has_plugins
        self.tabs.setTabVisible(3, show)
        self.plugins.clear()
        if not show:
            return
        from .games.bethesda import PLUGIN_EXTS, read_plugin
        root = inst.mod_path(mod)
        state = {p["name"].lower(): p["enabled"] for p in inst.plugins}
        n = 0
        for p in sorted(root.iterdir(), key=lambda p: p.name.lower()) if root.is_dir() else []:
            if p.is_file() and p.suffix.lower() in PLUGIN_EXTS:
                info = read_plugin(p)
                kind = "ESL" if info.is_light else ("ESM" if info.is_master else "ESP")
                on = state.get(p.name.lower())
                it = QTreeWidgetItem([p.name + ("" if on is None else ("" if on else "  (disabled)")),
                                      kind, ", ".join(info.masters)])
                self.plugins.addTopLevelItem(it)
                n += 1
        self.plugins.resizeColumnToContents(0)
        self.tabs.setTabText(3, f"Plugins ({n})" if n else "Plugins")

    def _fill_info(self, mod):
        inst = self.host.inst
        self.category.blockSignals(True)
        self.category.clear()
        cats = sorted({m.category for m in inst.mods if m.category}, key=str.lower)
        self.category.addItems(cats)
        self.category.setCurrentText(mod.category)
        self.category.blockSignals(False)
        self.notes.blockSignals(True)
        self.notes.setPlainText(mod.notes)
        self.notes.blockSignals(False)
        from datetime import datetime
        count, total = getattr(self, "_size", (0, 0))
        rows = [("Priority", f"{inst.mods.index(mod) + 1} of {len(inst.mods)}"),
                ("Installed", datetime.fromtimestamp(mod.installed_at).strftime("%Y-%m-%d %H:%M")
                 if mod.installed_at else ""),
                ("Size", f"{fmt_size(total)} in {count} files"),
                ("Nexus file", esc(mod.file_name)),
                ("Archive", esc(mod.archive)),
                ("Nexus IDs", f"mod {mod.nexus_mod_id}, file {mod.nexus_file_id}" if mod.nexus_mod_id else ""),
                ("Installer", f"FOMOD, {len(mod.fomod_choices)} saved choice(s)" if mod.fomod_choices else ""),
                ("Your rules", "<br>".join(
                    f"loads {r['type']} {esc(inst.get(r['mod']).name)}" for r in mod.rules
                    if inst.get(r.get("mod", ""))) +
                 " &nbsp;<a href='kiln:clear-rules'>clear</a>" if mod.rules else ""),
                ("Profiles", ", ".join(esc(n) for n in inst.profile_names()
                                      if mod.id in inst.profiles[n].get("enabled", []))
                 or "enabled in no profile"),
                ("Staging", f"<a href='file://{esc(inst.mod_path(mod))}'>{esc(inst.mod_path(mod))}</a>")]
        self.info.setText("<table cellspacing=3>" + "".join(
            f"<tr><td style='color:gray'>{k}</td><td>{v}</td></tr>" for k, v in rows if v) + "</table>")

    def _info_link(self, url: str):
        if url == "kiln:clear-rules" and self.mod:
            self.mod.rules = []
            self.host.inst.save()
            self.show_mod(self.mod)
        else:
            from PySide6.QtCore import QUrl
            from PySide6.QtGui import QDesktopServices
            QDesktopServices.openUrl(QUrl(url))

    def _save_category(self):
        if self.mod and self.category.currentText().strip() != self.mod.category:
            self.mod.category = self.category.currentText().strip()
            self.host.inst.save()
            self.host.refresh_mods(keep_panel=True)

    def _save_notes(self):
        if self.mod and self.notes.toPlainText() != self.mod.notes:
            self.mod.notes = self.notes.toPlainText()
            self.host.inst.save()
