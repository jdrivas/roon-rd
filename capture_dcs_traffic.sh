#!/bin/bash

# Script to capture dCS Vivaldi API traffic from iPhone
# Run this with: bash capture_dcs_traffic.sh

IPHONE_UDID="00008150000E35CC0C84401C"
VIVALDI_IP="192.168.50.31"
CAPTURE_FILE="dcs_vivaldi_traffic.pcap"

echo "=== dCS Vivaldi Traffic Capture Tool ==="
echo ""
echo "This will capture network traffic between your iPhone and the dCS Vivaldi"
echo "UDID: $IPHONE_UDID"
echo "Vivaldi IP: $VIVALDI_IP"
echo ""

# Step 1: Create virtual interface
echo "Step 1: Creating virtual network interface..."
sudo rvictl -s $IPHONE_UDID

if [ $? -ne 0 ]; then
    echo "Error: Failed to create virtual interface"
    echo "Make sure your iPhone is connected via USB"
    exit 1
fi

# Find the interface name (usually rvi0)
INTERFACE=$(ifconfig | grep "^rvi" | cut -d: -f1 | head -1)

if [ -z "$INTERFACE" ]; then
    echo "Error: Could not find virtual interface"
    sudo rvictl -x $IPHONE_UDID
    exit 1
fi

echo "Created interface: $INTERFACE"
echo ""

# Step 2: Start packet capture
echo "Step 2: Starting packet capture..."
echo "Capturing traffic to $CAPTURE_FILE"
echo ""
echo "==================== INSTRUCTIONS ===================="
echo "1. Open the dCS Mosaic app on your iPhone"
echo "2. Navigate around - view what's playing, etc."
echo "3. When done, press Ctrl+C to stop capture"
echo "======================================================"
echo ""

# Capture packets - both summary and full capture
sudo tcpdump -i $INTERFACE -n host $VIVALDI_IP -w $CAPTURE_FILE -v &
TCPDUMP_PID=$!

# Also show real-time HTTP traffic
echo "=== Real-time HTTP requests (press Ctrl+C when done) ==="
sudo tcpdump -i $INTERFACE -n host $VIVALDI_IP -A -s 0 | grep -E "GET|POST|Host:|HTTP/|path="

# Cleanup function
cleanup() {
    echo ""
    echo ""
    echo "Step 3: Stopping capture and cleaning up..."

    # Kill tcpdump
    sudo kill $TCPDUMP_PID 2>/dev/null

    # Remove virtual interface
    sudo rvictl -x $IPHONE_UDID

    echo ""
    echo "Capture saved to: $CAPTURE_FILE"
    echo ""
    echo "=== Analyzing capture for API calls ==="

    # Extract HTTP requests
    if [ -f "$CAPTURE_FILE" ]; then
        echo ""
        echo "HTTP GET/POST requests found:"
        tcpdump -r $CAPTURE_FILE -A 2>/dev/null | grep -E "GET |POST " | sort -u

        echo ""
        echo "API paths (looking for /api/getData):"
        tcpdump -r $CAPTURE_FILE -A 2>/dev/null | grep -oE "path=[^&\" ]*" | sort -u

        echo ""
        echo "Full capture file: $CAPTURE_FILE"
        echo "You can analyze it with: tcpdump -r $CAPTURE_FILE -A | less"
    fi

    echo ""
    echo "Done!"
}

# Set trap to cleanup on Ctrl+C
trap cleanup EXIT INT TERM

# Wait for user to press Ctrl+C
wait $TCPDUMP_PID
