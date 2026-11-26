//! Talking to a Beacon server over its web API (see PROTOCOL.md). Errors carry the server's own message,
//! never the session token.

use base64::Engine;
use serde_json::{json, Value};

pub struct Server {
    pub url: String,
    http: reqwest::Client,
    token: std::sync::Mutex<Option<String>>,
}

#[derive(Debug)]
pub struct ServerError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.status)
    }
}

pub type ServerResult<T> = Result<T, ServerError>;

pub fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn unb64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

impl Server {
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

    /// Listen on the server's live connection (`GET /v1/live`), calling `on_event` for each event, until
    /// it closes. The session token goes in the WebSocket protocol header, never in the URL.
    pub async fn listen(&self, mut on_event: impl FnMut(Value)) -> Result<(), String> {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::Message;
        let token = self.token.lock().unwrap().clone().ok_or("not signed in")?;
        let ws_url = if let Some(rest) = self.url.strip_prefix("https://") { format!("wss://{rest}/v1/live") } else { format!("ws://{}/v1/live", self.url.trim_start_matches("http://")) };
        let mut req = ws_url.into_client_request().map_err(|e| e.to_string())?;
        let proto = format!("lodestone-beacon.v1, {token}");
        req.headers_mut().insert("Sec-WebSocket-Protocol", proto.parse().map_err(|_| "bad token".to_string())?);
        let connector = if self.url.starts_with("https://") {
            use rustls_platform_verifier::ConfigVerifierExt;
            let config = rustls::ClientConfig::with_platform_verifier().map_err(|e| e.to_string())?;
            Some(tokio_tungstenite::Connector::Rustls(std::sync::Arc::new(config)))
        } else {
            Some(tokio_tungstenite::Connector::Plain)
        };
        let (ws, _) = tokio_tungstenite::connect_async_tls_with_config(req, None, true, connector).await.map_err(|e| format!("live connection: {}", e.to_string().replace(&token, "…")))?;
        let mut ws = ws;
        // A ping every 30 s keeps proxies (Cloudflare closes idle connections after ~100 s) from
        // dropping the connection, and notices a dead one.
        let mut ping = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tokio::select! {
                msg = ws.next() => match msg {
                    Some(Ok(Message::Text(t))) => {
                        if let Ok(v) = serde_json::from_str::<Value>(&t) {
                            on_event(v);
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {}
                },
                _ = ping.tick() => {
                    if futures_util::SinkExt::send(&mut ws, Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn call(&self, method: &str, path: &str, body: Option<Value>) -> ServerResult<Value> {
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
        let res = req.send().await.map_err(|e| ServerError { status: 0, message: if e.is_timeout() { "The server didn't answer in time".into() } else { "Can't reach the server".into() } })?;
        let status = res.status().as_u16();
        let v: Value = res.json().await.unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            let message = v["error"].as_str().unwrap_or("The server refused the request").to_string();
            return Err(ServerError { status, message });
        }
        Ok(v)
    }

    /// Upload a raw body (a community icon).
    pub async fn put_raw(&self, path: &str, bytes: Vec<u8>) -> ServerResult<Value> {
        self.send_raw(true, path, bytes).await
    }

    /// Upload a raw body with POST (an encrypted attachment).
    pub async fn post_raw(&self, path: &str, bytes: Vec<u8>) -> ServerResult<Value> {
        self.send_raw(false, path, bytes).await
    }

    async fn send_raw(&self, put: bool, path: &str, bytes: Vec<u8>) -> ServerResult<Value> {
        let url = format!("{}{path}", self.url);
        let mut req = if put { self.http.put(url) } else { self.http.post(url) }.body(bytes);
        if let Some(t) = self.token.lock().unwrap().clone() {
            req = req.bearer_auth(t);
        }
        let res = req.send().await.map_err(|_| ServerError { status: 0, message: "Can't reach the server".into() })?;
        let status = res.status().as_u16();
        let v: Value = res.json().await.unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            return Err(ServerError { status, message: v["error"].as_str().unwrap_or("The server refused the request").to_string() });
        }
        Ok(v)
    }

    /// Download a raw body with its content type (a community icon).
    pub async fn get_raw(&self, path: &str) -> ServerResult<(Vec<u8>, String)> {
        let mut req = self.http.get(format!("{}{path}", self.url));
        if let Some(t) = self.token.lock().unwrap().clone() {
            req = req.bearer_auth(t);
        }
        let res = req.send().await.map_err(|_| ServerError { status: 0, message: "Can't reach the server".into() })?;
        let status = res.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(ServerError { status, message: "No icon".into() });
        }
        let kind = res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        Ok((res.bytes().await.map_err(|_| ServerError { status: 0, message: "Download failed".into() })?.to_vec(), kind))
    }

    pub async fn get(&self, path: &str) -> ServerResult<Value> {
        self.call("GET", path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> ServerResult<Value> {
        self.call("POST", path, Some(body)).await
    }

    /// Send an MLS message into a channel. `Ok(None)` means the server refused a commit for being at an
    /// old epoch (someone else's commit got in first).
    pub async fn send(&self, channel: &str, kind: &str, epoch: u64, body: &[u8]) -> ServerResult<Option<i64>> {
        match self.post(&format!("/v1/channels/{channel}/messages"), json!({ "kind": kind, "epoch": epoch, "body": b64(body) })).await {
            Ok(v) => Ok(v["seq"].as_i64()),
            Err(e) if e.status == 409 => Ok(None),
            Err(e) => Err(e),
        }
    }
}
