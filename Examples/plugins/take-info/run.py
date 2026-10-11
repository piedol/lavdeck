#!/usr/bin/env python3
"""Example Lavdeck plugin: writes a short Markdown summary of a take next to its audio."""

import datetime as dt

from lavdeck_plugin import Request, done, fail, output, progress

req = Request.read()
take = req.take
audio = req.main_audio()
if audio is None:
    fail("This take has no audio.")

progress(0.3, "Summarising take")
length = take.get("duration") or 0
lines = [f"# Take {take['id']}", ""]
if take.get("started"):
    started = dt.datetime.fromisoformat(take["started"].replace("Z", "+00:00")).astimezone()
    lines.append(f"- Recorded: {started:%Y-%m-%d %H:%M:%S}")
lines.append(f"- Length: {int(length // 60)}:{int(length % 60):02d}")
lines.append(f"- Mode: {take.get('channelMode') or 'unknown'}")
for speaker in take.get("speakers", []):
    lines.append(f"- Mic {speaker['mic']}: {speaker['name']}")
lines += ["", "## Files", ""] + [f"- `{f['path'].rsplit('/', 1)[-1]}` ({f['role']})" for f in req.files()]
if req.setting("greeting"):
    lines += ["", str(req.setting("greeting"))]

path = req.output_path(".info.md")
path.write_text("\n".join(lines) + "\n")
output(path, kind="notes")
done(f"Wrote {path.name}")
