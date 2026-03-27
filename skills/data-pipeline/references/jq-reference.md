# jq Quick Reference

## Basics
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

## Filtering
```bash
# Select by condition
jq '.[] | select(.age > 30)' users.json

# Select by string match
jq '.[] | select(.name | test("^A"))' users.json

# Multiple conditions
jq '.[] | select(.active == true and .role == "admin")' users.json
```

## Transformation
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

## Aggregation
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

## Format Conversion
```bash
# JSON -> CSV
jq -r '.[] | [.name, .email, .age] | @csv' users.json

# JSON -> TSV
jq -r '.[] | [.name, .email, .age] | @tsv' users.json

# JSON Lines -> JSON array
jq -s '.' data.jsonl

# JSON array -> JSON Lines
jq -c '.[]' data.json
```

## Flags
| Flag | Purpose |
|------|---------|
| `-r` | Raw output (no quotes around strings) |
| `-c` | Compact output (one line per object) |
| `-s` | Slurp: read all inputs into array |
| `-S` | Sort object keys |
| `-e` | Exit with error if output is null/false |
| `--arg k v` | Pass variable: `jq --arg name "Alice" '.[] \| select(.name == $name)'` |
