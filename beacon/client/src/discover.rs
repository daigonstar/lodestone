//! Finding a server from what someone typed: a bare domain ("beacon.example.com"), or a full address.
//! See "Finding a server by its domain" in PROTOCOL.md.

use std::time::Duration;

/// Plain http is only allowed for this PC (testing).
fn is_local(authority: &str) -> bool {
    let server = authority.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map(|(h, _)| h).unwrap_or(authority);
    matches!(server, "127.0.0.1" | "localhost" | "[::1]")
}

/// The authority (server and port) of an address, lower-cased, without a default :443.
fn authority(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let a = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    a.strip_suffix(":443").unwrap_or(a)
}

/// Turn what was typed into a base address: `https://` is added to a bare domain; paths aren't allowed.
pub fn normalize(input: &str) -> Result<String, String> {
    let t = input.trim().trim_end_matches('/');
    if t.is_empty() {
        return Err("Enter the server's domain, like beacon.example.com".into());
    }
    let url = if t.contains("://") {
        t.to_string()
    } else if is_local(t) {
        format!("http://{t}")
    } else {
        format!("https://{t}")
    };
    let (scheme, rest) = url.split_once("://").unwrap_or_default();
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', ' ']) {
        return Err("Enter just the server's domain, like beacon.example.com".into());
    }
    match scheme {
        "https" => Ok(format!("https://{}", rest.to_ascii_lowercase())),
        "http" if is_local(rest) => Ok(format!("http://{}", rest.to_ascii_lowercase())),
        _ => Err("Servers must use https:// (plain http only for this PC)".into()),
    }
}

/// Ask the server for its exact address. It must be a Beacon server, and its address must be on the same
/// domain that was typed (so a domain can't quietly send you somewhere else).
pub async fn discover(input: &str) -> Result<String, String> {
    let base = normalize(input)?;
    let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?;
    let res = http.get(format!("{base}/.well-known/lodestone-beacon")).send().await.map_err(|_| format!("Can't reach {base}"))?;
    if !res.status().is_success() {
        return Err(format!("{base} doesn't look like a Lodestone Beacon server"));
    }
    let v: serde_json::Value = res.json().await.map_err(|_| format!("{base} doesn't look like a Lodestone Beacon server"))?;
    if v["protocol"] != "lodestone-beacon/v1" {
        return Err(format!("{base} runs a Beacon version this app doesn't support"));
    }
    let public = normalize(v["public_url"].as_str().unwrap_or_default()).map_err(|_| format!("{base} reports an address Beacon can't use; check its BEACON_PUBLIC_URL"))?;
    if !authority(&public).eq_ignore_ascii_case(authority(&base)) {
        return Err(format!("{base} says its address is {public}. Use that, or fix BEACON_PUBLIC_URL on the server."));
    }
    Ok(public)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_what_people_type() {
        assert_eq!(normalize("Beacon.Example.Invalid").unwrap(), "https://beacon.example.invalid");
        assert_eq!(normalize(" https://beacon.example.invalid/ ").unwrap(), "https://beacon.example.invalid");
        assert_eq!(normalize("127.0.0.1:8080").unwrap(), "http://127.0.0.1:8080");
        assert_eq!(normalize("localhost:8080").unwrap(), "http://localhost:8080");
        assert!(normalize("http://beacon.example.invalid").is_err(), "plain http only for this PC");
        assert!(normalize("beacon.example.invalid/some/path").is_err());
        assert!(normalize("user@beacon.example.invalid").is_err());
        assert!(normalize("ftp://beacon.example.invalid").is_err());
        assert!(normalize("").is_err());
        assert_eq!(authority("https://a.invalid:443"), "a.invalid");
        assert_eq!(authority("http://127.0.0.1:8080/x"), "127.0.0.1:8080");
    }
}
