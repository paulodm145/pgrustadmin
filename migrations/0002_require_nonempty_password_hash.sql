ALTER TABLE users
    ADD CONSTRAINT users_password_hash_not_empty CHECK (password_hash <> '');
