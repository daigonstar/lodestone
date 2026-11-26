"""Dashboard: your games, the current game at a glance, things that need
attention, quick actions, recent installs and what's new on Nexus."""
from __future__ import annotations

import os
import time
from typing import TYPE_CHECKING

from PySide6.QtCore import QRectF, QSize, Qt, Signal
from PySide6.QtGui import QColor, QFont, QLinearGradient, QPainter, QPainterPath, QPixmap
from PySide6.QtWidgets import (QFrame, QGridLayout, QHBoxLayout, QLabel, QPushButton, QScrollArea,
                               QSizePolicy, QVBoxLayout, QWidget)

from . import deploy

if TYPE_CHECKING:
    from .gui import MainWindow

CARD_CSS = ("QFrame#card { background: palette(base); border: 1px solid palette(mid);"
            " border-radius: 8px; }")


def _card(title: str = "") -> tuple[QFrame, QVBoxLayout]:
    f = QFrame()
    f.setObjectName("card")
    f.setStyleSheet(CARD_CSS)
    lay = QVBoxLayout(f)
    lay.setContentsMargins(14, 10, 14, 12)
    if title:
        t = QLabel(title)
        ft = t.font()
        ft.setPointSizeF(ft.pointSizeF() * 1.15)
        ft.setWeight(QFont.DemiBold)
        t.setFont(ft)
        lay.addWidget(t)
    return f, lay


def _size(path) -> int:
    total = 0
    for root, _dirs, files in os.walk(path):
        for f in files:
            try:
                total += os.lstat(os.path.join(root, f)).st_size
            except OSError:
                pass
    return total


def fmt_size(n: float) -> str:
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if n < 1024 or unit == "TB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024
    return ""


def ago(ts: float) -> str:
    if not ts:
        return "never"
    d = time.time() - ts
    for secs, unit in ((86400, "day"), (3600, "hour"), (60, "minute")):
        if d >= secs:
            n = int(d // secs)
            return f"{n} {unit}{'s' if n > 1 else ''} ago"
    return "just now"


class GameCard(QWidget):
    """Box art with the game's name and mod count; click to switch game."""
    clicked = Signal()

    def __init__(self, art: QPixmap, name: str, status: str, current: bool, found: bool):
        super().__init__()
        self.art, self.name, self.status = art, name, status
        self.current, self.found = current, found
        self.setFixedSize(150, 225)
        self.setCursor(Qt.PointingHandCursor)
        self.setToolTip(f"{name}\n{status}")

    def mouseReleaseEvent(self, e):
        if e.button() == Qt.LeftButton:
            self.clicked.emit()

    def paintEvent(self, _e):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        p.setRenderHint(QPainter.SmoothPixmapTransform)
        r = QRectF(self.rect()).adjusted(2, 2, -2, -2)
        path = QPainterPath()
        path.addRoundedRect(r, 8, 8)
        p.setClipPath(path)
        if not self.art.isNull():
            pm = self.art.scaled(self.size(), Qt.KeepAspectRatioByExpanding, Qt.SmoothTransformation)
            p.drawPixmap(0, 0, pm)
        else:
            p.fillRect(r, QColor("#2b2f38"))
            p.setPen(QColor("white"))
            p.drawText(r.adjusted(8, 8, -8, -60), Qt.AlignCenter | Qt.TextWordWrap, self.name)
        if not self.found:
            p.fillRect(r, QColor(0, 0, 0, 150))
        g = QLinearGradient(0, r.height() - 70, 0, r.height())
        g.setColorAt(0, QColor(0, 0, 0, 0))
        g.setColorAt(1, QColor(0, 0, 0, 220))
        p.fillRect(r, g)
        p.setPen(QColor("white"))
        f = p.font()
        f.setPointSizeF(f.pointSizeF() * 0.9)
        p.setFont(f)
        p.drawText(r.adjusted(8, 0, -8, -8), Qt.AlignBottom | Qt.AlignLeft | Qt.TextWordWrap, self.status)
        p.setClipping(False)
        if self.current:
            p.setPen(QColor("#ff9a3c"))
            pen = p.pen()
            pen.setWidth(3)
            p.setPen(pen)
            p.drawPath(path)


class Tile(QFrame):
    def __init__(self, label: str):
        super().__init__()
        self.setObjectName("card")
        self.setStyleSheet(CARD_CSS)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(12, 8, 12, 8)
        self.value = QLabel("–")
        f = self.value.font()
        f.setPointSizeF(f.pointSizeF() * 1.8)
        f.setWeight(QFont.DemiBold)
        self.value.setFont(f)
        self.label = QLabel(label)
        self.label.setStyleSheet("color: gray;")
        lay.addWidget(self.value)
        lay.addWidget(self.label)

    def set(self, value, color: str | None = None):
        self.value.setText(str(value))
        self.value.setStyleSheet(f"color: {color};" if color else "")


class Dashboard(QScrollArea):
    def __init__(self, host: "MainWindow"):
        super().__init__()
        self.host = host
        self.setWidgetResizable(True)
        self.setFrameShape(QFrame.NoFrame)
        body = QWidget()
        self.setWidget(body)
        self.lay = QVBoxLayout(body)
        self.lay.setContentsMargins(4, 0, 8, 8)
        self.lay.setSpacing(12)

        games, gl = _card("Your games")
        self.games_row = QHBoxLayout()
        self.games_row.setSpacing(10)
        gl.addLayout(self.games_row)
        self.lay.addWidget(games)

        self.tiles: dict[str, Tile] = {}
        tiles = QGridLayout()
        tiles.setSpacing(10)
        for i, (k, label) in enumerate((("mods", "mods installed"), ("enabled", "enabled in profile"),
                                        ("updates", "updates available"), ("conflicts", "mods losing conflicts"),
                                        ("plugins", "plugins active"), ("size", "staging size"),
                                        ("profiles", "profiles"), ("deployed", "last deployed"))):
            t = self.tiles[k] = Tile(label)
            tiles.addWidget(t, i // 4, i % 4)
        self.lay.addLayout(tiles)

        mid = QHBoxLayout()
        mid.setSpacing(12)
        att, self.att_lay = _card("Needs attention")
        self.att_body = QVBoxLayout()
        self.att_lay.addLayout(self.att_body)
        self.att_lay.addStretch(1)
        mid.addWidget(att, 3)
        act, al = _card("Quick actions")
        grid = QGridLayout()
        for i, (text, fn) in enumerate((
                ("▶  Play", host.launch), ("Deploy", host.run_deploy),
                ("⇅  Auto-sort load order", host.auto_sort), ("Check for updates", host.check_updates),
                ("Browse Nexus Mods", lambda: host.go_to(host.nexus_tab)),
                ("Collections", lambda: host.go_to(host.coll_tab)),
                ("Open Nexus in Kiln's browser", lambda: host.open_browser()),
                ("Set up essentials…", host.open_setup), ("Install from file…", host.install_from_file))):
            b = QPushButton(text)
            b.setMinimumHeight(34)
            b.clicked.connect(lambda _=False, fn=fn: fn())
            grid.addWidget(b, i // 2, i % 2)
        al.addLayout(grid)
        al.addStretch(1)
        mid.addWidget(act, 2)
        self.lay.addLayout(mid)

        bottom = QHBoxLayout()
        bottom.setSpacing(12)
        rec, rl = _card("Recently installed")
        self.recent = QGridLayout()
        self.recent.setSpacing(8)
        rl.addLayout(self.recent)
        rl.addStretch(1)
        bottom.addWidget(rec, 1)
        new, nl = _card("Most popular on Nexus")
        self.trend = QGridLayout()
        self.trend.setSpacing(8)
        nl.addLayout(self.trend)
        nl.addStretch(1)
        bottom.addWidget(new, 1)
        self.lay.addLayout(bottom)
        self.lay.addStretch(1)
        self._trend_for = None
        self._size_gen = 0

    # ------------------------------------------------------------------ refresh
    def refresh(self):
        h = self.host
        self._fill_games()
        inst = h.inst
        if not inst:
            for t in self.tiles.values():
                t.set("–")
            self._clear(self.att_body)
            self.att_body.addWidget(QLabel(f"<b>{h.game.name} wasn't found.</b> Use “Folders ▾” → "
                                           "“Find its folder…” in the banner to point Kiln at it."))
            self._clear(self.recent)
            return
        conf = deploy.conflict_summary(inst)
        losing = [m for m in inst.mods if m.enabled and conf.get(m.id, {}).get("loses")]
        ups = [m for m in inst.mods if m.id in h.updates]
        self.tiles["mods"].set(len(inst.mods))
        self.tiles["enabled"].set(sum(m.enabled for m in inst.mods))
        self.tiles["updates"].set(len(ups), "#e8590c" if ups else None)
        self.tiles["conflicts"].set(len(losing), "#c92a2a" if losing else None)
        self.tiles["plugins"].set(sum(p["enabled"] for p in inst.plugins) if inst.game.has_plugins else "n/a")
        self.tiles["profiles"].set(len(inst.profiles))
        mf = inst.staging / deploy.MANIFEST
        self.tiles["deployed"].set(ago(mf.stat().st_mtime) if mf.exists() and deploy.is_deployed(inst)
                                   else "not deployed",
                                   None if deploy.is_deployed(inst) else "#e8590c")
        self.tiles["deployed"].value.setStyleSheet(self.tiles["deployed"].value.styleSheet()
                                                   + "font-size: 15px;")
        self._size_gen += 1
        gen = self._size_gen
        from .gui import Task
        Task(_size, inst.mods_dir).start(lambda n: gen == self._size_gen and self.tiles["size"].set(fmt_size(n)))
        self._fill_attention(inst, losing, ups)
        self._fill_recent(inst)
        self._fill_trending()

    def _clear(self, layout):
        while layout.count():
            it = layout.takeAt(0)
            if it.widget():
                it.widget().deleteLater()
            elif it.layout():
                self._clear(it.layout())

    def _fill_games(self):
        h = self.host
        self._clear(self.games_row)
        hidden = 0
        for i, g in enumerate(h.games):
            found = h._detect(g) is not None
            if not found and g.id != h.game.id:
                hidden += 1           # supported but not installed: keep the row short
                continue
            inst = h.instances.get(g.id)
            if inst:
                status = f"{len(inst.mods)} mods · {inst.active_profile}"
            else:
                status = "ready" if found else "not found"
            art = h.game_art[g.id]
            pm = art.pixmap("capsule")
            card = GameCard(pm, g.name, f"{g.name}\n{status}", g.id == h.game.id, found)
            card.clicked.connect(lambda i=i: h.game_combo.setCurrentIndex(i))
            self.games_row.addWidget(card)
        if hidden:
            more = QLabel(f"+ {hidden} more supported games.\nInstall one in Steam and it shows up "
                          "here; or pick it from the game list.")
            more.setWordWrap(True)
            more.setMaximumWidth(220)
            more.setStyleSheet("color: gray;")
            self.games_row.addWidget(more)
        self.games_row.addStretch(1)

    def _attention(self, text: str, button: str = "", fn=None, color="#e8590c"):
        row = QHBoxLayout()
        dot = QLabel("●")
        dot.setStyleSheet(f"color: {color};")
        row.addWidget(dot)
        lab = QLabel(text)
        lab.setWordWrap(True)
        row.addWidget(lab, 1)
        if button:
            b = QPushButton(button)
            b.clicked.connect(lambda _=False: fn())
            row.addWidget(b)
        self.att_body.addLayout(row)

    def _fill_attention(self, inst, losing, ups):
        h = self.host
        self._clear(self.att_body)
        n = 0
        if not h.settings.get("nexus_api_key"):
            self._attention("No Nexus API key, so Kiln can't download. Add your key in Settings.",
                            "Settings", lambda: h.go_to(h.settings_tab)); n += 1
        # Troubleshoot's full check reads every mod file: run it in the background and
        # put its verdict at the top when it's done.
        self._health_row = QHBoxLayout()
        self.att_body.addLayout(self._health_row)
        self._health_note = QLabel("<span style='color:gray'>Checking your setup…</span>")
        self._health_row.addWidget(self._health_note)
        self._check_health(inst)
        if inst.mods and not deploy.is_deployed(inst):
            self._attention("Your mods aren't deployed to the game folder.", "Deploy", h.run_deploy); n += 1
        if ups:
            self._attention(f"{len(ups)} mod update(s) available: "
                            + ", ".join(m.name for m in ups[:3]) + ("…" if len(ups) > 3 else ""),
                            "Update all", h.update_all); n += 1
        miss = inst.missing_master_warnings() if inst.game.has_plugins else []
        if miss:
            self._attention(f"{len(miss)} plugin(s) are missing a master. The game may crash on load.",
                            "Plugins", lambda: h.go_to(h.plugins_tab), "#c92a2a"); n += 1
        if losing:
            self._attention(f"{len(losing)} enabled mod(s) have files overridden by others. Check "
                            "that's what you want.", "Show",
                            lambda: (h.go_to(h.mods_tab),
                                     h.mod_filter.setCurrentIndex(h.mod_filter.findData("conflict"))),
                            "#fab005"); n += 1
        from . import essentials
        miss = essentials.missing(inst)
        req = [e for e in miss if e.required]
        if miss:
            self._attention(
                (f"{len(req)} required essential(s) missing: " if req else
                 f"{len(miss)} recommended essential(s) not installed: ")
                + ", ".join(e.name for e in (req or miss)[:3]),
                "Set up", h.open_setup, "#c92a2a" if req else "#4dabf7"); n += 1
        from . import importers
        am = importers.amethyst_candidates(inst)
        if am:
            self._attention(f"{len(am)} mod(s) for this game are in Amethyst Mod Manager: "
                            + ", ".join(p.name for p in am[:3]) + ("…" if len(am) > 3 else ""),
                            "Import", h.import_amethyst, "#4dabf7"); n += 1
        # how many mods auto-sort would move: worked out in the background
        from . import autosort
        from .gui import Task
        sort_row = QHBoxLayout()
        self.att_body.addLayout(sort_row)
        gen = getattr(self, "_health_gen", 0)

        def sorted_done(moved):
            if not moved or gen != getattr(self, "_health_gen", 0) or inst is not h.inst:
                return
            dot = QLabel("●")
            dot.setStyleSheet("color: #4dabf7;")
            lab = QLabel(f"Auto-sort would move {moved} mod(s) into a better order.")
            lab.setWordWrap(True)
            b = QPushButton("Review")
            b.clicked.connect(lambda _=False: h.auto_sort())
            for wdg, st in ((dot, 0), (lab, 1), (b, 0)):
                sort_row.addWidget(wdg, st)

        Task(lambda: autosort.sort_mods(inst).moved([m.id for m in inst.mods])).start(
            sorted_done, lambda e: None)
        # (with nothing else to show, the health check's own line says all is well)

    def _check_health(self, inst):
        from . import doctor
        from .gui import Task
        h = self.host
        self._health_gen = getattr(self, "_health_gen", 0) + 1
        gen = self._health_gen
        ignored = set(h.settings.game(inst.game.id).get("doctor_ignored", []))

        def done(issues):
            if gen != self._health_gen or inst is not h.inst:
                return
            try:
                self._health_note.deleteLater()
            except RuntimeError:
                return
            level, sentence = doctor.verdict(issues)
            color = {"crash": "#c92a2a", "broken": "#e8590c", "warn": "#fab005"}.get(level, "#2f9e44")
            row = self._health_row
            dot = QLabel("●")
            dot.setStyleSheet(f"color: {color};")
            row.addWidget(dot)
            lab = QLabel(("Troubleshoot: " + sentence) if level != "ok" else sentence)
            lab.setWordWrap(True)
            row.addWidget(lab, 1)
            if level != "ok":
                b = QPushButton("Troubleshoot")
                b.clicked.connect(lambda _=False: h.go_to(h.doctor_tab))
                row.addWidget(b)
                h.doctor_tab.issues = issues            # the page opens with these straight away
                h.doctor_tab.checked_at = __import__("time").time()

        Task(doctor.diagnose, inst, ignored).start(done, lambda e: None)

    def _thumb_button(self, title: str, sub: str, url: str, fn) -> QWidget:
        w = QPushButton()
        w.setFlat(True)
        w.setCursor(Qt.PointingHandCursor)
        w.setMinimumHeight(64)
        w.setSizePolicy(QSizePolicy.Expanding, QSizePolicy.Fixed)
        lay = QHBoxLayout(w)
        lay.setContentsMargins(4, 2, 4, 2)
        pic = QLabel()
        pic.setFixedSize(QSize(96, 54))
        pic.setStyleSheet("background: #1d2026; border-radius: 4px;")
        lay.addWidget(pic)
        txt = QLabel(f"<b>{title}</b><br><span style='color:gray'>{sub}</span>")
        txt.setWordWrap(True)
        txt.setAttribute(Qt.WA_TransparentForMouseEvents)
        lay.addWidget(txt, 1)
        w.clicked.connect(fn)
        if url:
            def got(pm, pic=pic):
                try:
                    pic.setPixmap(pm.scaled(96, 54, Qt.KeepAspectRatioByExpanding, Qt.SmoothTransformation)
                                  .copy(0, 0, 96, 54))
                except RuntimeError:
                    pass
            self.host.fetch_image(url, got)
        return w

    def _fill_recent(self, inst):
        self._clear(self.recent)
        mods = sorted(inst.mods, key=lambda m: m.installed_at, reverse=True)[:6]
        if not mods:
            self.recent.addWidget(QLabel("<span style='color:gray'>Nothing installed yet. Browse Nexus "
                                         "or a collection to get started.</span>"), 0, 0)
        for i, m in enumerate(mods):
            sub = f"{m.category or ('Nexus' if m.nexus_mod_id else 'local file')} · {ago(m.installed_at)}"
            self.recent.addWidget(self._thumb_button(m.name, sub, m.picture,
                                                     lambda _=False, m=m: self.host.show_mod(m)), i, 0)

    def _fill_trending(self):
        h = self.host
        g = h.game
        if self._trend_for == g.id:
            return
        self._trend_for = g.id
        self._clear(self.trend)
        self.trend.addWidget(QLabel("<span style='color:gray'>Loading…</span>"), 0, 0)
        from . import nexus
        from .gui import Task

        def done(res, gid=g.id):
            if h.game.id != gid:
                return
            self._clear(self.trend)
            mods, _total = res
            installed = {m.nexus_mod_id for m in h.inst.mods} if h.inst else set()
            for i, s in enumerate(mods[:6]):
                sub = (f"{s.category} · {s.endorsements:,} endorsements"
                       + (" · ✔ installed" if s.mod_id in installed else ""))
                self.trend.addWidget(self._thumb_button(s.name, sub, s.thumb,
                                                        lambda _=False, s=s: h.show_nexus_mod(s)), i, 0)

        def fail(e):
            self._trend_for = None
            self._clear(self.trend)
            self.trend.addWidget(QLabel(f"<span style='color:gray'>Couldn't reach Nexus: {e}</span>"), 0, 0)
        Task(nexus.search, g.nexus_domain, "", "downloads", 1, 6,
             bool(h.settings.get("show_adult", False))).start(done, fail)
