#!/bin/bash

# Script to analyze an existing pcap file for dCS API calls
# Usage: bash analyze_dcs_capture.sh <pcap_file>

PCAP_FILE="${1:-dcs_vivaldi_traffic.pcap}"

if [ ! -f "$PCAP_FILE" ]; then
    echo "Error: File not found: $PCAP_FILE"
    echo "Usage: bash analyze_dcs_capture.sh <pcap_file>"
    exit 1
fi

echo "=== Analyzing dCS Vivaldi Traffic Capture ==="
echo "File: $PCAP_FILE"
echo ""

echo "=== 1. All HTTP Methods ==="
tcpdump -r "$PCAP_FILE" -A 2>/dev/null | grep -E "^(GET|POST|PUT|DELETE) " | sort -u
echo ""

echo "=== 2. HTTP Host Headers ==="
tcpdump -r "$PCAP_FILE" -A 2>/dev/null | grep -E "^Host: " | sort -u
echo ""

echo "=== 3. API Paths (getData calls) ==="
tcpdump -r "$PCAP_FILE" -A 2>/dev/null | grep -oE "/api/[^? ]*" | sort -u
echo ""

echo "=== 4. Query Parameters ==="
tcpdump -r "$PCAP_FILE" -A 2>/dev/null | grep -oE "path=[^&\" ]*" | sort -u
echo ""

echo "=== 5. Full /api/getData Requests ==="
tcpdump -r "$PCAP_FILE" -A 2>/dev/null | grep -E "GET /api/getData" | sort -u
echo ""

echo "=== 6. JSON Responses (first few) ==="
tcpdump -r "$PCAP_FILE" -A 2>/dev/null | grep -A 5 "^\[{" | head -30
echo ""

echo "=== 7. Unique Path Values ==="
echo "These are the data paths the app queries:"
tcpdump -r "$PCAP_FILE" -A 2>/dev/null | grep -oE "path=[^&\" ]*" | sed 's/path=//' | sort -u
echo ""

echo "=== Analysis Complete ==="
echo ""
echo "To view full capture: tcpdump -r '$PCAP_FILE' -A | less"
echo "To filter for specific path: tcpdump -r '$PCAP_FILE' -A | grep 'path_name'"
