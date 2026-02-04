#!/bin/bash
# Test file scanning functionality

# Create a test directory to simulate working directory
TEST_DIR="$HOME/.aleph/output/test-scan-$(date +%s)"
mkdir -p "$TEST_DIR"

echo "Test directory created: $TEST_DIR"

# Create test files with timestamps
sleep 1
echo "Test file 1" > "$TEST_DIR/file1.txt"
sleep 0.5
echo "Test file 2" > "$TEST_DIR/file2.md"
sleep 0.5
echo "Test file 3" > "$TEST_DIR/subdir/file3.json"

echo "Test files created:"
find "$TEST_DIR" -type f -ls

echo ""
echo "Test setup complete. You can now test the file scanning functionality."
echo "Test directory: $TEST_DIR"
echo ""
echo "To clean up: rm -rf $TEST_DIR"
