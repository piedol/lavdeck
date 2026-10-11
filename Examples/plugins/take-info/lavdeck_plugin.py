"""Minimal helper for writing Lavdeck plugins in Python (protocol 1). Standard library only.

Copy this file into your plugin folder. See docs/PLUGINS.md in the Lavdeck repository.

    from lavdeck_plugin import Request, progress, output, log, done, fail

    req = Request.read()
    progress(0.5, "Working")
    output(path, kind="text")
    done("Finished")
"""

from __future__ import annotations

import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path

PROTOCOL = 1


def _emit(**message) -> None:
    sys.stdout.write(json.dumps({k: v for k, v in message.items() if v is not None}) + "\n")
    sys.stdout.flush()


def progress(fraction: float, message: str | None = None) -> None:
    """Report progress between 0 and 1, with an optional status line."""
    _emit(type="progress", fraction=max(0.0, min(1.0, fraction)), message=message)


def output(path: str | os.PathLike, kind: str) -> None:
    """Announce a file you wrote, e.g. kind="transcript" or "audio"."""
    _emit(type="output", path=str(path), kind=kind)


def log(message: str) -> None:
    """A diagnostic line. Lavdeck keeps it with the job; users don't see it by default."""
    _emit(type="log", message=message)


def done(message: str | None = None) -> None:
    """Finish successfully, with an optional one-line summary."""
    _emit(type="done", message=message)


def fail(message: str, code: int = 1) -> None:
    """Report a user-readable error and exit."""
    _emit(type="error", message=message)
    sys.exit(code)


@dataclass
class Request:
    raw: dict

    @classmethod
    def read(cls) -> "Request":
        raw = json.load(sys.stdin)
        if raw.get("protocol") != PROTOCOL:
            fail(f"This plugin speaks protocol {PROTOCOL}, Lavdeck sent {raw.get('protocol')}.")
        return cls(raw)

    kind = property(lambda self: self.raw["kind"])            # "event" or "action"
    name = property(lambda self: self.raw["name"])            # e.g. "take.finished" or your action id
    take = property(lambda self: self.raw["take"])            # the take descriptor (see docs)
    settings = property(lambda self: self.raw.get("settings", {}))
    output_directory = property(lambda self: Path(self.raw["outputDirectory"]))
    plugin_directory = property(lambda self: Path(self.raw["pluginDirectory"]))

    def setting(self, key: str, default=None):
        value = self.settings.get(key)
        return default if value is None else value

    def files(self, role: str | None = None) -> list[dict]:
        return [f for f in self.take["files"] if role is None or f["role"] == role]

    def main_audio(self) -> dict | None:
        """The stereo file, else the mono mix, else the first single-mic track."""
        for role in ("stereo", "mix", "track"):
            found = self.files(role)
            if found:
                return found[0]
        return None

    def output_path(self, suffix: str) -> Path:
        """A path next to the take, e.g. output_path(".notes.txt") -> <folder>/<stem>.notes.txt."""
        return self.output_directory / f"{self.take['stem']}{suffix}"
