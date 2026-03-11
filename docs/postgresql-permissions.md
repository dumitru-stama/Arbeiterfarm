# PostgreSQL Permissions & pgvector

## Roles and Users

PostgreSQL has **roles**. A role with `LOGIN` is what you'd call a "user".

```sql
-- Create a login role (user)
CREATE USER af WITH PASSWORD 'af';
-- Equivalent to:
CREATE ROLE af WITH LOGIN PASSWORD 'af';

-- Create a role that can't log in (group-like)
CREATE ROLE readonly;
```

### Special roles

| Role | Meaning |
|------|---------|
| `postgres` | Default superuser, can do anything |
| `SUPERUSER` | Attribute — bypasses all permission checks |
| `CREATEDB` | Can create databases |
| `CREATEROLE` | Can create other roles |

```sql
-- See all roles
\du

-- Give a role superuser powers (dangerous)
ALTER ROLE af SUPERUSER;

-- Remove superuser
ALTER ROLE af NOSUPERUSER;

-- Change password
ALTER ROLE af WITH PASSWORD 'newpass';

-- Delete a role
DROP ROLE af;
```

## Ownership

Every database object (database, schema, table, extension) has an **owner**. The owner can do anything to that object.

```sql
-- See who owns the database
\l

-- See who owns tables
\dt

-- Transfer ownership
ALTER TABLE my_table OWNER TO af;

-- Transfer all tables in a schema
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO af;
```

## The permission hierarchy

```
Server
└── Database          ← CONNECT, CREATE (schemas), TEMP
    └── Schema        ← USAGE (see objects), CREATE (make objects)
        ├── Table     ← SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER
        ├── Sequence  ← USAGE, SELECT, UPDATE
        ├── Function  ← EXECUTE
        └── Extension ← only superuser or owner can CREATE EXTENSION
```

### Key concept: you need BOTH schema and object permissions

A role needs `USAGE` on the schema **and** `SELECT` on the table to query it.

## GRANT and REVOKE

```sql
-- Grant connect to a database
GRANT CONNECT ON DATABASE af TO af;

-- Grant everything on a database
GRANT ALL PRIVILEGES ON DATABASE af TO af;

-- Grant schema access (PG 15+ requires this explicitly)
GRANT ALL ON SCHEMA public TO af;

-- Grant table permissions
GRANT SELECT ON my_table TO af;
GRANT ALL ON ALL TABLES IN SCHEMA public TO af;
GRANT ALL ON ALL SEQUENCES IN SCHEMA public TO af;

-- Future objects too (so migrations create accessible tables)
ALTER DEFAULT PRIVILEGES IN SCHEMA public
  GRANT ALL ON TABLES TO af;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
  GRANT ALL ON SEQUENCES TO af;

-- Revoke
REVOKE INSERT ON my_table FROM af;
REVOKE ALL ON ALL TABLES IN SCHEMA public FROM af;
```

### See current permissions

```sql
-- Table permissions
\dp                        -- or \z
\dp my_table               -- specific table

-- Schema permissions
\dn+

-- Database permissions
\l+
```

The output format is `role=privileges/grantor`:
```
af=arwdDxt/postgres
```
Where: `a`=INSERT, `r`=SELECT, `w`=UPDATE, `d`=DELETE, `D`=TRUNCATE, `x`=REFERENCES, `t`=TRIGGER

## Extensions (the pgvector problem)

Extensions are **server-level packages** installed into a specific database. Creating one requires **superuser** or the `CREATE` privilege on the database (PG 13+, but most extensions still need superuser).

```sql
-- Only works as superuser (e.g. postgres role)
CREATE EXTENSION IF NOT EXISTS vector;

-- See installed extensions
\dx

-- See available (installed on OS but not enabled)
SELECT * FROM pg_available_extensions WHERE name = 'vector';

-- Drop an extension
DROP EXTENSION vector;
```

### Why `af` can't create extensions

The `af` role is a normal user — no `SUPERUSER` attribute. `CREATE EXTENSION` for pgvector requires superuser because the extension creates custom types and operators in C.

**The fix**: create the extension as `postgres`, then the `af` user can use the types/functions:

```bash
sudo -u postgres psql -d af -c "CREATE EXTENSION IF NOT EXISTS vector;"
```

After this, `af` can:
- Create tables with `vector(384)` columns
- Run `<->` distance queries
- Create HNSW/IVFFlat indexes

But `af` cannot:
- `CREATE EXTENSION` or `DROP EXTENSION`
- `ALTER EXTENSION` (upgrade it)

## pgvector specifics

### Install the OS package

```bash
# The package name includes the PG major version
sudo apt install postgresql-16-pgvector
```

This puts the shared library and SQL files where PostgreSQL can find them. It does NOT enable it in any database — you must still `CREATE EXTENSION`.

### Enable in a database

```bash
sudo -u postgres psql -d af -c "CREATE EXTENSION IF NOT EXISTS vector;"
```

### Use it

```sql
-- Column type
CREATE TABLE embeddings (
    id SERIAL PRIMARY KEY,
    content TEXT,
    embedding vector(384)    -- 384 dimensions
);

-- Insert
INSERT INTO embeddings (content, embedding)
VALUES ('hello', '[0.1, 0.2, ...]');

-- Nearest-neighbor search (L2 distance)
SELECT * FROM embeddings
ORDER BY embedding <-> '[0.1, 0.2, ...]'
LIMIT 10;

-- Cosine distance
SELECT * FROM embeddings
ORDER BY embedding <=> '[0.1, 0.2, ...]'
LIMIT 10;

-- HNSW index (fast approximate search)
CREATE INDEX ON embeddings
USING hnsw (embedding vector_cosine_ops);
```

## Common CLI recipes

All of these use `psql`. Connect as superuser for admin tasks:

```bash
# Connect as postgres superuser
sudo -u postgres psql

# Connect as af user to af database
psql postgres://af:af@localhost/af
```

### User management

```bash
# Create user
sudo -u postgres psql -c "CREATE USER myuser WITH PASSWORD 'mypass';"

# List users
sudo -u postgres psql -c "\du"

# Grant superuser (use sparingly)
sudo -u postgres psql -c "ALTER ROLE myuser SUPERUSER;"

# Remove superuser
sudo -u postgres psql -c "ALTER ROLE myuser NOSUPERUSER;"

# Delete user (must remove owned objects first)
sudo -u postgres psql -c "REASSIGN OWNED BY myuser TO postgres; DROP OWNED BY myuser; DROP ROLE myuser;"
```

### Database management

```bash
# Create database
sudo -u postgres psql -c "CREATE DATABASE mydb OWNER myuser;"

# Grant access
sudo -u postgres psql -c "GRANT ALL PRIVILEGES ON DATABASE mydb TO myuser;"
sudo -u postgres psql -d mydb -c "GRANT ALL ON SCHEMA public TO myuser;"

# Check who can access what
sudo -u postgres psql -c "\l+"
```

### Permission debugging

```bash
# "permission denied for table X"
sudo -u postgres psql -d af -c "GRANT ALL ON ALL TABLES IN SCHEMA public TO af;"
sudo -u postgres psql -d af -c "GRANT ALL ON ALL SEQUENCES IN SCHEMA public TO af;"

# "permission denied for schema public" (PG 15+)
sudo -u postgres psql -d af -c "GRANT ALL ON SCHEMA public TO af;"

# "permission denied to create extension"
sudo -u postgres psql -d af -c "CREATE EXTENSION IF NOT EXISTS vector;"

# Check what permissions a role has on tables
sudo -u postgres psql -d af -c "\dp"
```

## Arbeiterfarm setup summary

```bash
# 1. Install pgvector OS package
sudo apt install postgresql-16-pgvector

# 2. Create extension as superuser
sudo -u postgres psql -d af -c "CREATE EXTENSION IF NOT EXISTS vector;"

# 3. Now af migrations will work
make serve-local
```
