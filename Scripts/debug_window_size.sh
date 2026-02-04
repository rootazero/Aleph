#!/bin/bash

echo "=== Aleph Window Size Debug Tool ==="
echo ""
echo "This script will:"
echo "1. Kill any running Aleph process"
echo "2. Start Aleph in debug mode"
echo "3. Monitor window size related logs"
echo ""

# Kill existing Aleph process
echo "Killing existing Aleph process..."
killall Aleph 2>/dev/null
sleep 1

# Start Aleph in background
echo "Starting Aleph..."
open -a /Users/zouguojun/Library/Developer/Xcode/DerivedData/Aleph-*/Build/Products/Debug/Aleph.app &
sleep 2

# Monitor logs
echo ""
echo "=== Monitoring Aleph logs (Press Ctrl+C to stop) ==="
echo ""
log stream --predicate 'process == "Aleph"' --style compact 2>&1 | grep -E "AppDelegate|Window|size|minSize|contentSize"
