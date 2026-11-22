use std::sync::OnceLock;
use std::time::Duration;

pub fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            // Modrinth asks API clients for a descriptive user agent.
            .user_agent(concat!("lodestone-launcher/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .build()
            .expect("http client")
    })
}

/// An error as text, safe to show or log. Network errors repeat the address they were for, and some
/// addresses carry a secret in their query (the Steam Web API key goes in `?key=`), so the query part
/// of every web address in the message is replaced with "?…".
pub fn err(e: impl std::fmt::Display) -> String {
    hide_queries(&e.to_string())
}

pub fn hide_queries(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("http") {
        let (before, from) = rest.split_at(i);
        out.push_str(before);
        // The address runs until whitespace or a closing bracket/quote.
        let end = from.find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '>')).unwrap_or(from.len());
        let url = &from[..end];
        match url.find('?') {
            Some(q) if url.starts_with("http://") || url.starts_with("https://") => {
                out.push_str(&url[..q]);
                out.push_str("?…");
            }
            _ => out.push_str(url),
        }
        rest = &from[end..];
    }
    out.push_str(rest);
    out
}

/// Check a downloaded GitHub release file against the SHA-256 checksum GitHub publishes for it (the
/// asset's `digest`, "sha256:<hex>"), so a corrupted or swapped download is never installed or run.
/// Releases uploaded before GitHub kept checksums have none; those are accepted as before.
pub fn verify_release_file(asset: &serde_json::Value, bytes: &[u8]) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let Some(want) = asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")) else { return Ok(()) };
    let got: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    if got.eq_ignore_ascii_case(want) {
        Ok(())
    } else {
        Err(format!("{} didn't match GitHub's published checksum, so it wasn't installed", asset["name"].as_str().unwrap_or("The download")))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn checks_release_files_against_githubs_checksum() {
        let asset = |d: &str| serde_json::json!({ "name": "tool", "digest": d });
        // sha256("hello")
        let good = "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert!(super::verify_release_file(&asset(good), b"hello").is_ok());
        assert!(super::verify_release_file(&asset(good), b"hellO").is_err());
        assert!(super::verify_release_file(&serde_json::json!({ "name": "old" }), b"anything").is_ok());
    }

    #[test]
    fn hides_keys_in_error_messages() {
        let e = "error sending request for url (https://api.steampowered.com/ISteamUser/GetFriendList/v1/?key=ABC123&steamid=7656)";
        assert_eq!(super::hide_queries(e), "error sending request for url (https://api.steampowered.com/ISteamUser/GetFriendList/v1/?…)");
        assert_eq!(super::hide_queries("no address here"), "no address here");
        assert_eq!(super::hide_queries("see https://page.invalid/a and http://x.invalid/a?b=1 now"), "see https://page.invalid/a and http://x.invalid/a?… now");
    }
}

pub async fn get_json(url: &str, headers: &[(&str, &str)]) -> Result<serde_json::Value, String> {
    let mut rb = client().get(url);
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    let r = rb.send().await.map_err(err)?;
    let status = r.status();
    if !status.is_success() {
        let body = r.text().await.unwrap_or_default();
        return Err(format!("{status} from {}: {}", url.split('?').next().unwrap_or(url), body.chars().take(200).collect::<String>()));
    }
    r.json().await.map_err(err)
}
