#!/bin/bash

# Script to discover dCS NSDK API paths by testing common patterns
# Usage: bash discover_dcs_api.sh

VIVALDI="http://dcs-vivaldi.local"

echo "=== dCS Vivaldi NSDK API Discovery Tool ==="
echo "Testing: $VIVALDI"
echo ""

# Test function
test_path() {
    local path="$1"
    local response=$(curl -s "${VIVALDI}/api/getData?path=${path}&roles=value" 2>&1)

    if echo "$response" | grep -q "does not exist"; then
        echo "  ✗ $path (not found)"
    elif echo "$response" | grep -q "error"; then
        echo "  ✗ $path (error: $(echo "$response" | grep -oE '"message":"[^"]*"' | cut -d'"' -f4))"
    else
        echo "  ✓ $path (SUCCESS!)"
        echo "    Response: $response" | cut -c 1-100
        echo ""
    fi
}

echo "=== Testing Network Paths ==="
test_path "network:state"
test_path "network:state/current_state"
test_path "network"

echo ""
echo "=== Testing Player/Renderer Paths ==="
test_path "player"
test_path "player:state"
test_path "player:now_playing"
test_path "renderer"
test_path "renderer:state"
test_path "renderer:now_playing"
test_path "upnp"
test_path "upnp:state"
test_path "upnp:player"
test_path "playback"
test_path "playback:state"

echo ""
echo "=== Testing Transport Paths ==="
test_path "transport"
test_path "transport:state"
test_path "transport:info"
test_path "av_transport"
test_path "av_transport:state"

echo ""
echo "=== Testing Audio/Stream Paths ==="
test_path "audio"
test_path "audio:state"
test_path "audio:format"
test_path "audio:input"
test_path "stream"
test_path "stream:state"
test_path "stream:info"
test_path "stream:format"
test_path "source"
test_path "source:info"
test_path "input"
test_path "input:state"
test_path "input:info"

echo ""
echo "=== Testing Device Paths ==="
test_path "device"
test_path "device:state"
test_path "device:info"
test_path "system"
test_path "system:state"
test_path "system:info"

echo ""
echo "=== Testing Status Paths ==="
test_path "status"
test_path "state"
test_path "info"

echo ""
echo "=== Discovery Complete ==="
echo "Successful paths will be shown above with ✓"
