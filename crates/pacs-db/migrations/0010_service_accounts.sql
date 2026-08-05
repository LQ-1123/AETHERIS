-- Long-lived machine identities for external API integrations. API secrets are
-- never stored; only a SHA-256 digest and a non-secret lookup prefix remain.

CREATE TABLE service_accounts (
    id              UUID        PRIMARY KEY,
    institution_id  BIGINT      NOT NULL REFERENCES institutions(id),
    name            TEXT        NOT NULL,
    scopes          TEXT[]      NOT NULL,
    is_active       BOOLEAN     NOT NULL DEFAULT true,
    expires_at      TIMESTAMPTZ,
    created_by      BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at    TIMESTAMPTZ,
    UNIQUE (institution_id, name),
    CONSTRAINT service_accounts_name_not_blank CHECK (length(btrim(name)) BETWEEN 3 AND 100),
    CONSTRAINT service_accounts_scopes_valid CHECK (
        cardinality(scopes) > 0
        AND scopes <@ ARRAY['search', 'read', 'upload', 'export', 'route', 'admin']::TEXT[]
    )
);

CREATE INDEX service_accounts_list_idx
    ON service_accounts (institution_id, created_at DESC);
CREATE TRIGGER service_accounts_set_updated_at BEFORE UPDATE ON service_accounts
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE service_api_keys (
    id                  UUID        PRIMARY KEY,
    service_account_fk  UUID        NOT NULL REFERENCES service_accounts(id) ON DELETE CASCADE,
    key_prefix          TEXT        NOT NULL UNIQUE,
    secret_hash         BYTEA       NOT NULL UNIQUE,
    expires_at          TIMESTAMPTZ,
    revoked_at          TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at        TIMESTAMPTZ
);

CREATE INDEX service_api_keys_account_idx
    ON service_api_keys (service_account_fk, created_at DESC);
CREATE INDEX service_api_keys_active_prefix_idx
    ON service_api_keys (key_prefix)
    WHERE revoked_at IS NULL;
