"""The Troubleshoot page: everything the Mod Doctor found, in plain English, with
a button for each fix and one to fix everything that's safe to fix."""
from __future__ import annotations

import html
import time
from typing import TYPE_CHECKING

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QCheckBox, QFrame, QHBoxLayout, QLabel, QMessageBox, QPushButton, QScrollArea, QTabWidget,
    QToolButton, QVBoxLayout, QWidget,
)

from . import doctor, logreader

if TYPE_CHECKING:
    from .gui import MainWindow

COLORS = {"crash": "#e03131", "broken": "#f08c00", "warn": "#fab005", "info": "#4dabf7", "ok": "#2f9e44"}
ICONS = {"crash": "✖", "broken": "!", "warn": "⚠", "info": "i", "ok": "✔"}


class IssueCard(QFrame):
    def __init__(self, page: "DoctorPage", issue: doctor.Issue):
        super().__init__()
        self.page, self.issue = page, issue
        color = COLORS[issue.severity]
        self.setObjectName("issue")
        self.setStyleSheet(f"#issue {{ border: 1px solid palette(mid); border-left: 5px solid {color};"
                           " border-radius: 6px; }")
        lay = QVBoxLayout(self)
        lay.setContentsMargins(12, 8, 12, 10)
        top = QHBoxLayout()
        chip = QLabel(f"<span style='color:{color}; font-weight:600'>{ICONS[issue.severity]} "
                      f"{doctor.SEVERITY[issue.severity][1]}</span>")
        top.addWidget(chip)
        top.addStretch(1)
        if issue.mods:
            show = QToolButton()
            show.setText("Show mod" if len(issue.mods) == 1 else "Show mods")
            show.setAutoRaise(True)
            show.clicked.connect(lambda: page.host.show_mods_by_id(issue.mods))
            top.addWidget(show)
        if page.host.settings.get("ai_enabled", False):
            ask = QToolButton()
            ask.setText("Ask AI")
            ask.setAutoRaise(True)
            ask.setToolTip("Have your local AI explain this and suggest what to do")
            ask.clicked.connect(lambda: page.host.ask_ai(issue))
            top.addWidget(ask)
        ign = QToolButton()
        ign.setText("Ignore")
        ign.setAutoRaise(True)
        ign.setToolTip("Hide this from the list (you can show ignored items again at the bottom)")
        ign.clicked.connect(lambda: page.dismiss(issue))
        top.addWidget(ign)
        lay.addLayout(top)
        title = QLabel(f"<b style='font-size:14px'>{html.escape(issue.title, quote=False)}</b>")
        title.setWordWrap(True)
        lay.addWidget(title)
        text = QLabel(html.escape(issue.explain, quote=False).replace("\n", "<br>"))
        text.setWordWrap(True)
        text.setTextInteractionFlags(Qt.TextSelectableByMouse)
        lay.addWidget(text)
        if issue.steps:
            toggle = QToolButton()
            toggle.setText("▸ How to fix it yourself")
            toggle.setAutoRaise(True)
            steps = QLabel("<ol style='margin:0'>" + "".join(
                f"<li>{html.escape(s, quote=False)}</li>" for s in issue.steps) + "</ol>")
            steps.setWordWrap(True)
            steps.setTextInteractionFlags(Qt.TextSelectableByMouse)
            steps.hide()
            toggle.clicked.connect(lambda: (steps.setVisible(not steps.isVisible()),
                                            toggle.setText(("▾" if steps.isVisible() else "▸")
                                                           + " How to fix it yourself")))
            lay.addWidget(toggle, 0, Qt.AlignLeft)
            lay.addWidget(steps)
        if issue.actions:
            row = QHBoxLayout()
            for i, a in enumerate(issue.actions):
                b = QPushButton(a.label)
                if i == 0:
                    b.setDefault(True)
                b.clicked.connect(lambda _=False, a=a: page.run(a))
                row.addWidget(b)
            row.addStretch(1)
            lay.addLayout(row)


class DoctorPage(QWidget):
    def __init__(self, host: "MainWindow"):
        super().__init__()
        self.host = host
        self.issues: list[doctor.Issue] = []
        self.checked_at = 0.0
        lay = QVBoxLayout(self)
        head = QHBoxLayout()
        self.badge = QLabel()
        self.badge.setFixedSize(44, 44)
        self.badge.setAlignment(Qt.AlignCenter)
        head.addWidget(self.badge)
        txt = QVBoxLayout()
        self.headline = QLabel("<b>Troubleshoot</b>")
        self.headline.setWordWrap(True)
        self.sub = QLabel("Kiln checks your mods for the problems that make games crash or "
                          "mods not work, explains them, and fixes what it safely can.")
        self.sub.setWordWrap(True)
        txt.addWidget(self.headline)
        txt.addWidget(self.sub)
        head.addLayout(txt, 1)
        self.recheck = QPushButton("Check again")
        self.recheck.clicked.connect(self.refresh)
        self.fix_all = QPushButton("Fix everything")
        self.fix_all.setStyleSheet("QPushButton { font-weight: 600; padding: 6px 14px; }")
        self.fix_all.clicked.connect(self.fix_everything)
        self.ask_btn = QPushButton("Ask AI…")
        self.ask_btn.setToolTip("Ask your local AI about your setup and what Troubleshoot found")
        self.ask_btn.clicked.connect(lambda: self.host.ask_ai(None, self.issues))
        head.addWidget(self.ask_btn)
        head.addWidget(self.recheck)
        head.addWidget(self.fix_all)
        lay.addLayout(head)
        opts = QHBoxLayout()
        self.show_info = QCheckBox("Show “good to know” items")
        self.show_info.setChecked(True)
        self.show_info.toggled.connect(self._fill)
        opts.addWidget(self.show_info)
        opts.addStretch(1)
        self.busy_note = QLabel()
        self.busy_note.setStyleSheet("color: #f08c00;")
        opts.addWidget(self.busy_note)
        lay.addLayout(opts)
        self.scroll = QScrollArea()
        self.scroll.setWidgetResizable(True)
        self.scroll.setFrameShape(QFrame.NoFrame)
        self.body = QWidget()
        self.body_lay = QVBoxLayout(self.body)
        self.body_lay.setSpacing(10)
        self.scroll.setWidget(self.body)
        self.tabs = QTabWidget()
        self.tabs.setDocumentMode(True)
        self.tabs.addTab(self.scroll, "Problems")
        self.logs = LogsView(self)
        self.tabs.addTab(self.logs, "Logs")
        lay.addWidget(self.tabs, 1)

    # ------------------------------------------------------------- data
    def _dismissed(self) -> set[str]:
        g = self.host.inst.game.id if self.host.inst else ""
        return set(self.host.settings.game(g).get("doctor_ignored", [])) if g else set()

    def dismiss(self, issue: doctor.Issue):
        gs = self.host.settings.game(self.host.inst.game.id)
        gs["doctor_ignored"] = sorted(set(gs.get("doctor_ignored", [])) | {issue.key})
        self.host.settings.save()
        self.issues = [i for i in self.issues if i.key != issue.key]
        self._fill()

    def unignore(self):
        gs = self.host.settings.game(self.host.inst.game.id)
        gs["doctor_ignored"] = []
        self.host.settings.save()
        self.refresh()

    def refresh(self):
        inst = self.host.inst
        if not inst:
            self.issues = []
            self._fill()
            return
        self.headline.setText("<b>Checking your mods…</b>")
        self.recheck.setEnabled(False)
        from .gui import Task
        ignored = self._dismissed()

        def done(res):
            self.recheck.setEnabled(True)
            if inst is not self.host.inst:
                return
            self.issues = res
            self.checked_at = time.time()
            self._fill()

        def fail(e):
            self.recheck.setEnabled(True)
            self.headline.setText(f"<b>The check failed:</b> {html.escape(str(e), quote=False)}")

        Task(doctor.diagnose, inst, ignored).start(done, fail)
        self.logs.refresh()

    def busy(self) -> str:
        """Why fixes can't run right now ('' if they can)."""
        h = self.host
        if h.crun:
            return "A collection is still installing."
        if h.dl_pool.activeThreadCount() or h.active_list.count():
            return "Downloads are still running."
        if h.jobs.activeThreadCount():
            return "Kiln is still installing or deploying."
        return ""

    # ------------------------------------------------------------- view
    def _fill(self):
        while self.body_lay.count():
            it = self.body_lay.takeAt(0)
            if it.widget():
                it.widget().hide()
                it.widget().deleteLater()
        inst = self.host.inst
        if not inst:
            self.headline.setText("<b>Troubleshoot</b>")
            self.sub.setText("Pick a game that's set up to check it.")
            self.fix_all.setEnabled(False)
            self._set_badge("info")
            return
        level, sentence = doctor.verdict(self.issues)
        self._set_badge(level)
        self.headline.setText(f"<b style='font-size:15px'>{html.escape(sentence, quote=False)}</b>")
        n_mods = sum(m.enabled for m in inst.mods)
        when = time.strftime("%H:%M", time.localtime(self.checked_at)) if self.checked_at else ""
        self.sub.setText(f"Checked {n_mods} enabled mod(s) for {inst.game.name}"
                         + (f" at {when}" if when else "") + ".")
        self.ask_btn.setVisible(bool(self.host.settings.get("ai_enabled", False)))
        plan = doctor.plan_fix_all(self.issues)
        busy = self.busy()
        self.busy_note.setText(f"{busy} Fixes are available when it's done." if busy else "")
        self.fix_all.setText(f"Fix everything ({len(plan)})" if plan else "Fix everything")
        self.fix_all.setEnabled(bool(plan) and not busy)
        shown = [i for i in self.issues if self.show_info.isChecked() or i.severity != "info"]
        if not shown:
            ok = QLabel("✔  Nothing to fix. If the game still misbehaves, play until it happens "
                        "and come back: Kiln reads crash reports and loader logs too.")
            ok.setWordWrap(True)
            self.body_lay.addWidget(ok)
        for i in shown:
            self.body_lay.addWidget(IssueCard(self, i))
        ign = self._dismissed()
        if ign:
            b = QPushButton(f"Show {len(ign)} ignored item(s) again")
            b.setFlat(True)
            b.clicked.connect(self.unignore)
            self.body_lay.addWidget(b, 0, Qt.AlignLeft)
        self.body_lay.addStretch(1)

    def _set_badge(self, level: str):
        c = COLORS.get(level, COLORS["info"])
        self.badge.setText(ICONS.get(level, "i"))
        self.badge.setStyleSheet(f"background:{c}; color:white; border-radius:22px; font-size:22px;"
                                 " font-weight:700;")

    # ------------------------------------------------------------- fixing
    def run(self, a: doctor.Action):
        busy = self.busy()
        if busy and not a.fix.startswith(("nexus_page:", "search:", "open_file:", "plugins")):
            QMessageBox.information(self, "Please wait", f"{busy} Try this fix once it's finished, "
                                    "so Kiln doesn't change your mod list mid-install.")
            return
        self.host.run_doctor_action(a, after=self.refresh)

    def fix_everything(self):
        busy = self.busy()
        if busy:
            QMessageBox.information(self, "Please wait", f"{busy} Try again once it's finished.")
            return
        plan = doctor.plan_fix_all(self.issues)
        if not plan:
            return
        left = [i for i in self.issues if not i.auto_action and i.severity in ("crash", "broken")]
        msg = "Kiln will:\n\n" + "\n".join(f"• {a.describe or a.label}" for _i, a in plan)
        if left:
            msg += (f"\n\n{len(left)} other problem(s) need you (a download or a choice); they'll "
                    "stay on the list with their own buttons.")
        msg += "\n\nEverything here can be undone from the Mods and Plugins pages."
        if QMessageBox.question(self, "Fix everything", msg) != QMessageBox.Yes:
            return
        self.host.apply_doctor_fixes([a for _i, a in plan], after=self._after_fix_all)

    def _after_fix_all(self, n_done: int, errors: list[str]):
        self.refresh()
        text = f"Done: {n_done} fix(es) applied."
        if errors:
            text += "\n\nThese didn't work:\n" + "\n".join(f"• {e}" for e in errors)
        self.host.statusBar().showMessage(text.splitlines()[0], 10000)
        if errors:
            QMessageBox.warning(self, "Fix everything", text)


# ====================================================================== the Logs tab
LOG_COLORS = {"problem": COLORS["crash"], "warn": COLORS["warn"], "note": COLORS["info"], "ok": COLORS["ok"]}
LOG_ICONS = {"problem": "✖", "warn": "⚠", "note": "i", "ok": "✔"}


def _breakable(text: str) -> str:
    """Let long paths and addresses wrap (after slashes, underscores and dots)."""
    import re
    return re.sub(r"([\\/_.])(?=\S{8})", "\\1\u200b", text)


class LogCard(QFrame):
    """One log: a summary line; click to see every line, in plain English and as written."""

    def __init__(self, view: "LogsView", r: logreader.Report):
        super().__init__()
        self.view, self.r = view, r
        color = LOG_COLORS[r.level]
        self.setObjectName("logcard")
        self.setStyleSheet(f"#logcard {{ border: 1px solid palette(mid); border-left: 4px solid {color};"
                           " border-radius: 6px; }")
        lay = QVBoxLayout(self)
        lay.setContentsMargins(10, 6, 10, 8)
        top = QHBoxLayout()
        self.toggle = QToolButton()
        self.toggle.setAutoRaise(True)
        self.toggle.setText("▸")
        top.addWidget(self.toggle)
        when = time.strftime("%H:%M", time.localtime(r.mtime))
        if not r.last_run:
            when = time.strftime("%a %d %b %H:%M", time.localtime(r.mtime))
        head = QLabel(f"<span style='color:{color}; font-weight:700'>{LOG_ICONS[r.level]}</span> "
                      f"<b>{html.escape(r.title, quote=False)}</b> "
                      f"<span style='color:gray'>· {when} · {html.escape(r.path.name, quote=False)}</span>"
                      f"<br>{html.escape(r.summary, quote=False)}")
        head.setWordWrap(True)
        top.addWidget(head, 1)
        if r.mods:
            b = QToolButton()
            b.setText("Show mod")
            b.setAutoRaise(True)
            b.clicked.connect(lambda: view.page.host.show_mods_by_id(r.mods))
            top.addWidget(b)
        if view.page.host.settings.get("ai_enabled", False) and r.level in ("problem", "warn"):
            b = QToolButton()
            b.setText("Ask AI")
            b.setAutoRaise(True)
            b.clicked.connect(lambda: view.page.host.ask_ai(self.as_issue()))
            top.addWidget(b)
        opn = QToolButton()
        opn.setText("Open file")
        opn.setAutoRaise(True)
        opn.clicked.connect(lambda: view.page.run(doctor.Action(f"open_file:{r.path}", "Open file", auto=False)))
        top.addWidget(opn)
        lay.addLayout(top)
        self.detail = QLabel(self._detail_html())
        self.detail.setWordWrap(True)
        self.detail.setTextFormat(Qt.RichText)
        self.detail.setTextInteractionFlags(Qt.TextSelectableByMouse)
        self.detail.hide()
        lay.addWidget(self.detail)
        self.toggle.clicked.connect(self.flip)
        if r.level == "problem" and r.last_run:
            self.flip()

    def flip(self):
        self.detail.setHidden(not self.detail.isHidden())
        self.toggle.setText("▸" if self.detail.isHidden() else "▾")

    def _detail_html(self) -> str:
        rows = []
        for ln in self.r.lines:
            c = LOG_COLORS[ln.level]
            raw = (f"<br><span style='color:gray; font-family:monospace; font-size:11px'>"
                   f"{_breakable(html.escape(ln.raw, quote=False))}</span>") if ln.raw else ""
            rows.append(f"<tr><td valign='top' style='color:{c}; padding-right:6px'>{LOG_ICONS[ln.level]}</td>"
                        f"<td>{_breakable(html.escape(ln.plain, quote=False))}{raw}</td></tr>")
        if not rows:
            rows.append("<tr><td></td><td>Nothing notable in this log.</td></tr>")
        return "<table cellspacing='4'>" + "".join(rows) + "</table>"

    def as_issue(self) -> doctor.Issue:
        sev = {"problem": "broken", "warn": "warn"}.get(self.r.level, "info")
        return doctor.Issue(f"log:{self.r.path.name}", sev, f"{self.r.title}'s log: {self.r.summary}",
                            "\n".join(f"{ln.plain} [log line: {ln.raw}]" if ln.raw else ln.plain
                                       for ln in self.r.lines[:12]),
                            mods=list(self.r.mods), source="logs")


class LogsView(QWidget):
    """Everything the last launch wrote, in plain English."""

    def __init__(self, page: DoctorPage):
        super().__init__()
        self.page = page
        self.reports: list[logreader.Report] = []
        self.story: logreader.Story | None = None
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 8, 0, 0)
        self.story_frame = QFrame()
        self.story_frame.setObjectName("story")
        fl = QVBoxLayout(self.story_frame)
        fl.setContentsMargins(12, 8, 12, 8)
        self.story_box = QLabel()
        self.story_box.setWordWrap(True)
        self.story_box.setTextFormat(Qt.RichText)
        self.story_box.setTextInteractionFlags(Qt.TextSelectableByMouse)
        fl.addWidget(self.story_box)
        lay.addWidget(self.story_frame)
        opts = QHBoxLayout()
        self.only_bad = QCheckBox("Only logs with problems or warnings")
        self.only_bad.setChecked(True)
        self.only_bad.toggled.connect(self._fill)
        self.older = QCheckBox("Include logs from older launches")
        self.older.toggled.connect(self._fill)
        opts.addWidget(self.only_bad)
        opts.addWidget(self.older)
        opts.addStretch(1)
        self.count = QLabel()
        self.count.setStyleSheet("color: gray;")
        opts.addWidget(self.count)
        lay.addLayout(opts)
        self.scroll = QScrollArea()
        self.scroll.setWidgetResizable(True)
        self.scroll.setFrameShape(QFrame.NoFrame)
        self.body = QWidget()
        self.body_lay = QVBoxLayout(self.body)
        self.body_lay.setSpacing(8)
        self.scroll.setWidget(self.body)
        self.scroll.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        lay.addWidget(self.scroll, 1)

    def refresh(self):
        inst = self.page.host.inst
        if not inst:
            self.reports, self.story = [], None
            self._fill()
            return
        from .gui import Task
        from . import launchguard

        def work():
            running = launchguard.game_running(inst)
            reps = logreader.reports(inst, running)
            return reps, logreader.story(inst, reps, running)

        def done(res):
            if inst is not self.page.host.inst:
                return
            self.reports, self.story = res
            self._fill()

        Task(work).start(done, lambda e: self.story_box.setText(
            f"Kiln couldn't read the logs: {html.escape(str(e), quote=False)}"))

    def _fill(self):
        while self.body_lay.count():
            it = self.body_lay.takeAt(0)
            if it.widget():
                it.widget().hide()
                it.widget().deleteLater()
        st = self.story
        bad_now = [r for r in self.reports if r.last_run and r.level in ("problem", "warn")]
        self.page.tabs.setTabText(1, f"Logs ({len(bad_now)})" if bad_now else "Logs")
        if st is None:
            self.story_box.setText("Pick a game to read its logs.")
            self.count.setText("")
            self.body_lay.addStretch(1)
            return
        c = LOG_COLORS.get(st.level, LOG_COLORS["note"])
        details = "".join(f"<p style='margin:4px 0'>{html.escape(d, quote=False)}</p>" for d in st.details)
        self.story_box.setText(
            f"<b style='font-size:14px'>{html.escape(st.headline, quote=False)}</b>{details}")
        self.story_frame.setStyleSheet(f"#story {{ border: 1px solid palette(mid); border-left: 5px solid {c};"
                                       " border-radius: 6px; }")
        shown = [r for r in self.reports
                 if (self.older.isChecked() or r.last_run)
                 and (not self.only_bad.isChecked() or r.level in ("problem", "warn"))]
        self.count.setText(f"{len(shown)} of {len(self.reports)} log(s)")
        if not shown:
            msg = ("✔  None of the logs from your last launch report a problem. Untick the box above "
                   "to see every log anyway.") if self.reports else \
                  "No logs yet. Start the game from Kiln once and come back."
            q = QLabel(msg)
            q.setWordWrap(True)
            self.body_lay.addWidget(q)
        for r in shown:
            self.body_lay.addWidget(LogCard(self, r))
        self.body_lay.addStretch(1)
