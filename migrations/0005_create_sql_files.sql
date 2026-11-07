CREATE TABLE sql_files (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    storage_key UUID NOT NULL,
    size_bytes INTEGER NOT NULL,
    content_sha256 BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT sql_files_name_length CHECK (octet_length(name) BETWEEN 5 AND 255),
    CONSTRAINT sql_files_name_suffix CHECK (name LIKE '%.sql'),
    CONSTRAINT sql_files_size_range CHECK (size_bytes BETWEEN 0 AND 1048576),
    CONSTRAINT sql_files_content_sha256_length CHECK (octet_length(content_sha256) = 32),
    CONSTRAINT sql_files_storage_key_unique UNIQUE (storage_key),
    CONSTRAINT sql_files_owner_name_unique UNIQUE (owner_id, name)
);
