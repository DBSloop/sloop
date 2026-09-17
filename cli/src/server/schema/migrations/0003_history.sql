-- Backup and restore history.
--
-- **Counts and names, never rows.** Everything in here is a fact *about* a database — how
-- big the dump was, how many rows it held, how long it took, whether the counts matched.
-- Nothing that was ever inside one of the user's tables comes in here, which is the owner's
-- line and the same promise as the guarantee.
--
-- **Per-table counts stay in `manifest.json`**, beside the dump they describe. They are
-- already written there, they are unbounded per backup, and a backup carried to another
-- machine has to arrive with them — a copy in this database would answer a question that is
-- only ever asked while standing next to the file.

CREATE TABLE backup_run (
    id                     BIGSERIAL   PRIMARY KEY,
    -- Nulled rather than deleted when a database is unregistered. The history of a backup
    -- outlives the registration, and `label` below is what somebody goes looking for.
    registered_database_id BIGINT      REFERENCES registered_database (id) ON DELETE SET NULL,
    label                  TEXT        NOT NULL CHECK (label <> ''),
    engine_id              BIGINT      NOT NULL REFERENCES engine (id),
    -- The connection exactly as a server's own log would show it. Never a password.
    source                 TEXT        NOT NULL CHECK (source <> ''),
    -- UTC, always, for the reason every path is UTC: it sorts and it survives DST. Every
    -- display converts to the local clock.
    started_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at            TIMESTAMPTZ,
    outcome                TEXT        NOT NULL CHECK (
                               outcome IN ('running', 'succeeded', 'failed', 'mismatched')
                           ),
    -- The run's own exit code, so `6` — finished, but the counts disagreed — reads back as
    -- itself instead of being flattened into a failure it is not.
    exit_code              INTEGER,
    directory              TEXT,
    dump_bytes             BIGINT      CHECK (dump_bytes >= 0),
    dump_sha256            TEXT        CHECK (dump_sha256 ~ '^[0-9a-f]{64}$'),
    encrypted              BOOLEAN     NOT NULL DEFAULT FALSE,
    rows_counted           BIGINT      CHECK (rows_counted >= 0),
    tables_counted         INTEGER     CHECK (tables_counted >= 0),
    server_version         TEXT,
    took_seconds           DOUBLE PRECISION CHECK (took_seconds >= 0),
    -- What it said when it failed, in the words it printed. Nothing here that could not be
    -- pasted into a public issue.
    message                TEXT,
    CONSTRAINT finished_after_it_started CHECK (finished_at IS NULL OR finished_at >= started_at)
);

-- Newest first, by name and by registration: the two orders every listing wants.
CREATE INDEX backup_run_by_label ON backup_run (label, started_at DESC);
CREATE INDEX backup_run_by_database ON backup_run (registered_database_id, started_at DESC);

CREATE TABLE restore_run (
    id                     BIGSERIAL   PRIMARY KEY,
    -- Which backup went in, when sloop has a record of it. A restore from a directory sloop
    -- did not write is still a restore and is still recorded.
    backup_run_id          BIGINT      REFERENCES backup_run (id) ON DELETE SET NULL,
    registered_database_id BIGINT      REFERENCES registered_database (id) ON DELETE SET NULL,
    label                  TEXT        NOT NULL CHECK (label <> ''),
    -- Where it went, as a server's log would show it. Never a password.
    destination            TEXT        NOT NULL CHECK (destination <> ''),
    source_directory       TEXT,
    started_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at            TIMESTAMPTZ,
    outcome                TEXT        NOT NULL CHECK (
                               outcome IN ('running', 'succeeded', 'failed', 'mismatched')
                           ),
    exit_code              INTEGER,
    rows_restored          BIGINT      CHECK (rows_restored >= 0),
    -- NULL when verification was not asked for. True or false is an exact `count(*)` on both
    -- sides having agreed or not — never an estimate, which is `VERIFY=fast` and is labelled
    -- as one wherever it is shown.
    verified               BOOLEAN,
    took_seconds           DOUBLE PRECISION CHECK (took_seconds >= 0),
    message                TEXT,
    CONSTRAINT restore_finished_after_it_started
        CHECK (finished_at IS NULL OR finished_at >= started_at)
);

CREATE INDEX restore_run_by_label ON restore_run (label, started_at DESC);
CREATE INDEX restore_run_by_backup ON restore_run (backup_run_id);
