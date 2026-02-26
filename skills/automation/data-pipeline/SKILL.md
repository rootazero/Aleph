---
name: data-pipeline
description: Data processing — JSON/CSV/TSV transformation, filtering, aggregation via jq and Python
emoji: "🔀"
category: automation
cli-wrapper: true
allowed-tools:
  - Bash
requirements:
  binaries:
    - jq
    - python3
  platforms:
    - macos
    - linux
  install:
    - manager: brew
      package: jq
triggers:
  - jq
  - JSON process
  - CSV
  - data transform
  - filter data
  - parse data
---

# Data Processing Pipeline

## When to Use

Invoke this skill for any data transformation task: JSON filtering, CSV manipulation, format conversion, data cleaning, or report generation. Uses jq for quick queries and Python for complex transformations.

## Prerequisites

```bash
# jq (JSON processor)
brew install jq
jq --version

# Python 3 (usually pre-installed)
python3 --version
```

## Dual Path Strategy

| Complexity | Tool | When |
|-----------|------|------|
| Quick filter/extract | `jq` | One-liner, JSON data |
| Multi-step pipeline | `bash` (jq + awk + sort) | Chaining simple operations |
| Complex transform | `python3` | Joins, aggregation, validation, Excel |

## jq Quick Reference

### Basics
```bash
# Pretty print
cat data.json | jq .

# Extract field
jq '.name' data.json

# Nested access
jq '.user.address.city' data.json

# Array first element
jq '.[0]' data.json

# Iterate array
jq '.[]' data.json
```

### Filtering
```bash
# Select by condition
jq '.[] | select(.age > 30)' users.json

# Select by string match
jq '.[] | select(.name | test("^A"))' users.json

# Multiple conditions
jq '.[] | select(.active == true and .role == "admin")' users.json
```

### Transformation
```bash
# Reshape objects
jq '.[] | {name: .full_name, email: .contact.email}' users.json

# Add/modify fields
jq '.[] | . + {status: "active"}' users.json

# Delete fields
jq '.[] | del(.password, .internal_id)' users.json

# Map array
jq '[.[] | .name]' users.json

# Group by
jq 'group_by(.department) | map({dept: .[0].department, count: length})' users.json
```

### Aggregation
```bash
# Count
jq '.data | length' response.json

# Sum
jq '[.[] | .amount] | add' transactions.json

# Min/Max
jq '[.[] | .price] | min' products.json
jq '[.[] | .price] | max' products.json

# Unique values
jq '[.[] | .category] | unique' items.json
```

### Format Conversion
```bash
# JSON → CSV
jq -r '.[] | [.name, .email, .age] | @csv' users.json

# JSON → TSV
jq -r '.[] | [.name, .email, .age] | @tsv' users.json

# JSON Lines → JSON array
jq -s '.' data.jsonl

# JSON array → JSON Lines
jq -c '.[]' data.json
```

### Flags
| Flag | Purpose |
|------|---------|
| `-r` | Raw output (no quotes around strings) |
| `-c` | Compact output (one line per object) |
| `-s` | Slurp: read all inputs into array |
| `-S` | Sort object keys |
| `-e` | Exit with error if output is null/false |
| `--arg k v` | Pass variable: `jq --arg name "Alice" '.[] \| select(.name == $name)'` |

## CSV Operations (Python)

### Read and Filter
```python
#!/usr/bin/env python3
import csv, json, sys

with open(sys.argv[1]) as f:
    reader = csv.DictReader(f)
    rows = [row for row in reader]

# Filter
filtered = [r for r in rows if float(r.get('amount', 0)) > 100]
print(json.dumps(filtered, indent=2))
```

### Aggregate
```python
from collections import Counter, defaultdict

# Group and sum
totals = defaultdict(float)
for row in rows:
    totals[row['category']] += float(row['amount'])
print(json.dumps(dict(totals), indent=2))

# Count by field
counts = Counter(row['status'] for row in rows)
print(json.dumps(dict(counts), indent=2))
```

### Join Two CSVs
```python
import csv

def read_csv(path):
    with open(path) as f:
        return list(csv.DictReader(f))

left = {r['id']: r for r in read_csv('users.csv')}
right = read_csv('orders.csv')
joined = [{**left.get(r['user_id'], {}), **r} for r in right if r['user_id'] in left]
```

### Format Conversion
```bash
# CSV → JSON
python3 -c "import csv,json,sys; print(json.dumps(list(csv.DictReader(open(sys.argv[1]))),indent=2))" data.csv

# JSON → CSV
python3 -c "
import csv,json,sys
data = json.load(open(sys.argv[1]))
if data:
    w = csv.DictWriter(sys.stdout, fieldnames=data[0].keys())
    w.writeheader(); w.writerows(data)
" data.json

# Excel → JSON (requires openpyxl)
python3 -c "
import json
from openpyxl import load_workbook
wb = load_workbook('data.xlsx', read_only=True)
ws = wb.active
rows = list(ws.iter_rows(values_only=True))
headers = [str(h) for h in rows[0]]
data = [dict(zip(headers, row)) for row in rows[1:]]
print(json.dumps(data, indent=2, default=str))
"
```

## Large File Handling

```bash
# Stream JSON Lines (don't load all into memory)
cat huge.jsonl | jq -c 'select(.status == "error")' > errors.jsonl

# Process CSV in chunks
python3 -c "
import csv, sys
with open(sys.argv[1]) as f:
    reader = csv.reader(f)
    header = next(reader)
    count = sum(1 for _ in reader)
    print(f'Rows: {count}, Columns: {len(header)}')
    print(f'Headers: {header}')
" huge.csv
```

## Gotchas

- **jq string interpolation**: Use `\(expr)` inside strings, e.g., `"\(.name) is \(.age)"`
- **Null handling**: Use `// "default"` for fallback, e.g., `jq '.name // "unknown"'`
- **CSV encoding**: Force UTF-8: `open(f, encoding='utf-8')` or `iconv -f latin1 -t utf-8`
- **Floating point**: Use `decimal.Decimal` for financial data in Python

## Aleph Integration

- Synergy with `web-scraper` (A4): extract data → process with jq/python
- Synergy with `http-client` (A3): API response → jq pipeline for analysis
- Synergy with `doc` (F8): processed data → Markdown tables for documentation
