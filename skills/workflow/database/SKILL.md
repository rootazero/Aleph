---
name: database
description: Database design — selection decision tree, schema design, query optimization, migrations
scope: standalone
---

# Database Design

## When to Use

Invoke this skill when choosing a database, designing schemas, optimizing queries, or planning migrations.

## Database Selection

### Decision Tree

| Data Characteristic | Points To |
|-------------------|-----------|
| Structured with relationships | SQL (PostgreSQL) |
| ACID transactions required | SQL |
| Flexible / evolving schema | Document store (MongoDB) |
| Key-value lookups | Redis, DynamoDB |
| Time-series data | TimescaleDB, InfluxDB |
| Full-text search | Elasticsearch, Meilisearch |
| Graph relationships | Neo4j, or PostgreSQL + recursive CTEs |
| Embedded / local | SQLite |
| Vector embeddings | LanceDB, pgvector |

### Scale-Based Selection

| Records | Read Pattern | Write Pattern | Recommendation |
|---------|-------------|---------------|----------------|
| < 1M | Any | Any | PostgreSQL or SQLite |
| 1M-100M | Read-heavy | Low writes | PostgreSQL + read replicas |
| 1M-100M | Write-heavy | High writes | PostgreSQL + partitioning |
| > 100M | Global distribution | Variable | CockroachDB, Spanner |
| > 100M | Key-value pattern | Very high writes | Cassandra, DynamoDB |

**Default choice: PostgreSQL.** It handles 90% of use cases well.

## Schema Design

### From DDD Perspective

| DDD Concept | Database Mapping |
|-------------|-----------------|
| Aggregate Root | Own table, primary key = aggregate ID |
| Entity within aggregate | Own table, FK to aggregate root |
| Value Object | Embedded as columns, or JSON column |
| Bounded Context | Separate schema or database |

### Normalization Quick Guide

- **1NF**: No repeating groups, atomic values
- **2NF**: 1NF + no partial dependencies
- **3NF**: 2NF + no transitive dependencies

**Pragmatic rule:** Normalize by default, denormalize for measured performance needs.

### Indexing Strategy

```sql
-- Primary key (automatic)
-- Foreign keys (always index)
CREATE INDEX idx_posts_user_id ON posts(user_id);

-- Frequently filtered columns
CREATE INDEX idx_users_email ON users(email);

-- Composite for common query patterns
CREATE INDEX idx_orders_user_status ON orders(user_id, status);

-- Partial index for filtered subsets
CREATE INDEX idx_active_users ON users(email) WHERE active = true;
```

**Rules:**
- Index columns used in WHERE, JOIN, and ORDER BY
- Composite index column order matters: most selective first
- Don't over-index: each index slows writes
- Use EXPLAIN to verify index usage

## Query Optimization

### The EXPLAIN Workflow

```sql
EXPLAIN ANALYZE SELECT * FROM users WHERE email = 'test@example.com';
```

| Look For | Meaning | Action |
|----------|---------|--------|
| Seq Scan on large table | Full table scan | Add index |
| Nested Loop with large outer | N+1 pattern | Use JOIN or batch |
| Sort | Sorting without index | Add sorted index |
| Hash Join | Large join | Check join columns are indexed |

### N+1 Query Prevention

**Problem:** Loading a list, then querying related data for each item.

```python
# BAD: N+1
users = db.query("SELECT * FROM users")
for user in users:
    posts = db.query("SELECT * FROM posts WHERE user_id = ?", user.id)  # N queries

# GOOD: JOIN or batch
users_with_posts = db.query("""
    SELECT u.*, p.* FROM users u
    LEFT JOIN posts p ON p.user_id = u.id
""")
```

## Migration Best Practices

### Rules

1. **Every migration must be reversible** (include both `up` and `down`)
2. **Never modify a released migration** — create a new one
3. **One concern per migration** — don't mix schema and data changes
4. **Test migrations** on a copy of production data

### Zero-Downtime Patterns

| Change | Safe Approach |
|--------|---------------|
| Add column | Add as nullable, backfill, then add NOT NULL |
| Remove column | Stop reading it, deploy, then drop |
| Rename column | Add new, copy data, update code, drop old |
| Add index | `CREATE INDEX CONCURRENTLY` (PostgreSQL) |

## Anti-Patterns

- **Premature NoSQL**: Choosing MongoDB because "it's easier" when you need relationships
- **No indexes on FKs**: Causes full table scans on JOINs
- **SELECT ***: Fetches unnecessary columns, prevents index-only scans
- **Stringly-typed data**: Using VARCHAR for dates, numbers, booleans — use proper types
- **No connection pooling**: Opening a new connection per request
- **Mixing DDL and DML in migrations**: Schema changes and data changes should be separate
