"""The game banner at the top of Kiln's window, plus game artwork loading."""
from __future__ import annotations

from pathlib import Path

from PySide6.QtCore import QRectF, Qt, Signal
from PySide6.QtGui import QColor, QIcon, QLinearGradient, QPainter, QPainterPath, QPixmap
from PySide6.QtWidgets import (QComboBox, QHBoxLayout, QLabel, QPushButton, QSizePolicy,
                               QVBoxLayout, QWidget)

from . import steam
from .paths import cache_dir


def _pixmap(path: Path | None) -> QPixmap:
    if not path:
        return QPixmap()
    pm = QPixmap(str(path))
    if pm.isNull():                       # formats Qt lacks (e.g. WebP) go through Pillow
        from .gui import load_pixmap
        try:
            pm = load_pixmap(Path(path).read_bytes())
        except OSError:
            pass
    return pm


class GameArt:
    """Artwork for one game: Steam's local cache first, then a cached CDN copy."""

    def __init__(self, appid: int):
        self.appid = appid
        self.files = steam.art(appid)
        cdir = cache_dir() / "art" / str(appid)
        for kind in ("header", "hero", "logo", "capsule"):
            f = cdir / kind
            if kind not in self.files and f.is_file() and f.stat().st_size:
                self.files[kind] = f

    def missing_remote(self) -> dict[str, str]:
        urls = steam.art_cdn_urls(self.appid)
        return {k: u for k, u in urls.items() if k not in self.files}

    def fetch_missing(self) -> "GameArt":
        """Download artwork Steam hasn't cached (runs on a worker thread)."""
        from . import nexus
        cdir = cache_dir() / "art" / str(self.appid)
        cdir.mkdir(parents=True, exist_ok=True)
        for kind, url in self.missing_remote().items():
            f = cdir / kind
            try:
                f.write_bytes(nexus.fetch_bytes(url))
                self.files[kind] = f
            except Exception:  # noqa: BLE001  (art is optional)
                f.write_bytes(b"")     # remember the miss
        return self

    def pixmap(self, kind: str) -> QPixmap:
        return _pixmap(self.files.get(kind))

    def icon(self) -> QIcon:
        pm = self.pixmap("icon")
        if pm.isNull():
            cap = self.pixmap("capsule")
            if cap.isNull():
                cap = self.pixmap("header")
            if not cap.isNull():
                side = min(cap.width(), cap.height())
                pm = cap.copy((cap.width() - side) // 2, 0, side, side)
        return QIcon(pm) if not pm.isNull() else QIcon()


class GameHeader(QWidget):
    """Banner: the game's hero art behind its logo, with the game switcher,
    status line and the main actions (Play, Deploy, Purge)."""
    play = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setMinimumHeight(150)
        self.setSizePolicy(QSizePolicy.Expanding, QSizePolicy.Fixed)
        self.hero = QPixmap()
        self.logo = QPixmap()

        self.combo = QComboBox()
        self.combo.setIconSize(self.combo.iconSize() * 1.5)
        self.combo.setMinimumWidth(260)
        self.profile_combo = QComboBox()
        self.profile_combo.setMinimumWidth(170)
        self.profile_combo.setToolTip("Profile: which mods are enabled, their order and the plugin "
                                      "load order. Switching redeploys the game.")
        from PySide6.QtWidgets import QToolButton
        self.profile_btn = QToolButton(text="Profiles ▾")
        self.profile_btn.setPopupMode(QToolButton.InstantPopup)
        self.profile_btn.setStyleSheet("QToolButton { background: rgba(0,0,0,.45); color: white;"
                                       " border: 1px solid rgba(255,255,255,.35); border-radius: 4px;"
                                       " padding: 3px 8px; }")
        self.title = QLabel()
        self.title.setStyleSheet("color: white; font-size: 22px; font-weight: 600;")
        self.status = QLabel()
        self.status.setStyleSheet("color: rgba(255,255,255,.85);")
        self.status.setWordWrap(True)
        self.status.setTextInteractionFlags(Qt.TextSelectableByMouse)
        self.stats = QLabel()
        self.stats.setStyleSheet("color: rgba(255,255,255,.85);")
        for lab in (self.status, self.stats):      # long paths mustn't squeeze the buttons
            lab.setSizePolicy(QSizePolicy.Ignored, QSizePolicy.Preferred)
            lab.setMinimumWidth(0)

        self.play_btn = QPushButton("▶  Play")
        self.play_btn.setMinimumSize(130, 42)
        self.play_btn.setStyleSheet(
            "QPushButton { background: #2f9e44; color: white; font-size: 16px; font-weight: 600;"
            " border-radius: 6px; padding: 6px 18px; }"
            "QPushButton:hover { background: #37b24d; } QPushButton:disabled { background: #555; }")
        self.play_btn.clicked.connect(self.play)
        self.deploy_btn = QPushButton("Deploy")
        self.purge_btn = QPushButton("Purge")
        self.folder_btn = QPushButton("Folders ▾")
        self.folder_btn.setToolTip("Open the game's folders (game, Data, mods, saves, logs), or change "
                                   "which folder Kiln uses for this game")
        self.ess_btn = QPushButton("Essentials")
        self.ess_btn.hide()
        self.ver_btn = QPushButton("Version")
        self.ver_btn.hide()
        self.set_btn = QPushButton("⚙ Game settings")
        self.set_btn.setToolTip("Resolution, fullscreen and graphics quality (what the game's own launcher sets)")
        self.set_btn.hide()
        for b in (self.deploy_btn, self.purge_btn, self.folder_btn, self.ess_btn, self.ver_btn, self.set_btn):
            b.setStyleSheet("QPushButton { background: rgba(0,0,0,.45); color: white; border: 1px solid"
                            " rgba(255,255,255,.35); border-radius: 4px; padding: 5px 12px; }"
                            "QPushButton:hover { background: rgba(255,255,255,.18); }"
                            "QPushButton:disabled { color: rgba(255,255,255,.4); }"
                            "QPushButton[alert=\"required\"] { background: #c92a2a; border-color: #ff8787; }"
                            "QPushButton[alert=\"recommended\"] { background: #1c7ed6; border-color: #74c0fc; }")

        self.logo_space = QWidget()
        self.logo_space.setFixedWidth(250)
        self.logo_space.setAttribute(Qt.WA_TransparentForMouseEvents)
        info = QVBoxLayout()
        top = QHBoxLayout()
        top.addWidget(self.combo)
        plabel = QLabel("Profile:")
        plabel.setStyleSheet("color: white; margin-left: 10px;")
        top.addWidget(plabel)
        top.addWidget(self.profile_combo)
        top.addWidget(self.profile_btn)
        top.addStretch(1)
        info.addLayout(top)
        info.addStretch(1)
        info.addWidget(self.title)
        info.addWidget(self.stats)
        info.addWidget(self.status)
        actions = QVBoxLayout()
        actions.addWidget(self.play_btn, 0, Qt.AlignRight)
        actions.addStretch(1)
        row = QHBoxLayout()
        for b in (self.set_btn, self.ver_btn, self.ess_btn, self.folder_btn, self.purge_btn, self.deploy_btn):
            row.addWidget(b)
        actions.addLayout(row)
        lay = QHBoxLayout(self)
        lay.setContentsMargins(16, 12, 16, 12)
        lay.addWidget(self.logo_space)
        lay.addLayout(info, 1)
        lay.addLayout(actions)

    def set_art(self, art: GameArt | None, title: str):
        self.hero = art.pixmap("hero") if art else QPixmap()
        if self.hero.isNull() and art:
            self.hero = art.pixmap("header")
        self.logo = art.pixmap("logo") if art else QPixmap()
        self.title.setText(title)
        self.title.setVisible(self.logo.isNull())
        self._fit_logo()
        self.update()

    def _fit_logo(self):
        """The game logo takes 250px: drop it when the window is too narrow for it
        and the buttons."""
        self.logo_space.setVisible(not self.logo.isNull() and self.width() >= 1250)
        self.title.setVisible(not self.logo_space.isVisible())

    def resizeEvent(self, ev):
        super().resizeEvent(ev)
        self._fit_logo()

    def paintEvent(self, _ev):
        p = QPainter(self)
        p.setRenderHint(QPainter.SmoothPixmapTransform)
        p.setRenderHint(QPainter.Antialiasing)
        r = self.rect()
        clip = QPainterPath()
        clip.addRoundedRect(QRectF(r), 8, 8)
        p.setClipPath(clip)
        if not self.hero.isNull():
            pm = self.hero.scaled(r.size(), Qt.KeepAspectRatioByExpanding, Qt.SmoothTransformation)
            p.drawPixmap(0, 0, pm, (pm.width() - r.width()) // 2, (pm.height() - r.height()) // 3,
                         r.width(), r.height())
        else:
            p.fillRect(r, QColor("#2b2f38"))
        g = QLinearGradient(0, 0, r.width(), 0)
        g.setColorAt(0, QColor(0, 0, 0, 200))
        g.setColorAt(0.55, QColor(0, 0, 0, 120))
        g.setColorAt(1, QColor(0, 0, 0, 170))
        p.fillRect(r, g)
        if not self.logo.isNull() and self.logo_space.isVisible():
            box = self.logo_space.geometry().adjusted(0, 4, -16, -4)
            pm = self.logo.scaled(box.size(), Qt.KeepAspectRatio, Qt.SmoothTransformation)
            p.drawPixmap(box.x() + (box.width() - pm.width()) // 2,
                         box.y() + (box.height() - pm.height()) // 2, pm)
