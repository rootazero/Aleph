#!/bin/bash

echo "=== Aether Window Size Debug Tool ==="
echo ""
echo "This script will:"
echo "1. Kill any running Aether process"
echo "2. Start Aether in debug mode"
echo "3. Monitor window size related logs"
echo ""

# Kill existing Aether process
echo "Killing existing Aether process..."
killall Aether 2>/dev/null
sleep 1

# Start Aether in background
echo "Starting Aether..."
open -a /Users/zouguojun/Library/Developer/Xcode/DerivedData/Aether-*/Build/Products/Debug/Aether.app &
sleep 2

# Monitor logs
echo ""
echo "=== Monitoring Aether logs (Press Ctrl+C to stop) ==="
echo ""
log stream --predicate 'process == "Aether"' --style compact 2>&1 | grep -E "AppDelegate|Window|size|minSize|contentSize"
