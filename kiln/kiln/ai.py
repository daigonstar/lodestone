"""Optional AI helper: a local model (Ollama) that explains problems and answers
questions about your setup in plain English.

It only talks: it never changes anything. Kiln's own checks and fix buttons stay
in charge, and the AI is told what Kiln found so it doesn't have to guess.
Everything stays on your machine unless you point Kiln at a remote Ollama.
"""
from __future__ import annotations

import json
from typing import TYPE_CHECKING, Callable

import requests

if TYPE_CHECKING:
    from .doctor import Issue
    from .instance import Instance

DEFAULT_URL = "http://localhost:11434"

SYSTEM = """You are the help assistant inside Kiln, a mod manager for Linux (games run through Steam and Proton).
Your users are often new to modding. Answer in plain, friendly English, short paragraphs or a few steps.

Rules:
- Base your answer on the setup details and Kiln's findings below. If they don't say, say you're not sure
  rather than guessing. Never invent mod names, versions, file names or links.
- The user manages mods with Kiln. Never tell them to use Vortex or Mod Organizer, or to copy files into the
  game folder by hand: in Kiln you install from the Nexus Mods page or the Collections page, and a mod that's
  already downloaded for another profile is added with “Show all downloaded” on the Mods page (tick it).
- Kiln looks up the mods a question mentions and tells you their state (enabled in this profile, downloaded
  but not in this profile, or not downloaded). Trust that lookup; don't contradict it.
- Keep answers short: under 200 words, a few clear steps. Only mention mods named in the details.
- Prefer Kiln's own buttons over manual work: Troubleshoot fixes (“Fix everything”, “Find a version that works”,
  “Use the copy you have”, “Update it”), “Match my mods to this game version”, the essentials button in the
  banner, the Mods page (tick/untick, “Show all downloaded”), Plugins page and “Auto-sort”.
- Kiln can't run Windows tools like xEdit, BodySlide, Pandora or Nemesis for the user; explain how to run them
  through Proton when they're needed.
- Remember the game runs under Proton: Windows paths live in the game's Proton prefix.
- Never suggest playing online with mods in games that ban for it, or anything that breaks Nexus Mods' rules
  (like automating free downloads)."""


class AIError(RuntimeError):
    pass


def list_models(url: str = DEFAULT_URL) -> list[dict]:
    """[{"name", "thinking"}] from Ollama."""
    try:
        r = requests.get(url.rstrip("/") + "/api/tags", timeout=5)
        r.raise_for_status()
    except requests.RequestException as e:
        raise AIError(f"Couldn't reach Ollama at {url}. Is it running? ({e.__class__.__name__})") from None
    return [{"name": m["name"], "thinking": "thinking" in (m.get("capabilities") or [])}
            for m in r.json().get("models", [])]


# ------------------------------------------------------------------ what the model is told
def context(inst: "Instance", issue: "Issue | None" = None, issues: list | None = None) -> str:
    """A compact description of the setup (and the problem) for the model."""
    from . import essentials, gameversions
    g = inst.game
    enabled = [m for m in inst.mods if m.enabled]
    lines = [f"Game: {g.name}" + (f" {gameversions.short(gameversions.installed_version(inst))}"
                                  if gameversions.installed_version(inst) else ""),
             f"Profile: {inst.active_profile} ({len(enabled)} mods enabled of {len(inst.mods)} downloaded)"]
    try:
        ess = [f"{e.name}: {essentials.status(inst, e)}" for e in essentials.for_game(g.id)]
        if ess:
            lines.append("Essentials: " + "; ".join(ess))
    except Exception:  # noqa: BLE001
        pass
    if g.has_plugins:
        lines.append(f"Plugins: {sum(p['enabled'] for p in inst.plugins)} enabled")
    if issue is not None:
        lines += ["", "THE PROBLEM KILN FOUND:", f"{issue.title}", issue.explain]
        if issue.steps:
            lines.append("Kiln's suggested steps: " + " | ".join(issue.steps))
        if issue.actions:
            lines.append("Buttons Kiln offers for it: " + ", ".join(a.label for a in issue.actions))
        mods = [inst.get(i) for i in issue.mods]
        for m in [m for m in mods if m][:6]:
            lines.append(f"Involved mod: {m.name} {m.version} (Nexus mod {m.nexus_mod_id or '-'}, "
                         f"{'enabled' if m.enabled else 'disabled'})" + (f": {m.summary[:200]}" if m.summary else ""))
        if issue.key.startswith("crash:"):
            lines += ["", "CRASH LOG (start):", _crash_excerpt(inst)]
    if issues:
        lines += ["", "EVERYTHING KILN'S TROUBLESHOOT PAGE FOUND:"]
        lines += [f"- [{i.severity}] {i.title}" for i in issues[:25]]
    return "\n".join(lines)


ALIASES = {"po3": "powerofthree", "skse": "script extender", "usssep": "unofficial", "ussep": "unofficial"}
STOP = {"the", "and", "for", "with", "that", "this", "what", "why", "how", "does", "should", "work", "required",
        "install", "installed", "skyrim", "mod", "mods", "game", "error", "showed", "said", "says", "need",
        "needs", "plugin", "dll", "esp", "esm", "have", "from", "into", "not", "can", "will", "about"}


def _compact(s: str) -> str:
    return "".join(ch for ch in s.lower() if ch.isalnum())


def lookup(inst: "Instance", text: str) -> str:
    """The state of every mod a piece of text mentions (by name, file name or plugin file)."""
    words = []
    for w in __import__("re").findall(r"[A-Za-z0-9][A-Za-z0-9_.'-]{2,}", text):
        w = w.lower().removesuffix("'s").removesuffix(".dll").removesuffix(".esp").removesuffix(".esm")
        w = ALIASES.get(w, w)
        if w not in STOP and len(_compact(w)) >= 3:
            words.append(_compact(w))
    if not words:
        return ""
    found: dict[str, list] = {w: [] for w in set(words)}
    for m in inst.mods:
        hay = _compact(f"{m.name} {m.file_name} {m.id}")
        files = " ".join(_compact(r.rsplit("/", 1)[-1]) for r in inst.mod_files(m)
                         if r.lower().endswith((".dll", ".esp", ".esm", ".esl")))
        for w in found:
            if w in hay or (len(w) >= 6 and w in files):
                found[w].append(m)
    score: dict[str, float] = {}
    nwords: dict[str, int] = {}
    specific: set[str] = set()
    by_id = {}
    for w, ms in found.items():              # rare words count more than common ones (“tweaks”)
        for m in ms:
            score[m.id] = score.get(m.id, 0) + 1 / len(ms)
            nwords[m.id] = nwords.get(m.id, 0) + 1
            by_id[m.id] = m
            if len(ms) <= 4:
                specific.add(m.id)
    # a mod counts when a specific word names it, or when several words do
    keep = [mid for mid in score if mid in specific or nwords[mid] >= 2]
    if not keep:
        return "Kiln found no downloaded mod matching that."
    hits = sorted(((score[mid], by_id[mid]) for mid in keep), key=lambda sm: (-sm[0], sm[1].name))
    lines = []
    for _s, m in hits[:10]:
        state = ("enabled in this profile" if m.enabled else
                 "downloaded, in this profile but turned off" if inst.in_profile(m) else
                 "downloaded but NOT in this profile (tick it under “Show all downloaded”)")
        lines.append(f"- {m.name} {m.version}: {state}")
    return "\n".join(lines)


def _crash_excerpt(inst) -> str:
    from . import crashlogs
    from .doctor import Ctx
    try:
        files = crashlogs._crash_files(Ctx(inst))
        if files:
            return "\n".join(crashlogs._read(files[0], 200000).splitlines()[:120])
    except Exception:  # noqa: BLE001
        pass
    return "(none)"


# ------------------------------------------------------------------ asking
def chat(url: str, model: str, messages: list[dict], on_text: Callable[[str], None],
         cancelled: Callable[[], bool] = lambda: False, thinking: bool = False) -> str:
    """Stream an answer from Ollama, calling on_text with each new piece."""
    body = {"model": model, "messages": messages, "stream": True, "options": {"temperature": 0.3}}
    if thinking:
        body["think"] = False          # answer straight away; beginners don't need the musings
    try:
        r = requests.post(url.rstrip("/") + "/api/chat", json=body, stream=True, timeout=(10, 600))
        r.raise_for_status()
    except requests.RequestException as e:
        raise AIError(f"Ollama didn't answer: {e}") from None
    out = []
    for line in r.iter_lines():
        if cancelled():
            r.close()
            break
        if not line:
            continue
        d = json.loads(line)
        if d.get("error"):
            raise AIError(d["error"])
        piece = (d.get("message") or {}).get("content") or ""
        if piece:
            out.append(piece)
            on_text(piece)
        if d.get("done"):
            break
    return "".join(out)
