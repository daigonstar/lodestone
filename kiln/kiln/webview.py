"""Kiln's own Nexus browser window (Qt WebEngine).

Free accounts must click "Slow download" themselves. This window makes that one
click the only thing to do: nxm:// links the page produces go straight to Kiln,
plain file downloads are saved to the right game's downloads folder, and the
"Slow download" button is scrolled into view, highlighted and given keyboard
focus, so pressing Enter (or F2, from anywhere in the window) presses it: one
deliberate key press per file, easier on the hands than aiming a mouse. Kiln
never presses it on its own.
"""
from __future__ import annotations

from pathlib import Path

import time

from PySide6.QtCore import QEvent, QObject, Qt, QTimer, QUrl, Signal, Slot
from PySide6.QtGui import QDesktopServices, QKeyEvent, QKeySequence, QShortcut
from PySide6.QtWebEngineCore import (QWebEngineDownloadRequest, QWebEnginePage, QWebEngineProfile,
                                     QWebEngineSettings, QWebEngineUrlRequestJob, QWebEngineUrlScheme,
                                     QWebEngineUrlSchemeHandler)
from PySide6.QtWebEngineWidgets import QWebEngineView
from PySide6.QtWidgets import (QApplication, QHBoxLayout, QLabel, QLineEdit, QProgressBar,
                               QPushButton, QToolButton, QVBoxLayout, QWidget)

from .paths import data_dir

HOME = "https://www.nexusmods.com/"

# Chromium drops navigations to schemes it doesn't know, so nxm:// is registered
# as a real scheme. This must happen before QApplication is created, which is
# why gui.run() imports this module early.
_scheme = QWebEngineUrlScheme(b"nxm")
_scheme.setSyntax(QWebEngineUrlScheme.Syntax.Path)
_scheme.setFlags(QWebEngineUrlScheme.Flag.LocalAccessAllowed)
QWebEngineUrlScheme.registerScheme(_scheme)

DOWNLOAD_KEY = "F2"     # presses the highlighted button from anywhere in the window

# Shared by both scripts: find the visible "Slow download" button, looking inside
# shadow roots too (Nexus's download modal is a web component, so the button
# isn't reachable from document.querySelectorAll).
_FIND_JS = r"""
function __kilnFind() {
  var re = /^\s*slow download\s*$/i;
  function visible(e) { return e.isConnected && e.getClientRects().length > 0; }
  function walk(root) {
    var els = root.querySelectorAll('button, a, [role="button"]');
    for (var i = 0; i < els.length; i++) {
      if (re.test(els[i].textContent || '') && visible(els[i])) return els[i];
    }
    var all = root.querySelectorAll('*');
    for (var j = 0; j < all.length; j++) {
      if (all[j].shadowRoot) { var f = walk(all[j].shadowRoot); if (f) return f; }
    }
    return null;
  }
  return walk(document);
}
function __kilnActive() {
  var a = document.activeElement;
  while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement;
  return a;
}
"""

# Scroll the "Slow download" button into view, outline it and give it keyboard
# focus. It does not click, submit or call anything on the page.
HIGHLIGHT_JS = r"""
(function () {
  if (window.__kilnHl) return;
  window.__kilnHl = true;
""" + _FIND_JS + r"""
  var tries = 0;
  var t = setInterval(function () {
    tries++;
    var b = __kilnFind();
    if (b) {
      clearInterval(t);
      b.scrollIntoView({block: 'center'});
      b.style.outline = '4px solid #ff9a3c';
      b.style.outlineOffset = '4px';
      b.style.boxShadow = '0 0 0 10px rgba(255,154,60,.25)';
      window.__kilnBtn = b;
      b.focus({preventScroll: true});
    } else if (tries > 100) {
      clearInterval(t);
    }
  }, 300);
})();
"""


# Find the button again (the page may have redrawn it) and focus it. Returns
# whether it's there; the key press itself comes from the user's key.
FOCUS_JS = r"""
(function () {
""" + _FIND_JS + r"""
  var b = window.__kilnBtn;
  if (!b || !b.isConnected || b.getClientRects().length === 0) b = __kilnFind();
  if (!b) return false;
  window.__kilnBtn = b;
  b.scrollIntoView({block: 'center'});
  b.focus({preventScroll: true});
  return __kilnActive() === b;
})();
"""


class _UrlSink(QObject):
    """Receives nxm:// URLs WebEngine hands to QDesktopServices instead of navigating."""
    got = Signal(str)

    @Slot(QUrl)
    def handle(self, url: QUrl):
        self.got.emit(url.toString())


class _NxmHandler(QWebEngineUrlSchemeHandler):
    got = Signal(str)

    def requestStarted(self, job: QWebEngineUrlRequestJob):
        self.got.emit(job.requestUrl().toString())
        job.fail(QWebEngineUrlRequestJob.Error.RequestAborted)   # stay on the current page


class _Page(QWebEnginePage):
    nxm = Signal(str)

    def acceptNavigationRequest(self, url: QUrl, nav_type, is_main_frame: bool) -> bool:
        if url.scheme().lower() == "nxm":
            self.nxm.emit(url.toString())
            return False
        return super().acceptNavigationRequest(url, nav_type, is_main_frame)

    def createWindow(self, _type):
        # target=_blank links open in this same window
        return self


class NexusBrowser(QWidget):
    nxm = Signal(str)                    # an nxm:// link the page produced
    file_downloaded = Signal(str, dict)  # path, context of the page it came from
    skip_requested = Signal()
    pause_requested = Signal()

    def __init__(self, parent=None):
        super().__init__(parent, Qt.Window)
        self.setWindowTitle("Kiln · Nexus Mods")
        self.resize(1150, 850)
        self.context: dict = {}          # {"downloads_dir": Path, "mod_id", "file_id", ...}
        self._seen: set[str] = set()

        store = data_dir() / "web"
        self.profile = QWebEngineProfile("kiln", self)
        self.profile.setPersistentStoragePath(str(store))
        self.profile.setCachePath(str(store / "cache"))
        self.profile.setPersistentCookiesPolicy(QWebEngineProfile.ForcePersistentCookies)
        self.profile.downloadRequested.connect(self._download)
        self.nxm_handler = _NxmHandler(self)
        self.nxm_handler.got.connect(self._got_nxm)
        self.profile.installUrlSchemeHandler(b"nxm", self.nxm_handler)
        self.page_ = _Page(self.profile, self)
        self.page_.nxm.connect(self._got_nxm)
        self.page_.settings().setAttribute(QWebEngineSettings.FullScreenSupportEnabled, False)
        self.view = QWebEngineView(self)
        self.view.setPage(self.page_)
        self.view.loadFinished.connect(self._loaded)
        self.view.urlChanged.connect(lambda u: self.url_edit.setText(u.toString()))

        self.sink = _UrlSink(self)       # fallback route for nxm links
        self.sink.got.connect(self._got_nxm)
        QDesktopServices.setUrlHandler("nxm", self.sink, "handle")

        bar = QHBoxLayout()
        for text, fn in (("◀", self.view.back), ("▶", self.view.forward), ("⟳", self.view.reload),
                         ("⌂", lambda: self.view.load(QUrl(HOME)))):
            b = QToolButton(text=text)
            b.clicked.connect(fn)
            bar.addWidget(b)
        self.url_edit = QLineEdit()
        self.url_edit.returnPressed.connect(lambda: self.view.load(QUrl.fromUserInput(self.url_edit.text())))
        bar.addWidget(self.url_edit, 1)

        self.run_box = QWidget()
        rb = QHBoxLayout(self.run_box)
        rb.setContentsMargins(6, 4, 6, 4)
        self.run_label = QLabel()
        self.run_label.setWordWrap(True)
        self.run_bar = QProgressBar()
        self.run_bar.setMaximumWidth(260)
        self.skip_btn = QPushButton("Skip this mod")
        self.skip_btn.clicked.connect(self.skip_requested)
        self.pause_btn = QPushButton("Pause collection")
        self.pause_btn.clicked.connect(self.pause_requested)
        rb.addWidget(self.run_label, 1)
        rb.addWidget(self.run_bar)
        rb.addWidget(self.skip_btn)
        rb.addWidget(self.pause_btn)
        self.run_box.setStyleSheet("background: rgba(255,154,60,.18); border-radius: 4px;")
        self.run_box.hide()

        self.hint = QLabel()
        self.hint.setWordWrap(True)
        self.bg = QLabel()
        self.bg.setStyleSheet("color: gray;")

        self._last_press = 0.0
        self.key_shortcut = QShortcut(QKeySequence(DOWNLOAD_KEY), self)
        self.key_shortcut.setContext(Qt.WindowShortcut)
        self.key_shortcut.setAutoRepeat(False)
        self.key_shortcut.activated.connect(self.press_download)

        lay = QVBoxLayout(self)
        lay.setContentsMargins(4, 4, 4, 4)
        lay.addLayout(bar)
        lay.addWidget(self.run_box)
        lay.addWidget(self.hint)
        lay.addWidget(self.view, 1)
        lay.addWidget(self.bg)

    # ------------------------------------------------------------------ api
    def open(self, url: str, context: dict | None = None, hint: str = "") -> None:
        self.context = dict(context or {})
        if hint and "file_id=" in url:
            hint += (f" <b>Keyboard:</b> press Enter or {DOWNLOAD_KEY} to press “Slow download” "
                     "(no mouse needed).")
        self.hint.setText(hint)
        self.hint.setVisible(bool(hint))
        self.view.load(QUrl(url))
        self.show()
        self.raise_()
        self.activateWindow()

    def set_run(self, text: str, done: int, total: int) -> None:
        self.run_label.setText(text)
        self.run_bar.setRange(0, max(total, 1))
        self.run_bar.setValue(done)
        self.run_bar.setFormat(f"{done} of {total}")
        self.run_box.show()

    def clear_run(self) -> None:
        self.run_box.hide()

    def set_background(self, text: str) -> None:
        self.bg.setText(text)

    # ------------------------------------------------------------------ internals
    def _loaded(self, ok: bool):
        if ok and "file_id=" in self.view.url().toString():
            self.page_.runJavaScript(HIGHLIGHT_JS)
            if self.isActiveWindow():
                self.view.setFocus()          # so Enter reaches the focused button

    def press_download(self):
        """The download key: focus the highlighted “Slow download” button and send it
        the user's key press as Enter. Held keys and double presses are ignored, so
        it's always one press per file."""
        if "file_id=" not in self.view.url().toString():
            return
        now = time.monotonic()
        if now - self._last_press < 2.0:
            return
        self._last_press = now

        def focused(ok):
            if not ok:
                self.bg.setText("The “Slow download” button isn't on this page yet. Wait a moment "
                                f"and press {DOWNLOAD_KEY} again.")
                return
            self.view.setFocus()
            target = self.view.focusProxy() or self.view
            for kind in (QEvent.KeyPress, QEvent.KeyRelease):
                QApplication.sendEvent(target, QKeyEvent(kind, Qt.Key_Return, Qt.NoModifier, "\r"))
            self.bg.setText("Pressed “Slow download”.")

        self.page_.runJavaScript(FOCUS_JS, 0, focused)

    def _got_nxm(self, uri: str):
        # the same link can arrive through both routes; act on it once
        if uri in self._seen:
            return
        self._seen.add(uri)
        self.nxm.emit(uri)

    def _download(self, req: QWebEngineDownloadRequest):
        d = self.context.get("downloads_dir")
        if not d:     # Kiln's own folder: the Downloads-folder watcher mustn't install it twice
            d = data_dir() / "browser-downloads"
        d = Path(d)
        d.mkdir(parents=True, exist_ok=True)
        ctx = dict(self.context)
        req.setDownloadDirectory(str(d))
        name = req.downloadFileName()
        n, stem, suf = 2, Path(name).stem, Path(name).suffix
        while (d / name).exists():
            name = f"{stem} ({n}){suf}"
            n += 1
        req.setDownloadFileName(name)

        def changed():
            st = req.state()
            if st == QWebEngineDownloadRequest.DownloadCompleted:
                self.file_downloaded.emit(str(d / req.downloadFileName()), ctx)
            elif st in (QWebEngineDownloadRequest.DownloadCancelled,
                        QWebEngineDownloadRequest.DownloadInterrupted):
                self.bg.setText(f"Download of {name} stopped: {req.interruptReasonString()}")

        req.stateChanged.connect(changed)
        req.receivedBytesChanged.connect(
            lambda: self.bg.setText(f"Downloading {name}: {req.receivedBytes() // 1048576} MB"))
        req.accept()
