#!/usr/bin/env bash
# Walk through all tracks on a Roon zone and report libretto match results.
# Usage: ./diagnose_track_matching.sh [zone_id]
# If no zone_id given, picks the first zone with Figaro.

set -euo pipefail

BASE="http://localhost:3000"
LOG="/tmp/roon-rd.log"
ZONE_ID="${1:-}"
WAIT_AFTER_SKIP=4

get_zones() {
    curl -s "$BASE/now-playing" | python3 -c "
import json,sys
data = json.load(sys.stdin)
zones = data.get('now_playing', data) if isinstance(data, dict) else data
for z in zones:
    print(z['zone_id'] + '|' + z.get('zone_name','') + '|' + z.get('state','') + '|' + (z.get('track') or ''))
"
}

get_track() {
    get_zones | grep "^${ZONE_ID}" | head -1 | cut -d'|' -f4
}

control() {
    curl -s -X POST "$BASE/control" -H 'Content-Type: application/json' \
        -d "{\"zone_id\":\"$ZONE_ID\",\"control\":\"$1\"}" > /dev/null
}

# Auto-detect zone
if [[ -z "$ZONE_ID" ]]; then
    ZONE_ID=$(get_zones | grep -i 'figaro\|nozze' | head -1 | cut -d'|' -f1 || true)
    if [[ -z "$ZONE_ID" ]]; then
        echo "ERROR: No zone found playing Figaro. Pass zone_id manually."
        exit 1
    fi
fi
ZONE_NAME=$(get_zones | grep "^${ZONE_ID}" | head -1 | cut -d'|' -f2)
echo "Zone: $ZONE_NAME ($ZONE_ID)"

# Start playback so next/previous work
echo "Starting playback..."
control play
sleep 2

# Rewind to first track
echo "Rewinding to first track..."
for i in $(seq 1 35); do
    TRACK=$(get_track)
    if echo "$TRACK" | grep -qi "sinfonia\|overture"; then
        break
    fi
    control previous
    sleep 1.5
done

# Pause so we don't actually play audio while testing
sleep 1
echo "At: $(get_track)"
echo ""
echo "=== Track Matching Diagnosis ==="
echo ""

MATCHED=0
FAILED=0
TOTAL=0
PREV_TRACK=""

for i in $(seq 1 35); do
    # Get current track
    TRACK_INFO=$(get_track)

    if [[ -z "$TRACK_INFO" || "$TRACK_INFO" == "(none)" ]]; then
        echo "  [END] No more tracks"
        break
    fi

    # Skip if same track as before (next didn't advance — end of queue)
    if [[ "$TRACK_INFO" == "$PREV_TRACK" && $i -gt 1 ]]; then
        echo "  [END] Reached end of queue (track didn't change)"
        break
    fi
    PREV_TRACK="$TRACK_INFO"

    # Mark log position, wait for WS match to appear
    LOG_POS=$(wc -l < "$LOG" | tr -d ' ')
    sleep 2

    # Read new log lines for this track's match attempt
    TAIL_N=$(( $(wc -l < "$LOG" | tr -d ' ') - LOG_POS ))
    if [[ $TAIL_N -gt 0 ]]; then
        MATCH_LINE=$(tail -"$TAIL_N" "$LOG" | grep -E "album exact match|album prefix match|AlbumAndTitle|no track match found" | tail -1 || echo "")
    else
        MATCH_LINE=""
    fi

    TOTAL=$((TOTAL + 1))

    if echo "$MATCH_LINE" | grep -qE "album exact match|album prefix match|AlbumAndTitle"; then
        STATUS="✓ MATCH"
        MATCHED=$((MATCHED + 1))
    else
        STATUS="✗ FAIL "
        FAILED=$((FAILED + 1))
    fi

    SHORT=$(echo "$TRACK_INFO" | cut -c1-90)
    printf "  %s [%2d] %s\n" "$STATUS" "$i" "$SHORT"

    if [[ "$STATUS" == "✗ FAIL " && -n "$MATCH_LINE" ]]; then
        echo "         $(echo "$MATCH_LINE" | sed 's/.*\] //')"
    fi

    # Advance to next track
    control next
    sleep "$WAIT_AFTER_SKIP"
done

# Pause playback when done
control pause

echo ""
echo "=== Summary ==="
echo "Total: $TOTAL  Matched: $MATCHED  Failed: $FAILED"
