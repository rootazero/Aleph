# Aleph Raw Memory Migration Guide

> **Version**: 1.0
> **Date**: 2026-10-18
> **Target Audience**: Aleph users upgrading from SQLite raw_memories to Markdown storage

---

## 1. Overview

### 1.1 What's Changing

**Before** (Legacy):
- Raw memories stored in SQLite `raw_memories` table as TEXT
- Attachments as BLOBs in database
- Location: `~/Library/Application Support/aleph/aleph.db` (macOS) or equivalent

**After** (New):
- Sessions stored as Markdown files in `~/.aleph/raw/sessions/`
- Attachments as content-addressed files in `~/.aleph/raw/attachments/`
- SQLite only for IDs/indexes: `~/.aleph/metadata.db`

### 1.2 Why Migrate

✅ **Git-friendly**: Text diffs, version control
✅ **Cross-agent compatible**: Standard Markdown format
✅ **Human-readable**: Direct access with any text editor
✅ **Portable**: Entire `~/.aleph` relocatable
✅ **Deduplication**: Content-addressed attachments save space

### 1.3 Migration Strategy

**No Automatic Migration** — manual process:
1. Export existing data from SQLite
2. Transform to Markdown format
3. Import into new structure
4. Verify integrity
5. Archive old database

**Estimated Time**: 30 minutes to 2 hours (depends on data volume)

---

## 2. Pre-Migration Checklist

### 2.1 Backup Current Data

```bash
# 1. Stop Aleph
killall aleph 2>/dev/null || true

# 2. Backup existing database
mkdir -p ~/aleph-backup-$(date +%Y%m%d)
cp -r ~/Library/Application\ Support/aleph/*.db ~/aleph-backup-$(date +%Y%m%d)/

# 3. Backup existing memory notes
cp -r ~/.aleph/memory ~/aleph-backup-$(date +%Y%m%d)/

# 4. Verify backup
ls -lh ~/aleph-backup-$(date +%Y%m%d)/
```

### 2.2 Check Data Volume

```bash
# Check database size
du -h ~/Library/Application\ Support/aleph/aleph.db

# Count raw memories
sqlite3 ~/Library/Application\ Support/aleph/aleph.db \
  "SELECT COUNT(*) FROM raw_memories;"

# Count attachments (if stored as BLOBs)
sqlite3 ~/Library/Application\ Support/aleph/aleph.db \
  "SELECT COUNT(*) FROM attachments WHERE data IS NOT NULL;"
```

### 2.3 Install Dependencies

```bash
# Install migration tool (will be provided)
cargo install --path tools/aleph-migrate

# Or build from source
cd /path/to/aleph
cargo build --release --bin aleph-migrate
```

---

## 3. Migration Process

### 3.1 Export from SQLite

**Step 1: Export raw memories to JSON**

```bash
aleph-migrate export \
  --db ~/Library/Application\ Support/aleph/aleph.db \
  --output ~/aleph-export-$(date +%Y%m%d).json \
  --format json
```

**Export format** (`aleph-export-YYYYMMDD.json`):

```json
{
  "version": 1,
  "exported_at": "2026-10-18T18:00:00Z",
  "sessions": [
    {
      "session_id": "sess_2026-10-18_abc123",
      "agent_id": "main",
      "workspace_id": "ws_default",
      "started_at": 1729267535,
      "ended_at": 1729271792,
      "messages": [
        {
          "timestamp": 1729267535,
          "role": "user",
          "content": "帮我实现一个函数"
        },
        {
          "timestamp": 1729267542,
          "role": "assistant",
          "content": "好的，我来实现..."
        }
      ],
      "tool_outputs": [
        {
          "timestamp": 1729267550,
          "tool_name": "bash",
          "args": "ls -la",
          "stdout": "total 0\n",
          "stderr": "",
          "exit_code": 0
        }
      ],
      "attachments": [
        {
          "attachment_id": "att_xyz789",
          "data_base64": "iVBORw0KGgoAAAANSUhEUg...",
          "mime_type": "image/png",
          "original_name": "screenshot.png"
        }
      ],
      "tags": ["rust", "function"]
    }
  ]
}
```

**Step 2: Verify export**

```bash
# Check export file size
du -h ~/aleph-export-*.json

# Validate JSON structure
jq '.sessions | length' ~/aleph-export-*.json

# Check first session
jq '.sessions[0] | {session_id, agent_id, message_count: (.messages | length)}' \
  ~/aleph-export-*.json
```

### 3.2 Transform to Markdown

**Step 3: Convert JSON to Markdown sessions**

```bash
aleph-migrate transform \
  --input ~/aleph-export-$(date +%Y%m%d).json \
  --output ~/.aleph/raw/sessions/ \
  --attachments ~/.aleph/raw/attachments/
```

**What happens**:
1. Creates `~/.aleph/raw/sessions/{session_id}/` for each session
2. Generates `transcript.md` from messages
3. Generates `tool_outputs.md` from tool calls
4. Extracts attachments to `~/.aleph/raw/attachments/{type}/{hash}.{ext}`
5. Creates `metadata.json` for each session

**Example output**:

```
~/.aleph/raw/sessions/sess_2026-10-18_abc123/
├── transcript.md
├── tool_outputs.md
└── metadata.json

~/.aleph/raw/attachments/images/
└── a3f5b2c8d1e4f7a9.png
```

### 3.3 Import into SQLite Metadata

**Step 4: Build metadata index**

```bash
aleph-migrate index \
  --base-dir ~/.aleph \
  --db ~/.aleph/metadata.db
```

**What happens**:
1. Creates `metadata.db` with schema
2. Scans `raw/sessions/` for all sessions
3. Inserts metadata into `sessions` table
4. Inserts attachment info into `attachments` table
5. Builds FTS5 index for search

**Verify**:

```bash
# Check metadata.db was created
ls -lh ~/.aleph/metadata.db

# Count sessions
sqlite3 ~/.aleph/metadata.db \
  "SELECT COUNT(*) FROM sessions;"

# Check FTS5 index
sqlite3 ~/.aleph/metadata.db \
  "SELECT COUNT(*) FROM session_transcripts_fts;"
```

### 3.4 Verify Migration

**Step 5: Integrity checks**

```bash
aleph-migrate verify \
  --base-dir ~/.aleph \
  --db ~/.aleph/metadata.db \
  --report ~/migration-report.txt
```

**Checks performed**:
- ✅ All sessions have `transcript.md` and `tool_outputs.md`
- ✅ All attachments referenced in metadata exist on disk
- ✅ Content hashes match file contents
- ✅ No orphaned files
- ✅ SQLite row count matches file count

**Sample report** (`~/migration-report.txt`):

```
=== Aleph Migration Verification Report ===
Date: 2026-10-18 18:30:45

✅ Sessions: 127 (all valid)
✅ Attachments: 45 (all present, 0 orphaned)
✅ Content hashes: 45/45 match
✅ FTS5 index: 127 sessions indexed

Total disk usage:
  - Sessions (Markdown): 12.3 MB
  - Attachments: 8.7 MB
  - Metadata DB: 2.1 MB
  - Total: 23.1 MB

Old database size: 25.8 MB
Space saved: 2.7 MB (10.5% reduction via deduplication)

🎉 Migration successful! No errors found.
```

---

## 4. Post-Migration

### 4.1 Test with Aleph

```bash
# Start Aleph with new storage
aleph --config ~/.aleph/config.toml

# Test basic operations
aleph-cli memory list-sessions --limit 5
aleph-cli memory search "rust function"
aleph-cli memory get-session sess_2026-10-18_abc123
```

### 4.2 Verify Panel UI

1. Open Aleph Panel: `http://localhost:5173`
2. Navigate to **Settings → Memory → Sessions**
3. Verify session list displays
4. Click a session and verify transcript renders
5. Check attachments preview

### 4.3 Archive Old Database

**Only after confirming everything works**:

```bash
# Move old database to archive
mkdir -p ~/.aleph/archive/legacy
mv ~/Library/Application\ Support/aleph/aleph.db \
   ~/.aleph/archive/legacy/aleph-legacy-$(date +%Y%m%d).db

# Keep for 30 days, then delete
```

---

## 5. Troubleshooting

### 5.1 Export Fails

**Error**: `raw_memories table does not exist`

**Cause**: Database is already using new storage (no migration needed)

**Solution**: Skip migration, you're already on the new system.

---

**Error**: `database is locked`

**Cause**: Aleph is still running

**Solution**:
```bash
killall aleph
lsof | grep aleph.db  # verify no process holding it
```

---

### 5.2 Transform Fails

**Error**: `failed to create directory: Permission denied`

**Cause**: Insufficient permissions on `~/.aleph/`

**Solution**:
```bash
mkdir -p ~/.aleph/raw/{sessions,attachments}
chmod -R u+w ~/.aleph/
```

---

**Error**: `invalid UTF-8 in content`

**Cause**: Binary data stored in TEXT column

**Solution**:
```bash
# Re-export with binary-safe encoding
aleph-migrate export \
  --db ~/Library/Application\ Support/aleph/aleph.db \
  --output ~/aleph-export.json \
  --encoding base64
```

---

### 5.3 Verification Fails

**Error**: `content hash mismatch for attachment att_xyz789`

**Cause**: File corrupted during copy or deduplication bug

**Solution**:
```bash
# Re-transform that specific session
aleph-migrate transform \
  --input ~/aleph-export.json \
  --output ~/.aleph/raw/sessions/ \
  --session sess_2026-10-18_abc123 \
  --force
```

---

**Error**: `FTS5 index out of sync`

**Cause**: Index not updated after manual file edits

**Solution**:
```bash
# Rebuild FTS5 index
aleph-migrate reindex \
  --base-dir ~/.aleph \
  --db ~/.aleph/metadata.db
```

---

### 5.4 Common Questions

**Q: Can I edit Markdown files directly?**

A: Yes, but you must update `content_hash` in SQLite or rebuild index:
```bash
# After editing transcript.md
aleph-migrate update-hash \
  --session sess_2026-10-18_abc123 \
  --base-dir ~/.aleph
```

---

**Q: How do I restore from backup?**

A: Copy backup files back and restart Aleph:
```bash
cp -r ~/aleph-backup-YYYYMMDD/* ~/Library/Application\ Support/aleph/
rm -rf ~/.aleph/raw/  # remove new structure
# Restart Aleph (will use old database)
```

---

**Q: Can I run both old and new versions simultaneously?**

A: **No**. The new version does not read from `raw_memories` table. Migrate first.

---

**Q: What if migration takes too long?**

A: Migrate in batches:
```bash
# Export first 100 sessions
aleph-migrate export \
  --db ~/Library/Application\ Support/aleph/aleph.db \
  --output ~/export-batch1.json \
  --limit 100 \
  --offset 0

# Transform batch 1
aleph-migrate transform --input ~/export-batch1.json --output ~/.aleph/raw/sessions/

# Repeat for remaining batches
```

---

## 6. Migration Tool Reference

### 6.1 `aleph-migrate export`

**Usage**:
```bash
aleph-migrate export [OPTIONS]
```

**Options**:
- `--db <PATH>` — Path to legacy aleph.db (required)
- `--output <PATH>` — Output JSON file path (required)
- `--format <FORMAT>` — Output format: `json` (default)
- `--encoding <ENCODING>` — Binary data encoding: `base64` (default), `hex`
- `--limit <N>` — Limit number of sessions (optional)
- `--offset <N>` — Skip first N sessions (optional)
- `--agent <ID>` — Filter by agent_id (optional)
- `--start-date <DATE>` — Filter by start date (ISO 8601, optional)
- `--end-date <DATE>` — Filter by end date (ISO 8601, optional)

**Example**:
```bash
# Export all sessions for 'main' agent from 2026-10-01 onwards
aleph-migrate export \
  --db ~/Library/Application\ Support/aleph/aleph.db \
  --output ~/export-main-oct.json \
  --agent main \
  --start-date 2026-10-01
```

---

### 6.2 `aleph-migrate transform`

**Usage**:
```bash
aleph-migrate transform [OPTIONS]
```

**Options**:
- `--input <PATH>` — Input JSON export file (required)
- `--output <PATH>` — Output sessions directory (required)
- `--attachments <PATH>` — Output attachments directory (required)
- `--session <ID>` — Process only this session (optional)
- `--force` — Overwrite existing files (optional)
- `--dry-run` — Show what would be created without writing (optional)

**Example**:
```bash
# Dry-run to preview
aleph-migrate transform \
  --input ~/export.json \
  --output ~/.aleph/raw/sessions/ \
  --attachments ~/.aleph/raw/attachments/ \
  --dry-run

# Actually write files
aleph-migrate transform \
  --input ~/export.json \
  --output ~/.aleph/raw/sessions/ \
  --attachments ~/.aleph/raw/attachments/
```

---

### 6.3 `aleph-migrate index`

**Usage**:
```bash
aleph-migrate index [OPTIONS]
```

**Options**:
- `--base-dir <PATH>` — Base directory (`~/.aleph`, required)
- `--db <PATH>` — Metadata database path (default: `{base-dir}/metadata.db`)
- `--force` — Drop existing tables and rebuild (optional)

**Example**:
```bash
# Initial index
aleph-migrate index --base-dir ~/.aleph

# Rebuild from scratch
aleph-migrate index --base-dir ~/.aleph --force
```

---

### 6.4 `aleph-migrate verify`

**Usage**:
```bash
aleph-migrate verify [OPTIONS]
```

**Options**:
- `--base-dir <PATH>` — Base directory (required)
- `--db <PATH>` — Metadata database path (optional)
- `--report <PATH>` — Write report to file (optional)
- `--fix` — Attempt to auto-fix issues (optional)

**Example**:
```bash
# Verify and generate report
aleph-migrate verify \
  --base-dir ~/.aleph \
  --report ~/migration-report.txt

# Verify and auto-fix
aleph-migrate verify \
  --base-dir ~/.aleph \
  --fix
```

---

## 7. Advanced Topics

### 7.1 Incremental Migration

If you have ongoing work during migration, use incremental approach:

**Day 1**: Export current state
```bash
aleph-migrate export --db ~/Library/Application\ Support/aleph/aleph.db \
  --output ~/export-day1.json
```

**Day 2**: Export delta (new sessions only)
```bash
aleph-migrate export --db ~/Library/Application\ Support/aleph/aleph.db \
  --output ~/export-day2.json \
  --start-date 2026-10-19  # day 2 onwards
```

**Merge exports**:
```bash
jq -s '.[0] * .[1] | .sessions = ([.[0].sessions, .[1].sessions] | add)' \
  ~/export-day1.json ~/export-day2.json > ~/export-merged.json
```

**Transform merged**:
```bash
aleph-migrate transform --input ~/export-merged.json \
  --output ~/.aleph/raw/sessions/ \
  --attachments ~/.aleph/raw/attachments/
```

---

### 7.2 Selective Migration

Migrate only specific agents or date ranges:

```bash
# Export only 'main' agent
aleph-migrate export --db ~/Library/Application\ Support/aleph/aleph.db \
  --output ~/export-main.json \
  --agent main

# Export only last 30 days
aleph-migrate export --db ~/Library/Application\ Support/aleph/aleph.db \
  --output ~/export-recent.json \
  --start-date $(date -d '30 days ago' +%Y-%m-%d)
```

---

### 7.3 Custom Transformation

If you need custom Markdown formatting:

1. Export to JSON
2. Write custom script (Python/Node.js/Rust)
3. Generate Markdown files with your template
4. Run `aleph-migrate index` to build metadata

**Example Python script** (`custom-transform.py`):

```python
import json
from pathlib import Path
import hashlib
import base64

def transform_session(session, output_dir, attachments_dir):
    session_id = session['session_id']
    session_path = output_dir / session_id
    session_path.mkdir(parents=True, exist_ok=True)

    # Custom Markdown template
    transcript = f"""---
session_id: {session_id}
agent_id: {session['agent_id']}
---

# My Custom Session Format

"""

    for msg in session['messages']:
        role = msg['role'].capitalize()
        transcript += f"## {role} ({msg['timestamp']})\n\n{msg['content']}\n\n"

    (session_path / 'transcript.md').write_text(transcript)

    # ... handle tool_outputs and attachments

# Load export
with open('~/export.json') as f:
    data = json.load(f)

for session in data['sessions']:
    transform_session(session, Path('~/.aleph/raw/sessions'), Path('~/.aleph/raw/attachments'))
```

---

### 7.4 Migration Performance

**Optimize for large datasets**:

```bash
# Parallel processing (if safe)
aleph-migrate transform \
  --input ~/export.json \
  --output ~/.aleph/raw/sessions/ \
  --attachments ~/.aleph/raw/attachments/ \
  --parallel 4  # use 4 threads

# Disable FTS5 during bulk import
aleph-migrate index \
  --base-dir ~/.aleph \
  --skip-fts5

# Rebuild FTS5 after all sessions imported
aleph-migrate reindex --base-dir ~/.aleph --fts5-only
```

---

## 8. Rollback Plan

If migration fails or you need to revert:

**Step 1: Stop Aleph**
```bash
killall aleph
```

**Step 2: Remove new structure**
```bash
rm -rf ~/.aleph/raw/
rm ~/.aleph/metadata.db
```

**Step 3: Restore backup**
```bash
cp ~/aleph-backup-YYYYMMDD/aleph.db \
   ~/Library/Application\ Support/aleph/aleph.db
```

**Step 4: Restart Aleph**
```bash
aleph
```

**Step 5: Verify**
```bash
sqlite3 ~/Library/Application\ Support/aleph/aleph.db \
  "SELECT COUNT(*) FROM raw_memories;"
```

---

## 9. Migration Checklist

**Before Migration**:
- [ ] Backup `aleph.db` to `~/aleph-backup-YYYYMMDD/`
- [ ] Backup `~/.aleph/memory/` to `~/aleph-backup-YYYYMMDD/`
- [ ] Check disk space (need ~2x current DB size)
- [ ] Install `aleph-migrate` tool
- [ ] Stop Aleph process

**During Migration**:
- [ ] Export to JSON: `aleph-migrate export`
- [ ] Verify JSON structure with `jq`
- [ ] Transform to Markdown: `aleph-migrate transform`
- [ ] Index metadata: `aleph-migrate index`
- [ ] Verify integrity: `aleph-migrate verify`

**After Migration**:
- [ ] Test Aleph CLI: `aleph-cli memory list-sessions`
- [ ] Test Panel UI: Browse sessions in Settings
- [ ] Verify search works: `aleph-cli memory search "test"`
- [ ] Check attachments display correctly
- [ ] Archive old database after 7 days of successful use
- [ ] Delete backup after 30 days

---

## 10. Support

### 10.1 Report Issues

If you encounter problems during migration:

1. Save error output: `aleph-migrate verify --base-dir ~/.aleph > error.log 2>&1`
2. Create issue: https://github.com/your-org/aleph/issues
3. Include:
   - Aleph version
   - OS version
   - Error log
   - Migration report (redact sensitive content)

### 10.2 Community Help

- Discord: https://discord.gg/aleph
- Forum: https://discuss.aleph.ai
- Docs: https://docs.aleph.ai/migration

---

**Last Updated**: 2026-10-18
**Document Version**: 1.0
**Applies To**: Aleph v0.9.0+
