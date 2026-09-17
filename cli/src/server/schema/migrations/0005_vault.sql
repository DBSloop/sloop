-- `secrets.sealed`, as a row.
--
-- **The blob is byte for byte what the file held**, and that is the decision. The sealed
-- store is one Argon2id-derived key over one XChaCha20-Poly1305 ciphertext covering every
-- name/password pair at once; splitting it into a row per password would mean a new nonce
-- scheme, a new header, and re-deriving a key per read. So the format does not change — only
-- where the bytes live. `R3`'s crypto is untouched, and a vault written by the file version
-- opens here unaltered.
--
-- **Rule 3 is unharmed, because ciphertext is not a password.** What is in `blob` cannot be
-- read without the passphrase, which is never stored anywhere — not in this database, not in
-- a file, not in an environment variable sloop writes. A `pg_dump` of `sloop_database` is as
-- safe to hand over as the file was.
--
-- **sloop's own two passwords are not in here and cannot be**: the superuser's and
-- `sloop_db_admin`'s are what open this database, so they stay outside it, in the global
-- `secrets.sealed` beside `server.toml`. That file keeps existing for exactly those two.

CREATE TABLE sealed_vault (
    id         BIGSERIAL   PRIMARY KEY,
    scope      TEXT        NOT NULL CHECK (scope IN ('global', 'project')),
    project_id BIGINT      REFERENCES project (id) ON DELETE CASCADE,
    -- The whole `SLOOPSEC` store: header, salt, nonce and ciphertext, exactly as the file
    -- held it.
    blob       BYTEA       NOT NULL CHECK (octet_length(blob) > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT vault_scope_matches_project
        CHECK ((scope = 'project') = (project_id IS NOT NULL)),
    -- One vault per registry, the same way there is one key per registry.
    CONSTRAINT one_vault_per_registry
        UNIQUE NULLS NOT DISTINCT (project_id)
);
