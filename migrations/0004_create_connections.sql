CREATE TABLE connections (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    database_name TEXT NOT NULL,
    username TEXT NOT NULL,
    tls_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    credential_envelope BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT connections_name_nonempty CHECK (name <> ''),
    CONSTRAINT connections_name_length CHECK (char_length(name) <= 100),
    CONSTRAINT connections_host_nonempty CHECK (host <> ''),
    CONSTRAINT connections_host_length CHECK (char_length(host) <= 255),
    CONSTRAINT connections_port_range CHECK (port BETWEEN 1 AND 65535),
    CONSTRAINT connections_database_nonempty CHECK (database_name <> ''),
    CONSTRAINT connections_username_nonempty CHECK (username <> ''),
    CONSTRAINT connections_credential_envelope_length CHECK (octet_length(credential_envelope) >= 41),
    CONSTRAINT connections_owner_name_unique UNIQUE (owner_id, name)
);

CREATE INDEX connections_owner_id_idx ON connections (owner_id);
