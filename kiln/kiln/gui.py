"""Kiln main window (PySide6)."""
from __future__ import annotations

import json
import os
import re
import shutil
import sys
import time
import traceback
from datetime import datetime
from pathlib import Path

from PySide6.QtCore import QObject, QRunnable, QSize, Qt, QThreadPool, QTimer, QUrl, Signal
from PySide6.QtGui import QBrush, QColor, QDesktopServices, QIcon, QPixmap
from PySide6.QtNetwork import QLocalServer, QLocalSocket
from PySide6.QtWidgets import (
    QAbstractItemView, QApplication, QCheckBox, QComboBox, QFileDialog, QFormLayout, QFrame,
    QGroupBox, QHBoxLayout, QHeaderView, QLabel, QLineEdit, QListWidget, QListWidgetItem,
    QMainWindow, QMessageBox, QPushButton, QSplitter, QTabWidget, QTextBrowser, QTreeWidget,
    QTreeWidgetItem, QVBoxLayout, QWidget,
)

from . import APP_NAME, __version__, deploy, desktop, fomod, nexus, steam
from .archive import ci_resolve
from .games import GameDef, all_games, by_domain
from .instance import Instance, Mod, Prepared
from .paths import Settings, cache_dir

SERVER_NAME = f"kiln-{os.getuid()}"


# --------------------------------------------------------------------------
# background tasks
# --------------------------------------------------------------------------
class _Signals(QObject):
    done = Signal(object)
    error = Signal(str)
    progress = Signal(int, int)


class Task(QRunnable):
    """Run fn(*args) on a thread pool. progress=True passes progress= and
    cancelled= keyword arguments to fn."""
    _live: set["Task"] = set()

    def __init__(self, fn, *args, progress: bool = False, **kw):
        super().__init__()
        self.setAutoDelete(False)
        self.fn, self.args, self.kw = fn, args, kw
        self.sig = _Signals()
        self.cancel = False
        if progress:
            self.kw["progress"] = self.sig.progress.emit
            self.kw["cancelled"] = lambda: self.cancel

    def start(self, on_done=None, on_error=None, on_progress=None, pool: QThreadPool | None = None):
        for sig, fn in ((self.sig.done, on_done), (self.sig.error, on_error),
                        (self.sig.progress, on_progress)):
            if fn:
                sig.connect(fn)
        self.sig.done.connect(lambda *_: Task._live.discard(self))
        self.sig.error.connect(lambda *_: Task._live.discard(self))
        Task._live.add(self)
        (pool or QThreadPool.globalInstance()).start(self)
        return self

    def run(self):
        try:
            try:
                res = self.fn(*self.args, **self.kw)
            except Exception as e:  # noqa: BLE001
                traceback.print_exc()
                self.sig.error.emit(str(e) or e.__class__.__name__)
            else:
                self.sig.done.emit(res)
        except RuntimeError:
            pass  # app shutting down


class ModTree(QTreeWidget):
    """Mod list that also accepts archives dragged in from a file manager."""
    files_dropped = Signal(list)

    def __init__(self):
        super().__init__()
        self.setAcceptDrops(True)

    def _external(self, ev) -> bool:
        md = ev.mimeData()
        return md.hasUrls() and ev.source() is None and all(u.isLocalFile() for u in md.urls())

    def dragEnterEvent(self, ev):
        if self._external(ev):
            ev.acceptProposedAction()
        else:
            super().dragEnterEvent(ev)

    def dragMoveEvent(self, ev):
        if self._external(ev):
            ev.acceptProposedAction()
        else:
            super().dragMoveEvent(ev)

    def dropEvent(self, ev):
        if self._external(ev):
            self.files_dropped.emit([u.toLocalFile() for u in ev.mimeData().urls()])
            ev.acceptProposedAction()
        else:
            super().dropEvent(ev)


def load_image(data: bytes, max_side: int = 900):
    """Decode (and shrink) image bytes into a QImage. Safe in a worker thread, so big
    Nexus pictures don't freeze the window."""
    from PySide6.QtGui import QImage
    img = QImage()
    if not img.loadFromData(data):
        try:
            import io
            from PIL import Image
            buf = io.BytesIO()
            Image.open(io.BytesIO(data)).convert("RGBA").save(buf, "PNG")
            img.loadFromData(buf.getvalue())
        except Exception:  # noqa: BLE001
            return img
    if max(img.width(), img.height()) > max_side:
        img = img.scaled(max_side, max_side, Qt.KeepAspectRatio, Qt.SmoothTransformation)
    return img


def load_pixmap(data: bytes) -> QPixmap:
    """QPixmap from image bytes; WebP goes through Pillow when Qt lacks the plugin."""
    pm = QPixmap()
    if pm.loadFromData(data):
        return pm
    try:
        import io
        from PIL import Image
        buf = io.BytesIO()
        Image.open(io.BytesIO(data)).convert("RGBA").save(buf, "PNG")
        pm.loadFromData(buf.getvalue())
    except Exception:  # noqa: BLE001
        pass
    return pm


def fmt_size(n: float) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024 or unit == "GB":
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024
    return str(n)


def fmt_date(ts: int) -> str:
    return datetime.fromtimestamp(ts).strftime("%Y-%m-%d") if ts else ""


def clean_archive_name(name: str) -> str:
    """'SkyUI_5_2_SE-12604-5-2SE-1573...zip' -> 'SkyUI 5 2 SE'"""
    stem = re.sub(r"\.(zip|7z|rar|tar|gz|xz|bz2)$", "", name, flags=re.I)
    stem = re.sub(r"-\d+(-[\w]+)*-\d{9,}$", "", stem)
    return stem.replace("_", " ").strip() or name


# --------------------------------------------------------------------------
# main window
# --------------------------------------------------------------------------
class _NullItem:
    """Stands in for a list row when a download queue runs without a window showing it."""

    def setText(self, *_a):
        pass

    def setToolTip(self, *_a):
        pass

    def setCheckState(self, *_a):
        pass

    def text(self, *_a):
        return ""


class MainWindow(QMainWindow):
    def __init__(self, splash=None):
        super().__init__()
        self._splash = splash
        self._step("Finding your games…")
        self.setWindowTitle(f"{APP_NAME} {__version__}")
        self.resize(1250, 800)
        self.settings = Settings()
        # games you have installed first (in Nexus popularity order), then the rest
        games = all_games()
        found = {g.id for g in games if self._detect(g)}
        self.games = [g for g in games if g.id in found] + [g for g in games if g.id not in found]
        self.instances: dict[str, Instance] = {}
        self.inst: Instance | None = None
        self.user: nexus.User | None = None
        self.updates: dict[str, nexus.NexusFile] = {}
        self.thumbs: dict[str, QIcon] = {}
        self.warnings: list[str] = []
        self.active_downloads: dict[int, QListWidgetItem] = {}
        self.page = 1
        self.search_gen = 0
        # Mutating jobs (install, deploy) run one at a time.
        self.jobs = QThreadPool()
        self.jobs.setMaxThreadCount(1)
        # Downloads: a few at a time. Free accounts are throttled and more
        # parallel connections just make the server drop them.
        self.dl_pool = QThreadPool()
        self.dl_pool.setMaxThreadCount(3)
        self.deploy_timer = QTimer(self, singleShot=True, interval=700)
        self.deploy_timer.timeout.connect(self.run_deploy)

        self._step("Building the window…")
        central = QWidget()
        root = QVBoxLayout(central)
        root.addWidget(self._build_top())
        self.tabs = QTabWidget()
        from .dashboard import Dashboard
        self.dash_tab = Dashboard(self)
        self.tabs.addTab(self.dash_tab, "Dashboard")
        from .doctorpage import DoctorPage
        self.doctor_tab = DoctorPage(self)
        self.tabs.addTab(self.doctor_tab, "Troubleshoot")
        self.mods_tab = self._build_mods_tab()
        self.tabs.addTab(self.mods_tab, "Mods")
        self.plugins_tab = self._build_plugins_tab()
        self.tabs.addTab(self.plugins_tab, "Plugins")
        from .savespage import SavesPage
        self.saves_tab = SavesPage(self)
        self.tabs.addTab(self.saves_tab, "Saves")
        self.downloads_tab = self._build_downloads_tab()
        self.tabs.addTab(self.downloads_tab, "Downloads")
        self.nexus_tab = self._build_nexus_tab()
        self.tabs.addTab(self.nexus_tab, "Nexus Mods")
        self.crun: dict | None = None        # active collection install
        self._browser = None                 # Kiln's own Nexus browser window (lazy)
        self.coll_loaded_for = None
        self.coll_tab = self._build_collections_tab()
        self.tabs.addTab(self.coll_tab, "Collections")
        self.settings_tab = self._build_settings_tab()
        self.tabs.addTab(self.settings_tab, "Settings")
        self.tabs.currentChanged.connect(self._tab_changed)
        self.tabs.tabBar().hide()
        self.tabs.setDocumentMode(True)
        self.nav = QListWidget()
        self.nav.setFixedWidth(170)
        self.nav.setIconSize(QSize(20, 20))
        self.nav.setSpacing(2)
        self.nav.setStyleSheet("QListWidget { border: none; background: transparent; font-size: 14px; }"
                               "QListWidget::item { padding: 8px 6px; border-radius: 6px; }"
                               "QListWidget::item:selected { background: palette(highlight);"
                               " color: palette(highlighted-text); }")
        icons = ["go-home", "tools-report-bug", "view-list-details", "view-sort-ascending", "document-save",
                 "folder-download", "globe", "folder-favorites", "configure"]
        fallback = ["⌂", "✚", "▤", "☰", "💾", "⭳", "🌐", "★", "⚙"]
        for i in range(self.tabs.count()):
            ic = QIcon.fromTheme(icons[i])
            it = QListWidgetItem(ic, self.tabs.tabText(i)) if not ic.isNull() else \
                QListWidgetItem(f"{fallback[i]}  {self.tabs.tabText(i)}")
            self.nav.addItem(it)
        self.nav.currentRowChanged.connect(lambda r: r >= 0 and self.tabs.setCurrentIndex(r))
        self.tabs.currentChanged.connect(lambda i: self.nav.currentRow() != i and self.nav.setCurrentRow(i))
        self.nav.setCurrentRow(0)
        body = QHBoxLayout()
        body.addWidget(self.nav)
        body.addWidget(self.tabs, 1)
        root.addLayout(body, 1)
        self.setCentralWidget(central)
        self.warn_btn = QPushButton()
        self.warn_btn.setFlat(True)
        self.warn_btn.clicked.connect(self.show_warnings)
        self.warn_btn.hide()
        self.statusBar().addPermanentWidget(self.warn_btn)

        last = self.settings.get("last_game")
        idx = next((i for i, g in enumerate(self.games) if g.id == last), None)
        self._step("Loading your mods…")
        if idx is None:
            idx = next((i for i in range(len(self.games)) if self._detect(self.games[i])), 0)
        self.game_combo.setCurrentIndex(idx)
        self.select_game(idx)
        self._step("Almost ready…")
        if self.settings.get("nexus_api_key"):
            self.validate_key(quiet=True)
        if not self.settings.get("welcome_done") and not os.environ.get("KILN_NO_WELCOME"):
            QTimer.singleShot(400, self.show_welcome)
        self.watcher = None
        self.start_watching()

    # ----------------------------------------------------------- Downloads folder
    def watch_folder(self) -> Path:
        from .watcher import default_folder
        return Path(self.settings.get("watch_folder") or default_folder())

    def start_watching(self):
        """Watch the Downloads folder (if turned on) for mods downloaded by hand."""
        from .watcher import DownloadsWatcher
        if self.watcher is not None:
            self.watcher.deleteLater()
            self.watcher = None
        if not self.settings.get("watch_downloads", True):
            return
        self.watcher = DownloadsWatcher(self.watch_folder(), self)
        self.watcher.arrived.connect(self._watched_arrival)

    def _watched_arrival(self, path: Path):
        from .watcher import identify
        order = [self.game] + [g for g in self.games if g is not self.game and self._detect(g)]
        order = [g for g in order if self._detect(g)]
        if not order:
            return
        self.statusBar().showMessage(f"New download {path.name}: checking what it is…", 6000)

        def done(found):
            if found is None:
                self.statusBar().showMessage(
                    f"{path.name} arrived in {path.parent.name}, but Kiln doesn't recognise it as a mod "
                    "for your games. Drag it onto the Mods page to install it.", 12000)
                return
            g = next(g for g in self.games if g.id == found.game_id)
            inst = self.instances.get(g.id)
            if inst is None:
                gd = self._detect(g)
                gs = self.settings.game(g.id)
                staging = Path(gs.get("staging") or Instance.default_staging(g, gd))
                inst = self.instances[g.id] = Instance(g, gd, staging)
                gs["path"], gs["staging"] = str(gd), str(staging)
                self.settings.save()
            dest = inst.downloads_dir / path.name
            if not dest.exists():
                shutil.copy2(path, dest)                 # your copy in Downloads stays put
            meta = dict(found.meta, archive=dest.name, game=g.id)
            Instance.write_download_meta(dest, meta)
            title = f"Installing {found.how} into {g.name}"
            self.statusBar().showMessage(title + " (from your Downloads folder)", 10000)
            if shutil.which("notify-send"):
                import subprocess
                icon = str(Path(__file__).resolve().parent / "assets" / "kiln.svg")
                subprocess.Popen(["notify-send", "-a", "Kiln", "-i", icon, title,
                                  f"{path.name} arrived in your Downloads folder."],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            if inst is self.inst:
                self.refresh_downloads()
            self.install_archive(dest, meta, inst)

        Task(identify, path, order).start(done, lambda e: self.statusBar().showMessage(
            f"Couldn't check {path.name}: {e}", 8000))

    def _notify(self, title: str, body: str, urgent: bool = False) -> None:
        """A desktop notification (when notify-send is available)."""
        if shutil.which("notify-send"):
            import subprocess
            icon = str(Path(__file__).resolve().parent / "assets" / "kiln.svg")
            subprocess.Popen(["notify-send", "-a", "Kiln", "-i", icon, "-u", "critical" if urgent else "normal",
                              title, body], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def _step(self, text: str) -> None:
        if self._splash is not None:
            self._splash.step(text)

    def show_welcome(self):
        from .setupdialog import WelcomeDialog
        WelcomeDialog(self).exec()
        self.settings.set("welcome_done", True)

    # ================================================================ top bar
    def _build_top(self) -> QWidget:
        from .gameheader import GameArt, GameHeader
        self.game_art: dict[str, GameArt] = {}
        h = self.header = GameHeader()
        self.game_combo = h.combo
        for g in self.games:
            art = self.game_art[g.id] = GameArt(g.steam_appid)
            found = self._detect(g)
            self.game_combo.addItem(art.icon(), g.name + ("" if found else "  (not installed)"), g.id)
        self.game_combo.currentIndexChanged.connect(self.select_game)
        self.game_label = h.status
        from PySide6.QtWidgets import QMenu
        self.folder_menu = QMenu(h.folder_btn)
        self.folder_menu.aboutToShow.connect(self._fill_folder_menu)
        h.folder_btn.setMenu(self.folder_menu)
        self.deploy_btn = h.deploy_btn
        self.deploy_btn.setToolTip("Hardlink enabled mods into the game folder")
        self.deploy_btn.clicked.connect(self.run_deploy)
        h.purge_btn.setToolTip("Remove every deployed file and restore the original game files")
        h.purge_btn.clicked.connect(self.run_purge)
        h.ess_btn.clicked.connect(self.open_setup)
        h.ver_btn.clicked.connect(self.open_versions)
        h.set_btn.clicked.connect(self.open_game_settings)
        self.launch_btn = h.play_btn
        h.play.connect(self.launch)
        h.profile_combo.activated.connect(lambda i: self.switch_profile(h.profile_combo.itemText(i)))
        from PySide6.QtWidgets import QMenu
        m = QMenu(h.profile_btn)
        m.addAction("New empty profile…", lambda: self.new_profile(copy=False))
        m.addAction("Duplicate current profile…", lambda: self.new_profile(copy=True))
        m.addAction("Rename current profile…", self.rename_profile)
        m.addSeparator()
        self.delete_profile_menu = m.addMenu("Delete profile")
        self.delete_profile_menu.aboutToShow.connect(self._fill_delete_menu)
        h.profile_btn.setMenu(m)
        return h

    # ----------------------------------------------------------- profiles
    def refresh_profiles(self):
        c = self.header.profile_combo
        c.blockSignals(True)
        c.clear()
        inst = self.inst
        if inst:
            for n in inst.profile_names():
                mods_on, _ = inst.profile_stats(n)
                c.addItem(n)
                c.setItemData(c.count() - 1, f"{mods_on} mods enabled", Qt.ToolTipRole)
            c.setCurrentText(inst.active_profile)
        c.setEnabled(inst is not None)
        self.header.profile_btn.setEnabled(inst is not None)
        c.blockSignals(False)

    def switch_profile(self, name: str):
        inst = self.inst
        if not inst or name == inst.active_profile:
            return
        if self.crun:
            self.error("Profiles", "Finish or pause the collection install first.")
            self.refresh_profiles()
            return
        inst.switch_profile(name)
        self.statusBar().showMessage(f"Switched to profile “{name}”, deploying…")
        self.refresh_all()
        self.deploy_timer.stop()
        self.run_deploy()

    def new_profile(self, copy: bool):
        from PySide6.QtWidgets import QInputDialog
        inst = self.inst
        if not inst:
            return
        base = f"{inst.active_profile} copy" if copy else "New profile"
        name, ok = QInputDialog.getText(self, "New profile",
                                        "Name:" + ("" if copy else "\n(starts with every mod disabled)"),
                                        text=base)
        if not ok:
            return
        try:
            inst.create_profile(name, inst.active_profile if copy else None)
        except Exception as e:  # noqa: BLE001
            self.error("Profiles", e)
            return
        self.switch_profile(name.strip())

    def rename_profile(self):
        from PySide6.QtWidgets import QInputDialog
        inst = self.inst
        if not inst:
            return
        name, ok = QInputDialog.getText(self, "Rename profile", "New name:", text=inst.active_profile)
        if ok and name.strip() != inst.active_profile:
            old = inst.active_profile
            try:
                inst.rename_profile(old, name)
                from . import savegames
                if savegames.supported(inst):
                    savegames.rename_profile(inst, old, name.strip())
            except Exception as e:  # noqa: BLE001
                self.error("Profiles", e)
            self.refresh_profiles()
            self.saves_tab.refresh()

    def _fill_delete_menu(self):
        m = self.delete_profile_menu
        m.clear()
        inst = self.inst
        for n in (inst.profile_names() if inst else []):
            a = m.addAction(n, lambda n=n: self.delete_profile(n))
            a.setEnabled(n != inst.active_profile)

    def delete_profile(self, name: str):
        from . import savegames
        keep = (f"\n\nIts saves stay in Saves\\{savegames.folder_name(name)} (the Saves page can still "
                "copy them to another profile).") if self.inst and savegames.supported(self.inst) and \
            savegames.enabled(self.inst) else ""
        if QMessageBox.question(self, "Delete profile", f"Delete profile “{name}”? Installed mods "
                                "are kept; only that profile's choices and order go." + keep) != QMessageBox.Yes:
            return
        try:
            self.inst.delete_profile(name)
        except Exception as e:  # noqa: BLE001
            self.error("Profiles", e)
        self.refresh_profiles()

    # ----------------------------------------------------------- auto-sort
    def auto_sort(self):
        from .sortdialog import SortDialog
        if not self.inst:
            return
        dlg = SortDialog(self.inst, self)
        if dlg.exec() == SortDialog.Accepted:
            dlg.apply()
            self.refresh_mods(keep_panel=True)
            self.refresh_plugins()
            if self.inst.game.has_plugins:
                self.inst.game.write_plugins_txt(self.inst, [(p["name"], p["enabled"])
                                                             for p in self.inst.plugins])
            self.schedule_deploy()
            self.statusBar().showMessage("Load order sorted.", 6000)

    def auto_sort_quiet(self, inst: Instance):
        """Sort without asking (the "sort after installs" setting)."""
        from . import autosort
        r = autosort.sort_mods(inst)
        rank = {mid: i for i, mid in enumerate(r.order)}
        inst.mods.sort(key=lambda m: rank[m.id])
        if inst.game.has_plugins:
            inst.plugins = autosort.sort_plugins(inst)
        inst.save()

    def _show_game_art(self, g: GameDef):
        art = self.game_art[g.id]
        self.header.set_art(art, g.name)
        if art.missing_remote() and not getattr(art, "_fetching", False):
            art._fetching = True

            def done(a, gid=g.id):
                idx = next(i for i, x in enumerate(self.games) if x.id == gid)
                self.game_combo.setItemIcon(idx, a.icon())
                if self.game.id == gid:
                    self.header.set_art(a, self.game.name)
            Task(art.fetch_missing).start(done, lambda e: None)

    def _detect(self, g: GameDef) -> Path | None:
        conf = self.settings.game(g.id).get("path")
        if conf and g.validate(Path(conf)):
            return Path(conf)
        for appid in (g.steam_appid, *g.alt_appids):
            found = steam.find_app(appid)
            if found and (g.trust_steam or g.validate(found[0])):
                if appid != g.steam_appid:
                    g.steam_appid = appid          # the edition you own (e.g. Fallout 3 GOTY or base)
                return found[0]
        return None

    @property
    def game(self) -> GameDef:
        return self.games[self.game_combo.currentIndex()]

    def select_game(self, idx: int):
        g = self.games[idx]
        self.settings.set("last_game", g.id)
        self.updates = {}
        path = self._detect(g)
        if path is None:
            self.inst = None
        elif g.id not in self.instances:
            gs = self.settings.game(g.id)
            staging = Path(gs.get("staging") or Instance.default_staging(g, path))
            self._step(f"Reading your {g.name} mod list…")
            self.instances[g.id] = Instance(g, path, staging)
            gs["path"], gs["staging"] = str(path), str(staging)
            self.settings.save()
        self.inst = self.instances.get(g.id) if path else None
        pi = self.tabs.indexOf(self.plugins_tab)
        self.tabs.setTabVisible(pi, g.has_plugins)
        self.nav.item(pi).setHidden(not g.has_plugins)
        self._show_game_art(g)
        if self.instances.get(g.id):
            self._step(f"Laying out {len(self.instances[g.id].mods)} mods…")
        self.refresh_all()
        self.search_nexus(1)
        self.coll_loaded_for = None
        if self.tabs.currentWidget() is self.dash_tab:
            self.dash_tab.refresh()
        QTimer.singleShot(1200, self.check_essentials_version)
        # first time a game is opened after the welcome: offer its essentials
        gs = self.settings.game(g.id)
        if self.inst and self.settings.get("welcome_done") and not gs.get("essentials_offered") \
                and not os.environ.get("KILN_NO_WELCOME"):
            from . import essentials
            gs["essentials_offered"] = True
            self.settings.save()
            if essentials.missing(self.inst, required_only=True):
                QTimer.singleShot(300, self.open_setup)
        if self.tabs.currentWidget() is self.coll_tab:
            self.search_collections(1)

    def folder_places(self) -> list[tuple[str, Path]]:
        """(label, folder) for the banner's Folders menu, for the current game."""
        inst = self.inst
        if not inst:
            return []
        out = [("the game folder", inst.game_dir)]
        if inst.deploy_dir != inst.game_dir:
            out.append((f"{inst.deploy_dir.name} (where mods go)", inst.deploy_dir))
        out.append(("your mods (Kiln's staging folder)", inst.mods_dir))
        from . import logreader, savegames
        if savegames.supported(inst):
            d = savegames.saves_dir(inst)
            if d is not None:
                out.append(("saves" + (f" for “{inst.active_profile}”" if savegames.enabled(inst) else ""), d))
        x = logreader.XSE.get(inst.game.id)
        mg = logreader.mygames(inst)
        if x and mg:
            out.append((f"{x[0]} logs", mg / x[0]))
        if mg:
            out.append(("game settings (My Games)", mg))
        out.append(("downloaded archives", inst.downloads_dir))
        return out

    def _fill_folder_menu(self):
        m = self.folder_menu
        m.clear()
        for label, d in self.folder_places():
            a = m.addAction(f"Open {label}", lambda d=d: self.open_folder(d))
            a.setToolTip(str(d))
            a.setEnabled(d.is_dir())
        m.setToolTipsVisible(True)
        if m.actions():
            m.addSeparator()
        m.addAction("Change game folder…" if self.inst else f"Find {self.game.name}'s folder…",
                    self.choose_game_dir)

    def open_folder(self, d: Path):
        if not d.is_dir():
            self.statusBar().showMessage(f"{d} doesn't exist yet.", 6000)
            return
        if not QDesktopServices.openUrl(QUrl.fromLocalFile(str(d))):
            self.error("Open folder", f"Couldn't open {d} in your file manager.")

    def choose_game_dir(self):
        g = self.game
        start = self.inst.game_dir if self.inst else next(
            (p for p in (lib / "steamapps/common" for lib in steam.libraries()) if p.is_dir()), Path.home())
        d = QFileDialog.getExistingDirectory(self, f"{g.name} folder", str(start))
        if not d:
            return
        if not g.validate(Path(d)):
            QMessageBox.warning(self, "Wrong folder", f"{g.exe} was not found in that folder.")
            return
        self.settings.game(g.id)["path"] = d
        self.settings.game(g.id).pop("staging", None)
        self.settings.save()
        self.instances.pop(g.id, None)
        self.game_combo.setItemText(self.game_combo.currentIndex(), g.name)
        self.select_game(self.game_combo.currentIndex())

    def refresh_top(self):
        inst = self.inst
        if not inst:
            self.game_label.setText(f"<b>{self.game.name} wasn't found.</b> Use Folders ▾ → Find its folder.")
            self.header.stats.setText("")
        else:
            dep = deploy.is_deployed(inst)
            self.game_label.setText(f"<small>{inst.game_dir}</small>")
            on = sum(m.enabled for m in inst.mods)
            ups = sum(1 for m in inst.mods if m.id in self.updates and inst.in_profile(m))
            mine = sum(1 for m in inst.mods if inst.in_profile(m))
            self.header.stats.setText(
                f"<b>{mine}</b> mods in this profile · <b>{on}</b> enabled"
                + (f" · <span style='color:#ffa94d'><b>{ups}</b> update(s)</span>" if ups else "")
                + (f" · {len(inst.plugins)} plugins" if inst.game.has_plugins else "")
                + (" · <span style='color:#8ce99a'>deployed</span>" if dep
                   else " · <span style='color:#ffa94d'>not deployed</span>"))
        for b in (self.deploy_btn, self.launch_btn, self.header.purge_btn):
            b.setEnabled(inst is not None)
        self._refresh_essentials_btn()
        self._refresh_version_btn()
        from . import gamesettings
        self.header.set_btn.setVisible(bool(inst and gamesettings.supported(inst)))

    def _refresh_version_btn(self):
        """The banner's game-version button: the installed version, for games Kiln
        can switch (or has switched) versions of."""
        from . import gameversions as gv
        b, inst = self.header.ver_btn, self.inst
        if not inst or not (gv.for_game(inst.game.id) or gv.stored(inst)):
            b.hide()
            return
        cur = gv.short(gv.installed_version(inst))
        b.setText(f"Game {cur or '?'} ▾")
        b.setToolTip("Which version of the game is installed. Click to switch to the version your "
                     "mods and collections are made for.")
        b.show()

    # ----------------------------------------------------------- essentials vs game version
    def check_essentials_version(self):
        """Essentials made for another game version (after a Steam update or a switch):
        ask to swap in the right build, or do it, as the user chose in Settings."""
        from . import essentials
        inst = self.inst
        if not inst or not essentials.for_game(inst.game.id):
            return
        rt = essentials.game_runtime(inst)
        gs = self.settings.game(inst.game.id)
        last = gs.get("last_runtime")
        now = ".".join(map(str, rt)) if rt else ""
        if now and last != now:
            gs["last_runtime"] = now
            self.settings.save()
        wrong = essentials.mismatched(inst)
        seen = self.__dict__.setdefault("_ess_prompted", set())
        mode = self.settings.get("essentials_match", "ask")
        if not wrong or mode == "off" or (inst.game.id, now) in seen or self.crun:
            return
        seen.add((inst.game.id, now))
        if mode == "auto":
            self.swap_essentials(inst, wrong)
            return
        why = (f"Steam updated {inst.game.name} to {now}" if last and last != now
               else f"{inst.game.name} is version {now}")
        box = QMessageBox(self.dialog_parent())
        box.setIcon(QMessageBox.Warning)
        box.setWindowTitle("Essentials don't match your game")
        box.setText(f"{why}, but {', '.join(e.name for e in wrong)} "
                    f"{'is' if len(wrong) == 1 else 'are'} made for another version.")
        box.setInformativeText("Mods that need them won't load until they match. Kiln can get the "
                               f"builds made for {now} from Nexus and swap them in.")
        swap = box.addButton("Swap now", QMessageBox.AcceptRole)
        always = box.addButton("Always do this automatically", QMessageBox.AcceptRole)
        box.addButton("Not now", QMessageBox.RejectRole)
        box.exec()
        if box.clickedButton() is always:
            self.settings.set("essentials_match", "auto")
            if hasattr(self, "ess_mode"):
                self.ess_mode.setCurrentIndex(self.ess_mode.findData("auto"))
        if box.clickedButton() in (swap, always):
            self.swap_essentials(inst, wrong)

    def swap_essentials(self, inst, wrong):
        """Download the builds of these essentials made for the installed game version
        and put them in place of the current ones."""
        from . import collections as nc
        from . import essentials
        if self.crun:
            self.statusBar().showMessage("Kiln is busy installing; swap the essentials once it's done "
                                         "(banner: Fix essentials).", 10000)
            return
        rt = essentials.game_runtime(inst)
        rts = ".".join(map(str, rt or ()))
        gid = inst.game.nexus_game_id

        def work():
            out = []
            for e in wrong:
                if e.nexus_id:
                    files = nexus.mod_files(gid, e.nexus_id)
                    out.append((e, essentials.pick_file(files, e, rt, essentials.downloaded(inst, e))))
            return out

        def done(picks):
            entries, none = [], []
            for e, f in picks:
                if f is None:
                    none.append(e.name)
                    continue
                old = essentials.downloaded(inst, e)
                if old is not None:
                    self.replace_on_install(e.nexus_id, f.file_id, old.id)
                cm = nc.CollectionMod(e.nexus_id, f.file_id, e.name, f.name, f.version, False,
                                      f.size // 1024, f.category)
                entries.append({"kind": "nexus", "mod": cm, "item": _NullItem(), "status": "queued"})
            if none:
                self.error("Essentials", "Nexus has no build of " + ", ".join(none) + f" for {rts} yet.")
            if entries:
                self.statusBar().showMessage(f"Swapping in {len(entries)} essential(s) made for {rts}…", 10000)
                self.start_nexus_queue(inst, entries, f"{inst.game.name} essentials for {rts}",
                                       on_finish=lambda: (self.refresh_mods(), self.refresh_top()))

        Task(work).start(done, lambda e: self.error("Couldn't look up the essentials", e))

    def update_vcruntime(self, inst, after=None):
        """Download Microsoft's VC++ Redistributable and install it into the game's prefix."""
        from . import launchguard, vcruntime
        from .downloader import download
        if launchguard.game_running(inst):
            QMessageBox.information(self.dialog_parent(), "C++ runtime", f"Close {inst.game.name} first.")
            return
        proton = vcruntime.proton_for(inst)
        if QMessageBox.question(
                self.dialog_parent(), "Update the C++ runtime",
                f"Kiln will download Microsoft's Visual C++ Redistributable ({vcruntime.REDIST_URL}) and "
                f"install it into {inst.game.name}'s Proton folder using "
                f"{proton.parent.name if proton else 'its Proton'}. It runs silently and takes about a "
                "minute. Nothing outside that Proton folder changes.") != QMessageBox.Yes:
            return
        self.statusBar().showMessage("Downloading and installing the Visual C++ runtime…")

        def work(progress, cancelled):
            exe = download(vcruntime.REDIST_URL, cache_dir() / "vcredist", progress, cancelled,
                           fallback_name="vc_redist.x64.exe")
            return vcruntime.install(inst, exe)

        def done(v):
            self.statusBar().showMessage(f"Visual C++ runtime is now {'.'.join(map(str, v))}. "
                                         "Your DLL mods can start properly.", 12000)
            self._notify("C++ runtime updated", f"{inst.game.name}: {'.'.join(map(str, v))}")
            if after:
                after()

        Task(work, progress=True).start(done, lambda e: self.error("C++ runtime", e))

    # ----------------------------------------------------------- AI helper
    def refresh_ai_models(self):
        from . import ai
        if not self.settings.get("ai_enabled", False) and not self.ai_on.isChecked():
            return
        url = self.ai_url.text().strip() or ai.DEFAULT_URL

        def done(models):
            self._ai_loading = True
            self.ai_model.clear()
            for m in models:
                self.ai_model.addItem(m["name"], m)
            cur = self.settings.get("ai_model")
            i = next((i for i in range(self.ai_model.count()) if self.ai_model.itemText(i) == cur), 0)
            self.ai_model.setCurrentIndex(i)
            self._ai_loading = False
            self._ai_model_chosen()
            self.ai_status.setText(f"Connected: {len(models)} model(s) available." if models else
                                   "Ollama is running but has no models. Pull one, e.g. “ollama pull qwen3”.")

        Task(ai.list_models, url).start(done, lambda e: self.ai_status.setText(str(e)))

    def _ai_model_chosen(self, *_a):
        if getattr(self, "_ai_loading", False):
            return
        d = self.ai_model.currentData()
        if d:
            self.settings.set("ai_model", d["name"])
            self.settings.set("ai_model_thinking", d["thinking"])

    def ask_ai(self, issue=None, issues=None):
        from .aidialog import AskDialog
        if not self.inst:
            return
        self._ai_dlg = AskDialog(self, self.inst, issue, issues)
        self._ai_dlg.show()
        if issue is not None and self.settings.get("ai_model"):
            self._ai_dlg.ask()

    def install_nexus_for_game(self, inst, nexus_mod_id: int):
        """Install a Nexus mod: the file made for the installed game version if the mod
        says, else its newest main file."""
        from . import essentials, versionmatch

        def work():
            files = nexus.mod_files(inst.game.nexus_game_id, nexus_mod_id)
            rt = essentials.game_runtime(inst)
            if rt:
                f, _sure = versionmatch.best_file(files, rt)
                if f is not None:
                    return f
            mains = [f for f in files if f.category == "MAIN"] or [f for f in files if f.current]
            return max(mains, key=lambda f: (f.primary, f.date)) if mains else None

        def done(f):
            if f is None:
                self.error("Not found", "That mod has no file Kiln can install.")
                return
            self.statusBar().showMessage(f"Getting “{f.name}” {f.version} from Nexus…", 8000)
            self.download_nexus_file(inst, nexus_mod_id, f.file_id)

        Task(work).start(done, lambda e: self.error("Couldn't look up the mod's files", e))

    def install_nexus_named(self, inst, nexus_mod_id: int, word: str):
        """Install the current file of a Nexus mod whose name contains word (e.g. Engine
        Fixes' “SKSE64 Preloader”)."""
        def work():
            files = nexus.mod_files(inst.game.nexus_game_id, nexus_mod_id)
            cands = [f for f in files if word.lower() in f.name.lower()
                     and f.category in ("MAIN", "UPDATE", "OPTIONAL", "MISCELLANEOUS")]
            return max(cands, key=lambda f: (f.category == "MAIN", f.date)) if cands else None

        def done(f):
            if f is None:
                self.error("Not found", f"Couldn't find the “{word}” file on that mod's Nexus page.")
                return
            self.statusBar().showMessage(f"Getting “{f.name}” {f.version} from Nexus…", 8000)
            self.download_nexus_file(inst, nexus_mod_id, f.file_id)

        Task(work).start(done, lambda e: self.error("Couldn't look up the mod's files", e))

    def open_match(self):
        from .versiondialog import MatchDialog
        if not self.inst:
            return
        self._match_dlg = MatchDialog(self, self.inst)
        self._match_dlg.show()

    def replace_on_install(self, nexus_mod_id: int, file_id: int, mod_id: str):
        """When this Nexus file gets installed (however it arrives), it replaces mod_id."""
        if not hasattr(self, "_replace_for"):
            self._replace_for = {}
        self._replace_for[(int(nexus_mod_id), int(file_id))] = mod_id

    def open_game_settings(self):
        from .settingsdialog import GameSettingsDialog
        if self.inst:
            self._gs_dlg = GameSettingsDialog(self, self.inst)
            self._gs_dlg.show()

    def open_versions(self):
        from .versiondialog import VersionDialog
        if not self.inst:
            return
        self._ver_dlg = VersionDialog(self, self.inst)
        self._ver_dlg.show()

    def _refresh_essentials_btn(self):
        """The banner's Essentials button: shown for games that have essentials,
        highlighted when this profile is missing some."""
        from . import essentials
        b, inst = self.header.ess_btn, self.inst
        if not inst or not essentials.for_game(inst.game.id):
            b.hide()
            return
        b.show()
        miss = essentials.missing(inst)
        req = [e for e in miss if e.required]
        reuse = [e for e in miss if essentials.status(inst, e) in ("elsewhere", "disabled")]
        wrong = essentials.mismatched(inst)
        if wrong:
            rt = essentials.game_runtime(inst)
            b.setText(f"Fix essentials ({len(wrong)})")
            b.setToolTip(", ".join(e.name for e in wrong) + " " + ("is" if len(wrong) == 1 else "are")
                         + f" made for another game version (yours is {'.'.join(map(str, rt or ()))}). "
                         "Click to swap in the right build.")
            b.setProperty("alert", "required")
        elif miss:
            b.setText(f"Install essentials ({len(miss)})")
            b.setToolTip("Missing in this profile: " + ", ".join(e.name for e in miss)
                         + (f"\n{len(reuse)} already downloaded and will be reused." if reuse else ""))
            b.setProperty("alert", "required" if req else "recommended")
        else:
            b.setText("Essentials ✔")
            b.setToolTip("Everything this game needs is installed in this profile.")
            b.setProperty("alert", "")
        b.style().unpolish(b)
        b.style().polish(b)

    def launch(self, skip_checks: bool = False):
        """Play, guarded: check everything that stops mods loading, offer fixes,
        launch through Steam, then confirm the mod loaders started."""
        from . import launchguard
        inst = self.inst
        if not inst:
            self.error("Game not set up", f"Point Kiln at your {self.game.name} folder first.")
            return
        if launchguard.game_running(inst):
            self.stop_game(inst)
            return
        if launchguard.ensure_notice_mod(inst):
            self.refresh_mods(keep_panel=True)
            self.deploy_timer.stop()
            self._deploy_sync(inst)
        checks = launchguard.preflight(inst, pending_deploy=self.deploy_timer.isActive())
        bad = [c for c in checks if c.level != "ok"]
        # quiet auto-fixes: just deploy if that's all that's needed
        if bad and all(c.fix == "deploy" for c in bad):
            self.deploy_timer.stop()
            self._deploy_sync(inst)
            checks = launchguard.preflight(inst)
            bad = [c for c in checks if c.level != "ok"]
        if bad and not skip_checks:
            from .launchdialog import LaunchDialog
            r = LaunchDialog(self, checks).exec()
            if r == 2:
                QTimer.singleShot(0, self.launch)       # something was fixed: check again
                return
            if r != LaunchDialog.Accepted:
                return
        if not skip_checks and not self._doctor_gate(inst):
            return
        if inst.game.has_plugins:
            inst.game.write_plugins_txt(inst, [(p["name"], p["enabled"]) for p in inst.plugins])
        from . import savegames
        if savegames.supported(inst):
            try:
                savegames.point_game(inst)        # this profile's saves
            except OSError:
                pass
        try:
            steam.launch(self.game.steam_appid)
        except RuntimeError as e:
            self.error("Can't launch", str(e))
            return
        self._watch_launch(inst)

    def launch_vanilla(self):
        """Play without mods: everything but the loaders is taken out (and every mod plugin
        switched off); the next Play puts the full setup back."""
        from . import deploy, launchguard
        inst = self.inst
        if not inst:
            self.error("Game not set up", f"Point Kiln at your {self.game.name} folder first.")
            return
        if launchguard.game_running(inst):
            self.error("Already running", f"{inst.game.name} is already running.")
            return
        self.deploy_timer.stop()
        try:
            res = deploy.deploy_vanilla(inst)
        except OSError as e:
            self.error("Couldn't remove the mods", str(e))
            return
        for w in getattr(res, "warnings", [])[:3]:
            self.statusBar().showMessage(w, 8000)
        self.refresh_all()
        try:
            steam.launch(self.game.steam_appid)
        except RuntimeError as e:
            self.error("Can't launch", str(e))
            return
        self.statusBar().showMessage(f"Playing {inst.game.name} without mods. Play puts them back.", 10000)

    def _doctor_gate(self, inst) -> bool:
        """Before Play: warn about problems that will crash the game. True = go ahead."""
        from . import doctor
        try:
            ign = set(self.settings.game(inst.game.id).get("doctor_ignored", []))
            crash = [i for i in doctor.diagnose(inst, ign)
                     if i.severity == "crash" and not i.key.startswith("crash:")]
        except Exception:  # noqa: BLE001
            return True
        if not crash:
            return True
        box = QMessageBox(self)
        box.setIcon(QMessageBox.Warning)
        box.setWindowTitle("The game will probably crash")
        box.setText(f"Kiln found {len(crash)} problem(s) that usually crash {inst.game.name}:\n\n"
                    + "\n".join(f"• {i.title}" for i in crash[:6])
                    + ("\n…" if len(crash) > 6 else ""))
        box.setInformativeText("Troubleshoot explains each one and can fix most of them for you.")
        show = box.addButton("Show me", QMessageBox.AcceptRole)
        box.addButton("Play anyway", QMessageBox.DestructiveRole)
        box.addButton(QMessageBox.Cancel)
        box.exec()
        if box.clickedButton() is show:
            self.go_to(self.doctor_tab)
            return False
        return box.standardButton(box.clickedButton()) != QMessageBox.Cancel

    def _deploy_sync(self, inst):
        """Deploy now and wait for it (used right before launch)."""
        from PySide6.QtCore import QEventLoop
        loop, box = QEventLoop(), {}
        Task(deploy.deploy, inst).start(lambda r: (box.setdefault("r", r), loop.quit()),
                                        lambda e: (box.setdefault("e", e), loop.quit()), pool=self.jobs)
        loop.exec()
        if "e" in box:
            self.error("Deploy failed", box["e"])
        else:
            self._deployed(inst, box["r"], 0)

    # ----------------------------------------------------------- launch watch
    def _watch_launch(self, inst):
        from . import launchguard
        self.playing = {"inst": inst, "t0": time.time(), "seen_game": False, "reported": False,
                        "needs": [ld.name for ld, _ in launchguard.needed_loaders(inst)]}
        self.header.status.setText("<b>Launching…</b> Kiln is watching for the mod loaders to start.")
        self.statusBar().showMessage(f"Launching {inst.game.name} through Steam…", 8000)
        if not hasattr(self, "play_timer"):
            self.play_timer = QTimer(self, interval=3000)
            self.play_timer.timeout.connect(self._poll_launch)
        self.play_timer.start()

    def _poll_launch(self):
        from . import launchguard
        pl = getattr(self, "playing", None)
        if not pl:
            self.play_timer.stop()
            return
        inst = pl["inst"]
        running = launchguard.game_running(inst)
        elapsed = time.time() - pl["t0"]
        if running:
            pl["seen_game"] = True
            self._set_play_button(True)
            self._watch_for_hang(inst, pl)
        if not pl["reported"]:
            res = launchguard.check_logs(inst, pl["t0"])
            done = all(r.started and (r.ok or r.summary) for r in res)
            smapi_waiting = any(r.loader.id == "smapi" and not r.ok for r in res)
            if (res and done and not smapi_waiting) or (not res and pl["seen_game"] and elapsed > 8) \
                    or elapsed > 180:
                pl["reported"] = True
                self._report_launch(inst, res, running or pl["seen_game"])
        if pl["seen_game"] and not running and elapsed > 15:
            self.playing = None
            self.play_timer.stop()
            self._set_play_button(False)
            self.statusBar().showMessage(f"{inst.game.name} closed.", 8000)
            self._after_play(inst, pl["t0"])
            self.refresh_top()
            if getattr(self, "_deploy_pending", False):
                self._deploy_pending = False
                self.run_deploy()
        elif not pl["seen_game"] and elapsed > 240:
            self.playing = None
            self.play_timer.stop()
            self.refresh_top()

    def _set_play_button(self, running: bool):
        b = self.header.play_btn
        want = "■  Stop" if running else "▶  Play"
        if b.text() == want:
            return
        b.setText(want)
        b.setToolTip("End the game (for when it hangs)" if running else "")
        b.setStyleSheet(b.styleSheet().replace("#2f9e44", "#c92a2a").replace("#37b24d", "#e03131") if running
                        else b.styleSheet().replace("#c92a2a", "#2f9e44").replace("#e03131", "#37b24d"))

    def _watch_for_hang(self, inst, pl):
        """The game's window is gone but its process isn't: end it (the classic Skyrim
        hang on quit), so Steam doesn't think it's still running."""
        from . import gameproc
        pids = gameproc.game_pids(inst)
        win = gameproc.has_window(pids)
        if win is None or not pids:
            return
        if win:
            pl["window_seen"], pl["no_window_since"] = True, None
            return
        if not pl.get("window_seen"):
            return
        since = pl.get("no_window_since") or time.time()
        pl["no_window_since"] = since
        if time.time() - since < 10 or pl.get("ended"):
            return
        if not self.settings.get("end_hung_game", True):
            if not pl.get("hang_told"):
                pl["hang_told"] = True
                self.statusBar().showMessage(f"{inst.game.name}'s window closed but it's still running. "
                                             "Press Stop in the banner to end it.", 20000)
            return
        pl["ended"] = True
        gameproc.stop(pids)
        msg = (f"{inst.game.name}'s window closed but it kept running in the background, so Kiln ended it.")
        self.statusBar().showMessage(msg, 15000)
        self._notify(f"{inst.game.name} closed", msg)

    def stop_game(self, inst):
        """Stop pressed (or Play while the game runs): end the game after asking."""
        from . import gameproc
        pids = gameproc.game_pids(inst)
        if not pids:
            self.statusBar().showMessage(f"{inst.game.name} is starting or closing; try again in a moment.", 6000)
            return
        if QMessageBox.question(
                self.dialog_parent(), f"Stop {inst.game.name}",
                f"End {inst.game.name}? Use this when it has hung (for example after quitting). "
                "Anything unsaved in the game is lost.") != QMessageBox.Yes:
            return
        gameproc.stop(pids)
        self.statusBar().showMessage(f"Ended {inst.game.name}.", 8000)

    def _after_play(self, inst, t0: float):
        """The game closed: if it left a crash report or loader errors, say so."""
        from . import doctor
        try:
            new = [i for i in doctor.diagnose(inst) if i.source in ("crash", "logs")]
        except Exception:  # noqa: BLE001
            return
        fresh = [i for i in new if i.key.startswith("crash:") and int(i.key.rsplit(":", 1)[1]) >= t0 - 5]
        if not fresh and not new:
            return
        title = fresh[0].title if fresh else f"{inst.game.name}: {len(new)} problem(s) in the logs"
        self.header.status.setText(f"<b style='color:#ff8787'>{title}</b><br><small>Kiln looked into "
                                   "it: see Troubleshoot.</small>")
        if shutil.which("notify-send"):
            import subprocess
            icon = str(Path(__file__).resolve().parent / "assets" / "kiln.svg")
            subprocess.Popen(["notify-send", "-a", "Kiln", "-i", icon, "-u", "critical", title,
                              "Kiln read the crash report and loader logs. Open Troubleshoot to see "
                              "what went wrong."], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if fresh:
            self.go_to(self.doctor_tab)
            self.raise_()
            self.activateWindow()

    def _report_launch(self, inst, res, game_seen: bool):
        on = sum(1 for m in inst.mods if m.enabled)
        failed = [r for r in res if not r.started or not r.ok]
        if not res:
            title = f"{inst.game.name}: {on} mods deployed"
            body = "This game needs no mod loader; mods are in place."
            ok = True
        elif not failed:
            title = f"✔ {inst.game.name} is modded"
            body = " · ".join(f"{r.loader.name}: {r.summary}" for r in res)
            ok = True
        else:
            title = f"✖ {inst.game.name} may not be modded"
            body = " · ".join(f"{r.loader.name}: {r.summary if r.started else 'did not start'}"
                              for r in res)
            ok = False
        color = "#8ce99a" if ok else "#ff8787"
        self.header.status.setText(f"<b style='color:{color}'>{title}</b><br><small>{body}</small>")
        self.statusBar().showMessage(f"{title}: {body}", 20000)
        if shutil.which("notify-send"):
            import subprocess
            icon = str(Path(__file__).resolve().parent / "assets" / "kiln.svg")
            subprocess.Popen(["notify-send", "-a", "Kiln", "-i", icon,
                              "-u", "normal" if ok else "critical", title, body],
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    # ----------------------------------------------------------- mod doctor
    def show_mods_by_id(self, ids: list[str]):
        self.go_to(self.mods_tab)
        self.mod_search.clear()
        i = self.mod_filter.findData("all")
        if i >= 0:
            self.mod_filter.setCurrentIndex(i)
        self.mod_tree.clearSelection()
        first = None
        for n in range(self.mod_tree.topLevelItemCount()):
            it = self.mod_tree.topLevelItem(n)
            if it.data(0, Qt.UserRole) in ids:
                it.setSelected(True)
                first = first or it
        if first:
            self.mod_tree.setCurrentItem(first)
            self.mod_tree.scrollToItem(first)

    def run_doctor_action(self, a, after=None):
        """Carry out one fix the Mod Doctor offered."""
        from types import SimpleNamespace
        from . import doctor
        inst = self.inst
        kind, _, arg = a.fix.partition(":")
        try:
            needs_deploy = doctor.apply(inst, a.fix)
        except KeyError:
            pass
        except Exception as e:  # noqa: BLE001
            self.error("Fix failed", e)
            return
        else:
            self.refresh_mods(keep_panel=True)
            if inst.game.has_plugins:
                self.refresh_plugins()
            if needs_deploy:
                self.schedule_deploy()
            self.statusBar().showMessage(f"Done: {a.describe or a.label}.", 6000)
            if after:
                after()
            return
        if kind in ("deploy", "materialize", "loader", "launch_options", "deps", "plugins"):
            self.fix_check(SimpleNamespace(fix=a.fix, data=a.data), after)
        elif kind == "reinstall":
            m = inst.get(arg)
            if m:
                self.reinstall_mod(m)
        elif kind == "nexus_page":
            url = nexus.mod_page(inst.game.nexus_domain, int(arg)) + "?tab=files"
            self.open_browser(url) if self.browser() else QDesktopServices.openUrl(QUrl(url))
        elif kind == "search":
            from urllib.parse import quote
            url = f"{nexus.SITE}/games/{inst.game.nexus_domain}/mods?keyword={quote(arg)}"
            self.open_browser(url) if self.browser() else QDesktopServices.openUrl(QUrl(url))
        elif kind == "open_file":
            QDesktopServices.openUrl(QUrl.fromLocalFile(arg))
        elif kind == "url":
            QDesktopServices.openUrl(QUrl(arg))
        elif kind == "update_compatible":
            self._find_compatible(inst.get(arg))
        elif kind == "match_version":
            self.open_match()
        elif kind == "vcredist":
            self.update_vcruntime(inst, after)
        elif kind == "install_nexus":
            self.install_nexus_for_game(inst, int(arg))
        elif kind == "install_named":
            mid, _, word = arg.partition(":")
            self.install_nexus_named(inst, int(mid), word)
        else:
            self.statusBar().showMessage(f"Kiln doesn't know how to do “{a.label}” yet.", 6000)

    def _find_compatible(self, m):
        from . import doctor
        if not m or not m.nexus_mod_id:
            return
        inst = self.inst
        self.statusBar().showMessage(f"Looking for a version of {m.name} that works…", 8000)

        def done(res):
            f, why = res
            if f is None:
                r = QMessageBox.question(self, m.name, f"{why}\n\nOpen the mod's page to check yourself?")
                if r == QMessageBox.Yes:
                    self.run_doctor_action(doctor.Action(f"nexus_page:{m.nexus_mod_id}", ""))
                return
            r = QMessageBox.question(
                self, m.name, f"Kiln found {why} on the mod's page"
                + (f" (uploaded {time.strftime('%d %b %Y', time.localtime(f.date))})" if f.date else "")
                + ".\n\n" + (f.description[:400] + "\n\n" if f.description else "")
                + "Download and install it? It replaces your current copy (whose download stays "
                  "in Downloads).")
            if r == QMessageBox.Yes:
                self.replace_on_install(m.nexus_mod_id, f.file_id, m.id)
                self.download_nexus_file(inst, m.nexus_mod_id, f.file_id)

        Task(doctor.find_compatible, inst, m).start(
            done, lambda e: self.error("Couldn't look up the mod's files", e))

    def apply_doctor_fixes(self, actions, after=None):
        """“Fix everything”: data fixes in order, then one deploy."""
        from . import doctor, launchguard
        inst = self.inst
        done, errors, deploy_needed = 0, [], False
        for a in actions:
            if a.fix == "deploy":
                deploy_needed = True
                continue
            try:
                deploy_needed |= doctor.apply(inst, a.fix)
                done += 1
            except KeyError:
                errors.append(f"{a.label}: use its own button")
            except Exception as e:  # noqa: BLE001
                errors.append(f"{a.describe or a.label}: {e}")
        self.refresh_mods(keep_panel=True)
        if inst.game.has_plugins:
            self.refresh_plugins()
        if deploy_needed:
            if launchguard.game_running(inst):
                self.schedule_deploy()
            else:
                self.deploy_timer.stop()
                self._deploy_sync(inst)
                done += any(a.fix == "deploy" for a in actions)
        if after:
            after(done, errors)

    # ----------------------------------------------------------- fixes
    def fix_check(self, c, after=None):
        from . import launchguard, steamcfg
        inst = self.inst
        if c.fix == "deploy":
            self.deploy_timer.stop()
            self._deploy_sync(inst)
        elif c.fix == "materialize":
            if QMessageBox.question(
                    self, "Other mod manager's links",
                    f"Replace {len(c.data['links'])} link(s) in the game folder with real copies of "
                    "the files they point to? The game then no longer depends on the other mod "
                    "manager's folder. Broken links are removed.") != QMessageBox.Yes:
                return
            fixed, removed = launchguard.materialize_links([Path(x) for x in c.data["links"]])
            self.statusBar().showMessage(f"{fixed} link(s) made real, {removed} broken removed.", 8000)
        elif c.fix.startswith("loader:") and (c.data.get("nexus_file") or [0])[0]:
            mid, word = c.data["nexus_file"]
            self.install_nexus_named(inst, int(mid), word)
            return
        elif c.fix.startswith("loader:"):
            lid = c.fix.split(":", 1)[1]
            from . import importers
            amethyst = [p for p in importers.amethyst_candidates(inst)]
            if amethyst and lid != "smapi":
                self.import_amethyst()
            elif lid == "smapi":
                self.install_smapi_flow()
                return          # the flow re-checks on its own
            else:
                url = c.data.get("url", "")
                self.open_browser(url) if self.browser() and "nexusmods" in url else \
                    QDesktopServices.openUrl(QUrl(url))
                return
        elif c.fix == "launch_options":
            self.fix_launch_options(c.data["wanted"], after)
            return
        elif c.fix == "plugins":
            self.go_to(self.plugins_tab)
        elif c.fix == "deps":
            self.open_deps()
            return
        if after:
            after()

    def fix_launch_options(self, wanted: str, after=None):
        from . import steamcfg
        appid = self.game.steam_appid

        def write():
            try:
                bak = steamcfg.set_launch_options(appid, wanted)
            except Exception as e:  # noqa: BLE001
                self.error("Launch options", e)
                return
            self.statusBar().showMessage(f"Steam launch options set (backup: {bak.name}).", 10000)
            if after:
                after()

        if not steamcfg.steam_running():
            write()
            return
        r = QMessageBox.question(
            self, "Steam launch options",
            f"Kiln needs to set this game's Steam launch options to:\n\n{wanted}\n\n"
            "Steam only accepts this while it's closed. Close Steam now? Kiln will make the change "
            "and you can start Steam again (Play does that for you).\n\nOr choose No and paste it "
            "yourself: Steam → right-click the game → Properties → Launch options.",
            QMessageBox.Yes | QMessageBox.No)
        if r != QMessageBox.Yes:
            QApplication.clipboard().setText(wanted)
            self.statusBar().showMessage("Launch options copied to the clipboard.", 10000)
            return
        self.statusBar().showMessage("Closing Steam…")
        Task(steamcfg.shutdown_steam).start(
            lambda ok: write() if ok else self.error("Steam", "Steam didn't close. Close it yourself, "
                                                     "then press Play again."),
            lambda e: self.error("Steam", e))

    # ----------------------------------------------------------- essentials
    def open_setup(self):
        from .setupdialog import SetupDialog
        if not self.inst:
            self.error("Game not set up", f"Point Kiln at your {self.game.name} folder first.")
            return
        old = getattr(self, "_setup_dlg", None)
        if old is not None:
            try:
                old.close()
            except RuntimeError:
                pass
        self._setup_dlg = SetupDialog(self, self.inst)
        self._setup_dlg.show()

    def install_essentials(self, dlg, selected):
        """Install essentials: SMAPI from GitHub, the rest from Nexus through the
        collection download queue, then make Steam load them."""
        from types import SimpleNamespace
        from . import collections as nc
        from . import essentials, launchguard
        inst = dlg.inst
        if self.crun:
            self.error("Busy", "A collection is being installed. Finish or pause it first.")
            dlg.go.setEnabled(True)
            return
        nex = [e for e in selected if e.kind == "nexus"]
        # reuse what's already downloaded (switched off, or in another profile)
        reused = 0
        for e in list(nex):
            m = essentials.downloaded(inst, e)
            if m and not m.enabled:
                was_here = inst.in_profile(m)
                inst.add_to_profile([m])
                dlg.items[e.key].setText(4, "✔ enabled again" if was_here else "✔ reused")
                nex.remove(e)
                reused += 1
        inst.save()
        if reused:
            self.refresh_mods(keep_panel=True)
            self.schedule_deploy(inst)
        if nex and not self.settings.get("nexus_api_key"):
            self.error("Nexus API key needed", "Add your Nexus API key in Settings first; Kiln "
                       "needs it to download from Nexus.")
            dlg.go.setEnabled(True)
            return

        def after_all():
            self.refresh_mods()
            cur = __import__("kiln.steamcfg", fromlist=["x"]).get_launch_options(inst.game.steam_appid)
            want = launchguard.wanted_launch_options(inst, cur)
            try:
                dlg.fill()
            except RuntimeError:
                pass
            self.settings.game(inst.game.id)["essentials_done"] = True
            self.settings.save()
            if want != cur:
                self.fix_launch_options(want)

        def run_nexus():
            if not nex:
                self.deploy_timer.stop()
                self._deploy_sync(inst)
                after_all()
                return
            entries = []
            for e in nex:
                d, f = dlg.files[e.key]
                old = essentials.downloaded(inst, e)
                if old is not None and essentials.status(inst, e) == "mismatch":
                    self.replace_on_install(e.nexus_id, f.file_id, old.id)   # swap, not add
                cm = nc.CollectionMod(e.nexus_id, f.file_id, d.name, f.name, f.version,
                                      not e.required, f.size // 1024, f.category)
                it = dlg.items[e.key]
                it.setText(4, "queued")
                entries.append({"kind": "nexus", "mod": cm, "item": it, "status": "queued"})
            self.start_nexus_queue(inst, entries, f"{inst.game.name} essentials", after_all)

        smapi = [e for e in selected if e.kind == "smapi"]
        if smapi:
            it = dlg.items[smapi[0].key]
            it.setText(4, "downloading…")

            def work():
                from .downloader import download
                ver, url, _size = launchguard.smapi_release()
                z = download(url, inst.staging / "tools", fallback_name=f"SMAPI-{ver}-installer.zip")
                launchguard.install_smapi(inst, z)
                return ver

            def done(ver):
                it.setText(4, f"✔ SMAPI {ver} installed")
                run_nexus()

            def fail(err):
                it.setText(4, f"✖ {err}")
                run_nexus()
            Task(work).start(done, fail)
        else:
            run_nexus()

    def start_nexus_queue(self, inst, entries, title: str, on_finish=None):
        """Download+install Nexus files one after another (Premium: automatic; free:
        Kiln's browser), then deploy once. Uses the collection runner, without
        recording a collection."""
        from types import SimpleNamespace
        if self.crun:
            self.error("Busy", "Another install queue is running. Finish or pause it first.")
            return
        if not self.settings.get("nexus_api_key"):
            self.error("Nexus API key needed", "Add your Nexus API key in Settings first.")
            return
        self.crun = {"inst": inst, "entries": entries, "manifest": None, "waiting": None,
                     "rev": SimpleNamespace(slug="", name=title, revision=0),
                     "premium": bool(self.user and self.user.is_premium),
                     "started": time.time(), "essentials": True, "on_finish": on_finish}
        self._crun_buttons()
        self._crun_next()

    def open_deps(self):
        from .setupdialog import DepsDialog
        if not self.inst:
            return
        old = getattr(self, "_deps_dlg", None)
        if old is not None:
            try:
                old.close()
            except RuntimeError:
                pass
        self._deps_dlg = DepsDialog(self, self.inst)
        self._deps_dlg.show()

    def import_amethyst(self):
        from . import importers
        inst = self.inst
        cands = importers.amethyst_candidates(inst) if inst else []
        if not cands:
            self.statusBar().showMessage("No Amethyst mods to import for this game.", 6000)
            return
        if QMessageBox.question(
                self, "Import from Amethyst",
                f"Copy {len(cands)} mod(s) from Amethyst into Kiln?\n\n"
                + "\n".join(f"• {p.name}" for p in cands[:20])
                + "\n\nAmethyst's own copies are left alone.") != QMessageBox.Yes:
            return
        added = importers.import_amethyst(inst, cands)
        self.refresh_mods()
        self.statusBar().showMessage(f"Imported {len(added)} mod(s) from Amethyst.", 8000)
        self.deploy_timer.stop()
        self._deploy_sync(inst)

    def install_smapi_flow(self):
        from . import launchguard
        inst = self.inst

        def got(rel):
            ver, url, size = rel
            if QMessageBox.question(
                    self, "Install SMAPI",
                    f"Download SMAPI {ver} ({size / 1048576:.1f} MB) from its official GitHub "
                    f"release and install it into\n{inst.game_dir}?\n\nKiln unpacks the Linux "
                    "files from SMAPI's own installer and sets the Steam launch option.") != QMessageBox.Yes:
                return
            self.statusBar().showMessage(f"Downloading SMAPI {ver}…")

            def work():
                from .downloader import download
                z = download(url, inst.staging / "tools", fallback_name=f"SMAPI-{ver}-installer.zip")
                return launchguard.install_smapi(inst, z)

            def done(files):
                self.statusBar().showMessage(f"SMAPI {ver} installed ({len(files)} files).", 10000)
                QTimer.singleShot(0, self.launch)
            Task(work).start(done, lambda e: self.error("SMAPI", e))

        Task(launchguard.smapi_release).start(got, lambda e: self.error("SMAPI", e))

    # ================================================================ mods tab
    def _build_mods_tab(self) -> QWidget:
        from .modpanel import ModPanel
        w = QWidget()
        lay = QVBoxLayout(w)
        lay.setContentsMargins(0, 0, 0, 0)
        bar = QHBoxLayout()
        b = QPushButton("+ Install from file…")
        b.clicked.connect(self.install_from_file)
        bar.addWidget(b)
        self.mod_search = QLineEdit()
        self.mod_search.setPlaceholderText("Search installed mods…")
        self.mod_search.setClearButtonEnabled(True)
        self.mod_search.textChanged.connect(lambda _: self.refresh_mods(keep_panel=True))
        bar.addWidget(self.mod_search, 1)
        self.mod_filter = QComboBox()
        self.mod_filter.setMinimumWidth(170)
        self.mod_filter.currentIndexChanged.connect(lambda _: self.refresh_mods(keep_panel=True))
        bar.addWidget(self.mod_filter)
        self.show_all_mods = QPushButton("Show all downloaded")
        self.show_all_mods.setCheckable(True)
        self.show_all_mods.setToolTip(
            "Each profile only shows its own mods. Turn this on to also see mods downloaded for "
            "your other profiles, and tick one to add it to this profile (no new download needed).")
        self.show_all_mods.toggled.connect(lambda _: self.refresh_mods(keep_panel=True))
        bar.addWidget(self.show_all_mods)
        for text, fn in (("⇅ Auto-sort", self.auto_sort), ("Check for updates", self.check_updates),
                         ("Update all", self.update_all)):
            b = QPushButton(text)
            b.clicked.connect(fn)
            bar.addWidget(b)
        lay.addLayout(bar)

        split = QSplitter(Qt.Horizontal)
        left = QWidget()
        ll = QVBoxLayout(left)
        ll.setContentsMargins(0, 0, 0, 0)
        self.mod_tree = ModTree()
        self.mod_tree.files_dropped.connect(self._files_dropped)
        self.mod_tree.setHeaderLabels(["Mod", "Version", "Category", "Author", "Conflicts", "Source",
                                       "Installed"])
        self.mod_tree.setRootIsDecorated(False)
        self.mod_tree.setAlternatingRowColors(True)
        self.mod_tree.setUniformRowHeights(True)
        self.mod_tree.setDragDropMode(QAbstractItemView.InternalMove)
        self.mod_tree.setSelectionMode(QAbstractItemView.ExtendedSelection)
        self.mod_tree.header().setSectionResizeMode(0, QHeaderView.Stretch)
        self.mod_tree.setContextMenuPolicy(Qt.CustomContextMenu)
        self.mod_tree.customContextMenuRequested.connect(self._mod_menu)
        self.mod_tree.itemChanged.connect(self._mod_check_changed)
        self.mod_tree.model().rowsMoved.connect(lambda *a: QTimer.singleShot(0, self._mods_reordered))
        self.mod_tree.currentItemChanged.connect(self._show_mod_details)
        self.mod_tree.itemDoubleClicked.connect(
            lambda it, _c: (m := self._selected_mod()) and self.set_mods_enabled([m], not m.enabled))
        ll.addWidget(self.mod_tree, 1)
        self.mods_hint = QLabel()
        self.mods_hint.setStyleSheet("color: gray;")
        self.mods_hint.setWordWrap(True)
        ll.addWidget(self.mods_hint)
        split.addWidget(left)
        self.mod_panel = ModPanel(self)
        split.addWidget(self.mod_panel)
        split.setStretchFactor(0, 3)
        split.setStretchFactor(1, 2)
        split.setSizes([700, 430])
        lay.addWidget(split, 1)
        self._loading = False
        self._plan = None
        self.endorsements: dict = {}
        self.mod_info_cache: dict = {}
        self.img_cache: dict = {}
        return w

    # ----------------------------------------------------------- mod list
    def deploy_plan(self):
        if self._plan is None and self.inst:
            self._plan = deploy.plan(self.inst)
        return self._plan or ({}, {})

    def _refresh_filter_choices(self):
        inst = self.inst
        cur = self.mod_filter.currentData()
        opts = [("All mods", "all"), ("Enabled", "on"), ("Disabled", "off"),
                ("Update available", "update"), ("Has conflicts", "conflict"),
                ("From Nexus", "nexus"), ("Local files", "local")]
        if inst:
            opts += [(f"Collection: {c['name']}", "coll:" + c["slug"]) for c in inst.collections]
            opts += [(f"Category: {c}", "cat:" + c)
                     for c in sorted({m.category for m in inst.mods if m.category}, key=str.lower)]
        if [self.mod_filter.itemData(i) for i in range(self.mod_filter.count())] == [o[1] for o in opts]:
            return
        self.mod_filter.blockSignals(True)
        self.mod_filter.clear()
        for t, d in opts:
            self.mod_filter.addItem(t, d)
        i = self.mod_filter.findData(cur)
        self.mod_filter.setCurrentIndex(max(i, 0))
        self.mod_filter.blockSignals(False)

    def _mod_matches(self, m: Mod, conf) -> bool:
        if not self.show_all_mods.isChecked() and not self.inst.in_profile(m):
            return False
        q = self.mod_search.text().strip().lower()
        if q and not any(q in (x or "").lower() for x in (m.name, m.author, m.category, m.file_name,
                                                          m.notes, m.version)):
            return False
        f = self.mod_filter.currentData() or "all"
        return {"all": True, "on": m.enabled, "off": not m.enabled, "update": m.id in self.updates,
                "conflict": m.id in conf, "nexus": bool(m.nexus_mod_id),
                "local": not m.nexus_mod_id}.get(f, (f.startswith("coll:") and m.collection == f[5:])
                                                  or (f.startswith("cat:") and m.category == f[4:]))

    def refresh_mods(self, keep_panel: bool = False):
        self._loading = True
        self._plan = None
        cur = self.mod_tree.currentItem().data(0, Qt.UserRole) if self.mod_tree.currentItem() else None
        sel = {it.data(0, Qt.UserRole) for it in self.mod_tree.selectedItems()}
        self._refresh_filter_choices()
        self.mod_tree.clear()
        shown = 0
        filtered = bool(self.mod_search.text().strip()) or (self.mod_filter.currentData() or "all") != "all"
        italic = None
        if self.inst:
            conf = deploy.conflict_summary(self.inst)
            grey = QBrush(QColor("gray"))
            for i, m in enumerate(self.inst.mods):
                if not self._mod_matches(m, conf):
                    continue
                c = conf.get(m.id, {})
                ctext = ""
                if c.get("wins"):
                    ctext += f"▲ {len(c['wins'])}"
                if c.get("loses"):
                    ctext += ("  " if ctext else "") + f"▼ {len(c['loses'])}"
                ver = m.version + (f"  ⬆ {self.updates[m.id].version}" if m.id in self.updates else "")
                src = "Nexus" if m.nexus_mod_id else "Local"
                if m.collection:
                    src += " · collection"
                it = QTreeWidgetItem([m.name, ver, m.category, m.author, ctext, src,
                                      fmt_date(m.installed_at)])
                it.setData(0, Qt.UserRole, m.id)
                it.setToolTip(0, f"Priority {i + 1}" + (f"\n\nNotes: {m.notes}" if m.notes else ""))
                if c:
                    it.setToolTip(4, "▲ overrides files from " + str(len(c.get("wins", ()))) +
                                  " mod(s), ▼ is overridden by " + str(len(c.get("loses", ()))))
                it.setFlags((it.flags() | Qt.ItemIsUserCheckable | Qt.ItemIsDragEnabled)
                            & ~Qt.ItemIsDropEnabled)
                it.setCheckState(0, Qt.Checked if m.enabled else Qt.Unchecked)
                if not m.enabled:
                    for col in range(7):
                        it.setForeground(col, grey)
                if not self.inst.in_profile(m):
                    if italic is None:
                        italic = it.font(0)
                        italic.setItalic(True)
                    others = self.inst.profiles_using(m)
                    for col in range(7):
                        it.setFont(col, italic)
                    it.setText(5, "other profile" if others else "no profile")
                    it.setToolTip(0, ("In profile(s): " + ", ".join(others) if others else
                                      "Downloaded, but not used by any profile")
                                  + "\nTick it to add it to this profile.")
                if m.id in self.updates:
                    it.setForeground(1, QBrush(QColor("#e8590c")))
                if c.get("loses"):
                    it.setForeground(4, QBrush(QColor("#c92a2a")))
                elif c.get("wins"):
                    it.setForeground(4, QBrush(QColor("#2f9e44")))
                self.mod_tree.addTopLevelItem(it)
                shown += 1
                if m.id == cur:
                    self.mod_tree.setCurrentItem(it)
                if m.id in sel:
                    it.setSelected(True)
        for i in range(1, 7):
            self.mod_tree.resizeColumnToContents(i)
        self.mod_tree.setDragDropMode(QAbstractItemView.NoDragDrop if filtered
                                      else QAbstractItemView.InternalMove)
        total = len(self.inst.mods) if self.inst else 0
        mine = sum(1 for m in self.inst.mods if self.inst.in_profile(m)) if self.inst else 0
        hidden = total - mine
        prof = self.inst.active_profile if self.inst else ""
        scope = (f"{mine} mods in profile “{prof}”"
                 + (f" ({hidden} more downloaded for other profiles: “Show all downloaded” to see "
                    "and reuse them)" if hidden and not self.show_all_mods.isChecked() else "")
                 + (f", plus {hidden} from other profiles (in italics)"
                    if hidden and self.show_all_mods.isChecked() else ""))
        self.mods_hint.setText(
            (f"Showing {shown}: {scope}. Clear the search/filter to drag mods into order. "
             if filtered else f"{scope}. ") +
            "Tick to enable · drag to reorder (lower wins conflicts) · right-click for more · "
            "drop archives here to install.")
        self._loading = False
        if not keep_panel or (self.mod_panel.mod and self.inst and self.mod_panel.mod not in self.inst.mods):
            self._show_mod_details(self.mod_tree.currentItem())
        elif self.mod_panel.mod:
            self.mod_panel.show_mod(self.mod_panel.mod)
        self.refresh_top()

    def _selected_mod(self) -> Mod | None:
        it = self.mod_tree.currentItem()
        return self.inst.get(it.data(0, Qt.UserRole)) if (it and self.inst) else None

    def _selected_mods(self) -> list[Mod]:
        if not self.inst:
            return []
        ids = {it.data(0, Qt.UserRole) for it in self.mod_tree.selectedItems()}
        return [m for m in self.inst.mods if m.id in ids] or (
            [self._selected_mod()] if self._selected_mod() else [])

    def _mod_check_changed(self, it, col):
        if self._loading or not self.inst or col != 0:
            return
        m = self.inst.get(it.data(0, Qt.UserRole))
        if m and m.enabled != (it.checkState(0) == Qt.Checked):
            self.set_mods_enabled([m], it.checkState(0) == Qt.Checked)

    def _mods_reordered(self):
        if self._loading or not self.inst:
            return
        order = [self.mod_tree.topLevelItem(i).data(0, Qt.UserRole)
                 for i in range(self.mod_tree.topLevelItemCount())]
        self.inst.mods.sort(key=lambda m: order.index(m.id) if m.id in order else 1e9)
        self.inst.save()
        self.refresh_mods(keep_panel=True)
        self.schedule_deploy()

    def move_mod(self, delta):
        m = self._selected_mod()
        if m:
            self.move_mod_to(m, self.inst.mods.index(m) + delta)

    def move_mod_to(self, m: Mod, index: int):
        self.inst.move(m, index)
        self.refresh_mods(keep_panel=True)
        self.schedule_deploy()

    def _show_mod_details(self, it, _prev=None):
        m = self.inst.get(it.data(0, Qt.UserRole)) if (it and self.inst) else None
        self.mod_panel.show_mod(m)

    def _mod_menu(self, pos):
        from PySide6.QtWidgets import QMenu
        mods = self._selected_mods()
        if not mods:
            return
        one = mods[0] if len(mods) == 1 else None
        menu = QMenu(self)
        label = f"“{one.name}”" if one else f"{len(mods)} mods"
        menu.addSection(label)
        outside = [m for m in mods if not self.inst.in_profile(m)]
        if outside:
            menu.addAction(f"Add to this profile ({len(outside)})" if len(mods) > 1 else "Add to this profile",
                           lambda: self.add_mods_to_profile(outside))
        if any(not m.enabled for m in mods if m not in outside):
            menu.addAction("Enable", lambda: self.set_mods_enabled(mods, True))
        if any(m.enabled for m in mods):
            menu.addAction("Disable", lambda: self.set_mods_enabled(mods, False))
        inside = [m for m in mods if m not in outside]
        if inside and len(self.inst.profiles) > 1:
            menu.addAction("Remove from this profile (keep the files)",
                           lambda: self.remove_mods_from_profile(inside))
        ups = [m for m in mods if m.id in self.updates]
        if ups:
            menu.addAction(f"Update ({len(ups)})", lambda: [self.update_mod(m) for m in ups])
        menu.addSeparator()
        menu.addAction("Load first", lambda: [self.move_mod_to(m, 0) for m in reversed(mods)])
        menu.addAction("Load last (wins conflicts)", lambda: [self.move_mod_to(m, 10 ** 6) for m in mods])
        menu.addAction("Set category…", lambda: self.set_category(mods))
        if one:
            menu.addAction("Rename…", lambda: self.rename_mod(one))
            menu.addAction("Reinstall", lambda: self.reinstall_mod(one))
            menu.addSeparator()
            menu.addAction("Open folder", lambda: self.open_mod_folder(one))
            if one.nexus_mod_id:
                menu.addAction("Open on Nexus", lambda: self.open_mod_page(one))
        menu.addSeparator()
        menu.addAction(f"Remove {label}…", lambda: self.remove_mods(mods))
        menu.exec(self.mod_tree.viewport().mapToGlobal(pos))

    # ----------------------------------------------------------- mod actions
    def set_mods_enabled(self, mods: list[Mod], on: bool):
        for m in mods:
            m.enabled = on
        self.inst.save()
        self.refresh_mods(keep_panel=True)
        self.schedule_deploy()

    def set_category(self, mods: list[Mod]):
        from PySide6.QtWidgets import QInputDialog
        cats = sorted({m.category for m in self.inst.mods if m.category}, key=str.lower)
        text, ok = QInputDialog.getItem(self, "Category", f"Category for {len(mods)} mod(s):",
                                        cats or [""], 0, True)
        if ok:
            for m in mods:
                m.category = text.strip()
            self.inst.save()
            self.refresh_mods(keep_panel=True)

    def rename_mod(self, m: Mod):
        from PySide6.QtWidgets import QInputDialog
        text, ok = QInputDialog.getText(self, "Rename mod", "Name shown in Kiln:", text=m.name)
        if ok and text.strip():
            m.name = text.strip()
            self.inst.save()
            self.refresh_mods(keep_panel=True)

    def add_mods_to_profile(self, mods: list[Mod]):
        self.inst.add_to_profile(mods)
        self.refresh_mods(keep_panel=True)
        self.schedule_deploy()
        self.statusBar().showMessage(f"Added {len(mods)} mod(s) to “{self.inst.active_profile}”.", 6000)

    def remove_mods_from_profile(self, mods: list[Mod]):
        self.inst.remove_from_profile(mods)
        self.refresh_mods(keep_panel=True)
        self.schedule_deploy()
        self.statusBar().showMessage(
            f"Removed {len(mods)} mod(s) from “{self.inst.active_profile}”. They're still downloaded; "
            "“Show all downloaded” brings them back.", 8000)

    def remove_mods(self, mods: list[Mod]):
        if not mods:
            return
        what = f"“{mods[0].name}”" if len(mods) == 1 else f"{len(mods)} mods"
        others = sorted({n for m in mods for n in self.inst.profiles_using(m)} - {self.inst.active_profile})
        note = (f"\n\nThe profile(s) {', '.join(others)} use it too and will lose it. To take it out "
                "of this profile only, use “Remove from this profile” instead." if others else "")
        if QMessageBox.question(self, "Remove", f"Delete {what} from Kiln? Staged files are deleted; "
                                f"downloaded archives stay in Downloads.{note}") != QMessageBox.Yes:
            return
        for m in mods:
            self.inst.remove(m)
            self.updates.pop(m.id, None)
        self.mod_panel.show_mod(None)
        self.refresh_mods()
        self.schedule_deploy()

    def remove_mod(self):
        self.remove_mods(self._selected_mods())

    def update_mod(self, m: Mod):
        if m and m.id in self.updates:
            self.download_nexus_file(self.inst, m.nexus_mod_id, self.updates[m.id].file_id)

    def update_selected(self):
        m = self._selected_mod()
        if m and m.id in self.updates:
            self.update_mod(m)
        elif m:
            self.statusBar().showMessage("No update known for this mod; run “Check for updates”.", 5000)

    def update_all(self):
        ups = [m for m in (self.inst.mods if self.inst else []) if m.id in self.updates]
        if not ups:
            self.statusBar().showMessage("No updates known. Run “Check for updates” first.", 6000)
            return
        if QMessageBox.question(self, "Update all", f"Download and install {len(ups)} update(s)?"
                                ) == QMessageBox.Yes:
            for m in ups:
                self.update_mod(m)

    def reinstall_mod(self, m: Mod | None = None):
        m = m or self._selected_mod()
        if not m:
            return
        arc = self.inst.downloads_dir / m.archive if m.archive else None
        if not arc or not arc.is_file():
            self.error("Can't reinstall", "The original archive isn't in Downloads any more.")
            return
        meta = {"name": m.name, "version": m.version, "author": m.author,
                "nexus_mod_id": m.nexus_mod_id, "nexus_file_id": m.nexus_file_id,
                "file_name": m.file_name, "archive": m.archive, "replace_id": m.id}
        self.install_archive(arc, meta)

    def open_staging(self):
        m = self._selected_mod()
        if self.inst:
            self.open_mod_folder(m)

    def open_mod_folder(self, m: Mod | None):
        QDesktopServices.openUrl(QUrl.fromLocalFile(str(self.inst.mod_path(m) if m else self.inst.staging)))

    def open_mod_page(self, m: Mod):
        if m.nexus_mod_id:
            url = nexus.mod_page(self.inst.game.nexus_domain, m.nexus_mod_id)
            self.open_browser(url) if self.browser() else QDesktopServices.openUrl(QUrl(url))

    def endorse_mod(self, m: Mod):
        key = self.settings.get("nexus_api_key", "")
        dom = self.inst.game.nexus_domain
        undo = self.endorsements.get((dom, m.nexus_mod_id)) == "Endorsed"

        def done(status):
            self.endorsements[(dom, m.nexus_mod_id)] = status
            self.statusBar().showMessage(f"{'Endorsed' if status == 'Endorsed' else 'Withdrew endorsement of'}"
                                         f" {m.name}.", 6000)
            if self.mod_panel.mod is m:
                self.mod_panel.show_mod(m)
        Task(nexus.endorse, key, dom, m.nexus_mod_id, m.version, undo).start(
            done, lambda e: self.error("Endorse", e))

    def load_endorsements(self):
        key = self.settings.get("nexus_api_key", "")
        if key:
            Task(nexus.endorsements, key).start(
                lambda d: (self.endorsements.update(d), self.mod_panel.mod and
                           self.mod_panel.show_mod(self.mod_panel.mod)), lambda e: None)

    def mod_info(self, m: Mod, cb, err=None):
        """Nexus details for an installed mod (cached for the session)."""
        key = (self.inst.game.nexus_domain, m.nexus_mod_id)
        if key in self.mod_info_cache:
            cb(self.mod_info_cache[key])
            return
        g = self.inst.game

        def done(d):
            self.mod_info_cache[key] = d
            cb(d)
        Task(nexus.mod_details, g.nexus_domain, g.nexus_game_id, m.nexus_mod_id).start(
            done, err or (lambda e: None))

    def fetch_image(self, url: str, cb):
        """Load a remote image (memory + disk cache), calling cb(QPixmap)."""
        if url in self.img_cache:
            cb(self.img_cache[url])
            return
        import hashlib
        f = cache_dir() / "img" / hashlib.sha1(url.encode()).hexdigest()

        def work():
            if f.is_file():
                return load_image(f.read_bytes())
            data = nexus.fetch_bytes(url)
            f.parent.mkdir(parents=True, exist_ok=True)
            f.write_bytes(data)
            return load_image(data)

        def done(img):
            pm = QPixmap.fromImage(img)
            if not pm.isNull():
                self.img_cache[url] = pm
                cb(pm)
        Task(work).start(done, lambda e: None)

    def _files_dropped(self, paths: list[str]):
        if not self.inst:
            self.error("Game not set up", f"Point Kiln at your {self.game.name} folder first.")
            return
        for f in paths:
            p = Path(f)
            if p.is_file():
                self.install_archive(p, {"name": clean_archive_name(p.name)})

    def install_from_file(self):
        if not self.inst:
            return
        f, _ = QFileDialog.getOpenFileName(self, "Mod archive", str(Path.home() / "Downloads"),
                                           "Archives (*.zip *.7z *.rar *.tar *.gz *.xz);;All files (*)")
        if f:
            self.install_archive(Path(f), {"name": clean_archive_name(Path(f).name)})

    # ----------------------------------------------------------- installing
    def install_archive(self, path: Path, meta: dict, inst: Instance | None = None,
                        preset: list | None = None, after=None):
        """Install an archive. preset: saved FOMOD choices (from a collection).
        after(mod, error): called when done instead of showing error dialogs."""
        inst = inst or self.inst
        if not inst:
            return
        key = (int(meta.get("nexus_mod_id") or 0), int(meta.get("nexus_file_id") or 0))
        rep = getattr(self, "_replace_for", {}).pop(key, None)
        if rep and inst.get(rep) and not meta.get("replace_id"):
            meta = dict(meta, replace_id=rep)     # a version-matched file takes the old one's place
        name = meta.get("name") or clean_archive_name(path.name)
        self.statusBar().showMessage(f"Extracting {name}…")
        Task(inst.prepare, path, name, meta).start(
            lambda prep: self._prepared(inst, prep, preset, after),
            lambda e: self._install_failed(e, after), pool=self.jobs)

    def _install_failed(self, err, after):
        if after:
            after(None, err)
        else:
            self.error("Install failed", err)

    def _prepared(self, inst: Instance, prep: Prepared, preset=None, after=None):
        files = choices = None
        if prep.fomod_xml:
            try:
                cfg = fomod.parse(prep.fomod_xml)
            except fomod.FomodError as e:
                if QMessageBox.question(self, "Broken FOMOD", f"{e}\n\nInstall without the "
                                        "installer (guess the layout)?") != QMessageBox.Yes:
                    prep.cleanup()
                    self._install_failed("cancelled (broken FOMOD)", after) if after else None
                    return
            else:
                fs = self._file_state(inst)
                sel = fomod.selections_from_choices(cfg, preset, fs) if preset is not None else None
                if sel is not None:
                    files, choices = fomod.resolve(cfg, sel, fs), {f"{s}:{g}": v for (s, g), v in sel.items()}
                else:
                    from .fomod_dialog import FomodDialog
                    dlg = FomodDialog(cfg, prep.fomod_base, fs, self)
                    if dlg.exec() != FomodDialog.Accepted or dlg.result_files is None:
                        prep.cleanup()
                        self.statusBar().showMessage("Install cancelled.", 4000)
                        if after:
                            after(None, "installer cancelled")
                        return
                    files, choices = dlg.result_files, dlg.choices()
        Task(inst.finish_install, prep, files, choices).start(
            lambda m: self._installed(inst, m, after), lambda e: self._install_failed(e, after),
            pool=self.jobs)

    def _file_state(self, inst: Instance):
        enabled = {p["name"].lower(): p["enabled"] for p in inst.plugins}

        def state(f: str) -> str:
            low = f.replace("\\", "/").lower()
            if low in enabled:
                return "Active" if enabled[low] else "Inactive"
            return "Active" if ci_resolve(inst.deploy_dir, f) else "Missing"
        return state

    def _installed(self, inst: Instance, mod: Mod, after=None):
        self.updates.pop(mod.id, None)
        self.statusBar().showMessage(f"Installed {mod.name}.", 6000)
        if inst is self.inst:
            self.refresh_mods()
            self.refresh_downloads()
        if after:
            after(mod, None)   # collection runs deploy once at the end
        else:
            if self.settings.get("auto_sort", False):
                self.auto_sort_quiet(inst)
                if inst is self.inst:
                    self.refresh_mods(keep_panel=True)
            self.schedule_deploy(inst)

    # ----------------------------------------------------------- deploying
    def schedule_deploy(self, inst: Instance | None = None):
        pl = getattr(self, "playing", None)
        if pl and pl.get("seen_game") and (inst or self.inst) is pl["inst"]:
            self._deploy_pending = True
            self.statusBar().showMessage("The game is running; Kiln will deploy your changes when "
                                         "it closes.", 8000)
            return
        if self.settings.get("auto_deploy", True) and (inst or self.inst):
            self._deploy_target = inst or self.inst
            self.deploy_timer.start()
        else:
            self.refresh_top()

    def run_deploy(self):
        inst = getattr(self, "_deploy_target", None) or self.inst
        self._deploy_target = None
        if not inst:
            return
        self.statusBar().showMessage(f"Deploying {inst.game.name}…")
        t0 = time.time()
        Task(deploy.deploy, inst).start(
            lambda r: self._deployed(inst, r, time.time() - t0),
            lambda e: self.error("Deploy failed", e), pool=self.jobs)

    def _deployed(self, inst, res: deploy.DeployResult, secs: float):
        self.statusBar().showMessage(f"Deployed {inst.game.name}: {res.summary()} ({secs:.1f}s)", 8000)
        if self.tabs.currentWidget() is self.dash_tab:
            self.dash_tab.refresh()
        self.set_warnings(res.warnings)
        if inst is self.inst:
            self.refresh_top()
            self.refresh_plugins()

    def run_purge(self):
        inst = self.inst
        if not inst or QMessageBox.question(
                self, "Purge", f"Remove all deployed mod files from {inst.game.name} and restore "
                "original files? Mods stay installed in staging.") != QMessageBox.Yes:
            return
        self.deploy_timer.stop()
        Task(deploy.purge, inst).start(
            lambda r: (self.statusBar().showMessage(f"Purged: {r.summary()}", 8000),
                       self.set_warnings(r.warnings), self.refresh_top()),
            lambda e: self.error("Purge failed", e), pool=self.jobs)

    def set_warnings(self, w: list[str]):
        self.warnings = w
        self.warn_btn.setVisible(bool(w))
        self.warn_btn.setText(f"⚠ {len(w)} warning(s)")

    def show_warnings(self):
        QMessageBox.warning(self, "Warnings", "\n\n".join(self.warnings))

    # ================================================================ plugins
    def _build_plugins_tab(self) -> QWidget:
        w = QWidget()
        lay = QVBoxLayout(w)
        hint = QLabel("Load order for plugins (.esp/.esm/.esl), saved to plugins.txt in the "
                      "game's Proton prefix. Base game and Creation Club plugins load first "
                      "automatically and aren't listed. Drag to reorder.")
        hint.setWordWrap(True)
        lay.addWidget(hint)
        self.plugin_list = QListWidget()
        self.plugin_list.setDragDropMode(QAbstractItemView.InternalMove)
        self.plugin_list.itemChanged.connect(lambda *_: self._plugins_changed())
        self.plugin_list.model().rowsMoved.connect(lambda *_: QTimer.singleShot(0, self._plugins_changed))
        lay.addWidget(self.plugin_list, 1)
        row = QHBoxLayout()
        b = QPushButton("⇅ Auto-sort plugins")
        b.setToolTip("Masters first, every plugin after its masters, unofficial patches early and "
                     "compatibility patches late; otherwise keeps your order")
        b.clicked.connect(self.sort_plugins)
        row.addWidget(b)
        self.plugins_path = QLabel()
        self.plugins_path.setTextInteractionFlags(Qt.TextSelectableByMouse)
        row.addWidget(self.plugins_path, 1)
        lay.addLayout(row)
        return w

    def refresh_plugins(self):
        self._loading = True
        self.plugin_list.clear()
        inst = self.inst
        if inst and inst.game.has_plugins:
            from .games.bethesda import read_plugin
            missing = {w.split(" needs")[0] for w in inst.missing_master_warnings()}
            for i, p in enumerate(inst.plugins):
                info = read_plugin(inst.deploy_dir / p["name"])
                tags = []
                if info.is_master:
                    tags.append("master")
                if info.is_light:
                    tags.append("light")
                text = f"{i:02X}  {p['name']}" + (f"   [{', '.join(tags)}]" if tags else "")
                if p["name"] in missing:
                    text += "   ⚠ missing master"
                it = QListWidgetItem(text)
                it.setData(Qt.UserRole, p["name"])
                it.setFlags(it.flags() | Qt.ItemIsUserCheckable)
                it.setCheckState(Qt.Checked if p["enabled"] else Qt.Unchecked)
                if info.masters:
                    it.setToolTip("Masters: " + ", ".join(info.masters))
                self.plugin_list.addItem(it)
            path = inst.game.plugins_txt(inst)
            self.plugins_path.setText(f"<small>{path or 'No Proton prefix yet: run the game once.'}</small>")
        self._loading = False

    def _plugins_changed(self):
        if self._loading or not self.inst:
            return
        self.inst.plugins = [{"name": self.plugin_list.item(i).data(Qt.UserRole),
                              "enabled": self.plugin_list.item(i).checkState() == Qt.Checked}
                             for i in range(self.plugin_list.count())]
        self.inst.save()
        self.inst.game.write_plugins_txt(self.inst, [(p["name"], p["enabled"]) for p in self.inst.plugins])
        self.set_warnings(self.inst.missing_master_warnings())
        QTimer.singleShot(0, self.refresh_plugins)

    def sort_plugins(self):
        if self.inst:
            from . import autosort
            self.inst.plugins = autosort.sort_plugins(self.inst)
            self.inst.save()
            self.inst.game.write_plugins_txt(self.inst, [(p["name"], p["enabled"]) for p in self.inst.plugins])
            self.refresh_plugins()

    # ================================================================ downloads
    def _build_downloads_tab(self) -> QWidget:
        w = QWidget()
        lay = QVBoxLayout(w)
        lay.addWidget(QLabel("Active downloads"))
        self.active_list = QListWidget()
        self.active_list.setMaximumHeight(110)
        lay.addWidget(self.active_list)
        lay.addWidget(QLabel("Downloaded archives (kept so mods can be reinstalled)"))
        self.dl_tree = QTreeWidget()
        self.dl_tree.setHeaderLabels(["Archive", "Mod", "Size", "Date", "Installed"])
        self.dl_tree.setRootIsDecorated(False)
        self.dl_tree.header().setSectionResizeMode(0, QHeaderView.Stretch)
        lay.addWidget(self.dl_tree, 1)
        row = QHBoxLayout()
        for text, fn in (("Install", self.install_download), ("Delete", self.delete_download),
                         ("Open folder", lambda: self.inst and QDesktopServices.openUrl(
                             QUrl.fromLocalFile(str(self.inst.downloads_dir))))):
            b = QPushButton(text)
            b.clicked.connect(fn)
            row.addWidget(b)
        row.addStretch(1)
        lay.addLayout(row)
        return w

    def refresh_downloads(self):
        self.dl_tree.clear()
        if not self.inst:
            return
        installed = {m.archive for m in self.inst.mods}
        for f, meta in self.inst.downloads():
            st = f.stat()
            it = QTreeWidgetItem([f.name, meta.get("name", ""), fmt_size(st.st_size),
                                  fmt_date(int(st.st_mtime)), "✔" if f.name in installed else ""])
            it.setData(0, Qt.UserRole, (str(f), meta))
            self.dl_tree.addTopLevelItem(it)
        for i in range(1, 5):
            self.dl_tree.resizeColumnToContents(i)

    def install_download(self):
        it = self.dl_tree.currentItem()
        if it:
            f, meta = it.data(0, Qt.UserRole)
            meta = dict(meta, archive=Path(f).name)
            self.install_archive(Path(f), meta)

    def delete_download(self):
        it = self.dl_tree.currentItem()
        if not it:
            return
        f, _ = it.data(0, Qt.UserRole)
        if QMessageBox.question(self, "Delete download", f"Delete {Path(f).name}?") == QMessageBox.Yes:
            Path(f).unlink(missing_ok=True)
            Path(f + ".json").unlink(missing_ok=True)
            self.refresh_downloads()

    def start_download(self, inst: Instance, mod_id: int, file_id: int, key=None, expires=None,
                       install: bool = True, preset=None, after=None, extra_meta=None,
                       switch_tab: bool = True):
        """Download a Nexus file (Premium, or free with an nxm key) and install it."""
        api_key = self.settings.get("nexus_api_key", "")
        g = inst.game
        row = QListWidgetItem(f"{g.name}: mod {mod_id} file {file_id} — starting…")
        self.active_list.addItem(row)
        if switch_tab:
            self.go_to(self.downloads_tab)

        def work(progress, cancelled):
            det = nexus.mod_details(g.nexus_domain, g.nexus_game_id, mod_id)
            nf = next((f for f in det.files if f.file_id == file_id), None)
            from .downloader import download
            path = download(lambda: nexus.download_links(api_key, g.nexus_domain, mod_id, file_id,
                                                         key, expires),
                            inst.downloads_dir, progress, cancelled,
                            fallback_name=nf.file_name if nf else f"{mod_id}-{file_id}.zip")
            name = det.name
            if nf and nf.category not in ("MAIN", "") and nf.name and \
                    nf.name.lower() not in det.name.lower():
                name = f"{det.name}: {nf.name}"       # an optional file, not the main mod
            meta = {"name": name, "author": det.author,
                    "version": (nf.version if nf and nf.version else det.version),
                    "nexus_mod_id": mod_id, "nexus_file_id": file_id,
                    "file_name": nf.name if nf else "", "archive": path.name,
                    "game": g.id, "file_date": nf.date if nf else 0, "category": det.category,
                    "summary": det.summary, "picture": det.picture, **(extra_meta or {})}
            Instance.write_download_meta(path, meta)
            return path, meta

        def prog(done, total):
            pct = f"{100 * done // total}% " if total else ""
            row.setText(f"{g.name}: mod {mod_id} file {file_id} — {pct}{fmt_size(done)}"
                        + (f" / {fmt_size(total)}" if total else ""))

        def done(res):
            path, meta = res
            self.active_list.takeItem(self.active_list.row(row))
            self.statusBar().showMessage(f"Downloaded {path.name}", 6000)
            if inst is self.inst:
                self.refresh_downloads()
            if install:
                self.install_archive(path, meta, inst, preset=preset, after=after)

        def fail(e):
            self.active_list.takeItem(self.active_list.row(row))
            if after:
                after(None, f"download failed: {e}")
            else:
                self.error("Download failed", e)

        row.setText(f"{g.name}: mod {mod_id} file {file_id} — waiting for a free download slot…")
        Task(work, progress=True).start(done, fail, prog, pool=self.dl_pool)

    # ================================================================ nexus
    def _build_nexus_tab(self) -> QWidget:
        w = QWidget()
        lay = QVBoxLayout(w)
        bar = QHBoxLayout()
        self.n_search = QLineEdit()
        self.n_search.setPlaceholderText("Search Nexus Mods for this game…")
        self.n_search.returnPressed.connect(lambda: self.search_nexus(1))
        self.n_sort = QComboBox()
        for k, t in nexus.SORTS:
            self.n_sort.addItem(t, k)
        self.n_sort.currentIndexChanged.connect(lambda _: self.search_nexus(1))
        self.n_cat = QComboBox()
        self.n_cat.setMinimumWidth(200)
        self.n_cat.setToolTip("Show only mods from one Nexus category")
        self.n_cat.addItem("All categories", "")
        self.n_cat.currentIndexChanged.connect(lambda _: not self._cat_loading and self.search_nexus(1))
        self._cat_loading = False
        self._cats_for = None
        self.n_adult = QCheckBox("Adult content")
        self.n_adult.setChecked(bool(self.settings.get("show_adult", False)))
        self.n_adult.toggled.connect(lambda v: (self.settings.set("show_adult", v), self.search_nexus(1)))
        self.n_prev, self.n_next, self.n_page = QPushButton("◀"), QPushButton("▶"), QLabel()
        self.n_prev.clicked.connect(lambda: self.search_nexus(self.page - 1))
        self.n_next.clicked.connect(lambda: self.search_nexus(self.page + 1))
        for x in (self.n_search, self.n_cat, self.n_sort, self.n_adult, self.n_prev, self.n_page, self.n_next):
            bar.addWidget(x, 1 if x is self.n_search else 0)
        lay.addLayout(bar)
        split = QSplitter()
        self.n_results = QListWidget()
        self.n_results.setIconSize(QSize(120, 68))
        self.n_results.currentItemChanged.connect(self._nexus_selected)
        split.addWidget(self.n_results)
        det = QWidget()
        dl = QVBoxLayout(det)
        dl.setContentsMargins(0, 0, 0, 0)
        self.n_title = QLabel("<i>Select a mod</i>")
        self.n_title.setWordWrap(True)
        self.n_image = QLabel()
        self.n_image.setAlignment(Qt.AlignCenter)
        self.n_desc = QTextBrowser()
        self.n_desc.setOpenExternalLinks(True)
        self.n_files = QTreeWidget()
        self.n_files.setHeaderLabels(["File", "Version", "Category", "Size", "Date"])
        self.n_files.setRootIsDecorated(False)
        self.n_files.header().setSectionResizeMode(0, QHeaderView.Stretch)
        self.n_files.setMaximumHeight(170)
        row = QHBoxLayout()
        self.n_dl = QPushButton("Download && install")
        self.n_dl.clicked.connect(self.download_selected_file)
        self.n_open = QPushButton("Open on Nexus")
        self.n_open.clicked.connect(lambda: self.n_current and QDesktopServices.openUrl(
            QUrl(nexus.mod_page(self.game.nexus_domain, self.n_current.mod_id))))
        row.addWidget(self.n_dl)
        row.addWidget(self.n_open)
        for x in (self.n_title, self.n_image):
            dl.addWidget(x)
        dl.addWidget(self.n_desc, 1)
        dl.addWidget(self.n_files)
        dl.addLayout(row)
        split.addWidget(det)
        split.setSizes([460, 700])
        lay.addWidget(split, 1)
        self.n_current: nexus.ModDetails | None = None
        return w

    def search_nexus(self, page: int):
        if page < 1:
            return
        self.page = page
        self.search_gen += 1
        gen, g = self.search_gen, self.game
        self.n_results.clear()
        self.n_page.setText("loading…")
        self._load_categories(g)
        Task(nexus.search, g.nexus_domain, self.n_search.text(), self.n_sort.currentData(), page,
             20, self.n_adult.isChecked(), self.n_cat.currentData() or "").start(
            lambda r: self._nexus_results(gen, r),
            lambda e: (self.n_page.setText("error"), self.statusBar().showMessage(e, 8000)))

    def _load_categories(self, g):
        """Fill the category filters (Nexus page and Collections page) for this game."""
        if self._cats_for == g.id:
            return
        self._cats_for = g.id
        from . import collections as nc

        def fill(box, items, keep_all="All categories"):
            self._cat_loading = True
            cur = box.currentData()
            box.clear()
            box.addItem(keep_all, "")
            for name, count in items:
                box.addItem(f"{name} ({count:,})" if count is not None else name, name)
            i = box.findData(cur)
            box.setCurrentIndex(max(i, 0))
            self._cat_loading = False

        def work():
            mods = nexus.mod_categories(g.nexus_domain)
            try:
                colls = nc.categories(g.nexus_game_id)
            except nexus.NexusError:
                colls = []
            try:
                vers = nc.game_versions(g.nexus_domain)
            except nexus.NexusError:
                vers = []
            return mods, colls, vers

        def done(res, gid=g.id):
            if self.game.id != gid:
                return
            fill(self.n_cat, res[0])
            fill(self.c_cat, [(n, None) for n in res[1]])
            self._fill_versions(res[2])

        def fail(_e):
            self._cats_for = None            # try again next time

        if self.n_cat.currentData():
            self._cat_loading = True
            self.n_cat.setCurrentIndex(0)
            self.c_cat.setCurrentIndex(0)
            self.c_ver.setCurrentIndex(0)
            self._cat_loading = False
        Task(work).start(done, fail)

    def _installed_version(self) -> str:
        from . import gameversions as gv
        return gv.installed_version(self.inst) if self.inst else ""

    def _fill_versions(self, versions):
        """Game-version filter: your version first, then the rest with collection counts."""
        from . import gameversions as gv
        mine = self._installed_version()
        self._cat_loading = True
        cur = self.c_ver.currentData()
        self.c_ver.clear()
        self.c_ver.addItem("Any game version", "")
        for v, n in sorted(versions, key=lambda vn: not gv.same(vn[0], mine)):
            label = f"{gv.short(v)} ({n:,})"
            if gv.same(v, mine):
                label = f"✔ {gv.short(v)}: your version ({n:,})"
            self.c_ver.addItem(label, v)
        i = self.c_ver.findData(cur)
        self.c_ver.setCurrentIndex(max(i, 0))
        self._cat_loading = False

    def _nexus_results(self, gen, res):
        if gen != self.search_gen:
            return
        mods, total = res
        pages = max(1, -(-total // 20))
        self.n_page.setText(f"{self.page}/{pages} ({total:,})")
        self.n_prev.setEnabled(self.page > 1)
        self.n_next.setEnabled(self.page < pages)
        have = {m.nexus_mod_id for m in self.inst.mods} if self.inst else set()
        for m in mods:
            it = QListWidgetItem(f"{m.name}\n{m.author} · {m.category} · 👍 {m.endorsements:,} · "
                                 f"⬇ {m.downloads:,}" + ("  ✔ installed" if m.mod_id in have else ""))
            it.setData(Qt.UserRole, m)
            it.setToolTip(m.summary)
            it.setSizeHint(QSize(0, 74))
            self.n_results.addItem(it)
            if m.thumb:
                self._thumb(m.thumb, it, gen)

    def _thumb(self, url, item, gen):
        if url in self.thumbs:
            item.setIcon(self.thumbs[url])
            return

        def done(img):
            pm = QPixmap.fromImage(img)
            if not pm.isNull():
                self.thumbs[url] = QIcon(pm)
                if gen is None or gen == self.search_gen:
                    try:
                        item.setIcon(self.thumbs[url])
                    except RuntimeError:
                        pass
        Task(lambda: load_image(nexus.fetch_bytes(url), 240)).start(done, lambda e: None)

    def _nexus_selected(self, it, _prev=None):
        if not it:
            return
        s: nexus.ModSummary = it.data(Qt.UserRole)
        g = self.game
        self.n_current = None
        self.n_sel = s
        self.n_title.setText(f"<h3>{s.name}</h3>loading…")
        self.n_desc.clear()
        self.n_files.clear()
        self.n_image.clear()

        def done(d: nexus.ModDetails):
            if self.n_sel is not s:
                return
            self.n_current = d
            self.n_title.setText(f"<h3>{d.name}</h3>by <b>{d.author}</b> · v{d.version} · "
                                 f"{d.category} · 👍 {d.endorsements:,} · ⬇ {d.downloads:,}"
                                 f"<br><i>{d.summary}</i>")
            self.n_desc.setHtml(d.description_html)
            for f in d.files:
                fi = QTreeWidgetItem([f.name, f.version, f.category.replace("_", " ").title(),
                                      fmt_size(f.size), fmt_date(f.date)])
                fi.setData(0, Qt.UserRole, f)
                fi.setToolTip(0, f.description)
                if not f.current:
                    fi.setForeground(0, self.palette().placeholderText())
                self.n_files.addTopLevelItem(fi)
            if self.n_files.topLevelItemCount():
                self.n_files.setCurrentItem(self.n_files.topLevelItem(0))
            if d.picture:
                Task(lambda: load_image(nexus.fetch_bytes(d.picture))).start(
                    lambda img: self._set_picture(s, img), lambda e: None)

        Task(nexus.mod_details, g.nexus_domain, g.nexus_game_id, s.mod_id).start(
            done, lambda e: self.n_title.setText(f"<h3>{s.name}</h3>{e}"))

    def _set_picture(self, s, img):
        if self.n_sel is not s:
            return
        pm = QPixmap.fromImage(img)
        if not pm.isNull():
            self.n_image.setPixmap(pm.scaled(520, 200, Qt.KeepAspectRatio, Qt.SmoothTransformation))

    def download_selected_file(self):
        d, fi = self.n_current, self.n_files.currentItem()
        if not d or not fi or not self.inst:
            if not self.inst:
                self.error("Game not set up", f"Point Kiln at your {self.game.name} folder first.")
            return
        f: nexus.NexusFile = fi.data(0, Qt.UserRole)
        self.download_nexus_file(self.inst, d.mod_id, f.file_id)

    def download_nexus_file(self, inst: Instance, mod_id: int, file_id: int):
        if not self.settings.get("nexus_api_key"):
            self.error("Nexus API key needed", "Add your personal API key in Settings first. "
                       "Find it at nexusmods.com → your profile → Site preferences → API Keys.")
            return
        if self.user and self.user.is_premium:
            self.start_download(inst, mod_id, file_id)
            return
        # Free account: the site hands us an nxm:// link with a one-time key.
        if self.browser():
            self.open_download_page(inst, mod_id, file_id)
            return
        if not desktop.is_nxm_handler():
            r = QMessageBox.question(
                self, "Free account download",
                "Free Nexus accounts download through the website's “Mod Manager Download” "
                "button, which sends an nxm:// link. Kiln isn't the nxm handler yet, so the "
                "link would go to another app.\n\nMake Kiln the nxm handler now?")
            if r == QMessageBox.Yes:
                self.register_nxm()
        QDesktopServices.openUrl(QUrl(nexus.manual_download_page(inst.game.nexus_domain, mod_id, file_id)))
        self.statusBar().showMessage("Click “Mod Manager Download” (slow download) on the page; "
                                     "Kiln picks it up from there.", 15000)

    # ----------------------------------------------------------- nxm links
    def handle_command(self, cmd: str):
        """Messages from `kiln --game/--play` (e.g. sent by the Lodestone launcher)."""
        from . import cli
        play = cmd.startswith(cli.PLAY_CMD)
        vanilla = cmd.startswith(cli.VANILLA_CMD)
        prefix = cli.PLAY_CMD if play else cli.VANILLA_CMD if vanilla else cli.GAME_CMD
        game_id = cmd[len(prefix):].strip()
        idx = next((i for i, g in enumerate(self.games) if g.id == game_id), None)
        self.showNormal()
        self.raise_()
        self.activateWindow()
        if idx is None:
            self.error("Unsupported game", f"Kiln doesn't manage “{game_id}”.")
            return
        if self.game_combo.currentIndex() != idx:
            self.game_combo.setCurrentIndex(idx)   # triggers select_game
            if self.game.id != game_id:
                self.select_game(idx)
        if play:
            QTimer.singleShot(200, self.launch)
        elif vanilla:
            QTimer.singleShot(200, self.launch_vanilla)

    def handle_nxm(self, uri: str, from_browser: bool = False):
        if not from_browser:      # links from Kiln's own browser shouldn't steal focus
            self.showNormal()
            self.raise_()
            self.activateWindow()
        try:
            link = nexus.parse_nxm(uri)
        except ValueError as e:
            self.error("Can't use this link", str(e))
            return
        g = by_domain(link.domain)
        if not g:
            self.error("Unsupported game", f"Kiln doesn't manage “{link.domain}” yet.")
            return
        run = self.crun
        if isinstance(link, nexus.NxmLink) and run and run.get("waiting") \
                and run["inst"].game.id == g.id and run["waiting"][0] == link.mod_id:
            self._crun_nxm(link)          # a collection is waiting for exactly this mod
            return
        idx = next(i for i, x in enumerate(self.games) if x.id == g.id)
        if self.game_combo.currentIndex() != idx:
            self.game_combo.setCurrentIndex(idx)
        inst = self.instances.get(g.id)
        if not inst:
            self.error("Game not set up", f"Point Kiln at your {g.name} folder first.")
            return
        if isinstance(link, nexus.NxmCollection):
            self.tabs.setCurrentWidget(self.coll_tab)
            self.show_collection(link.slug, link.revision)
            return
        self.start_download(inst, link.mod_id, link.file_id, link.key, link.expires,
                            switch_tab=not from_browser)
        if from_browser and self._browser:
            self._browser.set_background(f"Mod {link.mod_id}: download started, Kiln will install it.")

    # ----------------------------------------------------------- in-app browser
    def browser(self):
        """Kiln's Nexus browser window, or None if Qt WebEngine isn't available."""
        if self._browser is None:
            try:
                from .webview import NexusBrowser
            except ImportError:
                self._browser = False
                return None
            b = NexusBrowser(self)
            b.nxm.connect(lambda u: self.handle_nxm(u, from_browser=True))
            b.file_downloaded.connect(self._browser_file)
            b.skip_requested.connect(lambda: self.crun and self._crun_done(None, "skipped by you"))
            b.pause_requested.connect(self._crun_stop)
            self._browser = b
        return self._browser or None

    def open_download_page(self, inst: Instance, mod_id: int, file_id: int, hint: str = ""):
        url = nexus.manual_download_page(inst.game.nexus_domain, mod_id, file_id)
        b = self.browser()
        if not b:
            QDesktopServices.openUrl(QUrl(url))
            return
        b.open(url, {"downloads_dir": inst.downloads_dir, "game": inst.game.id,
                     "mod_id": mod_id, "file_id": file_id},
               hint or "Click the highlighted <b>Slow download</b> button. Kiln saves the file to "
                       f"your {inst.game.name} downloads and installs it.")

    def open_browser(self, url: str = ""):
        from .webview import HOME
        b = self.browser()
        if not b:
            QDesktopServices.openUrl(QUrl(url or HOME))
            return
        inst = self.inst
        b.open(url or HOME, {"downloads_dir": inst.downloads_dir, "game": inst.game.id} if inst else {})

    def _browser_file(self, path: str, ctx: dict):
        """A plain (non-nxm) download finished in Kiln's browser."""
        inst = self.instances.get(ctx.get("game", ""))
        p = Path(path)
        if not inst:
            self.statusBar().showMessage(f"Saved {p.name} to {p.parent}", 8000)
            return
        meta = {"name": clean_archive_name(p.name), "archive": p.name, "game": inst.game.id}
        if ctx.get("mod_id"):
            meta.update(nexus_mod_id=ctx["mod_id"], nexus_file_id=ctx.get("file_id"))
        Instance.write_download_meta(p, meta)
        if inst is self.inst:
            self.refresh_downloads()
        run = self.crun
        if run and run.get("waiting") and run["inst"] is inst and ctx.get("mod_id") == run["waiting"][0]:
            e = self._crun_entry()
            run["waiting"] = None
            e["status"] = "downloading"
            e["item"].setText(4, "installing…")
            self.install_archive(p, {**meta, "collection": run["rev"].slug}, inst,
                                 preset=e.get("preset"), after=lambda m, err, e=e: self._crun_done(m, err, entry=e))
            QTimer.singleShot(0, self._crun_next)
            return
        self.install_archive(p, meta, inst)

    # ----------------------------------------------------------- updates
    def check_updates(self):
        inst = self.inst
        tracked = [m for m in inst.mods if m.nexus_mod_id] if inst else []
        if not tracked:
            QMessageBox.information(self, "Updates", "No installed mods came from Nexus.")
            return
        self.statusBar().showMessage(f"Checking {len(tracked)} mod(s) for updates…")
        g = inst.game

        def work():
            out = {}
            # one request per 25 mods for "last updated", then file lists only for mods
            # changed since they were installed (instead of one request per mod)
            changed = nexus.updated_at(g.nexus_domain, [m.nexus_mod_id for m in tracked])
            for m in tracked:
                when = changed.get(m.nexus_mod_id)
                if when is not None and m.installed_at and when <= m.installed_at:
                    continue
                try:
                    files = nexus.mod_files(g.nexus_game_id, m.nexus_mod_id)
                except nexus.NexusError as e:
                    if "slow down" in str(e):
                        raise
                    continue
                mine = next((f for f in files if f.file_id == m.nexus_file_id), None)
                newer = [f for f in files if f.current and f.category == "MAIN"
                         and f.file_id != m.nexus_file_id and (not mine or f.date > mine.date)]
                if newer and (mine is None or not mine.current or mine.category == "MAIN"):
                    out[m.id] = max(newer, key=lambda f: f.date)
            return out

        def done(res):
            self.updates = res
            self.refresh_mods()
            self.statusBar().showMessage(f"{len(res)} update(s) available." if res
                                         else "Everything is up to date.", 8000)

        Task(work).start(done, lambda e: self.error("Update check failed", e))

    def update_selected(self):
        m = self._selected_mod()
        if m and m.id in self.updates:
            self.download_nexus_file(self.inst, m.nexus_mod_id, self.updates[m.id].file_id)
        elif m:
            self.statusBar().showMessage("No update known for this mod; run “Check for updates”.", 5000)

    # ================================================================ collections
    def _build_collections_tab(self) -> QWidget:
        from . import collections as nc
        w = QWidget()
        lay = QVBoxLayout(w)
        bar = QHBoxLayout()
        self.c_search = QLineEdit()
        self.c_search.setPlaceholderText("Search Nexus collections for this game…")
        self.c_search.returnPressed.connect(lambda: self.search_collections(1))
        self.c_sort = QComboBox()
        for k, t in nc.SORTS:
            self.c_sort.addItem(t, k)
        self.c_sort.currentIndexChanged.connect(lambda _: self.search_collections(1))
        self.c_cat = QComboBox()
        self.c_cat.setMinimumWidth(160)
        self.c_cat.setToolTip("Show only collections from one category")
        self.c_cat.addItem("All categories", "")
        self.c_cat.currentIndexChanged.connect(lambda _: not self._cat_loading and self.search_collections(1))
        self.c_ver = QComboBox()
        self.c_ver.setMinimumWidth(190)
        self.c_ver.setToolTip("Show only collections made for one game version")
        self.c_ver.addItem("Any game version", "")
        self.c_ver.currentIndexChanged.connect(lambda _: not self._cat_loading and self.search_collections(1))
        self.c_link = QLineEdit()
        self.c_link.setPlaceholderText("…or paste a collection link")
        self.c_link.returnPressed.connect(self.open_collection_link)
        self.c_prev, self.c_next, self.c_page = QPushButton("◀"), QPushButton("▶"), QLabel()
        self.c_prev.clicked.connect(lambda: self.search_collections(self.c_pageno - 1))
        self.c_next.clicked.connect(lambda: self.search_collections(self.c_pageno + 1))
        for x in (self.c_search, self.c_cat, self.c_ver, self.c_sort, self.c_link, self.c_prev, self.c_page, self.c_next):
            bar.addWidget(x, 2 if x is self.c_search else (1 if x is self.c_link else 0))
        lay.addLayout(bar)

        split = QSplitter()
        self.c_list = QListWidget()
        self.c_list.setIconSize(QSize(120, 68))
        self.c_list.currentItemChanged.connect(
            lambda it, _p=None: it and self.show_collection(it.data(Qt.UserRole).slug))
        split.addWidget(self.c_list)
        det = QWidget()
        dl = QVBoxLayout(det)
        dl.setContentsMargins(0, 0, 0, 0)
        self.c_title = QLabel("<i>Select a collection</i>")
        self.c_title.setWordWrap(True)
        self.c_desc = QTextBrowser()
        self.c_desc.setOpenExternalLinks(True)
        self.c_mods = QTreeWidget()
        self.c_mods.setHeaderLabels(["Mod", "File", "Version", "Size", "Status"])
        self.c_mods.setRootIsDecorated(False)
        self.c_mods.header().setSectionResizeMode(0, QHeaderView.Stretch)
        split2 = QSplitter(Qt.Vertical)
        split2.addWidget(self.c_desc)
        split2.addWidget(self.c_mods)
        split2.setSizes([200, 380])
        row = QHBoxLayout()
        self.c_install = QPushButton("Install collection")
        self.c_install.clicked.connect(self.install_collection)
        self.c_reopen = QPushButton("Show download page")
        self.c_reopen.clicked.connect(lambda: self._crun_open_page())
        self.c_skip = QPushButton("Skip this mod")
        self.c_skip.clicked.connect(lambda: self._crun_done(None, "skipped by you"))
        self.c_cancel = QPushButton("Pause")
        self.c_cancel.clicked.connect(self._crun_stop)
        self.c_open = QPushButton("Open on Nexus")
        self.c_open.clicked.connect(lambda: self.c_rev and QDesktopServices.openUrl(QUrl(self.c_rev.page)))
        self.c_remove = QPushButton("Remove this collection's mods")
        self.c_remove.clicked.connect(self.remove_collection)
        for b in (self.c_install, self.c_reopen, self.c_skip, self.c_cancel, self.c_open, self.c_remove):
            row.addWidget(b)
        row.addStretch(1)
        self.c_progress = QLabel()
        self.c_progress.setWordWrap(True)
        dl.addWidget(self.c_title)
        dl.addWidget(split2, 1)
        dl.addWidget(self.c_progress)
        dl.addLayout(row)
        split.addWidget(det)
        split.setSizes([420, 760])
        lay.addWidget(split, 1)
        self.c_rev = None
        self.c_pageno = 1
        self.c_gen = 0
        self._crun_buttons()
        return w

    def go_to(self, widget):
        self.tabs.setCurrentWidget(widget)

    def show_mod(self, m: Mod):
        """Jump to a mod in the Mods page and show it in the details pane."""
        self.go_to(self.mods_tab)
        self.mod_search.clear()
        self.mod_filter.setCurrentIndex(0)
        for i in range(self.mod_tree.topLevelItemCount()):
            it = self.mod_tree.topLevelItem(i)
            if it.data(0, Qt.UserRole) == m.id:
                self.mod_tree.setCurrentItem(it)
                self.mod_tree.scrollToItem(it)

    def show_nexus_mod(self, s: "nexus.ModSummary"):
        """Open a Nexus mod in the Nexus Mods page."""
        self.go_to(self.nexus_tab)
        it = QListWidgetItem(s.name)
        it.setData(Qt.UserRole, s)
        self.n_results.insertItem(0, it)
        self.n_results.setCurrentItem(it)

    def _tab_changed(self, idx):
        if self.tabs.widget(idx) is self.dash_tab:
            self.dash_tab.refresh()
        if self.tabs.widget(idx) is self.doctor_tab:
            self.doctor_tab.refresh()
        if self.tabs.widget(idx) is self.saves_tab:
            self.saves_tab.refresh()
        if self.tabs.widget(idx) is self.coll_tab and self.coll_loaded_for != self.game.id:
            self.search_collections(1)

    def search_collections(self, page: int):
        from . import collections as nc
        if page < 1:
            return
        self.coll_loaded_for = self.game.id
        self.c_pageno = page
        self.c_gen += 1
        gen = self.c_gen
        self.c_list.clear()
        self.c_page.setText("loading…")
        self._load_categories(self.game)
        Task(nc.search, self.game.nexus_domain, self.c_search.text(), self.c_sort.currentData(),
             page, 20, self.n_adult.isChecked(), self.c_cat.currentData() or "",
             self.c_ver.currentData() or "").start(
            lambda r: self._collections_loaded(gen, r),
            lambda e: (self.c_page.setText("error"), self.statusBar().showMessage(e, 8000)))

    def _collections_loaded(self, gen, res):
        if gen != self.c_gen:
            return
        items, total = res
        pages = max(1, -(-total // 20))
        self.c_page.setText(f"{self.c_pageno}/{pages} ({total:,})")
        self.c_prev.setEnabled(self.c_pageno > 1)
        self.c_next.setEnabled(self.c_pageno < pages)
        have = {c["slug"] for c in self.inst.collections} if self.inst else set()
        for c in items:
            from . import gameversions as gv
            mine = self._installed_version()
            vtxt = ""
            if c.game_versions:
                ok = any(gv.same(v, mine) for v in c.game_versions)
                vtxt = " · for " + ", ".join(gv.short(v) for v in c.game_versions[:2]) + \
                    (" ✔" if ok else (" ⚠ (you have " + gv.short(mine) + ")" if mine else ""))
            it = QListWidgetItem(f"{c.name}\n{c.author} · {c.mod_count} mods · {fmt_size(c.size)} · "
                                 f"👍 {c.endorsements:,}{vtxt}" + ("  ✔ installed" if c.slug in have else ""))
            it.setData(Qt.UserRole, c)
            it.setToolTip(c.summary)
            it.setSizeHint(QSize(0, 74))
            self.c_list.addItem(it)
            if c.thumb:
                self._thumb(c.thumb, it, None)

    def open_collection_link(self):
        m = re.search(r"collections/([a-z0-9]+)(?:/revisions/(\d+))?", self.c_link.text(), re.I)
        if not m:
            self.error("Not a collection link",
                       "Paste a link like https://www.nexusmods.com/games/skyrimspecialedition/collections/abc123")
            return
        self.show_collection(m.group(1), int(m.group(2)) if m.group(2) else None)

    def show_collection(self, slug: str, rev: int | None = None):
        from . import collections as nc
        if self.crun:
            self.statusBar().showMessage("A collection is being installed; showing its progress.", 5000)
            return
        g = self.game
        self.c_rev = None
        self.c_want = slug
        self.c_title.setText("loading…")
        self.c_desc.clear()
        self.c_mods.clear()
        self._crun_buttons()

        def done(r: "nc.CollectionRevision"):
            if self.c_want != slug:
                return
            self.c_rev = r
            req = sum(1 for m in r.mods if not m.optional)
            self.c_title.setText(
                f"<h3>{r.name}</h3>by <b>{r.author}</b> · revision {r.revision} · {req} required"
                f" + {len(r.mods) - req} optional mods · {fmt_size(r.total_size)}"
                + (f" · {r.bundled_count} bundled" if r.bundled_count else "")
                + self._collection_version_note(r)
                + (f"<br><i>{r.summary}</i>" if r.summary else ""))
            self.c_desc.setMarkdown(r.description_md or r.summary)
            have = {(m.nexus_mod_id, m.nexus_file_id) for m in self.inst.mods
                    if self.inst.in_profile(m)} if self.inst else set()
            have_mod = {m.nexus_mod_id for m in self.inst.mods} if self.inst else set()
            for m in sorted(r.mods, key=lambda m: (m.optional, m.mod_name.lower())):
                status = ("installed" if (m.mod_id, m.file_id) in have
                          else "other version installed" if m.mod_id in have_mod else "")
                if m.category in ("ARCHIVED", "REMOVED", "DELETED"):
                    status = (status + " · " if status else "") + "file archived on Nexus"
                it = QTreeWidgetItem([m.mod_name + ("  (optional)" if m.optional else ""), m.file_name,
                                      m.version, fmt_size(m.size_kb * 1024), status])
                it.setData(0, Qt.UserRole, m)
                it.setFlags(it.flags() | Qt.ItemIsUserCheckable)
                it.setCheckState(0, Qt.Unchecked if m.optional or (m.mod_id, m.file_id) in have
                                 else Qt.Checked)
                self.c_mods.addTopLevelItem(it)
            for i in range(1, 5):
                self.c_mods.resizeColumnToContents(i)
            self._restore_paused(r)
            self._crun_buttons()

        Task(nc.revision, g.nexus_domain, slug, rev).start(
            done, lambda e: self.c_title.setText(f"<b>Couldn't load collection:</b> {e}"))

    def _collection_version_note(self, r) -> str:
        from . import gameversions as gv
        if not r.game_versions:
            return ""
        mine = self._installed_version()
        vs = ", ".join(gv.short(v) for v in r.game_versions)
        if not mine or any(gv.same(v, mine) for v in r.game_versions):
            return f"<br>Made for game version {vs}" + (" ✔ (yours)" if mine else "")
        return (f"<br><b style='color:#f08c00'>⚠ Made for game version {vs}; you have "
                f"{gv.short(mine)}.</b> Script-extender mods in it may not load. Use the "
                "“Game …” button in the banner to switch versions.")

    # ----------------------------------------------------------- pause / resume
    def _run_file(self, inst: Instance, slug: str) -> Path:
        return inst.staging / "collections" / f"{slug}.run.json"

    def _save_paused(self, inst: Instance, r, selected: list[tuple[int, int]]):
        f = self._run_file(inst, r.slug)
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_text(json.dumps({"revision": r.revision, "selected": selected}))

    def _restore_paused(self, r):
        """Tick the mods a paused run of this collection still has to install."""
        self.c_resume = 0
        if not self.inst:
            return
        try:
            saved = json.loads(self._run_file(self.inst, r.slug).read_text())
        except (OSError, ValueError):
            return
        if saved.get("revision") != r.revision:
            return
        want = {tuple(x) for x in saved.get("selected", [])}
        have = {(m.nexus_mod_id, m.nexus_file_id) for m in self.inst.mods if self.inst.in_profile(m)}
        left = 0
        for i in range(self.c_mods.topLevelItemCount()):
            it = self.c_mods.topLevelItem(i)
            m = it.data(0, Qt.UserRole)
            pending = (m.mod_id, m.file_id) in want and (m.mod_id, m.file_id) not in have
            it.setCheckState(0, Qt.Checked if pending else Qt.Unchecked)
            left += pending
        self.c_resume = left
        if left:
            self.c_progress.setText(f"Paused earlier with {left} mod(s) left. Press "
                                    "“Resume install” to carry on.")

    def _crun_buttons(self):
        running = bool(self.crun)
        waiting = bool(running and self.crun.get("waiting"))
        resume = getattr(self, "c_resume", 0)
        self.c_install.setText(f"Resume install ({resume} left)" if resume and not running
                               else "Install collection")
        self.c_install.setEnabled(bool(self.c_rev) and not running and self.inst is not None)
        self.c_reopen.setVisible(waiting)
        self.c_skip.setVisible(running)
        self.c_cancel.setVisible(running)
        self.c_remove.setVisible(bool(self.c_rev and self.inst and not running and
                                      any(m.collection == self.c_rev.slug for m in self.inst.mods)))

    def remove_collection(self):
        r, inst = self.c_rev, self.inst
        mods = [m for m in inst.mods if m.collection == r.slug] if (r and inst) else []
        if not mods or QMessageBox.question(
                self, "Remove collection", f"Remove the {len(mods)} mod(s) installed by “{r.name}”?"
                ) != QMessageBox.Yes:
            return
        for m in mods:
            inst.remove(m)
        inst.collections = [c for c in inst.collections if c["slug"] != r.slug]
        inst.save()
        self.refresh_mods()
        self.schedule_deploy()
        self.show_collection(r.slug)

    # ----------------------------------------------------------- collection runner
    def install_collection(self):
        from . import collections as nc
        r, inst = self.c_rev, self.inst
        if not r or not inst or self.crun:
            return
        key = self.settings.get("nexus_api_key", "")
        if not key:
            self.error("Nexus API key needed", "Collections need your Nexus API key (Settings).")
            return
        premium = bool(self.user and self.user.is_premium)
        entries = []
        for i in range(self.c_mods.topLevelItemCount()):
            it = self.c_mods.topLevelItem(i)
            if it.checkState(0) == Qt.Checked:
                entries.append({"kind": "nexus", "mod": it.data(0, Qt.UserRole), "item": it,
                                "status": "queued"})
                it.setText(4, "queued")
        if not entries and not r.bundled_count:
            self.error("Nothing selected", "Tick the mods to install.")
            return
        from . import gameversions as gv
        mine = gv.installed_version(inst)
        if r.game_versions and mine and not any(gv.same(v, mine) for v in r.game_versions) \
                and not getattr(self, "_skip_version_warning", False):
            want = r.game_versions[0]
            can = [v for v in gv.for_game(inst.game.id) if gv.same(v.version, want)] or \
                [v for v in gv.stored(inst) if gv.same(v, want)]
            box = QMessageBox(self)
            box.setIcon(QMessageBox.Warning)
            box.setWindowTitle("Different game version")
            box.setText(f"“{r.name}” was made for {inst.game.name} {gv.short(want)}, and you have "
                        f"{gv.short(mine)}.")
            box.setInformativeText(
                "Mods that hook into the game's code (script-extender plugins) are tied to a game "
                "version, so some of this collection may not load, or may crash the game. "
                + (f"Kiln can switch your game to {gv.short(want)} first (and back later)."
                   if can else "Kiln can't switch to that version for you yet."))
            sw = box.addButton(f"Switch to {gv.short(want)}…", QMessageBox.AcceptRole) if can else None
            box.addButton("Install anyway", QMessageBox.DestructiveRole)
            box.addButton(QMessageBox.Cancel)
            box.exec()
            if (sw is not None and box.clickedButton() is sw) or \
                    box.standardButton(box.clickedButton()) == QMessageBox.Cancel:
                for e in entries:
                    e["item"].setText(4, "")
                if sw is not None and box.clickedButton() is sw:
                    self.open_versions()
                return
        msg = (f"Install {len(entries)} mod(s) from “{r.name}”?\n\n")
        if premium:
            msg += "Premium: files download automatically, one after another."
        else:
            if self.browser():
                msg += ("Free account: Kiln opens each mod's download page in its own browser "
                        "window with the “Slow download” button highlighted. Click it and the "
                        "next mod's page opens straight away while the file downloads and "
                        "installs in the background. You can pause and resume later.")
            else:
                if not desktop.is_nxm_handler():
                    msg += ("⚠ Kiln isn't the nxm:// handler, so download buttons won't reach it. "
                            "Fix that in Settings first.\n\n")
                msg += ("Free account: Kiln opens each mod's download page in your browser. Click "
                        "“Slow download” each time and Kiln installs it, then opens the next one.")
        if QMessageBox.question(self, "Install collection", msg) != QMessageBox.Yes:
            for e in entries:
                e["item"].setText(4, "")
            return
        self.crun = {"inst": inst, "rev": r, "entries": entries, "manifest": None, "waiting": None,
                     "premium": premium, "started": time.time()}
        self.c_resume = 0
        self._save_paused(inst, r, [(e["mod"].mod_id, e["mod"].file_id) for e in entries])
        self._crun_buttons()
        self.c_progress.setText("Downloading the collection manifest…")

        def work():
            url = nc.manifest_download_url(key, r.download_path)
            from .downloader import download
            d = inst.staging / "collections"
            arc = download(url, d, fallback_name=f"{r.slug}-{r.revision}.7z")
            return nc.load_manifest(arc, inst.staging)

        def done(man):
            self.crun["manifest"] = man
            for m in man.bundled():
                it = QTreeWidgetItem([m.name + "  (bundled)", m.file_expression, "", "", "queued"])
                self.c_mods.addTopLevelItem(it)
                self.crun["entries"].append({"kind": "bundle", "man": m, "item": it, "status": "queued"})
            for m in man.external():
                it = QTreeWidgetItem([m.name + "  (external)", m.url, "", "", "manual: see summary"])
                self.c_mods.addTopLevelItem(it)
                self.crun["entries"].append({"kind": "external", "man": m, "item": it,
                                             "status": "manual"})
            self._crun_next()

        def fail(e):
            if QMessageBox.question(
                    self, "Collection manifest",
                    f"Couldn't get the collection's manifest:\n{e}\n\nInstall the mods anyway? "
                    "Saved installer choices, load-order rules and bundled files will be "
                    "missing.") == QMessageBox.Yes:
                self._crun_next()
            else:
                self._crun_stop()

        Task(work).start(done, fail)

    def _crun_entry(self):
        return next((e for e in self.crun["entries"] if e["status"] == "active"), None)

    def _crun_progress(self):
        run = self.crun
        total = len(run["entries"])
        done_n = sum(1 for e in run["entries"] if e["status"] in ("done", "failed", "manual"))
        bg = sum(1 for e in run["entries"] if e["status"] == "downloading")
        self.c_progress.setText(f"Progress: {done_n}/{total}"
                                + (f" · {bg} downloading/installing in the background" if bg else ""))
        if self._browser:
            self._browser.set_background(f"{bg} file(s) downloading or installing in the background."
                                         if bg else "")
        return done_n, total

    def _crun_next(self):
        run = self.crun
        if not run or self._crun_entry():
            return
        inst = run["inst"]
        done_n, total = self._crun_progress()
        nxt = next((e for e in run["entries"] if e["status"] == "queued"), None)
        if nxt is None:
            if not any(e["status"] == "downloading" for e in run["entries"]):
                self._crun_finish()
            elif self._browser:
                self._browser.set_run("All pages done. Waiting for the last downloads to finish…",
                                      done_n, total)
            return
        nxt["status"] = "active"
        it = nxt["item"]
        man = run["manifest"]
        extra = {"collection": run["rev"].slug}
        if nxt["kind"] == "bundle":
            p = man.bundled_path(nxt["man"])
            if p is None:
                self._crun_done(None, "bundled file missing from the collection")
                return
            it.setText(4, "installing…")
            meta = {"name": nxt["man"].name, **extra}
            prev = next((x for x in inst.mods if x.collection == run["rev"].slug
                         and x.name == nxt["man"].name and not x.nexus_mod_id), None)
            if prev:
                meta["replace_id"] = prev.id   # re-running the collection updates it in place
            self.install_archive(p, meta, inst, preset=nxt["man"].choices, after=self._crun_done)
            return
        m = nxt["mod"]
        mm = man.for_file(m.mod_id, m.file_id) if man else None
        nxt["preset"] = mm.choices if mm else None
        mine = inst.find_nexus_file(m.mod_id, m.file_id)
        if mine and mine.nexus_file_id == m.file_id:
            mine.collection = mine.collection or run["rev"].slug
            shared = not inst.in_profile(mine)
            inst.add_to_profile([mine])           # downloaded for another profile: reuse it
            self._crun_done(mine, None, status="✔ reused (already downloaded)" if shared
                            else "already installed")
            return
        if isinstance(it, QTreeWidgetItem) and it.treeWidget() is self.c_mods:
            self.c_mods.scrollToItem(it)
        if run["premium"]:
            it.setText(4, "downloading…")
            self.start_download(inst, m.mod_id, m.file_id, preset=nxt["preset"],
                                after=self._crun_done, extra_meta=extra, switch_tab=False)
        else:
            run["waiting"] = (m.mod_id, m.file_id)
            it.setText(4, "⏳ click “Slow download”")
            self._crun_buttons()
            self._crun_open_page()

    def _crun_open_page(self):
        run = self.crun
        if not (run and run.get("waiting")):
            return
        mid, fid = run["waiting"]
        e = self._crun_entry()
        done_n, total = self._crun_progress()
        b = self.browser()
        if b and e:
            pos = run["entries"].index(e) + 1
            b.set_run(f"<b>{run['rev'].name}</b> — mod {pos} of {total}: <b>{e['mod'].mod_name}</b>"
                      f" ({e['mod'].file_name})", done_n, total)
        self.open_download_page(run["inst"], mid, fid)

    def _crun_nxm(self, link: "nexus.NxmLink"):
        run = self.crun
        e = self._crun_entry()
        run["waiting"] = None
        self._crun_buttons()
        if not e:
            return
        # Download in the background and move straight on to the next mod's page.
        e["status"] = "downloading"
        e["item"].setText(4, "downloading…")
        self.start_download(run["inst"], link.mod_id, link.file_id, link.key, link.expires,
                            preset=e.get("preset"),
                            after=lambda m, err, e=e: self._crun_done(m, err, entry=e),
                            extra_meta={"collection": run["rev"].slug}, switch_tab=False)
        QTimer.singleShot(0, self._crun_next)

    def _crun_done(self, mod, err, status=None, entry=None):
        run = self.crun
        if not run:
            return
        e = entry or self._crun_entry()
        if e is None or e["status"] not in ("active", "downloading"):
            return
        if e["status"] == "active":
            run["waiting"] = None
        if mod is not None:
            e["status"], e["mod_id"] = "done", mod.id
            if mod.collection != run["rev"].slug and not mod.collection:
                mod.collection = run["rev"].slug
                run["inst"].save()
            e["item"].setText(4, status or "✔ installed")
        else:
            e["status"], e["error"] = "failed", str(err)
            e["item"].setText(4, f"✖ {str(err).splitlines()[0][:80]}")
            e["item"].setToolTip(4, str(err))
        self._crun_buttons()
        QTimer.singleShot(0, self._crun_next)

    def _crun_stop(self):
        run = self.crun
        if not run:
            return
        for e in run["entries"]:
            if e["status"] in ("queued", "active"):
                e["status"] = "paused"
                e["item"].setText(4, "paused")
            elif e["status"] == "downloading":
                e["item"].setText(4, "finishing in the background")
        self._crun_finish(stopped=True)

    def _crun_finish(self, stopped=False):
        from . import collections as nc
        run, self.crun = self.crun, None
        inst, r, man = run["inst"], run["rev"], run["manifest"]
        ok = [e for e in run["entries"] if e["status"] == "done"]
        failed = [e for e in run["entries"] if e["status"] == "failed"]
        manual = [e for e in run["entries"] if e["status"] == "manual"]
        paused = [e for e in run["entries"] if e["status"] in ("paused", "downloading")]
        if self._browser:
            self._browser.clear_run()
            self._browser.set_background("")
        if not stopped:
            self._run_file(inst, r.slug).unlink(missing_ok=True)
        else:
            self.c_resume = sum(1 for e in paused if e["kind"] == "nexus" and e["status"] == "paused")
        # load order from the collection's rules
        if man:
            ids = {m.nexus_mod_id: m.id for m in inst.mods if m.collection == r.slug and m.nexus_mod_id}
            order = nc.order_by_rules(man, ids, [m.id for m in inst.mods])
            inst.mods.sort(key=lambda m: order.index(m.id))
        if ok and not stopped and not run.get("essentials"):
            inst.collections = [c for c in inst.collections if c["slug"] != r.slug] + [
                {"slug": r.slug, "name": r.name, "revision": r.revision, "installed_at": int(time.time())}]
        inst.save()
        self.refresh_mods()
        self._crun_buttons()
        self.c_progress.setText(
            f"Paused: {len(ok)} installed"
            + (f", {sum(e['status'] == 'downloading' for e in paused)} finishing in the background"
               if any(e['status'] == 'downloading' for e in paused) else "")
            + f", {sum(e['status'] == 'paused' and e['kind'] == 'nexus' for e in paused)} left. "
            "Press “Resume install” to carry on, now or after restarting Kiln." if stopped else
            f"Finished: {len(ok)} installed, {len(failed)} failed, {len(manual)} need manual steps."
            + (f" Press “Install collection” to try the {len(failed)} failed one(s) again."
               if failed else ""))
        if failed and not stopped:
            for e in run["entries"]:              # leave only the failed ones ticked for a retry
                if e.get("item") is not None and e["status"] != "failed":
                    e["item"].setCheckState(0, Qt.Unchecked)
        self._crun_buttons()

        def deployed(res):
            if man and man.plugins and inst.game.has_plugins:
                inst.apply_plugin_order(man.plugins)
                inst.game.write_plugins_txt(inst, [(p["name"], p["enabled"]) for p in inst.plugins])
            self._deployed(inst, res, 0)
            lines = []
            if failed:
                lines.append("Not installed:\n" + "\n".join(
                    f"• {(e.get('mod') and e['mod'].mod_name) or e['man'].name}: {e.get('error')}"
                    for e in failed))
            if manual:
                lines.append("Install these yourself (outside Nexus):\n" + "\n".join(
                    f"• {e['man'].name}: {e['man'].url}" + (f"\n  {e['man'].instructions}"
                                                           if e['man'].instructions else "")
                    for e in manual))
            if man and man.info.get("installInstructions"):
                lines.append("Curator's instructions:\n" + man.info["installInstructions"])
            if run.get("on_finish") and not stopped:
                QTimer.singleShot(0, run["on_finish"])
            if stopped:
                if man:
                    man.cleanup()
                self.statusBar().showMessage(f"{r.name}: paused with {len(paused)} mod(s) left.", 10000)
                return
            summary = f"{len(ok)} mod(s) installed and deployed."
            # The browser window was only open for this run, and a dialog behind it
            # makes Kiln look frozen: close it and bring Kiln forward first.
            if self._browser is not None and self._browser.isVisible():
                self._browser.hide()
            self.showNormal()
            self.raise_()
            self.activateWindow()
            if run.get("essentials") and not lines:
                # a small queue (essentials, matching files) that went fine: no dialog to dismiss
                self.statusBar().showMessage(f"{r.name}: {summary}", 15000)
                self._notify(r.name, summary)
            else:
                QMessageBox.information(self, r.name, summary + ("\n\n" + "\n\n".join(lines) if lines else ""))
            if man:
                man.cleanup()

        Task(deploy.deploy, inst).start(deployed, lambda e: self.error("Deploy failed", e),
                                        pool=self.jobs)

    # ================================================================ settings
    def _build_settings_tab(self) -> QWidget:
        w = QWidget()
        lay = QVBoxLayout(w)
        box = QGroupBox("Nexus Mods account")
        f = QFormLayout(box)
        self.key_edit = QLineEdit(self.settings.get("nexus_api_key", ""))
        self.key_edit.setEchoMode(QLineEdit.Password)
        self.key_edit.setPlaceholderText("Personal API key from nexusmods.com/users/myaccount?tab=api")
        kb = QHBoxLayout()
        kb.addWidget(self.key_edit, 1)
        b = QPushButton("Save && check")
        b.clicked.connect(lambda: self.validate_key())
        kb.addWidget(b)
        b = QPushButton("Get key…")
        b.clicked.connect(lambda: QDesktopServices.openUrl(
            QUrl("https://www.nexusmods.com/users/myaccount?tab=api")))
        kb.addWidget(b)
        f.addRow("API key:", kb)
        self.user_label = QLabel("Not checked")
        f.addRow("Account:", self.user_label)
        b = QPushButton("Open Nexus in Kiln's browser (sign in once for free downloads)")
        b.clicked.connect(lambda: self.open_browser("https://users.nexusmods.com/auth/sign_in"))
        f.addRow("", b)
        lay.addWidget(box)

        box = QGroupBox("nxm:// links (“Mod Manager Download” buttons)")
        v = QVBoxLayout(box)
        self.nxm_label = QLabel()
        self.nxm_label.setWordWrap(True)
        v.addWidget(self.nxm_label)
        r = QHBoxLayout()
        b = QPushButton("Make Kiln the nxm handler")
        b.clicked.connect(self.register_nxm)
        r.addWidget(b)
        b = QPushButton("Give it back to the previous app")
        b.clicked.connect(self.unregister_nxm)
        r.addWidget(b)
        r.addStretch(1)
        v.addLayout(r)
        lay.addWidget(box)

        box = QGroupBox("Game versions")
        v = QVBoxLayout(box)
        r = QHBoxLayout()
        r.addWidget(QLabel("When essentials (script extender, Address Library) don't match the game version:"))
        self.ess_mode = QComboBox()
        for label, key in (("Ask me", "ask"), ("Swap in the right build automatically", "auto"),
                           ("Do nothing (the banner still shows it)", "off")):
            self.ess_mode.addItem(label, key)
        self.ess_mode.setCurrentIndex(max(0, self.ess_mode.findData(self.settings.get("essentials_match", "ask"))))
        self.ess_mode.currentIndexChanged.connect(
            lambda _i: self.settings.set("essentials_match", self.ess_mode.currentData()))
        r.addWidget(self.ess_mode)
        r.addStretch(1)
        v.addLayout(r)
        note = QLabel("This happens after Steam updates a game or you switch versions. Kiln picks the "
                      "build made for your version from the mod's Nexus files (older ones included).")
        note.setWordWrap(True)
        note.setStyleSheet("color: gray;")
        v.addWidget(note)

        cb = QCheckBox("End the game if its window closes but it keeps running (fixes hangs on quit)")
        cb.setChecked(self.settings.get("end_hung_game", True))
        cb.toggled.connect(lambda x: self.settings.set("end_hung_game", x))
        v.addWidget(cb)
        lay.addWidget(box)

        box = QGroupBox("AI helper (optional)")
        v = QVBoxLayout(box)
        self.ai_on = QCheckBox("Let a local AI explain problems and answer questions (Ollama)")
        self.ai_on.setChecked(self.settings.get("ai_enabled", False))
        self.ai_on.toggled.connect(lambda x: (self.settings.set("ai_enabled", x), self.doctor_tab._fill()))
        v.addWidget(self.ai_on)
        r = QHBoxLayout()
        r.addWidget(QLabel("Ollama:"))
        self.ai_url = QLineEdit(self.settings.get("ai_url") or "http://localhost:11434")
        self.ai_url.editingFinished.connect(lambda: (self.settings.set("ai_url", self.ai_url.text().strip()),
                                                     self.refresh_ai_models()))
        r.addWidget(self.ai_url, 1)
        r.addWidget(QLabel("Model:"))
        self.ai_model = QComboBox()
        self.ai_model.setMinimumWidth(240)
        self.ai_model.currentIndexChanged.connect(self._ai_model_chosen)
        r.addWidget(self.ai_model)
        b = QPushButton("Refresh")
        b.clicked.connect(self.refresh_ai_models)
        r.addWidget(b)
        v.addLayout(r)
        self.ai_status = QLabel("Install Ollama (ollama.com) and pull a model, e.g. “ollama pull qwen3”. "
                                "Everything stays on your computer. The AI only explains; it never changes "
                                "your mods.")
        self.ai_status.setWordWrap(True)
        self.ai_status.setStyleSheet("color: gray;")
        v.addWidget(self.ai_status)
        lay.addWidget(box)
        QTimer.singleShot(0, self.refresh_ai_models)

        box = QGroupBox("Mods you download yourself")
        v = QVBoxLayout(box)
        cb = QCheckBox("Install mod archives that arrive in this folder")
        cb.setToolTip("For files you download from the Nexus website (“Manual download”) or a "
                      "script extender's own site. Kiln recognises Nexus files by their name, checks "
                      "them on Nexus, copies them to its downloads and installs them into the right game.")
        cb.setChecked(self.settings.get("watch_downloads", True))
        cb.toggled.connect(lambda x: (self.settings.set("watch_downloads", x), self.start_watching()))
        v.addWidget(cb)
        r = QHBoxLayout()
        self.watch_label = QLabel(str(self.watch_folder()))
        r.addWidget(self.watch_label, 1)
        b = QPushButton("Change…")

        def pick():
            from PySide6.QtWidgets import QFileDialog
            d = QFileDialog.getExistingDirectory(self, "Folder to watch", str(self.watch_folder()))
            if d:
                self.settings.set("watch_folder", d)
                self.watch_label.setText(d)
                self.start_watching()
        b.clicked.connect(pick)
        r.addWidget(b)
        v.addLayout(r)
        lay.addWidget(box)

        box = QGroupBox("Deployment")
        v = QVBoxLayout(box)
        cb = QCheckBox("Auto-sort the load order after installing a mod")
        cb.setChecked(self.settings.get("auto_sort", False))
        cb.toggled.connect(lambda x: self.settings.set("auto_sort", x))
        v.addWidget(cb)
        cb = QCheckBox("Deploy automatically after every change")
        cb.setChecked(self.settings.get("auto_deploy", True))
        cb.toggled.connect(lambda x: self.settings.set("auto_deploy", x))
        v.addWidget(cb)
        self.game_notes = QLabel()
        self.game_notes.setWordWrap(True)
        v.addWidget(self.game_notes)
        lay.addWidget(box)
        lay.addStretch(1)
        return w

    def refresh_settings(self):
        cur = desktop.current_nxm_handler()
        if cur == desktop.DESKTOP_ID:
            self.nxm_label.setText("✔ Kiln handles nxm:// links.")
        else:
            self.nxm_label.setText(f"nxm:// links currently open in: <b>{cur or 'nothing'}</b>. "
                                   "Free accounts need Kiln to be the handler to download.")
        g = self.game
        notes = f"<b>{g.name}</b>: mods deploy into “{g.deploy_root or 'the game folder'}”. {g.notes}"
        if self.inst:
            notes += (f"<br>Staging: {self.inst.staging} (must be on the same drive as the game "
                      "for hardlinks; otherwise files are copied)")
        self.game_notes.setText(notes)

    def register_nxm(self):
        prev = desktop.current_nxm_handler()
        if prev and prev != desktop.DESKTOP_ID:
            self.settings.set("previous_nxm_handler", prev)
        desktop.register_nxm()
        self.refresh_settings()
        self.statusBar().showMessage("Kiln now handles nxm:// links.", 6000)

    def unregister_nxm(self):
        desktop.unregister_nxm(self.settings.get("previous_nxm_handler", ""))
        self.refresh_settings()

    def validate_key(self, quiet=False):
        key = self.key_edit.text().strip()
        self.settings.set("nexus_api_key", key)
        if not key:
            self.user = None
            self.user_label.setText("No key")
            return
        self.user_label.setText("checking…")

        def done(u: nexus.User):
            self.user = u
            self.load_endorsements()
            self.user_label.setText(f"{u.name} · {'Premium ✔ (direct downloads)' if u.is_premium else 'Free (downloads via the site button)'}")

        def fail(e):
            self.user = None
            self.user_label.setText(f"⚠ {e}")
            if not quiet:
                self.error("API key", e)

        Task(nexus.validate_key, key).start(done, fail)

    # ================================================================ misc
    def refresh_all(self):
        self.refresh_profiles()
        self.refresh_top()
        self.refresh_mods()
        self.refresh_plugins()
        self.refresh_downloads()
        self.refresh_settings()
        if self.tabs.currentWidget() is self.saves_tab:
            self.saves_tab.refresh()

    def dialog_parent(self):
        """The Kiln window you're looking at: dialogs open over it, not hidden behind it."""
        b = self._browser
        return b if (b is not None and b.isVisible() and b.isActiveWindow()) else self

    def error(self, title, msg):
        self.statusBar().clearMessage()
        QMessageBox.critical(self.dialog_parent(), title, str(msg))


# --------------------------------------------------------------------------
# entry point (single instance; nxm links are forwarded to the running window)
# --------------------------------------------------------------------------
def _forward(uri: str | None) -> bool:
    sock = QLocalSocket()
    sock.connectToServer(SERVER_NAME)
    if not sock.waitForConnected(300):
        return False
    sock.write((uri or "").encode() + b"\n")
    sock.flush()
    sock.waitForBytesWritten(1000)
    sock.disconnectFromServer()
    return True


def run(uri: str | None = None) -> int:
    QApplication.setApplicationName("kiln")
    QApplication.setDesktopFileName("kiln")
    QApplication.setAttribute(Qt.AA_ShareOpenGLContexts)
    try:
        from . import webview  # noqa: F401  (must load before QApplication)
    except ImportError:
        pass
    app = QApplication(sys.argv[:1])
    app.setWindowIcon(QIcon(str(Path(__file__).resolve().parent / "assets" / "kiln.svg")))
    if _forward(uri):
        return 0
    from .splash import Splash
    splash = Splash()
    splash.show()
    app.processEvents()
    win = MainWindow(splash)
    server = QLocalServer()
    QLocalServer.removeServer(SERVER_NAME)
    server.listen(SERVER_NAME)

    def on_conn():
        s = server.nextPendingConnection()

        def read():
            data = bytes(s.readAll()).decode(errors="replace").strip()
            if data.startswith("kiln:"):
                win.handle_command(data)
            elif data:
                win.handle_nxm(data)
            else:
                win.showNormal(); win.raise_(); win.activateWindow()
        s.readyRead.connect(read)
        s.disconnected.connect(s.deleteLater)

    server.newConnection.connect(on_conn)
    app.aboutToQuit.connect(QThreadPool.globalInstance().clear)
    win._splash = None
    splash.step("Ready")
    win.show()
    splash.close()
    if uri and uri.startswith("kiln:"):
        QTimer.singleShot(300, lambda: win.handle_command(uri))
    elif uri:
        QTimer.singleShot(300, lambda: win.handle_nxm(uri))
    return app.exec()
