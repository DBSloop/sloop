-- What the server's own counters said about an attached database — `R26`.
--
-- **The hour is the grain, and `R26`'s own `Done when` is why.** *"A day of samples rolls up to
-- a daily figure matching the sum of its hours"* — a day cannot be summed from its hours unless
-- the hours are kept. A day, a week, a month and a year are then all `sum` over a range of this
-- one table, which is `R27a`'s rule about `bandwidth_day` applied one grain finer. There are no
-- rollup tables to keep in step, nothing to backfill, and a year of one database is 8,760 rows.
--
-- **This is not `bandwidth_day`, and the two must not be confused.** `R27a` settles what that
-- table is for: **bytes sloop itself moved**, because sloop moved them — a dump read, a restore
-- written — which is exact. This table is the other half, and it is **rows and size**, because
-- per-database bytes do not exist. `pg_stat_database` counts rows; `blks_read` is disk, not
-- network; MySQL's `Bytes_sent` and `Bytes_received` are server-global. Every figure says which
-- of the two it came from, and a number labelled "bytes" that was really rows would be the one
-- dishonest number in this tool. See "Bandwidth is rows, because bytes do not exist per
-- database" in `docs/OWNER-DECISIONS.md`.
CREATE TABLE activity_hour (
    id                     BIGSERIAL   PRIMARY KEY,
    -- Cascaded, like the attachment: activity about a database that is no longer registered
    -- is activity nothing can name.
    registered_database_id BIGINT      NOT NULL
                                       REFERENCES registered_database (id) ON DELETE CASCADE,
    -- The start of the UTC hour, for the reason every path is UTC: it sorts, and it survives
    -- a clock going back. Every display converts.
    hour                   TIMESTAMPTZ NOT NULL,
    -- Rows written into the database over this hour, and rows read out of it. Deltas between
    -- consecutive readings, so they add up over a range -- which is what makes a day a `sum`.
    rows_in                BIGINT      NOT NULL DEFAULT 0 CHECK (rows_in >= 0),
    rows_out               BIGINT      NOT NULL DEFAULT 0 CHECK (rows_out >= 0),
    -- **A level, not a total.** How big the database was at the last reading in this hour;
    -- summing it over a range would be meaningless, so `R27` averages or takes the last.
    -- NULL where the engine would not say, which is not the same as zero.
    size_bytes             BIGINT      CHECK (size_bytes >= 0),
    -- How many readings went into this row. An hour with one reading in it is a service that
    -- was started late or stopped early, and `R27` has to be able to say so rather than
    -- drawing it as a quiet hour.
    readings               INTEGER     NOT NULL DEFAULT 0 CHECK (readings >= 0),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT one_row_per_database_per_hour UNIQUE (registered_database_id, hour)
);

-- For the reads that are by time across every database rather than by database.
CREATE INDEX activity_hour_by_hour ON activity_hour (hour);

-- Where the last reading got to, so a delta survives a restart.
--
-- **The counters these are taken from are cumulative and they reset.** `pg_stat_database`
-- counts since the statistics were last reset, which a `pg_stat_reset()` or a fresh initdb
-- puts back to zero; MySQL's table counters reset when the server restarts. So a sample is a
-- snapshot and what gets recorded is the difference from the one before — which means the one
-- before has to outlive the process that took it. `R26`'s *"a restart mid-day loses no
-- completed rollup"* is this column: without it the first reading after a restart would either
-- be dropped or be counted as if the whole cumulative total had happened in one minute.
--
-- On the attachment rather than in a table of their own, because there is exactly one of these
-- per attachment and a detach should forget the baseline — the next attach starts counting
-- again rather than inventing a delta across the gap.
ALTER TABLE monitored_database ADD COLUMN counted_rows_in BIGINT;
ALTER TABLE monitored_database ADD COLUMN counted_rows_out BIGINT;
ALTER TABLE monitored_database ADD COLUMN counted_at TIMESTAMPTZ;
