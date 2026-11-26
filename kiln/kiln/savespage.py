"""The Saves page: each profile's saves with their screenshots, who and where,
and a warning when a save was made with mods this profile doesn't load.
Copy or move saves between profiles, delete them (to the Trash), back them up."""
from __future__ import annotations

import html
import time
from typing import TYPE_CHECKING

from PySide6.QtCore import QSize, Qt, QUrl
from PySide6.QtGui import QDesktopServices, QIcon, QImage, QPixmap
from PySide6.QtWidgets import (
    QCheckBox, QComboBox, QFrame, QHBoxLayout, QLabel, QListWidget, QListWidgetItem, QMenu,
    QMessageBox, QPushButton, QSplitter, QToolButton, QVBoxLayout, QWidget,
)

from . import savegames as sg

if TYPE_CHECKING:
    from .gui import MainWindow

THUMB = QSize(160, 96)


def _pixmap(s: sg.Save, size: QSize | None = None) -> QPixmap | None:
    if not s.shot:
        return None
    w, h, pix = s.shot
    if not w or not h or len(pix) < w * h * 4:
        return None
    img = QImage(pix, w, h, w * 4, QImage.Format_RGBA8888).copy()
    pm = QPixmap.fromImage(img)
    return pm.scaled(size, Qt.KeepAspectRatio, Qt.SmoothTransformation) if size else pm


def _when(t: float) -> str:
    days = (time.time() - t) / 86400
    if days < 1 and time.localtime(t).tm_mday == time.localtime().tm_mday:
        return "today " + time.strftime("%H:%M", time.localtime(t))
    return time.strftime("%d %b %Y %H:%M", time.localtime(t))


class SavesPage(QWidget):
    def __init__(self, host: "MainWindow"):
        super().__init__()
        self.host = host
        self.saves: list[sg.Save] = []
        self.have: set[str] = set()
        self._gen = 0
        lay = QVBoxLayout(self)
        head = QHBoxLayout()
        txt = QVBoxLayout()
        self.title = QLabel("<b style='font-size:15px'>Saves</b>")
        self.where = QLabel()
        self.where.setStyleSheet("color: gray;")
        self.where.setTextInteractionFlags(Qt.TextSelectableByMouse)
        txt.addWidget(self.title)
        txt.addWidget(self.where)
        head.addLayout(txt, 1)
        self.backup_btn = QPushButton("Back up these saves")
        self.backup_btn.setToolTip("Zip every save in this folder into Kiln's backups folder")
        self.backup_btn.clicked.connect(self.backup)
        self.folder_btn = QPushButton("Open folder")
        self.folder_btn.clicked.connect(self.open_folder)
        self.more = QToolButton()
        self.more.setText("⋯")
        self.more.setPopupMode(QToolButton.InstantPopup)
        self.more_menu = QMenu(self.more)
        self.more.setMenu(self.more_menu)
        head.addWidget(self.backup_btn)
        head.addWidget(self.folder_btn)
        head.addWidget(self.more)
        lay.addLayout(head)

        # "one shared folder" banner
        self.banner = QFrame()
        self.banner.setObjectName("savebanner")
        self.banner.setStyleSheet("#savebanner { border: 1px solid palette(mid); border-left: 5px solid #4dabf7;"
                                  " border-radius: 6px; }")
        bl = QHBoxLayout(self.banner)
        self.banner_text = QLabel(
            "<b>All your profiles share one save folder.</b><br>Loading a save made with a different set of "
            "mods is one of the most common ways saves break. With separate saves, each profile only sees "
            "its own saves, and the game saves into that profile's folder.")
        self.banner_text.setWordWrap(True)
        bl.addWidget(self.banner_text, 1)
        self.on_btn = QPushButton("Give each profile its own saves…")
        self.on_btn.clicked.connect(self.turn_on)
        bl.addWidget(self.on_btn)
        lay.addWidget(self.banner)

        opts = QHBoxLayout()
        opts.addWidget(QLabel("Show:"))
        self.folder_combo = QComboBox()
        self.folder_combo.setMinimumWidth(220)
        self.folder_combo.currentIndexChanged.connect(lambda _i: self.refresh(keep_folder=True))
        opts.addWidget(self.folder_combo)
        opts.addSpacing(12)
        opts.addWidget(QLabel("Character:"))
        self.char_combo = QComboBox()
        self.char_combo.setMinimumWidth(200)
        self.char_combo.currentIndexChanged.connect(self._fill_list)
        opts.addWidget(self.char_combo)
        self.only_auto = QCheckBox("Hide autosaves")
        self.only_auto.toggled.connect(self._fill_list)
        opts.addWidget(self.only_auto)
        opts.addStretch(1)
        self.count = QLabel()
        self.count.setStyleSheet("color: gray;")
        opts.addWidget(self.count)
        lay.addLayout(opts)

        split = QSplitter()
        self.list = QListWidget()
        self.list.setIconSize(THUMB)
        self.list.setSpacing(2)
        self.list.setWordWrap(True)
        self.list.setSelectionMode(QListWidget.ExtendedSelection)
        self.list.itemSelectionChanged.connect(self._show_detail)
        self.list.setContextMenuPolicy(Qt.CustomContextMenu)
        self.list.customContextMenuRequested.connect(self._context_menu)
        split.addWidget(self.list)

        det = QWidget()
        dl = QVBoxLayout(det)
        self.shot = QLabel()
        self.shot.setAlignment(Qt.AlignCenter)
        self.shot.setMinimumHeight(200)
        self.shot.setStyleSheet("background: #111; border-radius: 6px;")
        dl.addWidget(self.shot)
        self.info = QLabel()
        self.info.setWordWrap(True)
        self.info.setTextFormat(Qt.RichText)
        self.info.setTextInteractionFlags(Qt.TextSelectableByMouse)
        dl.addWidget(self.info)
        self.warn = QLabel()
        self.warn.setWordWrap(True)
        self.warn.setTextFormat(Qt.RichText)
        self.warn.setTextInteractionFlags(Qt.TextSelectableByMouse)
        self.warn.setStyleSheet("QLabel { border: 1px solid palette(mid); border-left: 5px solid #f08c00;"
                                " border-radius: 6px; padding: 6px 8px; }")
        dl.addWidget(self.warn)
        row = QHBoxLayout()
        self.copy_btn = QToolButton()
        self.copy_btn.setText("Copy to profile ▾")
        self.copy_btn.setPopupMode(QToolButton.InstantPopup)
        self.move_btn = QToolButton()
        self.move_btn.setText("Move to profile ▾")
        self.move_btn.setPopupMode(QToolButton.InstantPopup)
        self.del_btn = QPushButton("Delete")
        self.del_btn.setToolTip("Move the save (and its script extender co-save) to the Trash")
        self.del_btn.clicked.connect(self.delete)
        for b in (self.copy_btn, self.move_btn, self.del_btn):
            row.addWidget(b)
        row.addStretch(1)
        dl.addLayout(row)
        self.busy_note = QLabel()
        self.busy_note.setStyleSheet("color: #f08c00;")
        self.busy_note.setWordWrap(True)
        dl.addWidget(self.busy_note)
        dl.addStretch(1)
        split.addWidget(det)
        split.setStretchFactor(0, 3)
        split.setStretchFactor(1, 2)
        self.split = split
        lay.addWidget(split, 1)
        self.empty = QLabel()
        self.empty.setWordWrap(True)
        self.empty.setAlignment(Qt.AlignCenter)
        lay.addWidget(self.empty)

    # ------------------------------------------------------------------ data
    def current_folder(self) -> str | None:
        """The profile whose saves are shown ('' = Saves\\ itself)."""
        d = self.folder_combo.currentData()
        return d if d is not None else None

    def refresh(self, keep_folder: bool = False):
        inst = self.host.inst
        supported = bool(inst) and sg.supported(inst)
        for w in (self.banner, self.split, self.backup_btn, self.folder_btn, self.more,
                  self.folder_combo, self.char_combo, self.only_auto):
            w.setVisible(supported)
        if not supported:
            self.title.setText("<b style='font-size:15px'>Saves</b>")
            self.where.setText("")
            self.empty.setText("Pick a game that's set up." if not inst else
                               f"Kiln doesn't manage {inst.game.name}'s saves yet. It does for Skyrim, "
                               "Fallout 4, Fallout 3, New Vegas and Oblivion.")
            self.empty.show()
            return
        self.empty.hide()
        on = sg.enabled(inst)
        self.banner.setVisible(not on)
        self.more_menu.clear()
        if on:
            self.more_menu.addAction("Go back to one shared save folder…", self.turn_off)
        self.more.setVisible(on)
        # folder choices
        want = self.current_folder() if keep_folder else None
        self.folder_combo.blockSignals(True)
        self.folder_combo.clear()
        for label, d in sg.profile_folders(inst):
            if label == sg.SHARED:
                if on and not (d.is_dir() and any(p.suffix.lower() == sg.SPECS[inst.game.id].ext
                                                  for p in d.iterdir() if p.is_file())):
                    continue
                text = "Saves from before profiles (Saves\\)" if on else "All saves (Saves\\)"
            else:
                text = f"Profile: {label}" + ("  (current)" if label == inst.active_profile else "")
            self.folder_combo.addItem(text, label)
        idx = self.folder_combo.findData(want if want is not None else
                                         (inst.active_profile if on else sg.SHARED))
        self.folder_combo.setCurrentIndex(max(0, idx))
        self.folder_combo.blockSignals(False)
        folder = self.current_folder()
        d = sg.saves_dir(inst, folder)
        if on and folder != sg.SHARED:
            self.title.setText(f"<b style='font-size:15px'>Saves for “{html.escape(folder)}”</b>")
        else:
            self.title.setText("<b style='font-size:15px'>Saves</b>")
        self.where.setText(str(d) if d else "")
        self._load(inst, folder)

    def _load(self, inst, folder):
        from .gui import Task
        self._gen += 1
        gen = self._gen
        self.count.setText("Reading saves…")

        def work():
            saves = sg.list_saves(inst, folder)
            return saves, sg.loaded_plugins(inst, folder or None)

        def done(res):
            if gen != self._gen or inst is not self.host.inst:
                return
            self.saves, self.have = res
            self._fill_chars()
            self._fill_list()

        Task(work).start(done, lambda e: self.count.setText(f"Couldn't read the saves: {e}"))

    def _fill_chars(self):
        cur = self.char_combo.currentData()
        self.char_combo.blockSignals(True)
        self.char_combo.clear()
        self.char_combo.addItem("Everyone", None)
        counts: dict[str, list] = {}
        for s in self.saves:
            counts.setdefault(s.character, []).append(s)
        for ch, ss in sorted(counts.items(), key=lambda kv: -max(s.mtime for s in kv[1])):
            lvl = max(s.level for s in ss)
            self.char_combo.addItem(f"{ss[0].player or '?'} · level {lvl} · {len(ss)} save(s)", ch)
        i = self.char_combo.findData(cur)
        self.char_combo.setCurrentIndex(max(0, i))
        self.char_combo.blockSignals(False)

    def visible_saves(self) -> list[sg.Save]:
        ch = self.char_combo.currentData()
        return [s for s in self.saves if (ch is None or s.character == ch)
                and not (self.only_auto.isChecked() and s.kind == "Autosave")]

    def _fill_list(self):
        self.list.clear()
        shown = self.visible_saves()
        inst = self.host.inst
        bad = 0
        for s in shown:
            miss = sg.missing(inst, s, self.have) if s.plugins else []
            bad += bool(miss)
            line2 = f"{s.kind} · {_when(s.mtime)} · {s.played}"
            line3 = f"⚠ {len(miss)} mod(s) it used are off in {self._which()}" if miss else ""
            text = f"{s.player} · level {s.level} · {s.location}\n{line2}" + (f"\n{line3}" if line3 else "")
            if s.error:
                text = f"{s.path.name}\n{s.error}"
            it = QListWidgetItem(text)
            pm = _pixmap(s, THUMB)
            if pm:
                it.setIcon(QIcon(pm))
            it.setData(Qt.UserRole, s)
            it.setToolTip(s.path.name)
            self.list.addItem(it)
        total = len(self.saves)
        self.count.setText(f"{len(shown)} of {total} save(s)" + (f" · {bad} use mods that are off" if bad else ""))
        if shown:
            self.list.setCurrentRow(0)
        else:
            self._show_detail()

    def _which(self) -> str:
        f = self.current_folder()
        inst = self.host.inst
        return f"profile “{f}”" if f and f != inst.active_profile else "this profile"

    # ------------------------------------------------------------------ detail
    def selected(self) -> list[sg.Save]:
        return [it.data(Qt.UserRole) for it in self.list.selectedItems()]

    def _show_detail(self):
        sel = self.selected()
        inst = self.host.inst
        running = self._running()
        self.busy_note.setText(f"{inst.game.name} is running: close it before moving or deleting saves."
                               if running and inst else "")
        for b in (self.copy_btn, self.move_btn, self.del_btn):
            b.setEnabled(bool(sel) and not running)
        self._fill_profile_menus()
        if not sel:
            self.shot.clear()
            self.info.setText("No save selected." if self.saves else
                              "No saves here yet. Start the game from Kiln and save: it goes into this folder.")
            self.warn.hide()
            return
        if len(sel) > 1:
            self.shot.clear()
            self.info.setText(f"<b>{len(sel)} saves selected</b>")
            self.warn.hide()
            return
        s = sel[0]
        pm = _pixmap(s)
        if pm:
            self.shot.setPixmap(pm.scaled(QSize(480, 288), Qt.KeepAspectRatio, Qt.SmoothTransformation))
        else:
            self.shot.clear()
        cos = [f.suffix for f in s.files[1:]]
        self.info.setText(
            f"<b style='font-size:14px'>{html.escape(s.player)}</b> · level {s.level} · "
            f"{html.escape(s.race)}<br>{html.escape(s.location)}<br>"
            f"{s.kind} #{s.number} · saved {_when(s.mtime)} · {html.escape(s.played)}<br>"
            f"<span style='color:gray'>{html.escape(s.path.name)} · {s.size / 1e6:.1f} MB"
            + (f" · with its {', '.join(cos)} co-save" if cos else "") + "</span>")
        miss = sg.missing(inst, s, self.have) if s.plugins else []
        if miss:
            shown = "".join(f"<li>{html.escape(p)}</li>" for p in miss[:12])
            more = f"<li>…and {len(miss) - 12} more</li>" if len(miss) > 12 else ""
            self.warn.setText(
                f"<b>⚠ This save was made with {len(miss)} mod(s) that are off in {self._which()}.</b> "
                "Loading it anyway can lose items, quests or characters those mods added, or crash. "
                "Load it with the profile it was made in, or turn these on first:"
                f"<ul style='margin:4px 0'>{shown}{more}</ul>")
            self.warn.show()
        else:
            self.warn.hide()

    def _fill_profile_menus(self):
        inst = self.host.inst
        here = self.current_folder()
        for btn, move in ((self.copy_btn, False), (self.move_btn, True)):
            m = QMenu(btn)
            if inst and sg.supported(inst) and sg.enabled(inst):
                for n in inst.profile_names():
                    if n != here:
                        m.addAction(n, lambda n=n, move=move: self.copy_to(n, move))
            if here != sg.SHARED:
                m.addAction("The shared folder (Saves\\)", lambda move=move: self.copy_to(sg.SHARED, move))
            btn.setMenu(m)
            btn.setVisible(bool(m.actions()))

    def _context_menu(self, pos):
        if not self.selected():
            return
        m = QMenu(self)
        for title, btn in (("Copy to profile", self.copy_btn), ("Move to profile", self.move_btn)):
            if btn.isVisible() and btn.isEnabled() and btn.menu():
                sub = m.addMenu(title)
                for a in btn.menu().actions():
                    sub.addAction(a)
        if self.del_btn.isEnabled():
            m.addAction("Delete", self.delete)
        m.addAction("Show in folder", self.open_folder)
        m.exec(self.list.mapToGlobal(pos))

    # ------------------------------------------------------------------ actions
    def _running(self) -> bool:
        inst = self.host.inst
        if not inst:
            return False
        from . import launchguard
        return launchguard.game_running(inst)

    def _guard(self) -> bool:
        if self._running():
            QMessageBox.information(self, "Saves", f"Close {self.host.inst.game.name} first.")
            return False
        return True

    def copy_to(self, profile: str, move: bool = False):
        if not self._guard():
            return
        inst = self.host.inst
        done, errors = 0, []
        for s in self.selected():
            try:
                sg.copy_to(inst, s, profile, move)
                done += 1
            except Exception as e:  # noqa: BLE001
                errors.append(f"{s.path.name}: {e}")
        where = f"“{profile}”" if profile != sg.SHARED else "the shared folder"
        self.host.statusBar().showMessage(f"{'Moved' if move else 'Copied'} {done} save(s) to {where}.", 8000)
        if errors:
            QMessageBox.warning(self, "Saves", "\n".join(errors))
        self.refresh(keep_folder=True)

    def delete(self):
        if not self._guard():
            return
        sel = self.selected()
        if not sel:
            return
        names = "\n".join(f"• {s.player}, {s.location} ({_when(s.mtime)})" for s in sel[:8])
        if QMessageBox.question(self, "Delete saves", f"Move {len(sel)} save(s) to the Trash?\n\n{names}"
                                + ("\n…" if len(sel) > 8 else "")
                                + "\n\nYou can get them back from the Trash.") != QMessageBox.Yes:
            return
        failed = [s.path.name for s in sel if not sg.delete(s)]
        if failed:
            QMessageBox.warning(self, "Delete saves", "These couldn't be moved to the Trash (they're "
                                "still there):\n" + "\n".join(failed))
        self.host.statusBar().showMessage(f"Moved {len(sel) - len(failed)} save(s) to the Trash.", 8000)
        self.refresh(keep_folder=True)

    def backup(self):
        inst = self.host.inst
        from .paths import data_dir
        try:
            z = sg.backup(inst, self.current_folder(), data_dir() / "save-backups")
        except Exception as e:  # noqa: BLE001
            self.host.error("Back up saves", e)
            return
        self.host.statusBar().showMessage(f"Saved a backup: {z}", 15000)
        box = QMessageBox(self)
        box.setWindowTitle("Back up saves")
        box.setText(f"Backed up {len(self.saves)} save(s) to:\n{z}")
        show = box.addButton("Show it", QMessageBox.ActionRole)
        box.addButton(QMessageBox.Ok)
        box.exec()
        if box.clickedButton() is show:
            QDesktopServices.openUrl(QUrl.fromLocalFile(str(z.parent)))

    def open_folder(self):
        inst = self.host.inst
        d = sg.saves_dir(inst, self.current_folder()) if inst else None
        if d:
            d.mkdir(parents=True, exist_ok=True)
            QDesktopServices.openUrl(QUrl.fromLocalFile(str(d)))

    def turn_on(self):
        inst = self.host.inst
        if not inst or not self._guard():
            return
        n = len(sg.list_saves(inst, sg.SHARED, read=False))
        box = QMessageBox(self)
        box.setWindowTitle("Separate saves for each profile")
        box.setText(f"Each profile will keep its saves in its own folder (Saves\\<profile>), and "
                    f"{inst.game.name} will save into the folder of the profile you play.")
        box.setInformativeText(
            "Other profiles start with no saves; copy any you want to them from this page." if n else
            "Other profiles start with no saves.")
        move = QCheckBox(f"Move my {n} existing save(s) into “{inst.active_profile}”") if n else None
        if move:
            move.setChecked(True)
            box.setCheckBox(move)
        box.addButton("Turn it on", QMessageBox.AcceptRole)
        box.addButton(QMessageBox.Cancel)
        if box.exec() == QMessageBox.Cancel or box.standardButton(box.clickedButton()) == QMessageBox.Cancel:
            return
        try:
            moved = sg.turn_on(inst, bool(move and move.isChecked()))
        except Exception as e:  # noqa: BLE001
            self.host.error("Separate saves", e)
            return
        self.host.statusBar().showMessage(
            f"Each profile now has its own saves." + (f" Moved {moved} save(s) into “{inst.active_profile}”."
                                                      if moved else ""), 10000)
        self.refresh()

    def turn_off(self):
        inst = self.host.inst
        if not inst or not self._guard():
            return
        if QMessageBox.question(self, "One shared save folder",
                                "Go back to one save folder for every profile? The game will save into "
                                "Saves\\ again. Each profile's saves stay in its folder; copy the ones "
                                "you want to the shared folder from this page.") != QMessageBox.Yes:
            return
        sg.turn_off(inst)
        self.refresh()
