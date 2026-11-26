"""The pre-launch check window: what's fine, what's wrong, one-click fixes."""
from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (QDialog, QFrame, QHBoxLayout, QLabel, QPushButton, QScrollArea,
                               QVBoxLayout, QWidget)

from . import launchguard

if TYPE_CHECKING:
    from .gui import MainWindow

ICON = {"ok": ("✔", "#2f9e44"), "warn": ("●", "#f08c00"), "error": ("✖", "#e03131")}


class LaunchDialog(QDialog):
    def __init__(self, host: "MainWindow", checks: list[launchguard.Check]):
        super().__init__(host)
        self.host = host
        self.setWindowTitle(f"Before launching {host.game.name}")
        self.resize(720, 460)
        lay = QVBoxLayout(self)
        errs = sum(c.level == "error" for c in checks)
        head = QLabel(
            f"<h3>{'Mods won’t load as things are' if errs else 'Almost ready'}</h3>"
            "Kiln checked that the game will start with your mods. Fix the problems below, "
            "or launch anyway.")
        head.setWordWrap(True)
        lay.addWidget(head)
        area = QScrollArea()
        area.setWidgetResizable(True)
        area.setFrameShape(QFrame.NoFrame)
        body = QWidget()
        bl = QVBoxLayout(body)
        for c in sorted(checks, key=lambda c: {"error": 0, "warn": 1, "ok": 2}[c.level]):
            row = QHBoxLayout()
            sym, col = ICON[c.level]
            ic = QLabel(sym)
            ic.setStyleSheet(f"color: {col}; font-size: 16px;")
            ic.setFixedWidth(22)
            ic.setAlignment(Qt.AlignTop)
            row.addWidget(ic)
            txt = QLabel(f"<b>{c.title}</b>" + (f"<br><span style='color:gray'>"
                                               f"{c.detail.replace(chr(10), '<br>')}</span>"
                                               if c.detail else ""))
            txt.setWordWrap(True)
            txt.setTextInteractionFlags(Qt.TextSelectableByMouse)
            row.addWidget(txt, 1)
            if c.fix and c.level != "ok":
                b = QPushButton(c.fix_label)
                b.clicked.connect(lambda _=False, c=c: self._fix(c))
                row.addWidget(b, 0, Qt.AlignTop)
            bl.addLayout(row)
        bl.addStretch(1)
        area.setWidget(body)
        lay.addWidget(area, 1)
        bb = QHBoxLayout()
        bb.addStretch(1)
        self.recheck = QPushButton("Check again")
        self.recheck.clicked.connect(lambda: self.done(2))
        anyway = QPushButton("Launch anyway")
        anyway.clicked.connect(self.accept)
        cancel = QPushButton("Cancel")
        cancel.clicked.connect(self.reject)
        for b in (self.recheck, cancel, anyway):
            bb.addWidget(b)
        lay.addLayout(bb)

    def _fix(self, c: launchguard.Check):
        self.host.fix_check(c, after=lambda: self.done(2))
