//! Talking to a Comms host over its web API (see PROTOCOL.md). Errors carry the host's own message,
//! never the session token.

use base64::Engine;
use serde_json::{json, Value};

pub struct Host {
    pub url: String,
    http: reqwest::Client,
    token: std::sync::Mutex<Option<String>>,
}

#[derive(Debug)]
pub struct HostError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.status)
    }
}

pub type HostResult<T> = Result<T, HostError>;

pub fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn unb64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

impl Host {
    pub fn new(url: &str) -> Self {
        let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().expect("HTTP client");
        Self { url: url.trim_end_matches('/').to_string(), http, token: Default::default() }
    }

    pub fn set_token(&self, t: Option<String>) {
        *self.token.lock().unwrap() = t;
    }

    pub fn signed_in(&self) -> bool {
        self.token.lock().unwrap().is_some()
    }

    pub async fn call(&self, method: &str, path: &str, body: Option<Value>) -> HostResult<Value> {
        let url = format!("{}{path}", self.url);
        let mut req = match method {
            "GET" => self.http.get(&url),
            "DELETE" => self.http.delete(&url),
            "PATCH" => self.http.patch(&url),
            "PUT" => self.http.put(&url),
            _ => self.http.post(&url),
        };
        if let Some(t) = self.token.lock().unwrap().clone() {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        // Network errors are reduced to their kind, so a URL with secrets in it can't reach logs or the UI.
        let res = req.send().await.map_err(|e| HostError { status: 0, message: if e.is_timeout() { "The host didn't answer in time".into() } else { "Can't reach the host".into() } })?;
        let status = res.status().as_u16();
        let v: Value = res.json().await.unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            let message = v["error"].as_str().unwrap_or("The host refused the request").to_string();
            return Err(HostError { status, message });
        }
        Ok(v)
    }

    /// Upload a raw body (a server icon).
    pub async fn put_raw(&self, path: &str, bytes: Vec<u8>) -> HostResult<Value> {
        let mut req = self.http.put(format!("{}{path}", self.url)).body(bytes);
        if let Some(t) = self.token.lock().unwrap().clone() {
            req = req.bearer_auth(t);
        }
        let res = req.send().await.map_err(|_| HostError { status: 0, message: "Can't reach the host".into() })?;
        let status = res.status().as_u16();
        let v: Value = res.json().await.unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            return Err(HostError { status, message: v["error"].as_str().unwrap_or("The host refused the request").to_string() });
        }
        Ok(v)
    }

    /// Download a raw body with its content type (a server icon).
    pub async fn get_raw(&self, path: &str) -> HostResult<(Vec<u8>, String)> {
        let mut req = self.http.get(format!("{}{path}", self.url));
        if let Some(t) = self.token.lock().unwrap().clone() {
            req = req.bearer_auth(t);
        }
        let res = req.send().await.map_err(|_| HostError { status: 0, message: "Can't reach the host".into() })?;
        let status = res.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(HostError { status, message: "No icon".into() });
        }
        let kind = res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        Ok((res.bytes().await.map_err(|_| HostError { status: 0, message: "Download failed".into() })?.to_vec(), kind))
    }

    pub async fn get(&self, path: &str) -> HostResult<Value> {
        self.call("GET", path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> HostResult<Value> {
        self.call("POST", path, Some(body)).await
    }

    /// Send an MLS message into a channel. `Ok(None)` means the host refused a commit for being at an
    /// old epoch (someone else's commit got in first).
    pub async fn send(&self, channel: &str, kind: &str, epoch: u64, body: &[u8]) -> HostResult<Option<i64>> {
        match self.post(&format!("/v1/channels/{channel}/messages"), json!({ "kind": kind, "epoch": epoch, "body": b64(body) })).await {
            Ok(v) => Ok(v["seq"].as_i64()),
            Err(e) if e.status == 409 => Ok(None),
            Err(e) => Err(e),
        }
    }
}
