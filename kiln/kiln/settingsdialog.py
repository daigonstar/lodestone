"""Game settings window: what the game's own launcher would set (resolution,
fullscreen, quality), for games Kiln starts without that launcher."""
from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtWidgets import (QCheckBox, QComboBox, QDialog, QFormLayout, QHBoxLayout, QLabel, QMessageBox,
                               QPushButton, QVBoxLayout)

from . import gamesettings as gs

if TYPE_CHECKING:
    from .gui import MainWindow
    from .instance import Instance

MODES = [("Fullscreen", "fullscreen"), ("Borderless window (fills the screen)", "borderless"), ("Window", "window")]
AA = [("TAA (smooth, a little soft)", "taa"), ("FXAA (sharper, some jaggies)", "fxaa"), ("Off", "off")]


class GameSettingsDialog(QDialog):
    def __init__(self, host: "MainWindow", inst: "Instance"):
        super().__init__(host.dialog_parent())
        self.host, self.inst = host, inst
        self.setWindowTitle(f"{inst.game.name}: game settings")
        self.resize(560, 380)
        cur = gs.current(inst)
        lay = QVBoxLayout(self)
        head = QLabel(f"<h3>Display and graphics</h3>{inst.game.name} sets these in its own launcher, which "
                      "Kiln skips so mods load. Kiln writes the same settings file the launcher does.")
        head.setWordWrap(True)
        lay.addWidget(head)
        f = QFormLayout()
        self.res = QComboBox()
        self.res.setEditable(True)
        sizes = gs.screen_sizes()
        for i, (w, h) in enumerate(sizes):
            self.res.addItem(f"{w} × {h}" + ("  (your screen)" if i == 0 else ""), (w, h))
        here = next((i for i in range(self.res.count()) if self.res.itemData(i) == (cur.width, cur.height)), None)
        if here is None:
            self.res.insertItem(0, f"{cur.width} × {cur.height}  (current)", (cur.width, cur.height))
            here = 0
        self.res.setCurrentIndex(here)
        f.addRow("Resolution:", self.res)
        self.mode = QComboBox()
        for label, key in MODES:
            self.mode.addItem(label, key)
        self.mode.setCurrentIndex(max(0, self.mode.findData(cur.mode)))
        f.addRow("Display mode:", self.mode)
        self.vsync = QCheckBox("V-Sync (no tearing; keeps the game's physics stable at 60 fps)")
        self.vsync.setChecked(cur.vsync)
        f.addRow("", self.vsync)
        self.aa = QComboBox()
        for label, key in AA:
            self.aa.addItem(label, key)
        self.aa.setCurrentIndex(max(0, self.aa.findData(cur.aa)))
        f.addRow("Antialiasing:", self.aa)
        self.preset = QComboBox()
        self.preset.addItem("Keep my current quality settings", "")
        for p in gs.presets(inst):
            self.preset.addItem(f"{p} (the launcher's preset)", p)
        f.addRow("Quality:", self.preset)
        lay.addLayout(f)
        note = QLabel("Borderless window is the most reliable choice under Proton and Wayland. Settings take "
                      "effect next time the game starts.")
        note.setWordWrap(True)
        note.setStyleSheet("color: gray;")
        lay.addWidget(note)
        lay.addStretch(1)
        row = QHBoxLayout()
        launcher = QPushButton(f"Open {inst.game.name}'s own launcher…")
        launcher.setToolTip("For settings not listed here. Use its Options, then close it: its Play button "
                            "starts the game without your script-extender mods.")
        launcher.clicked.connect(self.open_launcher)
        row.addWidget(launcher)
        row.addStretch(1)
        save = QPushButton("Save")
        save.setDefault(True)
        save.clicked.connect(self.save)
        close = QPushButton("Close")
        close.clicked.connect(self.close)
        row.addWidget(save)
        row.addWidget(close)
        lay.addLayout(row)

    def _size(self):
        d = self.res.currentData()
        if d and self.res.currentText().split("  ")[0] == self.res.itemText(self.res.currentIndex()).split("  ")[0]:
            return d
        import re
        m = re.findall(r"\d+", self.res.currentText())
        if len(m) >= 2:
            return int(m[0]), int(m[1])
        raise ValueError("Type the resolution as width × height, e.g. 2560 × 1440.")

    def save(self):
        try:
            w, h = self._size()
            done = gs.apply(self.inst, gs.Settings(w, h, self.mode.currentData(), self.vsync.isChecked(),
                                                   self.aa.currentData(), self.preset.currentData()))
        except Exception as e:  # noqa: BLE001
            self.host.error("Game settings", e)
            return
        self.host.statusBar().showMessage("Saved: " + "; ".join(done), 12000)
        self.close()

    def open_launcher(self):
        if QMessageBox.question(
                self, "Game launcher",
                f"Open {self.inst.game.name}'s own launcher? Change what you need under Options, then close it "
                "with Exit. Don't press its Play button: that starts the game without your script-extender "
                "mods. Use Kiln's Play instead.") != QMessageBox.Yes:
            return
        try:
            gs.open_launcher(self.inst)
        except Exception as e:  # noqa: BLE001
            self.host.error("Game launcher", e)
