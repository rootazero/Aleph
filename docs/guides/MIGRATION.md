# Aleph Memory Storage Migration Guide

**Version**: 1.0
**Last Updated**: 2026-10-18
**Target**: Users migrating from SQLite `raw_memories` to Markdown-based storage

---

## Table of Contents

1. [Overview](#overview)
2. [What's Changing](#whats-changing)
3. [Why Migrate](#why-migrate)
4. [Pre-Migration Checklist](#pre-migration-checklist)
5. [Migration Process](#migration-process)
6. [Post-Migration Verification](#post-migration-verification)
7. [Troubleshooting](#troubleshooting)
8. [Migration Tool Reference](#migration-tool-reference)
9. [Advanced Topics](#advanced-topics)
10. [Rollback Plan](#rollback-plan)

---

## 1. Overview

Aleph is transitioning from SQLite-based raw memory storage to a Markdown-first architecture. This guide walks you through migrating your existing session data to the new format.

### Migration Strategy

- **Manual process**: No automatic migration on upgrade
- **Tool-assisted**: Use `aleph-migrate` CLI for each step
- **Verification-heavy**: Multiple checkpoints to ensure data integrity
- **Rollback-ready**: Old database preserved until you confirm success

### Timeline

- **Small datasets** (< 100 sessions): 15–30 minutes
- **Medium datasets** (100–1000 sessions): 1–2 hours
- **Large datasets** (> 1000 sessions): 3–6 hours

---

## 2. What's Changing

### Before (SQLite)

```
~/.aleph/metadata.db  ← All session data in SQLite
~/.aleph/memory/note/                         ← Knowledge notes (unchanged)
```

**Storage**:
- Session transcripts → `raw_memories.content` TEXT column
- Attachments → scattered in `plugins/`, `output/`, `workspaces/`
- Metadata → SQLite tables

### After (Markdown)

```
~/.aleph/
├── metadata.db                    ← IDs, indexes, search only
├── raw/
│   ├── sessions/
│   │   ├── {session_id}/
│   │   │   ├── transcript.md      ← Human-readable transcript
│   │   │   ├── tool_outputs.md    ← Tool execution logs
│   │   │   └── metadata.json      ← Session metadata
│   └── attachments/
│       ├── images/                ← Content-addressed storage
│       ├── videos/
│       └── documents/
└── archive/                       ← Auto-archived old sessions
```

**Key Changes**:
1. **Content in Markdown**: Transcripts stored as `.md` files
2. **Unified attachments**: All binary data in `raw/attachments/`
3. **Content-addressed**: Attachments deduplicated by SHA256 hash
4. **Git-friendly**: All content is version-controllable
5. **Relocatable**: Entire `~/.aleph` can be moved across machines

---

## 3. Why Migrate

### Benefits

✅ **Version Control**: Git-track your session history
✅ **Portability**: Share data across AI agents (Pi, Claude, etc.)
✅ **Human-Readable**: Open `.md` files in any text editor
✅ **Deduplication**: Attachments stored once, referenced many times
✅ **Future-Proof**: Standard Markdown format outlives any tool

### What You Keep

- All session transcripts (messages, timestamps, roles)
- All tool execution outputs
- All attachments (images, PDFs, videos)
- All metadata (agent ID, workspace, tags, compression status)
- FTS5 search index (rebuilt from Markdown)

### What Changes

- File paths (from SQLite to Markdown directories)
- Storage format (from BLOB to content-addressed files)
- No loss of data or functionality

---

## 4. Pre-Migration Checklist

### 4.1 Backup Your Data

**Critical**: Always backup before migration.

```bash
# 1. Backup entire Aleph directory
cd ~/.aleph
tar -czf ~/aleph-backup-$(date +%Y%m%d).tar.gz .

# 2. Backup old database (if exists)
if [ -f ~/.aleph/metadata.db ]; then
    cp ~/.aleph/metadata.db ~/aleph.db.backup
fi

# 3. Verify backup integrity
tar -tzf ~/aleph-backup-$(date +%Y%m%d).tar.gz | wc -l
```

### 4.2 Check Data Volume

```bash
# Count sessions in old database
sqlite3 ~/.aleph/metadata.db \
  "SELECT COUNT(*) FROM raw_memories WHERE source='session';"

# Estimate migration time
# < 100 sessions: 15–30 min
# 100–1000 sessions: 1–2 hours
# > 1000 sessions: 3–6 hours
```

### 4.3 Install Dependencies

```bash
# Ensure you have the migration tool
aleph-migrate --version

# If not installed, build from source
cd /path/to/Aleph
cargo build --release --bin aleph-migrate
export PATH="$PWD/target/release:$PATH"
```

### 4.4 Disk Space Check

```bash
# Estimate required space (3x current database size)
du -sh ~/.aleph/metadata.db
df -h ~/.aleph  # Ensure sufficient free space
```

---

## 5. Migration Process

### Overview

The migration is a **4-step process**:

1. **Export**: SQLite → JSON (intermediate format)
2. **Transform**: JSON → Markdown sessions + attachments
3. **Index**: Build `metadata.db` from Markdown files
4. **Verify**: Integrity checks + diff comparison

### Step 1: Export from SQLite

```bash
# Export all sessions to JSON
aleph-migrate export \
  --db ~/.aleph/metadata.db \
  --output ~/.aleph/migration/export.json \
  --include-attachments

# Verify export
jq '.sessions | length' ~/.aleph/migration/export.json
```

**Options**:
- `--agent-id <ID>`: Export only specific agent's sessions
- `--start-date <YYYY-MM-DD>`: Export sessions after date
- `--end-date <YYYY-MM-DD>`: Export sessions before date
- `--compression-status <pending|compressed|archived>`: Filter by status

**Output Format** (JSON):

```json
{
  "version": "1.0",
  "exported_at": 1729267535,
  "sessions": [
    {
      "session_id": "sess_001",
      "agent_id": "agent_main",
      "workspace_id": "ws_001",
      "started_at": 1729267535,
      "ended_at": 1729270000,
      "messages": [
        {
          "timestamp": 1729267536,
          "role": "user",
          "content": "Hello, world!"
        },
        {
          "timestamp": 1729267540,
          "role": "assistant",
          "content": "Hi there!"
        }
      ],
      "tool_outputs": [
        {
          "timestamp": 1729267545,
          "tool_name": "bash",
          "args": "ls -la",
          "exit_code": 0,
          "stdout": "...",
          "stderr": "",
          "duration_ms": 123
        }
      ],
      "attachments": [
        {
          "attachment_id": "att_001",
          "mime_type": "image/png",
          "size_bytes": 12345,
          "data_base64": "iVBORw0KG..."
        }
      ],
      "tags": ["important", "project-alpha"],
      "compression_status": "pending"
    }
  ]
}
```

### Step 2: Transform to Markdown

```bash
# Transform JSON to Markdown sessions
aleph-migrate transform \
  --input ~/.aleph/migration/export.json \
  --output ~/.aleph/raw/sessions \
  --attachments-dir ~/.aleph/raw/attachments

# This will create:
# ~/.aleph/raw/sessions/{session_id}/transcript.md
# ~/.aleph/raw/sessions/{session_id}/tool_outputs.md
# ~/.aleph/raw/sessions/{session_id}/metadata.json
# ~/.aleph/raw/attachments/{type}/{hash_prefix}.{ext}
```

**What Happens**:
1. Each session → separate directory
2. Messages → `transcript.md` with timestamps
3. Tool outputs → `tool_outputs.md` with execution details
4. Attachments → content-addressed files (SHA256)
5. Metadata → `metadata.json` (preserves all fields)

**Example Output** (`transcript.md`):

```markdown
# Session sess_001

Started: 2024-10-18T18:05:35Z

## User - 18:05:36

Hello, world!

## Assistant - 18:05:40

Hi there!
```

### Step 3: Index Metadata

```bash
# Build metadata.db from Markdown files
aleph-migrate index \
  --sessions-dir ~/.aleph/raw/sessions \
  --output ~/.aleph/metadata.db

# This will:
# 1. Scan all session directories
# 2. Create metadata.db
# 3. Populate sessions, agents, workspaces tables
# 4. Build FTS5 search index
```

**Index Progress**:

```
[1/4] Scanning session directories... 142 found
[2/4] Creating database schema... OK
[3/4] Indexing sessions... 142/142 (100%)
[4/4] Building FTS5 index... OK

✓ Indexed 142 sessions
✓ Registered 3 agents
✓ Registered 5 workspaces
✓ Indexed 1,247 attachments
✓ FTS5 index: 98,432 tokens
```

### Step 4: Verify Integrity

```bash
# Run comprehensive verification
aleph-migrate verify \
  --sessions-dir ~/.aleph/raw/sessions \
  --metadata-db ~/.aleph/metadata.db \
  --original-db ~/.aleph/metadata.db \
  --fix

# Verification checks:
# 1. Session count matches
# 2. All referenced attachments exist
# 3. Content hashes match
# 4. Metadata consistency
# 5. FTS5 index integrity
```

**Verification Report**:

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Verification Report
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

✓ Session count: 142 (matches original)
✓ Attachment files: 1,247/1,247 present
✓ Content hashes: 142/142 valid
✓ Metadata.json: 142/142 parseable
✓ Database indexes: OK
✓ FTS5 search: OK

Warnings:
  ⚠ 3 sessions missing ended_at timestamp
    → sess_120, sess_135, sess_141

Errors: 0

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Migration Status: SUCCESS
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

**Auto-Fix**:
- `--fix` flag repairs common issues:
  - Regenerates missing `metadata.json`
  - Recomputes content hashes
  - Rebuilds FTS5 index
  - Fixes file permissions

---

## 6. Post-Migration Verification

### 6.1 Manual Spot Checks

```bash
# 1. Check a random session
ls -la ~/.aleph/raw/sessions/sess_042/
cat ~/.aleph/raw/sessions/sess_042/transcript.md

# 2. Verify attachment deduplication
find ~/.aleph/raw/attachments -type f | wc -l
# Should be < original count if duplicates existed

# 3. Test FTS5 search
sqlite3 ~/.aleph/metadata.db \
  "SELECT session_id FROM session_transcripts_fts WHERE content MATCH 'error';"
```

### 6.2 Launch Aleph

```bash
# Start Aleph and verify UI
aleph

# Check:
# 1. Settings → Memory → Sessions (should list all sessions)
# 2. Search works (FTS5 index operational)
# 3. Attachments render correctly
# 4. No error logs
```

### 6.3 Archive Old Database

**Only after confirming everything works**:

```bash
# Move old database to archive
mkdir -p ~/.aleph/archive/pre-migration
mv ~/.aleph/metadata.db \
   ~/.aleph/archive/pre-migration/aleph.db.$(date +%Y%m%d)

# Keep backup for 30 days, then delete
```

---

## 7. Troubleshooting

### Issue: Export fails with "table not found"

**Cause**: Old database schema, missing `raw_memories` table.

**Solution**:

```bash
# Check if table exists
sqlite3 ~/.aleph/metadata.db \
  ".schema raw_memories"

# If missing, database may be empty (no data to migrate)
```

### Issue: Transform fails with "invalid JSON"

**Cause**: Export JSON corrupted or incomplete.

**Solution**:

```bash
# Validate JSON
jq empty ~/.aleph/migration/export.json

# If invalid, re-run export
aleph-migrate export --db ... --output ... --force
```

### Issue: Attachment files missing

**Cause**: Attachments were in old database but files deleted.

**Solution**:

```bash
# Run verification to identify missing files
aleph-migrate verify --fix

# If files unrecoverable, remove references
aleph-migrate verify --fix --remove-orphans
```

### Issue: FTS5 index search returns no results

**Cause**: Index not built or corrupted.

**Solution**:

```bash
# Rebuild FTS5 index
sqlite3 ~/.aleph/metadata.db \
  "DELETE FROM session_transcripts_fts; \
   INSERT INTO session_transcripts_fts(session_id, content) \
   SELECT session_id, content FROM sessions;"

# Or re-run index command
aleph-migrate index --sessions-dir ~/.aleph/raw/sessions \
                     --output ~/.aleph/metadata.db \
                     --rebuild-fts
```

### Issue: Migration is very slow

**Cause**: Large attachments, slow disk I/O.

**Solution**:

```bash
# Use parallel processing
aleph-migrate transform \
  --input ~/.aleph/migration/export.json \
  --output ~/.aleph/raw/sessions \
  --parallel 4  # Use 4 CPU cores

# Or migrate incrementally (see Advanced Topics)
```

---

## 8. Migration Tool Reference

### `aleph-migrate export`

Export sessions from SQLite to JSON.

**Usage**:

```bash
aleph-migrate export [OPTIONS]
```

**Options**:

| Option | Description | Default |
|--------|-------------|---------|
| `--db <PATH>` | Path to old SQLite database | `~/.aleph/metadata.db` |
| `--output <PATH>` | Output JSON file path | `./export.json` |
| `--agent-id <ID>` | Export only this agent's sessions | All agents |
| `--start-date <DATE>` | Export sessions after this date (YYYY-MM-DD) | No filter |
| `--end-date <DATE>` | Export sessions before this date (YYYY-MM-DD) | No filter |
| `--compression-status <STATUS>` | Filter by status (`pending`, `compressed`, `archived`) | All |
| `--include-attachments` | Include attachment binary data (base64) | Enabled |
| `--force` | Overwrite existing output file | Disabled |

**Examples**:

```bash
# Export all sessions
aleph-migrate export --db ~/old.db --output export.json

# Export only main agent's sessions from 2024
aleph-migrate export \
  --agent-id agent_main \
  --start-date 2024-01-01 \
  --end-date 2024-12-31

# Export without attachments (faster, smaller JSON)
aleph-migrate export --no-include-attachments
```

---

### `aleph-migrate transform`

Transform JSON export to Markdown sessions.

**Usage**:

```bash
aleph-migrate transform [OPTIONS]
```

**Options**:

| Option | Description | Default |
|--------|-------------|---------|
| `--input <PATH>` | Input JSON file (from export) | Required |
| `--output <PATH>` | Output sessions directory | `~/.aleph/raw/sessions` |
| `--attachments-dir <PATH>` | Attachments directory | `~/.aleph/raw/attachments` |
| `--parallel <N>` | Number of parallel workers | 1 |
| `--overwrite` | Overwrite existing sessions | Disabled |

**Examples**:

```bash
# Basic transform
aleph-migrate transform --input export.json

# Fast parallel transform
aleph-migrate transform \
  --input export.json \
  --parallel 8

# Custom output paths
aleph-migrate transform \
  --input export.json \
  --output /mnt/storage/sessions \
  --attachments-dir /mnt/storage/attachments
```

---

### `aleph-migrate index`

Build `metadata.db` from Markdown sessions.

**Usage**:

```bash
aleph-migrate index [OPTIONS]
```

**Options**:

| Option | Description | Default |
|--------|-------------|---------|
| `--sessions-dir <PATH>` | Sessions directory to index | `~/.aleph/raw/sessions` |
| `--output <PATH>` | Output database path | `~/.aleph/metadata.db` |
| `--rebuild-fts` | Rebuild FTS5 index even if exists | Disabled |

**Examples**:

```bash
# Basic indexing
aleph-migrate index

# Rebuild FTS5 index
aleph-migrate index --rebuild-fts

# Custom paths
aleph-migrate index \
  --sessions-dir /mnt/storage/sessions \
  --output /mnt/storage/metadata.db
```

---

### `aleph-migrate verify`

Verify migration integrity.

**Usage**:

```bash
aleph-migrate verify [OPTIONS]
```

**Options**:

| Option | Description | Default |
|--------|-------------|---------|
| `--sessions-dir <PATH>` | Sessions directory to verify | `~/.aleph/raw/sessions` |
| `--metadata-db <PATH>` | Metadata database path | `~/.aleph/metadata.db` |
| `--original-db <PATH>` | Original SQLite database (for comparison) | Optional |
| `--fix` | Auto-fix common issues | Disabled |
| `--remove-orphans` | Remove orphaned attachment references | Disabled |

**Examples**:

```bash
# Basic verification
aleph-migrate verify

# Verify with auto-fix
aleph-migrate verify --fix

# Compare against original database
aleph-migrate verify \
  --original-db ~/old.db \
  --fix
```

---

## 9. Advanced Topics

### 9.1 Incremental Migration

For large datasets, migrate in batches:

```bash
# Export by date ranges
aleph-migrate export \
  --db ~/old.db \
  --start-date 2024-01-01 \
  --end-date 2024-03-31 \
  --output q1.json

aleph-migrate export \
  --db ~/old.db \
  --start-date 2024-04-01 \
  --end-date 2024-06-30 \
  --output q2.json

# Transform each batch
aleph-migrate transform --input q1.json
aleph-migrate transform --input q2.json

# Final index (once)
aleph-migrate index
```

### 9.2 Selective Migration

Migrate only specific agents or workspaces:

```bash
# Export agent_main only
aleph-migrate export --agent-id agent_main --output main.json

# Export workspace ws_project_alpha only
aleph-migrate export --workspace-id ws_project_alpha --output alpha.json

# Transform and index
aleph-migrate transform --input main.json
aleph-migrate index
```

### 9.3 Custom Transformation

Edit JSON before transformation:

```bash
# Export
aleph-migrate export --output raw.json

# Edit JSON (e.g., remove sensitive data, fix timestamps)
jq '.sessions |= map(select(.agent_id != "agent_test"))' raw.json > filtered.json

# Transform edited JSON
aleph-migrate transform --input filtered.json
```

### 9.4 Performance Tuning

```bash
# For SSDs (high IOPS)
aleph-migrate transform --parallel 16

# For HDDs (sequential writes)
aleph-migrate transform --parallel 2

# For network storage
aleph-migrate transform --parallel 1 --batch-size 10
```

---

## 10. Rollback Plan

If migration fails or you need to revert:

### Step 1: Restore Old Database

```bash
# Copy backup back
cp ~/aleph.db.backup ~/.aleph/metadata.db
```

### Step 2: Remove New Files

```bash
# Delete Markdown files
rm -rf ~/.aleph/raw/sessions
rm -rf ~/.aleph/raw/attachments
rm ~/.aleph/metadata.db
```

### Step 3: Restart Aleph

```bash
# Launch with old database
aleph
```

### Step 4: Report Issue

If migration failed, please report:

1. Migration command used
2. Error message
3. Verification report (if reached that step)
4. Dataset size (number of sessions, total size)

**Report to**: https://github.com/YourRepo/Aleph/issues

---

## Migration Checklist

Use this checklist to track your progress:

### Before Migration

- [ ] Backup entire `~/.aleph` directory
- [ ] Backup old database (`~/.aleph/metadata.db`)
- [ ] Verify backup integrity
- [ ] Check available disk space (3x current database size)
- [ ] Install `aleph-migrate` tool
- [ ] Estimate migration time based on dataset size

### During Migration

- [ ] Export: SQLite → JSON
- [ ] Verify export JSON is valid
- [ ] Transform: JSON → Markdown
- [ ] Check session directories were created
- [ ] Index: Build `metadata.db`
- [ ] Verify: Run integrity checks
- [ ] Review verification report
- [ ] Fix any reported issues

### After Migration

- [ ] Spot-check random sessions
- [ ] Launch Aleph and verify UI
- [ ] Test FTS5 search
- [ ] Verify attachments render correctly
- [ ] Check error logs (no migration-related errors)
- [ ] Archive old database (keep for 30 days)
- [ ] Delete backup after 30 days (if everything works)

---

## Need Help?

- **Documentation**: [docs/specs/2026-10-18-markdown-storage-design.md](../specs/2026-10-18-markdown-storage-design.md)
- **Issues**: https://github.com/YourRepo/Aleph/issues
- **Discord**: https://discord.gg/aleph-community

---

**Last Updated**: 2026-10-18
**Document Version**: 1.0
**Migration Tool Version**: 0.1.0
