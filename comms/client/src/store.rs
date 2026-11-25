//! The client's local database: this device's key, openmls's group state, and the messages you can read.
//! It lives in the client's own data folder, readable only by you (folder 700, file 600). Decrypted
//! messages are kept here so they stay readable; encrypting this file at rest is planned.

use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

pub struct Store(Mutex<Connection>);

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS settings (k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS mls (k BLOB PRIMARY KEY, v BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS channels (
    id TEXT PRIMARY KEY,
    space TEXT NOT NULL,
    name TEXT NOT NULL,
    joined_epoch INTEGER NOT NULL,
    last_seq INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS messages (
    channel TEXT NOT NULL,
    seq INTEGER NOT NULL,
    sender TEXT NOT NULL,
    sender_device TEXT NOT NULL,
    body TEXT NOT NULL,
    sent_at INTEGER NOT NULL,
    PRIMARY KEY (channel, seq)
);
";

/// A message you can read.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Message {
    pub channel: String,
    pub seq: i64,
    pub sender: String,
    pub sender_device: String,
    pub body: String,
    pub sent_at: i64,
}

impl Store {
    pub fn open(dir: &Path) -> rusqlite::Result<Self> {
        let _ = std::fs::create_dir_all(dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let path = dir.join("comms.db");
        let c = Connection::open(&path)?;
        c.execute_batch(SCHEMA)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(Self(Mutex::new(c)))
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        f(&mut self.0.lock().unwrap())
    }

    pub fn get(&self, k: &str) -> Option<String> {
        self.with(|c| c.query_row("SELECT v FROM settings WHERE k = ?1", [k], |r| r.get(0)).optional()).ok().flatten()
    }

    pub fn set(&self, k: &str, v: &str) -> rusqlite::Result<()> {
        self.with(|c| c.execute("INSERT INTO settings (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v", params![k, v]).map(|_| ()))
    }

    /// openmls's key-value state, loaded once at start.
    pub fn load_mls(&self) -> rusqlite::Result<HashMap<Vec<u8>, Vec<u8>>> {
        self.with(|c| c.prepare("SELECT k, v FROM mls")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
    }

    /// Save openmls's state: written as a whole, in one transaction, after every change.
    pub fn save_mls(&self, values: &HashMap<Vec<u8>, Vec<u8>>) -> rusqlite::Result<()> {
        self.with(|c| {
            let tx = c.transaction()?;
            tx.execute("DELETE FROM mls", [])?;
            {
                let mut ins = tx.prepare("INSERT INTO mls (k, v) VALUES (?1, ?2)")?;
                for (k, v) in values {
                    ins.execute(params![k, v])?;
                }
            }
            tx.commit()
        })
    }

    pub fn add_channel(&self, id: &str, space: &str, name: &str, joined_epoch: u64) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO channels (id, space, name, joined_epoch) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(id) DO UPDATE SET name = excluded.name",
                params![id, space, name, joined_epoch as i64],
            )
            .map(|_| ())
        })
    }

    /// Channels you're in: (id, space, name, joined epoch, last seen sequence number).
    pub fn channels(&self) -> rusqlite::Result<Vec<(String, String, String, u64, i64)>> {
        self.with(|c| {
            c.prepare("SELECT id, space, name, joined_epoch, last_seq FROM channels ORDER BY space, name")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? as u64, r.get(4)?)))?
                .collect()
        })
    }

    pub fn last_seq(&self, channel: &str) -> i64 {
        self.with(|c| c.query_row("SELECT last_seq FROM channels WHERE id = ?1", [channel], |r| r.get(0)).optional()).ok().flatten().unwrap_or(0)
    }

    pub fn set_last_seq(&self, channel: &str, seq: i64) -> rusqlite::Result<()> {
        self.with(|c| c.execute("UPDATE channels SET last_seq = MAX(last_seq, ?2) WHERE id = ?1", params![channel, seq]).map(|_| ()))
    }

    pub fn add_message(&self, m: &Message) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO messages (channel, seq, sender, sender_device, body, sent_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![m.channel, m.seq, m.sender, m.sender_device, m.body, m.sent_at],
            )
            .map(|_| ())
        })
    }

    pub fn messages(&self, channel: &str, limit: i64) -> rusqlite::Result<Vec<Message>> {
        self.with(|c| {
            let mut v: Vec<Message> = c
                .prepare("SELECT channel, seq, sender, sender_device, body, sent_at FROM messages WHERE channel = ?1 ORDER BY seq DESC LIMIT ?2")?
                .query_map(params![channel, limit], |r| {
                    Ok(Message { channel: r.get(0)?, seq: r.get(1)?, sender: r.get(2)?, sender_device: r.get(3)?, body: r.get(4)?, sent_at: r.get(5)? })
                })?
                .collect::<Result<_, _>>()?;
            v.reverse();
            Ok(v)
        })
    }
}
