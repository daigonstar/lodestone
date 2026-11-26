"""FOMOD (ModuleConfig.xml) parser and evaluator, independent of the GUI.

Supports install steps with visibility conditions, the five group types,
plugin type descriptors (static and dependency-based), condition flags,
required files and conditional file installs.
"""
from __future__ import annotations

import re
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from pathlib import Path


class FomodError(RuntimeError):
    pass


@dataclass
class FileInstall:
    source: str
    destination: str
    is_folder: bool
    priority: int = 0


@dataclass
class Dependency:
    """A tree of conditions: operator And/Or over flag/file/game checks."""
    operator: str = "And"
    flags: list[tuple[str, str]] = field(default_factory=list)       # (flag, value)
    files: list[tuple[str, str]] = field(default_factory=list)       # (file, Active|Inactive|Missing)
    children: list["Dependency"] = field(default_factory=list)

    def evaluate(self, flags: dict[str, str], file_state) -> bool:
        results = [flags.get(f, "") == v for f, v in self.flags]
        results += [file_state(f) == s for f, s in self.files]
        results += [c.evaluate(flags, file_state) for c in self.children]
        if not results:
            return True
        return all(results) if self.operator.lower() == "and" else any(results)


@dataclass
class Plugin:
    name: str
    description: str = ""
    image: str = ""
    files: list[FileInstall] = field(default_factory=list)
    flags: list[tuple[str, str]] = field(default_factory=list)
    default_type: str = "Optional"
    patterns: list[tuple[Dependency, str]] = field(default_factory=list)

    def type(self, flags, file_state) -> str:
        for dep, t in self.patterns:
            if dep.evaluate(flags, file_state):
                return t
        return self.default_type


@dataclass
class Group:
    name: str
    type: str                     # SelectExactlyOne, SelectAtMostOne, SelectAtLeastOne, SelectAll, SelectAny
    plugins: list[Plugin]


@dataclass
class Step:
    name: str
    groups: list[Group]
    visible: Dependency | None = None


@dataclass
class ModuleConfig:
    name: str
    image: str = ""
    required: list[FileInstall] = field(default_factory=list)
    steps: list[Step] = field(default_factory=list)
    conditional: list[tuple[Dependency, list[FileInstall]]] = field(default_factory=list)
    module_deps: Dependency | None = None


# ------------------------------------------------------------------ parsing
def _read_xml(path: Path) -> ET.Element:
    data = path.read_bytes()
    if data.startswith(b"\xff\xfe") or data.startswith(b"\xfe\xff"):
        text = data.decode("utf-16")
    elif data.startswith(b"\xef\xbb\xbf"):
        text = data[3:].decode("utf-8", "replace")
    else:
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            text = data.decode("cp1252", "replace")
    text = re.sub(r"^\s*<\?xml[^>]*\?>", "", text)
    try:
        return ET.fromstring(text)
    except ET.ParseError as e:
        raise FomodError(f"ModuleConfig.xml is not valid XML: {e}") from e


def _strip_ns(el: ET.Element) -> None:
    for e in el.iter():
        if isinstance(e.tag, str) and "}" in e.tag:
            e.tag = e.tag.split("}", 1)[1]


def _files(el: ET.Element | None) -> list[FileInstall]:
    out = []
    if el is None:
        return out
    for c in el:
        if c.tag not in ("file", "folder"):
            continue
        src = (c.get("source") or "").replace("\\", "/")
        dst = c.get("destination")
        if dst is None:
            dst = src if c.tag == "file" else src
        out.append(FileInstall(src, dst.replace("\\", "/"), c.tag == "folder",
                               int(c.get("priority") or 0)))
    return out


def _dep(el: ET.Element | None) -> Dependency | None:
    if el is None:
        return None
    d = Dependency(operator=el.get("operator", "And"))
    for c in el:
        if c.tag == "flagDependency":
            d.flags.append((c.get("flag", ""), c.get("value", "")))
        elif c.tag == "fileDependency":
            d.files.append((c.get("file", ""), c.get("state", "Active")))
        elif c.tag == "dependencies":
            sub = _dep(c)
            if sub:
                d.children.append(sub)
        # gameDependency / fommDependency / foseDependency: assumed satisfied
    return d


def _order(items, el: ET.Element | None, key):
    order = (el.get("order") if el is not None else None) or "Ascending"
    if order == "Explicit":
        return items
    return sorted(items, key=lambda i: key(i).lower(), reverse=(order == "Descending"))


def parse(path: Path) -> ModuleConfig:
    root = _read_xml(path)
    _strip_ns(root)
    cfg = ModuleConfig(name=(root.findtext("moduleName") or "").strip() or "FOMOD installer")
    img = root.find("moduleImage")
    if img is not None:
        cfg.image = (img.get("path") or "").replace("\\", "/")
    cfg.module_deps = _dep(root.find("moduleDependencies"))
    cfg.required = _files(root.find("requiredInstallFiles"))

    steps_el = root.find("installSteps")
    steps = []
    for st in (steps_el.findall("installStep") if steps_el is not None else []):
        groups = []
        ogs = st.find("optionalFileGroups")
        for g in (ogs.findall("group") if ogs is not None else []):
            plugins = []
            pls = g.find("plugins")
            for p in (pls.findall("plugin") if pls is not None else []):
                pl = Plugin(name=p.get("name", "?"),
                            description=(p.findtext("description") or "").strip())
                im = p.find("image")
                if im is not None:
                    pl.image = (im.get("path") or "").replace("\\", "/")
                pl.files = _files(p.find("files"))
                cf = p.find("conditionFlags")
                if cf is not None:
                    pl.flags = [(f.get("name", ""), (f.text or "").strip()) for f in cf.findall("flag")]
                td = p.find("typeDescriptor")
                if td is not None:
                    t = td.find("type")
                    if t is not None:
                        pl.default_type = t.get("name", "Optional")
                    dt = td.find("dependencyType")
                    if dt is not None:
                        d = dt.find("defaultType")
                        if d is not None:
                            pl.default_type = d.get("name", "Optional")
                        pats = dt.find("patterns")
                        for pat in (pats.findall("pattern") if pats is not None else []):
                            dep = _dep(pat.find("dependencies"))
                            t = pat.find("type")
                            if dep and t is not None:
                                pl.patterns.append((dep, t.get("name", "Optional")))
                plugins.append(pl)
            groups.append(Group(g.get("name", ""), g.get("type", "SelectAny"),
                                _order(plugins, pls, lambda x: x.name)))
        steps.append(Step(st.get("name", ""), _order(groups, ogs, lambda x: x.name),
                          _dep(st.find("visible"))))
    cfg.steps = _order(steps, steps_el, lambda x: x.name)

    cfi = root.find("conditionalFileInstalls")
    if cfi is not None:
        pats = cfi.find("patterns")
        for pat in (pats.findall("pattern") if pats is not None else []):
            dep = _dep(pat.find("dependencies")) or Dependency()
            cfg.conditional.append((dep, _files(pat.find("files"))))
    return cfg


# ------------------------------------------------------------------ evaluation
def default_selection(group: Group, flags, file_state) -> list[int]:
    """Indices pre-selected for a group, following the plugin types."""
    types = [p.type(flags, file_state) for p in group.plugins]
    usable = [i for i, t in enumerate(types) if t != "NotUsable"]
    if group.type == "SelectAll":
        return list(range(len(group.plugins)))
    chosen = [i for i, t in enumerate(types) if t in ("Required", "Recommended")]
    if group.type in ("SelectExactlyOne", "SelectAtMostOne"):
        if chosen:
            return chosen[:1]
        return usable[:1] if group.type == "SelectExactlyOne" else []
    if group.type == "SelectAtLeastOne" and not chosen:
        return usable[:1]
    return chosen


def flags_after(cfg: ModuleConfig, selections: dict[tuple[int, int], list[int]],
                upto_step: int, file_state) -> dict[str, str]:
    """Condition flags set by selections in visible steps before upto_step."""
    flags: dict[str, str] = {}
    for si, step in enumerate(cfg.steps[:upto_step]):
        if step.visible and not step.visible.evaluate(flags, file_state):
            continue
        for gi, group in enumerate(step.groups):
            for pi in selections.get((si, gi), []):
                for k, v in group.plugins[pi].flags:
                    flags[k] = v
    return flags


def step_visible(cfg: ModuleConfig, si: int, selections, file_state) -> bool:
    step = cfg.steps[si]
    if not step.visible:
        return True
    return step.visible.evaluate(flags_after(cfg, selections, si, file_state), file_state)


def resolve(cfg: ModuleConfig, selections: dict[tuple[int, int], list[int]],
            file_state) -> list[tuple[str, str, bool]]:
    """Final list of (source, destination, is_folder), lowest priority first so
    later entries overwrite earlier ones."""
    flags = flags_after(cfg, selections, len(cfg.steps), file_state)
    chosen: list[FileInstall] = list(cfg.required)
    for si, step in enumerate(cfg.steps):
        if not step_visible(cfg, si, selections, file_state):
            continue
        for gi, group in enumerate(step.groups):
            for pi in selections.get((si, gi), []):
                chosen += group.plugins[pi].files
    for dep, files in cfg.conditional:
        if dep.evaluate(flags, file_state):
            chosen += files
    indexed = sorted(enumerate(chosen), key=lambda t: (t[1].priority, t[0]))
    return [(f.source, f.destination, f.is_folder) for _, f in indexed]


def selections_from_choices(cfg: ModuleConfig, options: list[dict],
                            file_state) -> dict[tuple[int, int], list[int]] | None:
    """Turn a collection's saved FOMOD choices ([{name, groups:[{name, choices:[{name, idx}]}]}])
    into selections. Returns None if a named step/group/option can't be matched, so the
    caller can fall back to the wizard."""
    sel: dict[tuple[int, int], list[int]] = {}
    by_step = {(o.get("name") or "").lower(): o for o in options or []}
    for si, step in enumerate(cfg.steps):
        flags = flags_after(cfg, sel, si, file_state)
        saved = by_step.get(step.name.lower())
        groups = {(g.get("name") or "").lower(): g for g in (saved or {}).get("groups") or []}
        for gi, group in enumerate(step.groups):
            g = groups.get(group.name.lower())
            if g is None:
                sel[(si, gi)] = default_selection(group, flags, file_state)
                continue
            picks = []
            for c in g.get("choices") or []:
                idx = next((i for i, p in enumerate(group.plugins)
                            if p.name.lower() == (c.get("name") or "").lower()), None)
                if idx is None and isinstance(c.get("idx"), int) and 0 <= c["idx"] < len(group.plugins):
                    idx = c["idx"]
                if idx is None:
                    return None
                picks.append(idx)
            if group.type == "SelectAll":
                picks = list(range(len(group.plugins)))
            sel[(si, gi)] = picks
    # every saved step must exist in this installer
    names = {s.name.lower() for s in cfg.steps}
    if any(k and k not in names for k in by_step):
        return None
    return sel
