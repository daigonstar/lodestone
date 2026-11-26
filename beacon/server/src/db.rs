//! The server's database (SQLite). Message bodies are MLS ciphertext the server can't read; secrets the server
//! hands out (session tokens, invite and setup codes) are stored only as SHA-256 hashes.

use crate::util::{hash, new_code};
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub struct Db(Mutex<Connection>);

const SCHEMA: &str = "
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS accounts (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    server_admin INTEGER NOT NULL DEFAULT 0,
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS devices (
    key TEXT PRIMARY KEY,
    account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS challenges (
    nonce TEXT PRIMARY KEY,
    device_key TEXT NOT NULL,
    expires INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT PRIMARY KEY,
    device_key TEXT NOT NULL REFERENCES devices(key) ON DELETE CASCADE,
    expires INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS setup (code_hash TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS communities (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    owner INTEGER NOT NULL REFERENCES accounts(id),
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS members (
    community TEXT NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    joined INTEGER NOT NULL,
    PRIMARY KEY (community, account)
);
CREATE TABLE IF NOT EXISTS bans (
    community TEXT NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    PRIMARY KEY (community, account)
);
CREATE TABLE IF NOT EXISTS channels (
    id TEXT PRIMARY KEY,
    community TEXT NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    epoch INTEGER NOT NULL DEFAULT 0,
    next_seq INTEGER NOT NULL DEFAULT 1,
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS categories (
    id TEXT PRIMARY KEY,
    community TEXT NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS invites (
    code_hash TEXT PRIMARY KEY,
    community TEXT NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    created_by INTEGER NOT NULL REFERENCES accounts(id),
    uses INTEGER NOT NULL DEFAULT 0,
    max_uses INTEGER NOT NULL,
    expires INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS keypackages (
    id INTEGER PRIMARY KEY,
    device_key TEXT NOT NULL REFERENCES devices(key) ON DELETE CASCADE,
    body BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    channel TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    sender TEXT NOT NULL,
    body BLOB NOT NULL,
    sent_at INTEGER NOT NULL,
    PRIMARY KEY (channel, seq)
);
CREATE TABLE IF NOT EXISTS blobs (
    id TEXT PRIMARY KEY,
    community TEXT NOT NULL REFERENCES communities(id) ON DELETE CASCADE,
    channel TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    uploader INTEGER NOT NULL REFERENCES accounts(id),
    size INTEGER NOT NULL,
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS pairings (
    code_hash TEXT PRIMARY KEY,
    account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    expires INTEGER NOT NULL,
    device_key TEXT
);
CREATE TABLE IF NOT EXISTS welcomes (
    id INTEGER PRIMARY KEY,
    device_key TEXT NOT NULL REFERENCES devices(key) ON DELETE CASCADE,
    channel TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    body BLOB NOT NULL,
    created INTEGER NOT NULL
);
";

/// Columns added after the first release of the schema, added in place to existing databases.
const ADDED: &[(&str, &str, &str)] = &[
    ("communities", "description", "TEXT NOT NULL DEFAULT ''"),
    ("communities", "icon", "BLOB"),
    ("communities", "icon_type", "TEXT"),
    ("communities", "banner", "BLOB"),
    ("communities", "banner_type", "TEXT"),
    ("channels", "topic", "TEXT NOT NULL DEFAULT ''"),
    ("channels", "position", "INTEGER NOT NULL DEFAULT 0"),
    ("accounts", "status", "TEXT NOT NULL DEFAULT 'online'"),
    ("accounts", "status_text", "TEXT NOT NULL DEFAULT ''"),
    ("channels", "kind", "TEXT NOT NULL DEFAULT 'text'"),
    ("channels", "category", "TEXT"),
    ("channels", "parent", "TEXT"),
    ("channels", "post_rule", "TEXT NOT NULL DEFAULT 'everyone'"),
];

fn migrate(c: &Connection) -> rusqlite::Result<()> {
    for (table, column, kind) in ADDED {
        let has: bool = c.prepare(&format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1"))?.exists([column])?;
        if !has {
            c.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"))?;
        }
    }
    Ok(())
}

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let c = Connection::open(path)?;
        c.execute_batch(SCHEMA)?;
        migrate(&c)?;
        // The database holds account metadata: keep it readable only by the server's user.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(Self(Mutex::new(c)))
    }

    #[cfg(test)]
    pub fn memory() -> Self {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SCHEMA).unwrap();
        migrate(&c).unwrap();
        Self(Mutex::new(c))
    }

    /// Run something with the connection (kept short: one lock for the whole server).
    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        f(&mut self.0.lock().unwrap())
    }

    /// On a server with no accounts, make a one-time setup code for the first (admin) account.
    /// Returns it only when it was just made; afterwards only its hash exists.
    pub fn setup_code_if_new(&self) -> rusqlite::Result<Option<String>> {
        self.with(|c| {
            let accounts: i64 = c.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))?;
            if accounts > 0 {
                return Ok(None);
            }
            // A restart before anyone registered makes a new code (the old one is forgotten).
            c.execute("DELETE FROM setup", [])?;
            let code = new_code();
            c.execute("INSERT INTO setup (code_hash) VALUES (?1)", [hash(&code)])?;
            Ok(Some(code))
        })
    }
}
