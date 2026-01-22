#!/bin/bash
# Test script for Volcengine/Doubao provider integration
# This script verifies that all Volcengine provider aliases work correctly

set -e  # Exit on error

echo "🧪 Testing Volcengine/Doubao Provider Integration"
echo "=================================================="
echo ""

# Navigate to core directory
cd "$(dirname "$0")/../core"

echo "📦 Running Volcengine-specific unit tests..."
echo ""

# Run the three Volcengine tests
cargo test --lib test_volcengine_default_base_url -- --nocapture
cargo test --lib test_doubao_default_base_url -- --nocapture
cargo test --lib test_ark_default_base_url -- --nocapture

echo ""
echo "✅ All Volcengine provider tests passed!"
echo ""
echo "📋 Summary:"
echo "  - volcengine alias: ✓"
echo "  - doubao alias: ✓"
echo "  - ark alias: ✓"
echo ""
echo "🎉 Volcengine integration is working correctly!"
