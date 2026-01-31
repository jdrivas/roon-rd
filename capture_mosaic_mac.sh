#!/bin/bash

# Capture dCS Mosaic traffic from Mac to Vivaldi
# This captures YOUR Mac's traffic, so it will definitely work

VIVALDI_IP="192.168.50.31"
CAPTURE_FILE="mosaic_mac_traffic.pcap"

echo "=== Capturing Mac Mosaic → Vivaldi Traffic ==="
echo ""
echo "Vivaldi IP: $VIVALDI_IP"
echo "Capture file: $CAPTURE_FILE"
echo ""
echo "==================== INSTRUCTIONS ===================="
echo "1. Keep this terminal window visible"
echo "2. Open dCS Mosaic app on THIS Mac"
echo "3. Navigate around in Mosaic:"
echo "   - Go to Device Settings"
echo "   - Change upsampler rate or filter"
echo "   - Adjust display brightness"
echo "   - View what's playing"
echo "   - Browse music sources"
echo "4. Watch API calls appear below in REAL-TIME"
echo "5. Press Ctrl+C when done"
echo "======================================================"
echo ""
echo "=== Live API Calls ==="
echo ""

# Cleanup function
cleanup() {
    echo ""
    echo ""
    echo "=== Capture Complete ==="
    echo ""

    if [ -f "$CAPTURE_FILE" ]; then
        echo "Analyzing captured traffic..."
        echo ""

        echo "=== Unique API Paths Discovered ==="
        tcpdump -r "$CAPTURE_FILE" -A 2>/dev/null | \
            grep -oE "path=[^&\" ]*" | \
            sed 's/path=//' | \
            sort -u
        echo ""

        echo "=== Unique Roles Used ==="
        tcpdump -r "$CAPTURE_FILE" -A 2>/dev/null | \
            grep -oE "roles=[^&\" ]*" | \
            sed 's/roles=//' | \
            sort -u
        echo ""

        echo "=== Sample HTTP Requests (first 10) ==="
        tcpdump -r "$CAPTURE_FILE" -A 2>/dev/null | \
            grep -E "GET |POST " | \
            head -10
        echo ""

        echo "Full capture saved to: $CAPTURE_FILE"
        echo "Analyze with: tcpdump -r $CAPTURE_FILE -A | less"
    fi

    echo ""
    echo "Done!"
}

# Set trap for cleanup
trap cleanup EXIT INT TERM

# Capture on ANY interface to/from Vivaldi
# Using -i any captures on all interfaces
# The -l flag enables line buffering
# We write to file AND display live
sudo tcpdump -i any -n -A -s 0 \
    "host $VIVALDI_IP and tcp port 80" \
    -l -w "$CAPTURE_FILE" 2>&1 | \
    grep --line-buffered -E "GET |POST |path=|roles=|Host:" | \
    while read -r line; do
        timestamp=$(date "+%H:%M:%S.%3N")

        # Extract and display API paths in real-time
        if echo "$line" | grep -q "path="; then
            path=$(echo "$line" | grep -oE "path=[^&\" ]*" | sed 's/path=//' | head -1)
            roles=$(echo "$line" | grep -oE "roles=[^&\" ]*" | sed 's/roles=//' | head -1)
            if [ -n "$path" ]; then
                if [ -n "$roles" ]; then
                    echo "[$timestamp] 📍 $path (roles: $roles)"
                else
                    echo "[$timestamp] 📍 $path"
                fi
            fi
        elif echo "$line" | grep -qE "GET /|POST /"; then
            method=$(echo "$line" | grep -oE "(GET|POST) [^ ]*" | head -1)
            echo "[$timestamp] 🌐 $method"
        fi
    done
