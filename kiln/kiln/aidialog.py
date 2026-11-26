"""The “Ask AI” window: a short conversation with the local model about a problem
or the whole setup. It shows exactly what Kiln sends."""
from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtCore import QObject, QRunnable, QThreadPool, Signal
from PySide6.QtWidgets import (QDialog, QHBoxLayout, QLabel, QPlainTextEdit, QPushButton, QTextBrowser,
                               QToolButton, QVBoxLayout)

from . import ai

if TYPE_CHECKING:
    from .gui import MainWindow


class _Stream(QObject):
    text = Signal(str)
    finished = Signal(str)
    failed = Signal(str)


class _Job(QRunnable):
    def __init__(self, url, model, messages, thinking, sig: _Stream):
        super().__init__()
        self.args = (url, model, messages)
        self.thinking = thinking
        self.sig = sig
        self.stop = False

    def run(self):
        try:
            full = ai.chat(*self.args, on_text=self.sig.text.emit, cancelled=lambda: self.stop,
                           thinking=self.thinking)
            self.sig.finished.emit(full)
        except Exception as e:  # noqa: BLE001
            self.sig.failed.emit(str(e))


class AskDialog(QDialog):
    def __init__(self, host: "MainWindow", inst, issue=None, issues=None):
        super().__init__(host.dialog_parent())
        self.host, self.inst = host, inst
        s = host.settings
        self.url = s.get("ai_url") or ai.DEFAULT_URL
        self.model = s.get("ai_model") or ""
        self.thinking = bool(s.get("ai_model_thinking"))
        self.setWindowTitle("Ask AI" + (f": {issue.title}" if issue else ""))
        self.resize(820, 640)
        self.ctx = ai.context(inst, issue, issues)
        self.messages = [{"role": "system", "content": ai.SYSTEM + "\n\nSETUP DETAILS:\n" + self.ctx}]
        self.job = None
        lay = QVBoxLayout(self)
        head = QLabel(f"<b>{self.model or 'No model chosen'}</b> via Ollama ({self.url}). "
                      "The AI only explains; Kiln's buttons do the fixing. Answers can be wrong.")
        head.setWordWrap(True)
        head.setStyleSheet("color: gray;")
        lay.addWidget(head)
        self.show_ctx = QToolButton()
        self.show_ctx.setText("▸ What Kiln sends to the AI")
        self.show_ctx.setAutoRaise(True)
        self.ctx_view = QPlainTextEdit(self.ctx)
        self.ctx_view.setReadOnly(True)
        self.ctx_view.setMaximumHeight(160)
        self.ctx_view.hide()
        self.show_ctx.clicked.connect(lambda: (self.ctx_view.setVisible(not self.ctx_view.isVisible()),
                                               self.show_ctx.setText(("▾" if self.ctx_view.isVisible() else "▸")
                                                                     + " What Kiln sends to the AI")))
        lay.addWidget(self.show_ctx)
        lay.addWidget(self.ctx_view)
        self.view = QTextBrowser()
        self.view.setOpenExternalLinks(True)
        lay.addWidget(self.view, 1)
        self.q = QPlainTextEdit()
        self.q.setMaximumHeight(80)
        self.q.setPlaceholderText("Ask a question…")
        if issue is not None:
            self.q.setPlainText("Explain this problem in simple terms and tell me the best way to fix it.")
        lay.addWidget(self.q)
        row = QHBoxLayout()
        self.status = QLabel()
        self.status.setStyleSheet("color: gray;")
        row.addWidget(self.status, 1)
        self.stop_btn = QPushButton("Stop")
        self.stop_btn.clicked.connect(self.stop)
        self.stop_btn.hide()
        self.ask_btn = QPushButton("Ask")
        self.ask_btn.setDefault(True)
        self.ask_btn.clicked.connect(self.ask)
        row.addWidget(self.stop_btn)
        row.addWidget(self.ask_btn)
        lay.addLayout(row)
        self.transcript = ""
        if not self.model:
            self.view.setMarkdown("Pick a model in **Settings → AI helper** first.")
            self.ask_btn.setEnabled(False)

    def ask(self):
        text = self.q.toPlainText().strip()
        if not text or self.job:
            return
        found = ai.lookup(self.inst, text)
        self.messages.append({"role": "user", "content": text + (
            f"\n\n[Kiln looked up the mods mentioned:\n{found}]" if found else "")})
        self.transcript += f"\n\n**You:** {text}\n\n**AI:** "
        self.answer = ""
        self.q.clear()
        self._render()
        self.sig = _Stream()
        self.sig.text.connect(self._piece)
        self.sig.finished.connect(self._done)
        self.sig.failed.connect(self._failed)
        self.job = _Job(self.url, self.model, list(self.messages), self.thinking, self.sig)
        self.job.setAutoDelete(False)
        QThreadPool.globalInstance().start(self.job)
        self.status.setText("Thinking…")
        self.ask_btn.setEnabled(False)
        self.stop_btn.show()

    def _piece(self, t: str):
        self.answer += t
        self.status.setText("Answering…")
        self._render()

    def _render(self):
        self.view.setMarkdown((self.transcript + getattr(self, "answer", "")).strip())
        sb = self.view.verticalScrollBar()
        sb.setValue(sb.maximum())

    def _done(self, full: str):
        self.messages.append({"role": "assistant", "content": full or self.answer})
        self.transcript += self.answer
        self.answer = ""
        self._finish("")

    def _failed(self, err: str):
        self.transcript += f"*{err}*"
        self.answer = ""
        self._finish(err)

    def _finish(self, err: str):
        self.job = None
        self._render()
        self.status.setText("" if not err else "Something went wrong.")
        self.ask_btn.setEnabled(True)
        self.stop_btn.hide()

    def stop(self):
        if self.job:
            self.job.stop = True

    def closeEvent(self, e):
        self.stop()
        super().closeEvent(e)
