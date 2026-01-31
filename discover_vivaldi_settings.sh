#!/bin/bash

# Script to discover Vivaldi-specific settings API paths
# Based on known Mosaic features: upsampler, filter, display

VIVALDI="http://dcs-vivaldi.local"

echo "=== dCS Vivaldi Settings API Discovery ==="
echo "Testing paths for device settings Mosaic can control"
echo ""

test_path() {
    local path="$1"
    local response=$(curl -s "${VIVALDI}/api/getData?path=${path}&roles=value" 2>&1)

    if echo "$response" | grep -q "does not exist"; then
        return 1
    elif echo "$response" | grep -q '"error"'; then
        return 1
    else
        echo "✓ FOUND: $path"
        echo "  $response" | python3 -m json.tool 2>/dev/null || echo "  $response"
        echo ""
        return 0
    fi
}

echo "=== Upsampler Settings ==="
test_path "upsampler"
test_path "upsampler:rate"
test_path "upsampler:output"
test_path "upsampler:output_rate"
test_path "upsampler:settings"
test_path "upsampler:config"
test_path "upsampler:state"
test_path "settings:upsampler"
test_path "config:upsampler"

echo "=== Filter Settings ==="
test_path "filter"
test_path "filter:type"
test_path "filter:settings"
test_path "upsampler:filter"
test_path "dac:filter"
test_path "settings:filter"

echo "=== Display Settings ==="
test_path "display"
test_path "display:brightness"
test_path "display:state"
test_path "display:settings"
test_path "settings:display"
test_path "ui:display"
test_path "ui:brightness"

echo "=== Device/Hardware Settings ==="
test_path "hardware"
test_path "hardware:state"
test_path "settings"
test_path "settings:device"
test_path "config"
test_path "config:device"

echo "=== Input/Source Settings ==="
test_path "input:selected"
test_path "input:current"
test_path "input:active"
test_path "source:selected"
test_path "source:current"
test_path "source:active"

echo "=== Trying generic 'all' paths ==="
test_path "settings:all"
test_path "config:all"
test_path "state:all"

echo ""
echo "=== Discovery complete ==="
