#!/usr/bin/env python3
"""
Collect all Roon track titles by playing through the queue.
Outputs titles to stdout, one per line.
"""
import json
import time
import urllib.request
import urllib.parse
import sys

BASE = "http://localhost:3000"
ZONE_ID = sys.argv[1] if len(sys.argv) > 1 else "1601fe542568c93b4f41b0e8a3d1378f972f"
WAIT = 3  # seconds between skips

def get_track():
    data = json.loads(urllib.request.urlopen(f"{BASE}/now-playing").read())
    zones = data.get("now_playing", data) if isinstance(data, dict) else data
    for z in zones:
        if z["zone_id"] == ZONE_ID:
            return z.get("track"), z.get("state")
    return None, None

def control(cmd):
    req = urllib.request.Request(
        f"{BASE}/control/{urllib.parse.quote(ZONE_ID, safe='')}",
        data=json.dumps({"control": cmd}).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    urllib.request.urlopen(req)

# Start playback
print("Starting playback...", file=sys.stderr)
control("play")
time.sleep(2)

# No rewind — user has positioned on the first track.
# Just collect current track, then push next.
print("Collecting from current position (no rewind)...", file=sys.stderr)

# Collect all titles
titles = []
prev = None
for i in range(80):
    track, state = get_track()
    if not track:
        break
    if track == prev and i > 0:
        # Track didn't change — we've reached the end
        break
    titles.append(track)
    print(f"[{i+1:2d}] {track}", file=sys.stderr)
    prev = track
    control("next")
    time.sleep(WAIT)

# Pause when done
control("pause")

# Output JSON array of titles
print(json.dumps(titles, indent=2))
print(f"\nCollected {len(titles)} titles", file=sys.stderr)
