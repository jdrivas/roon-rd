#!/bin/bash

# Comprehensive dCS Vivaldi API discovery script
# This will take a few minutes to run

VIVALDI="http://dcs-vivaldi.local"
OUTPUT_FILE="dcs_api_paths.txt"

echo "=== dCS Vivaldi Comprehensive API Discovery ===" | tee "$OUTPUT_FILE"
echo "Started: $(date)" | tee -a "$OUTPUT_FILE"
echo "" | tee -a "$OUTPUT_FILE"

test_path() {
    local path="$1"
    local response=$(curl -s "${VIVALDI}/api/getData?path=${path}&roles=value" 2>&1)

    if echo "$response" | grep -q "does not exist"; then
        return 1
    elif echo "$response" | grep -q '"error"'; then
        return 1
    else
        echo "✓ $path" | tee -a "$OUTPUT_FILE"
        echo "$response" | python3 -m json.tool 2>/dev/null | sed 's/^/  /' | tee -a "$OUTPUT_FILE"
        echo "" | tee -a "$OUTPUT_FILE"
        return 0
    fi
}

echo "=== CONFIRMED WORKING PATHS ===" | tee -a "$OUTPUT_FILE"
echo "" | tee -a "$OUTPUT_FILE"

# Known working paths
test_path "network:state"
test_path "network:state/current_state"
test_path "settings:/deviceName"
test_path "settings:/airplay/password"
test_path "firmwareUpdate:updateStatus"

echo "" | tee -a "$OUTPUT_FILE"
echo "=== TESTING SETTINGS PATHS ===" | tee -a "$OUTPUT_FILE"
echo "" | tee -a "$OUTPUT_FILE"

# Try various settings paths with forward slash notation
for setting in "upsampler" "filter" "display" "brightness" "outputRate" "output" "input" "source" "volume" "mute" "audioFormat" "sampleRate" "bitDepth"; do
    test_path "settings:/$setting"
done

echo "" | tee -a "$OUTPUT_FILE"
echo "=== TESTING NESTED SETTINGS ===" | tee -a "$OUTPUT_FILE"
echo "" | tee -a "$OUTPUT_FILE"

# Try nested paths
for prefix in "upsampler" "dac" "display" "audio"; do
    for suffix in "rate" "filter" "brightness" "state" "config" "output" "input"; do
        test_path "settings:/$prefix/$suffix"
    done
done

echo "" | tee -a "$OUTPUT_FILE"
echo "=== TESTING PLAYBACK/TRANSPORT PATHS ===" | tee -a "$OUTPUT_FILE"
echo "" | tee -a "$OUTPUT_FILE"

# Try different notation styles for playback
for path in "playback" "player" "transport" "renderer" "upnp"; do
    test_path "$path:state"
    test_path "$path:info"
    test_path "$path:now_playing"
    test_path "$path:/state"
    test_path "$path:/info"
done

echo "" | tee -a "$OUTPUT_FILE"
echo "=== TESTING AUDIO FORMAT PATHS ===" | tee -a "$OUTPUT_FILE"
echo "" | tee -a "$OUTPUT_FILE"

test_path "audio:format"
test_path "audio:sampleRate"
test_path "audio:input"
test_path "audio:/format"
test_path "audio:/sampleRate"
test_path "stream:info"
test_path "stream:/info"

echo "" | tee -a "$OUTPUT_FILE"
echo "=== Discovery Complete ===" | tee -a "$OUTPUT_FILE"
echo "Finished: $(date)" | tee -a "$OUTPUT_FILE"
echo "" | tee -a "$OUTPUT_FILE"
echo "Results saved to: $OUTPUT_FILE"
echo "Successful paths are marked with ✓"
