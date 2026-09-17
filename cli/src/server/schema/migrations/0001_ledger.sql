-- The ledger: which migrations this database has had, and what each one hashed to.
--
-- It is migration 1 because nothing else can be recorded until it exists, and it is the one
-- table the runner has to create before it can read anything. That is why `IF NOT EXISTS` is
-- here and in none of the files after it: every other migration runs exactly once, decided
-- by what this table says, and a second `CREATE TABLE` of anything else is a bug worth
-- failing on rather than a line to skip.

CREATE TABLE IF NOT EXISTS schema_migration (
    id         BIGSERIAL   PRIMARY KEY,
    version    BIGINT      NOT NULL UNIQUE CHECK (version > 0),
    name       TEXT        NOT NULL CHECK (name <> ''),
    -- SHA-256 of the migration's own SQL. A file edited after it was applied here is then a
    -- failure that names the migration, rather than a schema that quietly disagrees with the
    -- build that is reading it.
    checksum   TEXT        NOT NULL CHECK (checksum ~ '^[0-9a-f]{64}$'),
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- The sloop that ran it. Worth exactly one thing, and it is the thing somebody wants at
    -- the moment they are working out why this machine's schema is not what the code expects.
    applied_by TEXT        NOT NULL
);

COMMENT ON TABLE schema_migration IS
    'Which migrations have run against this database. Written by sloop, never by hand.';
