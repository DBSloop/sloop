-- The registry, as tables: what `registry.toml` and the project pointers hold today.
--
-- `R19c4` moves the code onto this; what is here is the shape it moves onto.
--
-- **No password is in any of it, and the server enforces that as well as the code does.**
-- Every `*_route` column holds a *route* — the word `keyring`, the word `encrypted-file`,
-- `${SOME_VARIABLE}`, or `command:` and a command line. The CHECK beneath each one is the
-- same refusal `secret::Route::parse` makes in Rust, so a row typed into this database by
-- hand cannot carry a plaintext password either. That is rule 3 by construction, twice.

-- The engines this build speaks.
--
-- Rows are reconciled from `Engine::ALL` on every run and **never deleted**: registrations
-- and history point at them, and a build that stopped speaking an engine should leave the
-- old rows readable rather than cascade a year of backup history out of existence. Dropping
-- support sets `supported` to false.
CREATE TABLE engine (
    id           BIGSERIAL PRIMARY KEY,
    -- The canonical spelling, which is the one `Engine::scheme` uses and the one a URL uses.
    name         TEXT      NOT NULL UNIQUE CHECK (name <> ''),
    -- How it is written for a person: `PostgreSQL`, not `postgres`.
    display_name TEXT      NOT NULL CHECK (display_name <> ''),
    default_port INTEGER   NOT NULL CHECK (default_port BETWEEN 1 AND 65535),
    supported    BOOLEAN   NOT NULL DEFAULT TRUE
);

-- A directory with a `.sloop` in it.
--
-- The directory itself, not the `.sloop` inside it, because that is what the user names with
-- `-C` and what the walk up from the working directory finds.
CREATE TABLE project (
    id         BIGSERIAL   PRIMARY KEY,
    directory  TEXT        NOT NULL UNIQUE CHECK (directory <> ''),
    first_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One registered database. Host, port, user, and which route its password takes.
CREATE TABLE registered_database (
    id             BIGSERIAL   PRIMARY KEY,
    -- The label typed at the command line. Not the database's own name, which is
    -- `database_name` below — the two are different strings and conflating them is how a
    -- rename orphans a password.
    label          TEXT        NOT NULL CHECK (label <> ''),
    engine_id      BIGINT      NOT NULL REFERENCES engine (id),
    host           TEXT        NOT NULL CHECK (host <> ''),
    port           INTEGER     NOT NULL CHECK (port BETWEEN 1 AND 65535),
    database_name  TEXT        NOT NULL CHECK (database_name <> ''),
    username       TEXT        NOT NULL CHECK (username <> ''),
    password_route TEXT        NOT NULL CHECK (
                       password_route IN ('keyring', 'encrypted-file')
                       OR password_route ~ '^\$\{[^$[:space:]]+\}$'
                       OR password_route ~ '^command:[[:space:]]*[^[:space:]]'
                   ),
    -- Global, or this project's.
    scope          TEXT        NOT NULL CHECK (scope IN ('global', 'project')),
    project_id     BIGINT      REFERENCES project (id) ON DELETE CASCADE,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Global means no project and a project means one. The two columns can never disagree,
    -- which is what lets a query trust either of them alone.
    CONSTRAINT database_scope_matches_project
        CHECK ((scope = 'project') = (project_id IS NOT NULL)),
    -- One label per registry. `NULLS NOT DISTINCT` because the global store's `project_id`
    -- is NULL, and PostgreSQL's default treats every NULL as its own value — which would let
    -- the global store hold the same label twice while the constraint looked like it was
    -- there.
    CONSTRAINT one_label_per_registry
        UNIQUE NULLS NOT DISTINCT (project_id, label)
);

-- The project first, then the global store: the search order, as an index.
CREATE INDEX registered_database_by_registry ON registered_database (project_id, label);

-- The backup keypair, one per registry.
--
-- **The asymmetry is the point.** The public key sits here in the open because it can only
-- encrypt — a scheduled backup runs with nothing unlocked. The private key takes one of the
-- four routes and only a restore ever needs it.
CREATE TABLE backup_key (
    id                BIGSERIAL   PRIMARY KEY,
    scope             TEXT        NOT NULL CHECK (scope IN ('global', 'project')),
    project_id        BIGINT      REFERENCES project (id) ON DELETE CASCADE,
    public_key        TEXT        NOT NULL CHECK (public_key <> ''),
    private_key_route TEXT        NOT NULL CHECK (
                          private_key_route IN ('keyring', 'encrypted-file')
                          OR private_key_route ~ '^\$\{[^$[:space:]]+\}$'
                          OR private_key_route ~ '^command:[[:space:]]*[^[:space:]]'
                      ),
    -- `exported` or `declined`, once somebody has answered. NULL until they have, which is
    -- what makes the first encrypted backup refuse to run.
    key_kept          TEXT        CHECK (key_kept IN ('exported', 'declined')),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT key_scope_matches_project
        CHECK ((scope = 'project') = (project_id IS NOT NULL)),
    CONSTRAINT one_key_per_registry
        UNIQUE NULLS NOT DISTINCT (project_id)
);
