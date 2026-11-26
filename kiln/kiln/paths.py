"""XDG locations and persisted settings."""
from __future__ import annotations

import json
import os
from pathlib import Path

APP = "kiln"


def _xdg(var: str, fallback: str) -> Path:
    p = Path(os.environ.get(var) or Path.home() / fallback) / APP
    p.mkdir(parents=True, exist_ok=True)
    return p


def config_dir() -> Path:
    d = _xdg("XDG_CONFIG_HOME", ".config")
    try:
        os.chmod(d, 0o700)  # holds the Nexus API key
    except OSError:
        pass
    return d


def cache_dir() -> Path:
    return _xdg("XDG_CACHE_HOME", ".cache")


def data_dir() -> Path:
    return _xdg("XDG_DATA_HOME", ".local/share")


class Settings:
    """settings.json: {"nexus_api_key": ..., "games": {game_id: {...}}, ...}"""

    def __init__(self) -> None:
        self.file = config_dir() / "settings.json"
        try:
            self.data = json.loads(self.file.read_text())
        except (OSError, ValueError):
            self.data = {}

    def get(self, key, default=None):
        return self.data.get(key, default)

    def set(self, key, value) -> None:
        self.data[key] = value
        self.save()

    def game(self, game_id: str) -> dict:
        return self.data.setdefault("games", {}).setdefault(game_id, {})

    def save(self) -> None:
        tmp = self.file.with_suffix(".tmp")
        # Holds the Nexus API key: created readable only by you, never world-readable even for a moment.
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(json.dumps(self.data, indent=2))
        os.chmod(tmp, 0o600)
        tmp.replace(self.file)
