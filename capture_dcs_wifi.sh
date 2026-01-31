#!/bin/bash

# Script to capture dCS Vivaldi API traffic via WiFi
# This works without rvictl - just uses regular network capture
# Run this with: bash capture_dcs_wifi.sh

VIVALDI_IP="192.168.50.31"
CAPTURE_FILE="dcs_vivaldi_traffic.pcap"

echo "=== dCS Vivaldi Traffic Capture Tool (WiFi Method) ==="
echo ""
echo "This will capture network traffic between any device and the dCS Vivaldi"
echo "Vivaldi IP: $VIVALDI_IP"
echo ""

# Find the network interface with IP in 192.168.50.x range
INTERFACE=$(ifconfig | grep -B5 "inet 192.168.50" | grep "^[a-z]" | cut -d: -f1 | head -1)

if [ -z "$INTERFACE" ]; then
    echo "Error: Could not find network interface on 192.168.50.x network"
    echo "Your interfaces:"
    ifconfig | grep "^[a-z]" | cut -d: -f1
    exit 1
fi

echo "Using network interface: $INTERFACE"
echo ""

echo "==================== INSTRUCTIONS ===================="
echo "1. Make sure your iPhone/iPad is on the same WiFi network"
echo "2. Open the dCS Mosaic app on your iPhone/iPad"
echo "3. Navigate around - view what's playing, settings, etc."
echo "4. When done, press Ctrl+C to stop capture"
echo "======================================================"
echo ""
echo "Starting capture in 3 seconds..."
sleep 3

# Cleanup function
cleanup() {
    echo ""
    echo ""
    echo "Stopping capture and analyzing..."
    echo ""

    if [ -f "$CAPTURE_FILE" ]; then
        echo "=== Capture saved to: $CAPTURE_FILE ==="
        echo ""

        echo "=== HTTP GET/POST Requests ==="
        tcpdump -r "$CAPTURE_FILE" -A 2>/dev/null | grep -E "GET |POST " | head -20
        echo ""

        echo "=== API Paths Found ==="
        tcpdump -r "$CAPTURE_FILE" -A 2>/dev/null | grep -oE "path=[^&\" ]*" | sed 's/path=//' | sort -u
        echo ""

        echo "=== getData Requests ==="
        tcpdump -r "$CAPTURE_FILE" -A 2>/dev/null | grep -E "GET /api/getData" | head -10
        echo ""

        echo "Full capture available at: $CAPTURE_FILE"
        echo "Analyze with: bash analyze_dcs_capture.sh $CAPTURE_FILE"
    fi

    echo ""
    echo "Done!"
}

# Set trap to cleanup on Ctrl+C
trap cleanup EXIT INT TERM

# Start capture - show live output AND save to file
echo "=== Live HTTP traffic (press Ctrl+C when done) ==="
echo ""
sudo tcpdump -i $INTERFACE -n -A -s 0 host $VIVALDI_IP -w $CAPTURE_FILE 2>&1 | \
    grep --line-buffered -E "GET|POST|path=|HTTP/|Host:"
