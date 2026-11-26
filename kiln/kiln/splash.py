"""The window shown while Kiln starts: logo, a progress bar and what it's doing."""
from __future__ import annotations

from pathlib import Path

from PySide6.QtCore import Qt
from PySide6.QtGui import QIcon
from PySide6.QtWidgets import QApplication, QLabel, QProgressBar, QVBoxLayout, QWidget

from . import APP_NAME, __version__

STEPS = 7


class Splash(QWidget):
    def __init__(self):
        super().__init__(None, Qt.SplashScreen | Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint)
        self.setObjectName("splash")
        self.setFixedSize(460, 300)
        self.setStyleSheet("#splash { background: #1b1d22; border: 1px solid #3a3d45; border-radius: 10px; }"
                           "QLabel { color: #e9ecef; } QProgressBar { background: #2b2e36; border: none;"
                           " border-radius: 4px; height: 8px; } QProgressBar::chunk { background: #f08c00;"
                           " border-radius: 4px; }")
        lay = QVBoxLayout(self)
        lay.setContentsMargins(28, 26, 28, 22)
        logo = QLabel()
        logo.setPixmap(QIcon(str(Path(__file__).resolve().parent / "assets" / "kiln.svg")).pixmap(96, 96))
        logo.setAlignment(Qt.AlignCenter)
        lay.addWidget(logo)
        title = QLabel(f"<span style='font-size:22px; font-weight:700'>{APP_NAME}</span>"
                       f" <span style='color:#868e96'>{__version__}</span>")
        title.setAlignment(Qt.AlignCenter)
        lay.addWidget(title)
        lay.addStretch(1)
        self.text = QLabel("Starting…")
        self.text.setStyleSheet("color: #adb5bd;")
        lay.addWidget(self.text)
        self.bar = QProgressBar()
        self.bar.setRange(0, STEPS)
        self.bar.setTextVisible(False)
        self.bar.setFixedHeight(8)
        lay.addWidget(self.bar)
        self.done = 0
        scr = QApplication.primaryScreen()
        if scr:
            g = scr.availableGeometry()
            self.move(g.center().x() - self.width() // 2, g.center().y() - self.height() // 2)

    def step(self, text: str) -> None:
        """Show the next step and let the window repaint (startup blocks the event loop)."""
        self.done = min(self.done + 1, STEPS)
        self.bar.setValue(self.done)
        self.text.setText(text)
        QApplication.processEvents()
