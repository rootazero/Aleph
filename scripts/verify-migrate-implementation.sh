#!/bin/bash
# Verification script for aleph-migrate implementation

set -e

echo "==================================="
echo "Aleph Migrate - Code Verification"
echo "==================================="
echo ""

# Check all source files exist
echo "[1/5] Checking source files..."
FILES=(
    "src/memory/store/session.rs"
    "src/memory/store/markdown/store_impl.rs"
    "src/memory/store/markdown/utils.rs"
    "src/memory/store/sqlite/schema/session_ddl.rs"
    "src/bin/aleph-migrate/main.rs"
    "src/bin/aleph-migrate/export.rs"
    "src/bin/aleph-migrate/transform.rs"
    "src/bin/aleph-migrate/index.rs"
    "src/bin/aleph-migrate/verify.rs"
    "src/bin/aleph-migrate/mod.rs"
)

for file in "${FILES[@]}"; do
    if [ -f "$file" ]; then
        lines=$(wc -l < "$file")
        echo "  ✓ $file ($lines lines)"
    else
        echo "  ✗ $file (missing)"
        exit 1
    fi
done

# Check documentation
echo ""
echo "[2/5] Checking documentation..."
DOCS=(
    "docs/specs/2026-10-18-markdown-storage-design.md"
    "docs/specs/2026-10-18-panel-ui-memory-adaptation.md"
    "docs/guides/MIGRATION.md"
    "docs/investigation/2026-10-18-storage-architecture-survey.md"
)

for doc in "${DOCS[@]}"; do
    if [ -f "$doc" ]; then
        size=$(wc -c < "$doc" | awk '{printf "%.1fKB", $1/1024}')
        echo "  ✓ $doc ($size)"
    else
        echo "  ✗ $doc (missing)"
        exit 1
    fi
done

# Check Cargo.toml configuration
echo ""
echo "[3/5] Checking Cargo.toml..."
if grep -q "name = \"aleph-migrate\"" Cargo.toml; then
    echo "  ✓ Binary declaration found"
else
    echo "  ✗ Binary declaration missing"
    exit 1
fi

if grep -q "rayon = \"1.10\"" Cargo.toml; then
    echo "  ✓ rayon dependency found"
else
    echo "  ✗ rayon dependency missing"
    exit 1
fi

if grep -q "env_logger" Cargo.toml; then
    echo "  ✓ env_logger dependency found"
else
    echo "  ✗ env_logger dependency missing"
    exit 1
fi

# Check for common compilation issues
echo ""
echo "[4/5] Checking for known compilation issues..."
if grep -r "AlephError::io\|AlephError::database\|AlephError::serialization" src/memory/store/markdown/ src/bin/aleph-migrate/ 2>/dev/null; then
    echo "  ✗ Found deprecated error constructors"
    exit 1
else
    echo "  ✓ No deprecated error constructors"
fi

if grep -r 'format!("{:x}"' src/memory/store/markdown/ src/bin/aleph-migrate/ 2>/dev/null; then
    echo "  ✗ Found SHA256 formatting issues"
    exit 1
else
    echo "  ✓ No SHA256 formatting issues"
fi

# Summary
echo ""
echo "[5/5] Summary"
echo "  Total source files: ${#FILES[@]}"
echo "  Total documentation: ${#DOCS[@]}"
echo "  All checks passed: ✓"
echo ""
echo "==================================="
echo "Code verification complete!"
echo "==================================="
echo ""
echo "Next steps:"
echo "  1. Run: cargo build --bin aleph-migrate"
echo "  2. Test: target/debug/aleph-migrate --help"
echo "  3. Migrate: Follow docs/guides/MIGRATION.md"
