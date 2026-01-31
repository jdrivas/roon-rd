#!/bin/bash
# Watch server log for zone data building and broadcasting

echo "Monitoring zone data processing..."
echo "==================================="
echo ""

tail -f server.log | grep --line-buffered -E "(build_ws_zone_data|Processing zone|dCS format|Broadcasting zone|Zone .* is dCS|not eligible for dCS)"
