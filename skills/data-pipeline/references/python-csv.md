# Python CSV/Data Operations

## Read and Filter
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

## Aggregate
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

## Join Two CSVs
```python
import csv

def read_csv(path):
    with open(path) as f:
        return list(csv.DictReader(f))

left = {r['id']: r for r in read_csv('users.csv')}
right = read_csv('orders.csv')
joined = [{**left.get(r['user_id'], {}), **r} for r in right if r['user_id'] in left]
```

## Format Conversion
```bash
# CSV -> JSON
python3 -c "import csv,json,sys; print(json.dumps(list(csv.DictReader(open(sys.argv[1]))),indent=2))" data.csv

# JSON -> CSV
python3 -c "
import csv,json,sys
data = json.load(open(sys.argv[1]))
if data:
    w = csv.DictWriter(sys.stdout, fieldnames=data[0].keys())
    w.writeheader(); w.writerows(data)
" data.json

# Excel -> JSON (requires openpyxl)
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
