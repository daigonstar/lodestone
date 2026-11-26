"""Command-line interface for other apps (e.g. the Lodestone launcher).

    kiln --games                 JSON: every game Kiln supports, with Steam app ids and setup state
    kiln --status <game_id>      JSON: profile, mod counts, deploy state and pre-flight checks
    kiln --game <game_id>        open Kiln on that game (forwards to a running Kiln)
    kiln --play <game_id>        Guarded Play for that game (checks, fixes, then launch)
    kiln --play-vanilla <id>     play without mods (keeps only the loaders Steam needs)

The JSON commands only read Kiln's files and never start the GUI.
"""
from __future__ import annotations

import json
from pathlib import Path

#: Messages forwarded to a running Kiln over its local socket (same channel as nxm:// links).
GAME_CMD = "kiln:game:"
PLAY_CMD = "kiln:play:"
VANILLA_CMD = "kiln:vanilla:"


def _game_path(settings, g) -> Path | None:
    """Kiln's configured folder for a game, else Steam's install location (same order as the GUI)."""
    from . import steam
    conf = settings.game(g.id).get("path") if g.id in settings.data.get("games", {}) else None
    if conf and g.validate(Path(conf)):
        return Path(conf)
    for appid in (g.steam_appid, *getattr(g, "alt_appids", ())):
        found = steam.find_app(appid)
        if found and (getattr(g, "trust_steam", False) or g.validate(found[0])):
            return found[0]
    return None


def games() -> list[dict]:
    from .games import all_games
    from .paths import Settings
    s = Settings()
    configured = s.data.get("games", {})
    out = []
    for g in all_games():
        path = _game_path(s, g)
        staging = configured.get(g.id, {}).get("staging")
        mods = 0
        if staging:
            try:
                mods = len(json.loads((Path(staging) / "kiln.json").read_text()).get("mods") or [])
            except (OSError, ValueError):
                pass
        out.append({
            "id": g.id,
            "name": g.name,
            "steam_appid": g.steam_appid,
            "alt_appids": list(getattr(g, "alt_appids", ())),
            "nexus_domain": g.nexus_domain,
            "installed": path is not None,
            # Opened in Kiln before (it saved the game folder and a staging folder).
            "managed": bool(staging),
            "mods": mods,
        })
    return out


def status(game_id: str) -> dict:
    from . import deploy, launchguard
    from .games import by_id
    from .instance import Instance
    from .paths import Settings
    g = by_id(game_id)
    if not g:
        return {"id": game_id, "error": "Kiln doesn't support this game"}
    s = Settings()
    gs = s.data.get("games", {}).get(g.id, {})
    path = _game_path(s, g)
    base = {"id": g.id, "name": g.name, "steam_appid": g.steam_appid, "installed": path is not None,
            "managed": bool(gs.get("staging"))}
    if path is None or not gs.get("staging"):
        return base
    inst = Instance(g, path, Path(gs["staging"]))
    members = inst.members()
    in_profile = [m for m in inst.mods if m.id in members]
    checks = launchguard.preflight(inst)
    return base | {
        "profile": inst.active_profile,
        "profiles": sorted(inst.profiles),
        "mods": len(in_profile),
        "enabled": sum(1 for m in in_profile if m.enabled),
        "downloaded": len(inst.mods),
        "plugins": len(inst.plugins),
        "deployed": deploy.is_deployed(inst),
        "vanilla": deploy.is_vanilla(inst),
        "essentials_done": bool(gs.get("essentials_done")),
        "issues": [{"level": c.level, "title": c.title, "detail": c.detail} for c in checks if c.level != "ok"],
    }


def print_json(data) -> int:
    print(json.dumps(data, indent=2))
    return 0
