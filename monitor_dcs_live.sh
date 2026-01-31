#!/bin/bash

# Monitor dCS Vivaldi traffic in real-time
# Captures traffic from iPhone/iPad to Vivaldi and extracts API calls

VIVALDI_IP="192.168.50.31"
IPHONE_IP="192.168.50.162"  # jdripho17promax from ARP table
CAPTURE_FILE="dcs_mosaic_capture.pcap"

echo "=== dCS Mosaic Live Traffic Monitor ==="
echo ""
echo "Monitoring traffic between devices and Vivaldi:"
echo "  Known iPhone: $IPHONE_IP"
echo "  Vivaldi: $VIVALDI_IP"
echo ""

# Find the active network interface on 192.168.50.x
INTERFACE=$(ifconfig | grep -B5 "inet 192.168.50" | grep "^[a-z]" | cut -d: -f1 | head -1)

if [ -z "$INTERFACE" ]; then
    echo "Error: Could not find network interface on 192.168.50.x"
    echo "Your interfaces:"
    ifconfig | grep "^[a-z]" | cut -d: -f1
    exit 1
fi

echo "Network Interface: $INTERFACE"
echo "Capture file: $CAPTURE_FILE"
echo ""
echo "==================== INSTRUCTIONS ===================="
echo "1. Keep this terminal window visible"
echo "2. Open dCS Mosaic app on your iPhone/iPad"
echo "3. Navigate around:"
echo "   - Go to Settings"
echo "   - Change upsampler rate or filter"
echo "   - Adjust display brightness"
echo "   - View what's playing"
echo "4. Watch for API paths appearing below"
echo "5. Press Ctrl+C when done to see summary"
echo "======================================================"
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

        echo "=== HTTP Requests Found ==="
        tcpdump -r "$CAPTURE_FILE" -A 2>/dev/null | \
            grep -E "GET |POST " | \
            head -20
        echo ""

        echo "=== API Paths Discovered ==="
        tcpdump -r "$CAPTURE_FILE" -A 2>/dev/null | \
            grep -oE "path=[^&\" ]*" | \
            sed 's/path=//' | \
            sort -u
        echo ""

        echo "=== getData Calls ==="
        tcpdump -r "$CAPTURE_FILE" -A 2>/dev/null | \
            grep "getData" | \
            head -10
        echo ""

        echo "=== setData Calls ==="
        tcpdump -r "$CAPTURE_FILE" -A 2>/dev/null | \
            grep "setData" | \
            head -10
        echo ""

        echo "Full capture saved to: $CAPTURE_FILE"
        echo "Analyze with: tcpdump -r $CAPTURE_FILE -A | less"
        echo "Or: bash analyze_dcs_capture.sh $CAPTURE_FILE"
    fi

    echo ""
    echo "Done!"
}

# Set trap for cleanup
trap cleanup EXIT INT TERM

echo "=== Live API Activity ==="
echo "(Each line shows time and API path being queried)"
echo ""

# Capture with both file output and live display
# Filter for HTTP traffic to/from Vivaldi
# Note: tcpdump's -l flag enables line buffering (works on macOS)
sudo tcpdump -i $INTERFACE -n -A -s 0 \
    "host $VIVALDI_IP and tcp port 80" \
    -l -w "$CAPTURE_FILE" 2>&1 | \
    grep --line-buffered -E "GET |POST |path=|Host:|roles=" | \
    while read -r line; do
        timestamp=$(date "+%H:%M:%S")

        # Extract and display API paths
        if echo "$line" | grep -q "path="; then
            path=$(echo "$line" | grep -oE "path=[^&\" ]*" | sed 's/path=//')
            roles=$(echo "$line" | grep -oE "roles=[^&\" ]*" | sed 's/roles=//')
            if [ -n "$path" ]; then
                echo "[$timestamp] 📍 Path: $path ${roles:+(roles: $roles)}"
            fi
        elif echo "$line" | grep -q "GET /api"; then
            method=$(echo "$line" | grep -oE "GET [^ ]*" || echo "GET")
            echo "[$timestamp] 🌐 Request: $method"
        elif echo "$line" | grep -q "POST /api"; then
            method=$(echo "$line" | grep -oE "POST [^ ]*" || echo "POST")
            echo "[$timestamp] 🌐 Request: $method"
        fi
    done
