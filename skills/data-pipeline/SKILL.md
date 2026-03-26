---
name: data-pipeline
description: "Data processing and transformation — JSON filtering, CSV manipulation, format conversion, data cleaning, and report generation via jq and Python. Use when processing JSON (jq queries, filtering, reshaping), CSV/TSV (reading, aggregating, joining), converting between formats (JSON/CSV/TSV/Excel), or building data transformation pipelines."
---

# Data Processing Pipeline

## Prerequisites

```bash
brew install jq       # JSON processor
python3 --version     # usually pre-installed
```

## Dual Path Strategy

| Complexity | Tool | When |
|-----------|------|------|
| Quick filter/extract | `jq` | One-liner, JSON data |
| Multi-step pipeline | `bash` (jq + awk + sort) | Chaining simple operations |
| Complex transform | `python3` | Joins, aggregation, validation, Excel |

## jq Essentials

For full jq reference, see [references/jq-reference.md](references/jq-reference.md).

```bash
# Common patterns
jq '.[] | select(.age > 30)' users.json              # filter
jq '.[] | {name: .full_name, email: .contact.email}'  # reshape
jq '[.[] | .amount] | add' transactions.json          # aggregate
jq -r '.[] | [.name, .email] | @csv' users.json       # to CSV
```

## CSV/Python Essentials

For full Python CSV reference, see [references/python-csv.md](references/python-csv.md).

```bash
# CSV -> JSON
python3 -c "import csv,json,sys; print(json.dumps(list(csv.DictReader(open(sys.argv[1]))),indent=2))" data.csv
```

## Large File Handling

```bash
# Stream JSON Lines (don't load all into memory)
cat huge.jsonl | jq -c 'select(.status == "error")' > errors.jsonl

# Count CSV rows without loading
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

- Synergy with `web-scraper` (A4): extract data -> process with jq/python
- Synergy with `http-client` (A3): API response -> jq pipeline for analysis
- Synergy with `doc` (F8): processed data -> Markdown tables for documentation
