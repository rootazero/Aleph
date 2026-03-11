#!/bin/bash
# POE Phase 2+3 Probe Test Suite
#
# Usage:
#   1. Build: just build (or cargo build --release --bin aleph)
#   2. Start server: /tmp/start-aleph.sh  (or with RUST_LOG=info)
#   3. Run: bash scripts/poe_probe_test.sh
#   4. Analyze: bash scripts/poe_probe_test.sh analyze
#
# Each test targets specific Phase 2+3 features via the probe system.

WS_URL="ws://127.0.0.1:18790/ws"
LOG_FILE="${ALEPH_LOG_FILE:-/tmp/aleph-test.log}"

send_rpc() {
    local payload="$1"
    local timeout="${2:-30}"
    echo "$payload" | websocat -1 --ping-interval 5 "$WS_URL" 2>/dev/null
}

wait_for_completion() {
    local task_id="$1"
    local max_wait="${2:-120}"
    local elapsed=0

    while [ $elapsed -lt $max_wait ]; do
        local status_json=$(send_rpc "{\"jsonrpc\":\"2.0\",\"method\":\"poe.status\",\"params\":{\"task_id\":\"$task_id\"},\"id\":99}")
        local status=$(echo "$status_json" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('result',{}).get('status','unknown'))" 2>/dev/null)

        if [ "$status" = "success" ] || [ "$status" = "failed" ] || [ "$status" = "cancelled" ]; then
            echo "$status_json" | python3 -m json.tool 2>/dev/null
            return 0
        fi

        echo "  ⏳ Status: $status (${elapsed}s elapsed)"
        sleep 5
        elapsed=$((elapsed + 5))
    done

    echo "  ⚠️ Timeout after ${max_wait}s"
    return 1
}

# ============================================================================
# Test 1: Simple Success (Baseline — validates full P→O→E cycle + probes)
# ============================================================================
test_1_simple_success() {
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "TEST 1: Simple Success (P→O→E baseline)"
    echo "Expected probes: VALIDATE, ENTROPY (1 attempt, distance=0)"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    local result=$(send_rpc '{
        "jsonrpc":"2.0",
        "method":"poe.run",
        "params":{
            "manifest":{
                "task_id":"probe-t1-simple",
                "objective":"Create a simple text file",
                "hard_constraints":[
                    {"type":"FileExists","params":{"path":"/tmp/poe-workspace/probe-t1.txt"}}
                ],
                "soft_metrics":[],
                "max_attempts":3
            },
            "instruction":"Create a file at /tmp/poe-workspace/probe-t1.txt containing the text: probe test 1 success",
            "stream":false
        },
        "id":1
    }')
    echo "  Submitted: $(echo $result | python3 -c "import sys,json; print(json.load(sys.stdin).get('result',{}).get('task_id','ERROR'))" 2>/dev/null)"

    wait_for_completion "probe-t1-simple" 60
    echo ""
}

# ============================================================================
# Test 2: Blast Radius — Dangerous Command (should be REJECTED)
# ============================================================================
test_2_blast_radius_reject() {
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "TEST 2: Blast Radius — Dangerous Command Rejection"
    echo "Expected probes: BLAST_RADIUS → REJECTED (rm -rf /)"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    local result=$(send_rpc '{
        "jsonrpc":"2.0",
        "method":"poe.run",
        "params":{
            "manifest":{
                "task_id":"probe-t2-danger",
                "objective":"Dangerous operation test",
                "hard_constraints":[
                    {"type":"CommandPasses","params":{"cmd":"rm","args":["-rf","/"],"timeout_ms":5000}}
                ],
                "soft_metrics":[],
                "max_attempts":1
            },
            "instruction":"This should be rejected by blast radius",
            "stream":false
        },
        "id":2
    }')
    echo "  Result: $result" | python3 -m json.tool 2>/dev/null || echo "  $result"

    sleep 2
    # Check if task was rejected (should not be running)
    send_rpc '{"jsonrpc":"2.0","method":"poe.status","params":{"task_id":"probe-t2-danger"},"id":22}' | python3 -m json.tool 2>/dev/null
    echo ""
}

# ============================================================================
# Test 3: Blast Radius — Safe Command (should PASS assessment)
# ============================================================================
test_3_blast_radius_safe() {
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "TEST 3: Blast Radius — Safe Command (Low risk)"
    echo "Expected probes: BLAST_RADIUS → Low, then VALIDATE, ENTROPY"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    local result=$(send_rpc '{
        "jsonrpc":"2.0",
        "method":"poe.run",
        "params":{
            "manifest":{
                "task_id":"probe-t3-safe",
                "objective":"Run a safe command",
                "hard_constraints":[
                    {"type":"CommandPasses","params":{"cmd":"echo","args":["hello"],"timeout_ms":5000}}
                ],
                "soft_metrics":[],
                "max_attempts":3
            },
            "instruction":"Run: echo hello. This is a safe operation.",
            "stream":false
        },
        "id":3
    }')
    echo "  Submitted: $(echo $result | python3 -c "import sys,json; print(json.load(sys.stdin).get('result',{}).get('task_id','ERROR'))" 2>/dev/null)"

    wait_for_completion "probe-t3-safe" 60
    echo ""
}

# ============================================================================
# Test 4: Entropy Degradation + Stuck Detection
# (Impossible constraint → multiple failures → stuck detection)
# ============================================================================
test_4_stuck_detection() {
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "TEST 4: Entropy Degradation + Stuck Detection"
    echo "Expected probes: ENTROPY ×3 (flat), TABOO tags, STUCK detected"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    local result=$(send_rpc '{
        "jsonrpc":"2.0",
        "method":"poe.run",
        "params":{
            "manifest":{
                "task_id":"probe-t4-stuck",
                "objective":"Trigger stuck detection",
                "hard_constraints":[
                    {"type":"FileContains","params":{"path":"/nonexistent/impossible/file.txt","pattern":"impossible_pattern_xyz"}}
                ],
                "soft_metrics":[],
                "max_attempts":5
            },
            "instruction":"Make the file /nonexistent/impossible/file.txt contain the string impossible_pattern_xyz. The directory does not exist and cannot be created.",
            "stream":false,
            "config":{"stuck_window":3}
        },
        "id":4
    }')
    echo "  Submitted: $(echo $result | python3 -c "import sys,json; print(json.load(sys.stdin).get('result',{}).get('task_id','ERROR'))" 2>/dev/null)"

    wait_for_completion "probe-t4-stuck" 300
    echo ""
}

# ============================================================================
# Test 5: Taboo Micro-Taboo Detection
# (Same failure pattern repeated → TABOO WARNING injected)
# ============================================================================
test_5_taboo_detection() {
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "TEST 5: Taboo Micro-Taboo Detection"
    echo "Expected probes: TABOO buffer records, TABOO micro triggered"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    local result=$(send_rpc '{
        "jsonrpc":"2.0",
        "method":"poe.run",
        "params":{
            "manifest":{
                "task_id":"probe-t5-taboo",
                "objective":"Trigger taboo detection via repeated same failures",
                "hard_constraints":[
                    {"type":"FileExists","params":{"path":"/root/protected_file_that_cannot_exist.txt"}}
                ],
                "soft_metrics":[],
                "max_attempts":5
            },
            "instruction":"Create the file /root/protected_file_that_cannot_exist.txt. You will repeatedly fail with permission denied, which should trigger taboo detection.",
            "stream":false,
            "config":{"stuck_window":4}
        },
        "id":5
    }')
    echo "  Submitted: $(echo $result | python3 -c "import sys,json; print(json.load(sys.stdin).get('result',{}).get('task_id','ERROR'))" 2>/dev/null)"

    wait_for_completion "probe-t5-taboo" 300
    echo ""
}

# ============================================================================
# Test 6: Mixed Pass/Fail (E-stage decomposition detection)
# ============================================================================
test_6_e_stage_decomposition() {
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "TEST 6: E-stage Decomposition Detection (mixed pass/fail)"
    echo "Expected probes: E-DECOMP DETECTED (pass some + fail others)"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    # Create a file that will satisfy one constraint but not the other
    mkdir -p /tmp/poe-workspace
    echo "existing content" > /tmp/poe-workspace/probe-t6-exists.txt

    local result=$(send_rpc '{
        "jsonrpc":"2.0",
        "method":"poe.run",
        "params":{
            "manifest":{
                "task_id":"probe-t6-decomp",
                "objective":"Mixed constraints - some pass, some fail",
                "hard_constraints":[
                    {"type":"FileExists","params":{"path":"/tmp/poe-workspace/probe-t6-exists.txt"}},
                    {"type":"FileExists","params":{"path":"/root/impossible_probe_t6.txt"}},
                    {"type":"FileContains","params":{"path":"/tmp/poe-workspace/probe-t6-exists.txt","pattern":"existing content"}}
                ],
                "soft_metrics":[],
                "max_attempts":5
            },
            "instruction":"One file already exists with correct content, but /root/impossible_probe_t6.txt cannot be created (permission denied). This tests mixed pass/fail patterns.",
            "stream":false,
            "config":{"stuck_window":4}
        },
        "id":6
    }')
    echo "  Submitted: $(echo $result | python3 -c "import sys,json; print(json.load(sys.stdin).get('result',{}).get('task_id','ERROR'))" 2>/dev/null)"

    wait_for_completion "probe-t6-decomp" 300
    echo ""
}

# ============================================================================
# Test 7: Blast Radius — Critical (force push)
# ============================================================================
test_7_blast_radius_critical() {
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "TEST 7: Blast Radius — Critical (git push --force)"
    echo "Expected probes: BLAST_RADIUS → Critical (MandatorySignature)"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    local result=$(send_rpc '{
        "jsonrpc":"2.0",
        "method":"poe.run",
        "params":{
            "manifest":{
                "task_id":"probe-t7-critical",
                "objective":"Force push (critical blast radius)",
                "hard_constraints":[
                    {"type":"CommandPasses","params":{"cmd":"git","args":["push","--force","origin","main"],"timeout_ms":5000}}
                ],
                "soft_metrics":[],
                "max_attempts":1
            },
            "instruction":"This tests critical blast radius detection",
            "stream":false
        },
        "id":7
    }')
    echo "  Result: $(echo $result | python3 -m json.tool 2>/dev/null || echo $result)"

    sleep 2
    send_rpc '{"jsonrpc":"2.0","method":"poe.status","params":{"task_id":"probe-t7-critical"},"id":77}' | python3 -m json.tool 2>/dev/null
    echo ""
}

# ============================================================================
# Test 8: Multi-constraint Success (verifies validation with multiple rules)
# ============================================================================
test_8_multi_constraint() {
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "TEST 8: Multi-constraint Success"
    echo "Expected probes: VALIDATE hard=3 soft=0, BLAST_RADIUS, ENTROPY"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    local result=$(send_rpc '{
        "jsonrpc":"2.0",
        "method":"poe.run",
        "params":{
            "manifest":{
                "task_id":"probe-t8-multi",
                "objective":"Create multiple files with specific content",
                "hard_constraints":[
                    {"type":"FileExists","params":{"path":"/tmp/poe-workspace/probe-t8-a.txt"}},
                    {"type":"FileExists","params":{"path":"/tmp/poe-workspace/probe-t8-b.txt"}},
                    {"type":"FileContains","params":{"path":"/tmp/poe-workspace/probe-t8-a.txt","pattern":"alpha"}}
                ],
                "soft_metrics":[],
                "max_attempts":3
            },
            "instruction":"Create two files: /tmp/poe-workspace/probe-t8-a.txt containing \"alpha\" and /tmp/poe-workspace/probe-t8-b.txt containing \"beta\"",
            "stream":false
        },
        "id":8
    }')
    echo "  Submitted: $(echo $result | python3 -c "import sys,json; print(json.load(sys.stdin).get('result',{}).get('task_id','ERROR'))" 2>/dev/null)"

    wait_for_completion "probe-t8-multi" 90
    echo ""
}

# ============================================================================
# Analyze: Extract probe data from logs
# ============================================================================
analyze_probes() {
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "PROBE ANALYSIS — Extracting Phase 2+3 feature activation"
    echo "Log file: $LOG_FILE"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""

    echo "=== 💥 BLAST RADIUS ==="
    /usr/bin/grep "BLAST_RADIUS" "$LOG_FILE" 2>/dev/null | tail -20 || echo "  (none)"
    echo ""

    echo "=== 📊 ENTROPY TRACKING ==="
    /usr/bin/grep "ENTROPY" "$LOG_FILE" 2>/dev/null | tail -20 || echo "  (none)"
    echo ""

    echo "=== 🛑 STUCK DETECTION ==="
    /usr/bin/grep "STUCK" "$LOG_FILE" 2>/dev/null | tail -10 || echo "  (none)"
    echo ""

    echo "=== 🏷️ TABOO ==="
    /usr/bin/grep "TABOO" "$LOG_FILE" 2>/dev/null | tail -20 || echo "  (none)"
    echo ""

    echo "=== ⚖️ VALIDATION ==="
    /usr/bin/grep "VALIDATE" "$LOG_FILE" 2>/dev/null | tail -20 || echo "  (none)"
    echo ""

    echo "=== 🔍 DECOMPOSITION ==="
    /usr/bin/grep "DECOMP" "$LOG_FILE" 2>/dev/null | tail -10 || echo "  (none)"
    echo ""

    echo "=== 🧠 META-COGNITION ==="
    /usr/bin/grep "META_COGNITION" "$LOG_FILE" 2>/dev/null | tail -10 || echo "  (none)"
    echo ""

    echo "=== 🧠 MEMORY DECAY ==="
    /usr/bin/grep "DECAY" "$LOG_FILE" 2>/dev/null | tail -10 || echo "  (none)"
    echo ""

    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "FEATURE ACTIVATION SUMMARY"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    local blast=$(/usr/bin/grep -c "BLAST_RADIUS" "$LOG_FILE" 2>/dev/null || echo 0)
    local entropy=$(/usr/bin/grep -c "ENTROPY" "$LOG_FILE" 2>/dev/null || echo 0)
    local stuck=$(/usr/bin/grep -c "STUCK detected" "$LOG_FILE" 2>/dev/null || echo 0)
    local taboo_buf=$(/usr/bin/grep -c "TABOO buffer" "$LOG_FILE" 2>/dev/null || echo 0)
    local taboo_micro=$(/usr/bin/grep -c "TABOO micro-taboo" "$LOG_FILE" 2>/dev/null || echo 0)
    local validate=$(/usr/bin/grep -c "VALIDATE" "$LOG_FILE" 2>/dev/null || echo 0)
    local decomp_e=$(/usr/bin/grep -c "E-DECOMP DETECTED" "$LOG_FILE" 2>/dev/null || echo 0)
    local decomp_e_skip=$(/usr/bin/grep -c "E-DECOMP skipped" "$LOG_FILE" 2>/dev/null || echo 0)
    local meta=$(/usr/bin/grep -c "META_COGNITION" "$LOG_FILE" 2>/dev/null || echo 0)
    local decay=$(/usr/bin/grep -c "DECAY" "$LOG_FILE" 2>/dev/null || echo 0)

    printf "  %-30s %s\n" "💥 Blast Radius assessments:" "$blast"
    printf "  %-30s %s\n" "📊 Entropy observations:" "$entropy"
    printf "  %-30s %s\n" "🛑 Stuck detections:" "$stuck"
    printf "  %-30s %s\n" "🏷️ Taboo buffer records:" "$taboo_buf"
    printf "  %-30s %s\n" "🔴 Taboo micro triggers:" "$taboo_micro"
    printf "  %-30s %s\n" "⚖️ Validation calls:" "$validate"
    printf "  %-30s %s\n" "🔍 E-decomp detected:" "$decomp_e"
    printf "  %-30s %s\n" "🔍 E-decomp skipped:" "$decomp_e_skip"
    printf "  %-30s %s\n" "🧠 Meta-cognition calls:" "$meta"
    printf "  %-30s %s\n" "🧠 Memory decay events:" "$decay"
}

# ============================================================================
# Main
# ============================================================================

if [ "$1" = "analyze" ]; then
    analyze_probes
    exit 0
fi

echo ""
echo "╔══════════════════════════════════════════════════════╗"
echo "║   POE Phase 2+3 Probe Test Suite                    ║"
echo "║   8 tests × 7 feature categories                   ║"
echo "╚══════════════════════════════════════════════════════╝"
echo ""
echo "WebSocket: $WS_URL"
echo "Log file:  $LOG_FILE"
echo ""

# Run specific test or all
case "${1:-all}" in
    1) test_1_simple_success ;;
    2) test_2_blast_radius_reject ;;
    3) test_3_blast_radius_safe ;;
    4) test_4_stuck_detection ;;
    5) test_5_taboo_detection ;;
    6) test_6_e_stage_decomposition ;;
    7) test_7_blast_radius_critical ;;
    8) test_8_multi_constraint ;;
    all)
        test_1_simple_success
        test_2_blast_radius_reject
        test_3_blast_radius_safe
        test_7_blast_radius_critical
        test_8_multi_constraint
        echo "⚠️  Tests 4-6 are long-running (multi-attempt). Run individually:"
        echo "  bash scripts/poe_probe_test.sh 4   # Stuck detection (~2-5min)"
        echo "  bash scripts/poe_probe_test.sh 5   # Taboo detection (~2-5min)"
        echo "  bash scripts/poe_probe_test.sh 6   # E-decomposition (~2-5min)"
        ;;
    *)
        echo "Usage: $0 [1-8|all|analyze]"
        exit 1
        ;;
esac

echo ""
echo "Done! Run 'bash scripts/poe_probe_test.sh analyze' to extract probe data."
