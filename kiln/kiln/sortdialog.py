"""Preview of an automatic load order before it's applied."""
from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtGui import QBrush, QColor
from PySide6.QtWidgets import (QCheckBox, QDialog, QDialogButtonBox, QHeaderView, QLabel,
                               QTreeWidget, QTreeWidgetItem, QVBoxLayout)

from . import autosort

if TYPE_CHECKING:
    from .instance import Instance


class SortDialog(QDialog):
    def __init__(self, inst: "Instance", parent=None):
        super().__init__(parent)
        self.setWindowTitle("Auto-sort load order")
        self.resize(820, 620)
        self.inst = inst
        self.result_mods = autosort.sort_mods(inst)
        before = [m.id for m in inst.mods]
        moved = self.result_mods.moved(before)
        lay = QVBoxLayout(self)
        intro = QLabel(
            "Kiln orders mods by common practice: frameworks and fixes first, broad texture "
            "overhauls before targeted ones, then gameplay, characters and quests, with "
            "compatibility patches and your own tweaks last (lower in the list wins conflicts). "
            "A mod that needs another mod's plugins always loads after it, and the rules you "
            "made with “Make this mod win” are kept.")
        intro.setWordWrap(True)
        lay.addWidget(intro)
        lay.addWidget(QLabel(f"<b>{moved}</b> of {len(before)} mods change position."
                             if moved else "<b>Your mod order already matches.</b>"))
        tree = QTreeWidget()
        tree.setHeaderLabels(["New", "Mod", "Was", "Group / reason"])
        tree.setRootIsDecorated(False)
        tree.header().setSectionResizeMode(3, QHeaderView.Stretch)
        pos = {mid: i for i, mid in enumerate(before)}
        for i, mid in enumerate(self.result_mods.order):
            m = inst.get(mid)
            was = pos[mid]
            it = QTreeWidgetItem([str(i + 1), m.name, "" if was == i else
                                  f"{was + 1} ({'↑' if i < was else '↓'}{abs(i - was)})",
                                  self.result_mods.reasons[mid]])
            if was != i:
                it.setForeground(2, QBrush(QColor("#e8590c")))
            if mid in self.result_mods.cycles:
                it.setToolTip(3, "Its rules form a loop; Kiln placed it by group.")
            tree.addTopLevelItem(it)
        for c in range(3):
            tree.resizeColumnToContents(c)
        lay.addWidget(tree, 1)
        if self.result_mods.cycles:
            lay.addWidget(QLabel("⚠ Some rules contradict each other; Kiln broke the loop at: "
                                 + ", ".join(inst.get(i).name for i in self.result_mods.cycles)))
        self.plugins_cb = QCheckBox("Also sort plugins (masters first, each plugin after its "
                                    "masters, fixes early, patches late)")
        self.plugins_cb.setChecked(True)
        self.plugins_cb.setVisible(inst.game.has_plugins)
        lay.addWidget(self.plugins_cb)
        bb = QDialogButtonBox(QDialogButtonBox.Ok | QDialogButtonBox.Cancel)
        bb.button(QDialogButtonBox.Ok).setText("Apply order")
        bb.accepted.connect(self.accept)
        bb.rejected.connect(self.reject)
        lay.addWidget(bb)

    def apply(self) -> None:
        inst = self.inst
        rank = {mid: i for i, mid in enumerate(self.result_mods.order)}
        inst.mods.sort(key=lambda m: rank[m.id])
        if self.plugins_cb.isChecked() and inst.game.has_plugins:
            inst.plugins = autosort.sort_plugins(inst)
        inst.save()
