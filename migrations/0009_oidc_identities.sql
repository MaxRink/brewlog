CREATE TABLE oidc_identities (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (issuer, subject),
    UNIQUE (user_id)
);
CREATE INDEX idx_oidc_identities_user_id ON oidc_identities(user_id);
