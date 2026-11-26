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
    community TEXT NOT NULL,
    name TEXT NOT NULL,
    joined_epoch INTEGER NOT NULL,
    last_seq INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS profiles (
    user TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    color TEXT NOT NULL,
    avatar TEXT,
    sent_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS reactions (
    channel TEXT NOT NULL,
    target INTEGER NOT NULL,
    emoji TEXT NOT NULL,
    user TEXT NOT NULL,
    PRIMARY KEY (channel, target, emoji, user)
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

/// Columns added later, added in place to existing databases.
fn migrate(c: &Connection) -> rusqlite::Result<()> {
    let has: bool = c.prepare("SELECT 1 FROM pragma_table_info('messages') WHERE name = 'attachment'")?.exists([])?;
    if !has {
        c.execute_batch("ALTER TABLE messages ADD COLUMN attachment TEXT")?;
    }
    let has: bool = c.prepare("SELECT 1 FROM pragma_table_info('messages') WHERE name = 'thread'")?.exists([])?;
    if !has {
        c.execute_batch("ALTER TABLE messages ADD COLUMN thread INTEGER; ALTER TABLE messages ADD COLUMN title TEXT")?;
    }
    Ok(())
}

/// A message you can read.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Message {
    pub channel: String,
    pub seq: i64,
    pub sender: String,
    pub sender_device: String,
    pub body: String,
    pub sent_at: i64,
    /// An attached file: id, name, mime, size (the key stays in the database, never in this).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment: Option<serde_json::Value>,
    /// In a forum: the post this is a reply to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<i64>,
    /// In a forum: this message starts a post with this title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Store {
    pub fn open(dir: &Path) -> rusqlite::Result<Self> {
        let _ = std::fs::create_dir_all(dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let path = dir.join("beacon.db");
        let c = Connection::open(&path)?;
        c.execute_batch(SCHEMA)?;
        migrate(&c)?;
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

    pub fn add_channel(&self, id: &str, community: &str, name: &str, joined_epoch: u64) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO channels (id, community, name, joined_epoch) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(id) DO UPDATE SET name = excluded.name",
                params![id, community, name, joined_epoch as i64],
            )
            .map(|_| ())
        })
    }

    /// Channels you're in: (id, community, name, joined epoch, last seen sequence number).
    pub fn channels(&self) -> rusqlite::Result<Vec<(String, String, String, u64, i64)>> {
        self.with(|c| {
            c.prepare("SELECT id, community, name, joined_epoch, last_seq FROM channels ORDER BY community, name")?
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
                "INSERT OR IGNORE INTO messages (channel, seq, sender, sender_device, body, sent_at, attachment, thread, title) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![m.channel, m.seq, m.sender, m.sender_device, m.body, m.sent_at, m.attachment.as_ref().map(|a| a.to_string()), m.thread, m.title],
            )
            .map(|_| ())
        })
    }

    pub fn set_reaction(&self, channel: &str, target: i64, emoji: &str, user: &str, on: bool) -> rusqlite::Result<()> {
        self.with(|c| {
            if on {
                c.execute("INSERT OR IGNORE INTO reactions (channel, target, emoji, user) VALUES (?1, ?2, ?3, ?4)", params![channel, target, emoji, user])
            } else {
                c.execute("DELETE FROM reactions WHERE channel = ?1 AND target = ?2 AND emoji = ?3 AND user = ?4", params![channel, target, emoji, user])
            }
            .map(|_| ())
        })
    }

    /// Save someone's profile if it's newer than the one we have.
    pub fn set_profile(&self, user: &str, name: &str, color: &str, avatar: Option<&serde_json::Value>, sent_at: i64) -> rusqlite::Result<bool> {
        self.with(|c| {
            let n = c.execute(
                "INSERT INTO profiles (user, name, color, avatar, sent_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(user) DO UPDATE SET name = excluded.name, color = excluded.color, avatar = excluded.avatar, sent_at = excluded.sent_at
                 WHERE excluded.sent_at >= profiles.sent_at",
                params![user, name, color, avatar.map(|a| a.to_string()), sent_at],
            )?;
            Ok(n > 0)
        })
    }

    /// Everyone's profiles: (user, display name, colour, avatar reference with its key).
    pub fn profiles(&self) -> rusqlite::Result<Vec<(String, String, String, Option<serde_json::Value>)>> {
        self.with(|c| {
            c.prepare("SELECT user, name, color, avatar FROM profiles ORDER BY user")?
                .query_map([], |r| {
                    let a: Option<String> = r.get(3)?;
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, a.and_then(|x| serde_json::from_str(&x).ok())))
                })?
                .collect()
        })
    }

    /// A channel's reactions: (message seq, emoji, users), in the order they were first added.
    pub fn reactions(&self, channel: &str) -> rusqlite::Result<Vec<(i64, String, Vec<String>)>> {
        let rows: Vec<(i64, String, String)> = self.with(|c| {
            c.prepare("SELECT target, emoji, user FROM reactions WHERE channel = ?1 ORDER BY target, rowid")?.query_map([channel], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect()
        })?;
        let mut out: Vec<(i64, String, Vec<String>)> = Vec::new();
        for (t, e, u) in rows {
            match out.iter_mut().find(|x| x.0 == t && x.1 == e) {
                Some(x) => x.2.push(u),
                None => out.push((t, e, vec![u])),
            }
        }
        Ok(out)
    }

    /// A message's attachment, with its key (for decrypting it on this device).
    pub fn attachment(&self, channel: &str, seq: i64) -> Option<serde_json::Value> {
        self.with(|c| c.query_row("SELECT attachment FROM messages WHERE channel = ?1 AND seq = ?2", params![channel, seq], |r| r.get::<_, Option<String>>(0)).optional())
            .ok()
            .flatten()
            .flatten()
            .and_then(|a| serde_json::from_str(&a).ok())
    }

    /// Your recent messages (and their reactions) for copying to a new device of yours: the last `per_channel`
    /// messages of every channel, attachments with their keys. Stops adding once `max_bytes` is reached.
    /// Profiles and MLS state are not included.
    pub fn export_history(&self, per_channel: i64, max_bytes: usize) -> rusqlite::Result<serde_json::Value> {
        let mut size = 0usize;
        let mut messages = Vec::new();
        let mut reactions = Vec::new();
        self.with(|c| {
            let channels: Vec<String> = c.prepare("SELECT id FROM channels ORDER BY id")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            for ch in channels {
                let rows: Vec<(i64, String, String, String, i64, Option<String>, Option<i64>, Option<String>)> = c
                    .prepare("SELECT seq, sender, sender_device, body, sent_at, attachment, thread, title FROM messages WHERE channel = ?1 ORDER BY seq DESC LIMIT ?2")?
                    .query_map(params![ch, per_channel], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))?
                    .collect::<Result<_, _>>()?;
                let Some(oldest) = rows.last().map(|r| r.0) else { continue };
                for (seq, sender, device, body, sent_at, att, thread, title) in rows {
                    size += body.len() + att.as_ref().map_or(0, |a| a.len()) + 160;
                    if size > max_bytes {
                        break;
                    }
                    messages.push(serde_json::json!({ "c": ch, "s": seq, "u": sender, "d": device, "b": body, "t": sent_at, "a": att.and_then(|a| serde_json::from_str::<serde_json::Value>(&a).ok()), "th": thread, "ti": title }));
                }
                let rs: Vec<(i64, String, String)> = c
                    .prepare("SELECT target, emoji, user FROM reactions WHERE channel = ?1 AND target >= ?2")?
                    .query_map(params![ch, oldest], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<Result<_, _>>()?;
                for (t, e, u) in rs {
                    size += e.len() + u.len() + 60;
                    if size > max_bytes {
                        break;
                    }
                    reactions.push(serde_json::json!({ "c": ch, "t": t, "e": e, "u": u }));
                }
            }
            Ok(())
        })?;
        Ok(serde_json::json!({ "v": 1, "messages": messages, "reactions": reactions }))
    }

    /// Add history from `export_history`. Messages you already have are left alone. Returns how many were added.
    pub fn import_history(&self, h: &serde_json::Value) -> rusqlite::Result<usize> {
        if h["v"] != 1 {
            return Ok(0);
        }
        self.with(|c| {
            let tx = c.transaction()?;
            let mut added = 0;
            for m in h["messages"].as_array().into_iter().flatten() {
                let (Some(ch), Some(seq), Some(u), Some(d), Some(b), Some(t)) = (m["c"].as_str(), m["s"].as_i64(), m["u"].as_str(), m["d"].as_str(), m["b"].as_str(), m["t"].as_i64()) else { continue };
                let att = if m["a"].is_null() { None } else { Some(m["a"].to_string()) };
                added += tx.execute(
                    "INSERT OR IGNORE INTO messages (channel, seq, sender, sender_device, body, sent_at, attachment, thread, title) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![ch, seq, u, d, b, t, att, m["th"].as_i64(), m["ti"].as_str()],
                )?;
            }
            for r in h["reactions"].as_array().into_iter().flatten() {
                let (Some(ch), Some(t), Some(e), Some(u)) = (r["c"].as_str(), r["t"].as_i64(), r["e"].as_str(), r["u"].as_str()) else { continue };
                tx.execute("INSERT OR IGNORE INTO reactions (channel, target, emoji, user) VALUES (?1, ?2, ?3, ?4)", params![ch, t, e, u])?;
            }
            tx.commit()?;
            Ok(added)
        })
    }

    pub fn messages(&self, channel: &str, limit: i64) -> rusqlite::Result<Vec<Message>> {
        self.with(|c| {
            let mut v: Vec<Message> = c
                .prepare("SELECT channel, seq, sender, sender_device, body, sent_at, attachment, thread, title FROM messages WHERE channel = ?1 ORDER BY seq DESC LIMIT ?2")?
                .query_map(params![channel, limit], |r| {
                    let att: Option<String> = r.get(6)?;
                    Ok(Message {
                        channel: r.get(0)?,
                        seq: r.get(1)?,
                        sender: r.get(2)?,
                        sender_device: r.get(3)?,
                        body: r.get(4)?,
                        sent_at: r.get(5)?,
                        thread: r.get(7)?,
                        title: r.get(8)?,
                        // Shown without its key.
                        attachment: att.and_then(|a| serde_json::from_str::<serde_json::Value>(&a).ok()).map(|mut a| {
                            if let Some(o) = a.as_object_mut() {
                                o.remove("key");
                                o.remove("nonce");
                            }
                            a
                        }),
                    })
                })?
                .collect::<Result<_, _>>()?;
            v.reverse();
            Ok(v)
        })
    }
}
