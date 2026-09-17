-- Reaching a database that is only reachable from the server — `R19e`.
--
-- **Columns on `registered_database`, not a table of servers.** A `Reach` sits inside every
-- registered database in Rust, and this is the same shape in SQL: a row says where its
-- database is *and* how to get there. A shared `ssh_server` table would be tidier on a
-- machine with five databases behind one bastion, and it was not chosen, because two
-- databases already share one login without sharing a row — `ssh::Server::credential_key`
-- decides that at run time, from the host, the port and the identity. A table would add a
-- second place for that decision to live and a lifetime question — when does a server row
-- nobody points at go away — for a benefit the run time already gives.
--
-- **Not one of these columns is a secret, and that is not an accident of what happens to be
-- stored.** A host, a port, a username and a *path* to a private key are all things `ps`
-- shows anyway when `ssh` runs; the key file is never read by sloop and never copied. The
-- passphrase that opens it takes `R3`'s four routes exactly as a database password does, and
-- `ssh_secret_route` holds the route — the word `keyring`, the word `encrypted-file`,
-- `${SOME_VARIABLE}`, or `command:` and a command line. The CHECK below is the same refusal
-- `secret::Route::parse` makes in Rust, so a passphrase typed into this column by hand is
-- refused by the server as well as by the code.
--
-- **NULL `ssh_host` is a direct connection**, which is every database registered before this
-- migration and most of them after it. The constraints keep the other four columns from
-- meaning anything without it.

ALTER TABLE registered_database ADD COLUMN ssh_host TEXT;
ALTER TABLE registered_database ADD COLUMN ssh_port INTEGER;
ALTER TABLE registered_database ADD COLUMN ssh_user TEXT;
ALTER TABLE registered_database ADD COLUMN ssh_identity TEXT;
ALTER TABLE registered_database ADD COLUMN ssh_secret_route TEXT;

ALTER TABLE registered_database
    -- A port is a port.
    ADD CONSTRAINT ssh_port_is_a_port
        CHECK (ssh_port IS NULL OR ssh_port BETWEEN 1 AND 65535),

    -- An empty string is not a host, a user or a path. NULL is how "not given" is spelled
    -- here, and a column that accepted both would have two spellings of the same state.
    ADD CONSTRAINT ssh_text_is_not_empty
        CHECK ((ssh_host     IS NULL OR ssh_host     <> '')
           AND (ssh_user     IS NULL OR ssh_user     <> '')
           AND (ssh_identity IS NULL OR ssh_identity <> '')),

    -- Every SSH detail hangs off there being a server. Without one they would describe a
    -- tunnel that is never opened, which is worse than not being there: it reads like a
    -- database that goes over SSH and is not one.
    ADD CONSTRAINT ssh_details_need_a_server
        CHECK (ssh_host IS NOT NULL
            OR (ssh_port         IS NULL
            AND ssh_user         IS NULL
            AND ssh_identity     IS NULL
            AND ssh_secret_route IS NULL)),

    -- And a server always has a port, written down rather than assumed. The default is 22
    -- and Rust fills it in; storing it means a row answers the question on its own, the same
    -- way `port` does for the database.
    ADD CONSTRAINT ssh_server_has_a_port
        CHECK ((ssh_host IS NULL) = (ssh_port IS NULL)),

    -- Rule 3, in the schema, for the second credential a registration can have.
    ADD CONSTRAINT ssh_secret_route_is_a_route
        CHECK (ssh_secret_route IS NULL
            OR ssh_secret_route IN ('keyring', 'encrypted-file')
            OR ssh_secret_route ~ '^\$\{[^$[:space:]]+\}$'
            OR ssh_secret_route ~ '^command:[[:space:]]*[^[:space:]]');
