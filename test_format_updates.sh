#!/bin/bash

# Test script to monitor dCS format updates
# Run this while changing tracks in Roon

echo "=== Testing dCS Format Updates ==="
echo ""
echo "Instructions:"
echo "1. Note the current format shown below"
echo "2. Change to a track with a DIFFERENT sample rate in Roon"
echo "3. Wait a few seconds and press Enter"
echo "4. Script will check if values changed"
echo ""

# Get initial values
echo "=== Initial Values ==="
echo -n "dCS API - Bit Depth: "
curl -s "http://192.168.50.31/api/getData?path=dcsworker:/dcs/currentBitDepth&roles=value" | python3 -c "import sys, json; data=json.load(sys.stdin); print(data[0]['i32_'])"

echo -n "dCS API - Sample Rate: "
curl -s "http://192.168.50.31/api/getData?path=dcsworker:/dcs/inputSampleRateCurrent&roles=value" | python3 -c "import sys, json; data=json.load(sys.stdin); print(data[0]['i32_'])"

echo ""
echo -n "Server API - Now Playing: "
curl -s "http://localhost:3000/api/now-playing" | python3 -c "import sys, json; data=json.load(sys.stdin); zones=[z for z in data['now_playing'] if 'dCS Vivaldi' in z['zone_name']]; print(zones[0]['dcs_format'] if zones else 'No dCS zone found')" 2>/dev/null || echo "Error"

echo ""
read -p "Press Enter after changing to a different track..."
echo ""

# Get updated values
echo "=== Updated Values ==="
echo -n "dCS API - Bit Depth: "
curl -s "http://192.168.50.31/api/getData?path=dcsworker:/dcs/currentBitDepth&roles=value" | python3 -c "import sys, json; data=json.load(sys.stdin); print(data[0]['i32_'])"

echo -n "dCS API - Sample Rate: "
curl -s "http://192.168.50.31/api/getData?path=dcsworker:/dcs/inputSampleRateCurrent&roles=value" | python3 -c "import sys, json; data=json.load(sys.stdin); print(data[0]['i32_'])"

echo ""
echo -n "Server API - Now Playing: "
curl -s "http://localhost:3000/api/now-playing" | python3 -c "import sys, json; data=json.load(sys.stdin); zones=[z for z in data['now_playing'] if 'dCS Vivaldi' in z['zone_name']]; print(zones[0]['dcs_format'] if zones else 'No dCS zone found')" 2>/dev/null || echo "Error"

echo ""
echo "=== Test Complete ==="
