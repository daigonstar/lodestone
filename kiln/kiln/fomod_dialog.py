"""Qt wizard for FOMOD installers."""
from __future__ import annotations

from pathlib import Path

from PySide6.QtCore import Qt
from PySide6.QtGui import QPixmap
from PySide6.QtWidgets import (
    QAbstractButton, QButtonGroup, QCheckBox, QDialog, QGroupBox, QHBoxLayout, QLabel,
    QMessageBox, QPushButton, QRadioButton, QScrollArea, QSplitter, QTextBrowser, QVBoxLayout,
    QWidget,
)

from . import fomod
from .archive import ci_resolve


class FomodDialog(QDialog):
    def __init__(self, cfg: fomod.ModuleConfig, base: Path, file_state, parent=None):
        super().__init__(parent)
        self.cfg, self.base, self.file_state = cfg, base, file_state
        self.setWindowTitle(f"Install: {cfg.name}")
        self.resize(980, 640)
        self.selections: dict[tuple[int, int], list[int]] = {}
        self.visited: set[int] = set()
        self.history: list[int] = []
        self.result_files: list[tuple[str, str, bool]] | None = None

        lay = QVBoxLayout(self)
        self.header = QLabel()
        self.header.setStyleSheet("font-size: 15px; font-weight: bold;")
        lay.addWidget(self.header)
        split = QSplitter()
        self.scroll = QScrollArea()
        self.scroll.setWidgetResizable(True)
        split.addWidget(self.scroll)
        right = QWidget()
        rl = QVBoxLayout(right)
        rl.setContentsMargins(0, 0, 0, 0)
        self.image = QLabel()
        self.image.setAlignment(Qt.AlignCenter)
        self.image.setMinimumHeight(220)
        self.desc = QTextBrowser()
        self.desc.setOpenExternalLinks(True)
        rl.addWidget(self.image, 3)
        rl.addWidget(self.desc, 2)
        split.addWidget(right)
        split.setSizes([480, 500])
        lay.addWidget(split, 1)
        btns = QHBoxLayout()
        btns.addStretch(1)
        self.back_btn = QPushButton("◀ Back")
        self.next_btn = QPushButton("Next ▶")
        cancel = QPushButton("Cancel")
        self.back_btn.clicked.connect(self.back)
        self.next_btn.clicked.connect(self.next)
        cancel.clicked.connect(self.reject)
        for b in (self.back_btn, self.next_btn, cancel):
            btns.addWidget(b)
        lay.addLayout(btns)

        self._show_image(cfg.image)
        first = self._next_visible(-1)
        if first is None:
            self.desc.setPlainText("This installer has no options.")
            self.header.setText(cfg.name)
            self.cur = None
            self.next_btn.setText("Install")
            self.back_btn.setEnabled(False)
        else:
            self._goto(first)

    # navigation ----------------------------------------------------------
    def _next_visible(self, after: int):
        for si in range(after + 1, len(self.cfg.steps)):
            if fomod.step_visible(self.cfg, si, self.selections, self.file_state):
                return si
        return None

    def _goto(self, si: int):
        self.cur = si
        step = self.cfg.steps[si]
        if si not in self.visited:
            flags = fomod.flags_after(self.cfg, self.selections, si, self.file_state)
            for gi, g in enumerate(step.groups):
                self.selections[(si, gi)] = fomod.default_selection(g, flags, self.file_state)
            self.visited.add(si)
        self.header.setText(f"{self.cfg.name} — {step.name}")
        self._build_step(si)
        self.back_btn.setEnabled(bool(self.history))
        self.next_btn.setText("Next ▶" if self._next_visible(si) is not None else "Install")

    def back(self):
        if self.history:
            self._goto(self.history.pop())

    def next(self):
        if self.cur is not None:
            err = self._validate(self.cur)
            if err:
                QMessageBox.warning(self, "Choose an option", err)
                return
            # Forget choices in later steps: earlier choices may change what they offer.
            for key in [k for k in self.selections if k[0] > self.cur]:
                del self.selections[key]
            self.visited = {s for s in self.visited if s <= self.cur}
            nxt = self._next_visible(self.cur)
            if nxt is not None:
                self.history.append(self.cur)
                self._goto(nxt)
                return
        self.result_files = fomod.resolve(self.cfg, self.selections, self.file_state)
        self.accept()

    def _validate(self, si: int) -> str:
        for gi, g in enumerate(self.cfg.steps[si].groups):
            n = len(self.selections.get((si, gi), []))
            if g.type == "SelectExactlyOne" and n != 1:
                return f"Pick one option in “{g.name}”."
            if g.type == "SelectAtLeastOne" and n < 1:
                return f"Pick at least one option in “{g.name}”."
        return ""

    def choices(self) -> dict:
        return {f"{s}:{g}": v for (s, g), v in self.selections.items()}

    # step UI -------------------------------------------------------------
    def _build_step(self, si: int):
        w = QWidget()
        v = QVBoxLayout(w)
        flags = fomod.flags_after(self.cfg, self.selections, si, self.file_state)
        self._groups = []
        for gi, g in enumerate(self.cfg.steps[si].groups):
            box = QGroupBox(g.name or " ")
            bl = QVBoxLayout(box)
            radio = g.type in ("SelectExactlyOne", "SelectAtMostOne")
            bgroup = QButtonGroup(box)
            bgroup.setExclusive(radio)
            sel = self.selections.setdefault((si, gi), [])
            for pi, p in enumerate(g.plugins):
                t = p.type(flags, self.file_state)
                b: QAbstractButton = QRadioButton(p.name) if radio else QCheckBox(p.name)
                b.setChecked(pi in sel)
                if t == "NotUsable":
                    b.setEnabled(False)
                    b.setToolTip("Not usable with your setup")
                if g.type == "SelectAll" or (t == "Required" and not radio):
                    b.setChecked(True)
                    b.setEnabled(False)
                    if pi not in sel:
                        sel.append(pi)
                if t == "Recommended":
                    b.setText(f"{p.name}  (recommended)")
                bgroup.addButton(b, pi)
                bl.addWidget(b)
                b.toggled.connect(lambda on, si=si, gi=gi, pi=pi: self._toggled(si, gi, pi, on))
                b.installEventFilter(self)
                b.setProperty("fomod_plugin", (si, gi, pi))
            if g.type == "SelectAtMostOne":
                none = QRadioButton("None")
                none.setChecked(not sel)
                bgroup.addButton(none, -1)
                bl.addWidget(none)
            v.addWidget(box)
        v.addStretch(1)
        self.scroll.setWidget(w)
        # show the first selected plugin's info
        for (s, g), pis in self.selections.items():
            if s == si and pis:
                self._show_plugin(si, g, pis[0])
                break

    def _toggled(self, si, gi, pi, on):
        sel = self.selections.setdefault((si, gi), [])
        g = self.cfg.steps[si].groups[gi]
        if on:
            if g.type in ("SelectExactlyOne", "SelectAtMostOne"):
                sel.clear()
            if pi not in sel:
                sel.append(pi)
            self._show_plugin(si, gi, pi)
        elif pi in sel:
            sel.remove(pi)

    def eventFilter(self, obj, ev):
        if ev.type() == ev.Type.Enter:
            data = obj.property("fomod_plugin")
            if data:
                self._show_plugin(*data)
        return False

    def _show_plugin(self, si, gi, pi):
        p = self.cfg.steps[si].groups[gi].plugins[pi]
        self.desc.setPlainText(p.description or p.name)
        self._show_image(p.image or self.cfg.image)

    def _show_image(self, rel: str):
        self.image.clear()
        if not rel:
            return
        path = ci_resolve(self.base, rel)
        if path and path.is_file():
            pm = QPixmap(str(path))
            if not pm.isNull():
                self.image.setPixmap(pm.scaled(460, 260, Qt.KeepAspectRatio, Qt.SmoothTransformation))
