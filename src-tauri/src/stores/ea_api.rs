//! Your EA library (owned games, installed or not) from EA's own web API.
//!
//! The EA app keeps its library cache encrypted, so Lodestone signs in once on EA's official sign-in
//! page (in a Lodestone window). That window keeps EA's sign-in cookies, and Lodestone asks EA for a
//! short-lived access token the same way EA's website does; when it runs out, a hidden window fetches
//! a new one without asking you again.

use crate::{net, state};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub const LOGIN_URL: &str = "https://www.ea.com/login";
/// EA's web SDK token endpoint: answers with JSON when you're signed in, an error when not.
pub const TOKEN_URL: &str = "https://accounts.ea.com/connect/auth?client_id=ORIGIN_JS_SDK&response_type=token&redirect_uri=nucleus:rest&prompt=none";
const API: &str = "https://service-aggregation-layer.juno.ea.com/graphql";

/// Runs on EA's pages in the sign-in window. On the token page it hands the token to Lodestone
/// (or says sign-in is needed) through a marker URL that never loads. After signing in on ea.com,
/// it goes back to the token page.
pub const LOGIN_SCRIPT: &str = r#"(function(){
  var TOKEN = 'https://accounts.ea.com/connect/auth?client_id=ORIGIN_JS_SDK&response_type=token&redirect_uri=nucleus:rest&prompt=none';
  function ready(f){ if (document.readyState !== 'loading') f(); else document.addEventListener('DOMContentLoaded', f); }
  var h = location.hostname;
  if (h === 'accounts.ea.com' && location.pathname.indexOf('/connect/auth') === 0 && location.search.indexOf('nucleus') >= 0) {
    ready(function(){ try {
      var j = JSON.parse(document.body.innerText);
      if (j.access_token) location.replace('https://lodestone.localhost/auth?ea=' + encodeURIComponent(j.access_token) + '&expires=' + (j.expires_in || 3600));
      else location.replace('https://lodestone.localhost/auth?ea_login=1');
    } catch (e) { location.replace('https://lodestone.localhost/auth?ea_login=1'); } });
    return;
  }
  if (h === 'www.ea.com') {
    try {
      if (location.pathname.indexOf('/login') >= 0) { localStorage.setItem('lodestone-ea', '1'); return; }
      if (localStorage.getItem('lodestone-ea')) { localStorage.removeItem('lodestone-ea'); ready(function(){ setTimeout(function(){ location.replace(TOKEN); }, 800); }); }
    } catch (e) {}
  }
})();"#;

#[derive(Serialize, Deserialize, Default, Clone)]
struct Token {
    access_token: String,
    expires_at: u64,
    name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Owned {
    /// Content id: what the EA app's links and installed games' installerdata.xml use.
    pub content_id: String,
    pub offer_id: String,
    pub name: String,
    pub cover: Option<String>,
    pub hero: Option<String>,
}

/// Write a file only the user can read (it holds a sign-in token).
pub fn write_private(p: &Path, text: &str) -> Result<(), String> {
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = p.with_extension("tmp");
    crate::os::write_private(&tmp, text.as_bytes()).map_err(|e| e.to_string())?;
    fs::rename(&tmp, p).map_err(|e| e.to_string())
}

fn token_file() -> PathBuf {
    crate::model::home().join(".config/lodestone/ea.json")
}

fn owned_file() -> PathBuf {
    state::data_dir().join("ea_owned.json")
}

fn load() -> Option<Token> {
    serde_json::from_str(&fs::read_to_string(token_file()).ok()?).ok()
}

/// Signed in (we've had a token at some point; the cookies in the sign-in window renew it).
pub fn account() -> Option<String> {
    load().map(|t| if t.name.is_empty() { "EA account".into() } else { t.name })
}

pub fn logout() {
    let _ = fs::remove_file(token_file());
    let _ = fs::remove_file(owned_file());
}

pub fn owned() -> Vec<Owned> {
    fs::read_to_string(owned_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

async fn gql(token: &str, query: &str, vars: Value) -> Result<Value, (u16, String)> {
    let r = net::client()
        .post(API)
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) QtWebEngine/5.15.2 Chrome/87.0 Safari/537.36")
        .json(&serde_json::json!({ "query": query, "variables": vars }))
        .send()
        .await
        .map_err(|e| (0, crate::net::err(e)))?;
    let code = r.status().as_u16();
    if !r.status().is_success() {
        return Err((code, format!("EA answered {code}")));
    }
    let v: Value = r.json().await.map_err(|e| (0, crate::net::err(e)))?;
    if let Some(e) = v["errors"].as_array().and_then(|a| a.first()) {
        let msg = e["message"].as_str().unwrap_or("query failed").to_string();
        let auth = msg.to_lowercase().contains("auth") || msg.to_lowercase().contains("token");
        return Err((if auth { 401 } else { 0 }, format!("EA: {msg}")));
    }
    Ok(v)
}

/// Save a token handed over by the sign-in window, with the account's display name.
pub async fn login(token: &str, expires_in: u64) -> Result<String, String> {
    let name = gql(token, "query{me{player{displayName}}}", serde_json::json!({}))
        .await
        .ok()
        .and_then(|v| v["data"]["me"]["player"]["displayName"].as_str().map(String::from))
        .unwrap_or_default();
    let t = Token { access_token: token.to_string(), expires_at: state::now() + expires_in.max(60), name: name.clone() };
    write_private(&token_file(), &serde_json::to_string(&t).map_err(|e| e.to_string())?)?;
    Ok(if name.is_empty() { "EA account".into() } else { name })
}

const ENTITLEMENTS: &str = r#"query getEntitlements($limit: Int, $next: String) {
  me { ownedGameProducts(locale: "DEFAULT", entitlementEnabled: true, storefronts: [EA], type: [DIGITAL_FULL_GAME, PACKAGED_FULL_GAME], platforms: [PC], paging: { limit: $limit, next: $next }) {
    next items { originOfferId product { baseItem { gameType } } } } } }"#;

const OFFERS: &str = r#"query getOffers($offerIds: [String!]!) {
  legacyOffers(offerIds: $offerIds, locale: "DEFAULT") { offerId: id contentId }
  gameProducts(offerIds: $offerIds, locale: "DEFAULT") { items { id originOfferId baseItem { title packArt { largestImage { path } } keyArt { largestImage { path } } } } } }"#;

fn parse_offers(v: &Value) -> Vec<Owned> {
    let products: Vec<&Value> = v["data"]["gameProducts"]["items"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let by_offer = |id: &str| products.iter().find(|p| p["originOfferId"].as_str() == Some(id) || p["id"].as_str() == Some(id)).copied();
    v["data"]["legacyOffers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            let offer = o["offerId"].as_str()?;
            let content = o["contentId"].as_str().filter(|c| !c.is_empty())?;
            let p = by_offer(offer).or_else(|| by_offer(content))?;
            let img = |k: &str| p["baseItem"][k]["largestImage"]["path"].as_str().filter(|s| !s.is_empty()).map(String::from);
            Some(Owned {
                content_id: content.to_string(),
                offer_id: offer.to_string(),
                name: p["baseItem"]["title"].as_str()?.trim().to_string(),
                cover: img("packArt"),
                hero: img("keyArt"),
            })
        })
        .collect()
}

async fn fetch_library(token: &str) -> Result<Vec<Owned>, (u16, String)> {
    let mut offers = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let v = gql(token, ENTITLEMENTS, serde_json::json!({ "limit": 100, "next": next })).await?;
        let p = &v["data"]["me"]["ownedGameProducts"];
        for it in p["items"].as_array().into_iter().flatten() {
            if it["product"]["baseItem"]["gameType"] == "BASE_GAME" {
                offers.extend(it["originOfferId"].as_str().map(String::from));
            }
        }
        next = p["next"].as_str().map(String::from);
        if next.is_none() {
            break;
        }
    }
    let mut out = Vec::new();
    for chunk in offers.chunks(100) {
        let v = gql(token, OFFERS, serde_json::json!({ "offerIds": chunk })).await?;
        out.extend(parse_offers(&v));
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.content_id == b.content_id);
    Ok(out)
}

/// Fetch the owned-games list with the current token. Err("expired") means a new token is needed.
pub async fn refresh() -> Result<usize, String> {
    let t = load().ok_or("not signed in to EA")?;
    if t.expires_at <= state::now() + 60 {
        return Err("expired".into());
    }
    let list = match fetch_library(&t.access_token).await {
        Ok(l) => l,
        Err((401 | 403, _)) => return Err("expired".into()),
        Err((_, e)) => return Err(e),
    };
    let _ = fs::create_dir_all(state::data_dir());
    fs::write(owned_file(), serde_json::to_string(&list).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(list.len())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn reads_ea_offers() {
        let v = json!({"data": {
            "legacyOffers": [{"offerId": "Origin.OFR.50.0004121", "contentId": "1035052"}, {"offerId": "x", "contentId": ""}],
            "gameProducts": {"items": [{"id": "p1", "originOfferId": "Origin.OFR.50.0004121", "baseItem": {"title": "Mass Effect Legendary Edition", "packArt": {"largestImage": {"path": "https://img/pack.jpg"}}, "keyArt": null}}]}
        }});
        let o = super::parse_offers(&v);
        assert_eq!(o.len(), 1);
        assert_eq!((o[0].content_id.as_str(), o[0].name.as_str(), o[0].cover.as_deref(), o[0].hero.as_deref()), ("1035052", "Mass Effect Legendary Edition", Some("https://img/pack.jpg"), None));
    }
}
