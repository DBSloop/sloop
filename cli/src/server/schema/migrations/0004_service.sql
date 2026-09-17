-- The service, what it watches, and what that traffic cost.
--
-- The tables exist here; nothing writes to them yet. They are on the owner's list for this
-- schema and `R19c4` moves the registry onto the tables above, so the alternative is a
-- migration in every later release for something that is already decided.

-- Whether the service is set up on this machine.
--
-- Keyed by name rather than being a single row, so a second service later is a row instead
-- of another migration.
CREATE TABLE service (
    id           BIGSERIAL   PRIMARY KEY,
    name         TEXT        NOT NULL UNIQUE CHECK (name <> ''),
    installed    BOOLEAN     NOT NULL DEFAULT FALSE,
    -- Which of the three platforms' service managers is holding it.
    mechanism    TEXT        CHECK (mechanism IN ('windows-service', 'systemd', 'launchd')),
    installed_at TIMESTAMPTZ,
    last_seen_at TIMESTAMPTZ,
    -- Installed and nobody knows where is not a state worth being able to reach.
    CONSTRAINT installed_names_its_mechanism
        CHECK (installed = FALSE OR mechanism IS NOT NULL)
);

-- Which databases are attached to it.
CREATE TABLE monitored_database (
    id                     BIGSERIAL   PRIMARY KEY,
    service_id             BIGINT      NOT NULL REFERENCES service (id) ON DELETE CASCADE,
    -- Cascaded, unlike the history tables: an attachment to a database that is no longer
    -- registered is not history, it is an instruction to watch something that is gone.
    registered_database_id BIGINT      NOT NULL
                                       REFERENCES registered_database (id) ON DELETE CASCADE,
    enabled                BOOLEAN     NOT NULL DEFAULT TRUE,
    attached_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT one_attachment_per_database UNIQUE (service_id, registered_database_id)
);

-- Bandwidth, by database, by day.
--
-- **One row per database per UTC day, which is the whole design.** A day, a week, a month
-- and a year are all `sum` over a range of this one table — no rollup tables to keep in
-- step, nothing to backfill, and a year of one database is 365 rows.
CREATE TABLE bandwidth_day (
    id                     BIGSERIAL   PRIMARY KEY,
    registered_database_id BIGINT      NOT NULL
                                       REFERENCES registered_database (id) ON DELETE CASCADE,
    -- The UTC day, for the reason every path is UTC. Every display converts.
    day                    DATE        NOT NULL,
    -- From sloop's side of the wire. `in` is what came down — a dump being read — and `out`
    -- is what went up, a restore being written.
    bytes_in               BIGINT      NOT NULL DEFAULT 0 CHECK (bytes_in >= 0),
    bytes_out              BIGINT      NOT NULL DEFAULT 0 CHECK (bytes_out >= 0),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT one_row_per_database_per_day UNIQUE (registered_database_id, day)
);

-- For the reads that are by date across every database rather than by database.
CREATE INDEX bandwidth_day_by_day ON bandwidth_day (day);
