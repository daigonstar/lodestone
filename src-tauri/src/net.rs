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

pub fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
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
