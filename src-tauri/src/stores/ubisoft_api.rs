//! Your Ubisoft library (owned games, installed or not) from Ubisoft's own web API.
//!
//! Ubisoft Connect keeps its library cache encrypted, so Lodestone signs in once on Ubisoft's official
//! sign-in page (in a Lodestone window) and keeps the session it gets, like the Ubisoft Connect website
//! does. The session renews itself with Ubisoft's "remember me" ticket.

use crate::{net, state};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

/// Ubisoft Connect website's app id (the same one the Ubisoft Connect site and other launchers use).
const APP_ID: &str = "82b650c0-6cb3-40c0-9f41-25a53b62b206";
const GENOME_ID: &str = "42d07c95-9914-4450-8b38-267c4e462b21";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

pub fn login_url() -> String {
    format!("https://connect.ubisoft.com/login?appId={APP_ID}&genomeId={GENOME_ID}&lang=en-US&nextUrl=https:%2F%2Fconnect.ubisoft.com%2Fready")
}

/// Runs on Ubisoft's pages in the sign-in window: once signed in, the site keeps the session in its
/// local storage; hand it to Lodestone through a marker URL that never loads.
pub const LOGIN_SCRIPT: &str = r#"(function(){
  if (location.hostname !== 'connect.ubisoft.com') return;
  var p = location.pathname;
  if (!p.startsWith('/ready') && !p.startsWith('/change_domain')) return;
  var tries = 0;
  function go(){
    var d = localStorage.getItem('PRODloginData');
    if (!d) { if (++tries < 40) return setTimeout(go, 250); if (p.startsWith('/ready')) location.replace('https://connect.ubisoft.com/change_domain/'); return; }
    var all = '[' + [d, localStorage.getItem('PRODrememberMe'), localStorage.getItem('PRODlastProfile')].map(function(x){ return x || 'null'; }).join(',') + ']';
    location.replace('https://lodestone.localhost/auth?ubisoft=' + encodeURIComponent(all));
  }
  if (document.readyState !== 'loading') go(); else document.addEventListener('DOMContentLoaded', go);
})();"#;

#[derive(Serialize, Deserialize, Default, Clone)]
struct Session {
    ticket: String,
    session_id: String,
    user_id: String,
    name: String,
    remember: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Owned {
    pub space_id: String,
    pub name: String,
    pub cover: Option<String>,
    pub hero: Option<String>,
}

fn session_file() -> PathBuf {
    crate::model::home().join(".config/lodestone/ubisoft.json")
}

fn owned_file() -> PathBuf {
    state::data_dir().join("ubisoft_owned.json")
}

fn load() -> Option<Session> {
    serde_json::from_str(&fs::read_to_string(session_file()).ok()?).ok()
}

fn save(s: &Session) -> Result<(), String> {
    super::ea_api::write_private(&session_file(), &serde_json::to_string(s).map_err(|e| e.to_string())?)
}

pub fn account() -> Option<String> {
    load().map(|s| s.name).filter(|n| !n.is_empty())
}

pub fn logout() {
    let _ = fs::remove_file(session_file());
    let _ = fs::remove_file(owned_file());
}

pub fn owned() -> Vec<Owned> {
    fs::read_to_string(owned_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// What the sign-in page handed over: a JSON array of the site's stored login objects.
fn session_from(data: &str) -> Option<Session> {
    let arr: Vec<Value> = serde_json::from_str(data).ok()?;
    let find = |k: &str| arr.iter().find_map(|o| o[k].as_str().map(String::from));
    Some(Session {
        ticket: find("ticket")?,
        session_id: find("sessionId")?,
        user_id: find("userId").unwrap_or_default(),
        name: find("nameOnPlatform").unwrap_or_default(),
        remember: find("rememberMeTicket"),
    })
}

pub async fn login(data: &str) -> Result<String, String> {
    let s = session_from(data).ok_or("Ubisoft sign-in didn't finish; try again")?;
    save(&s)?;
    refresh().await?;
    Ok(if s.name.is_empty() { "Ubisoft account".into() } else { s.name })
}

fn headers(rb: reqwest::RequestBuilder, s: &Session) -> reqwest::RequestBuilder {
    rb.header("Ubi-AppId", APP_ID)
        .header("Ubi-SessionId", &s.session_id)
        .header("Authorization", format!("Ubi_v1 t={}", s.ticket))
        .header("User-Agent", UA)
        .header("Origin", "https://connect.ubisoft.com")
        .header("Referer", "https://connect.ubisoft.com/")
}

/// A fresh session from the "remember me" ticket (sessions last a few hours).
async fn renew(s: &Session) -> Result<Session, String> {
    let rm = s.remember.as_deref().ok_or("Ubisoft session expired: sign in again in Accounts")?;
    let r = net::client()
        .post("https://public-ubiservices.ubi.com/v3/profiles/sessions")
        .header("Ubi-AppId", APP_ID)
        .header("Authorization", format!("rm_v1 t={rm}"))
        .header("User-Agent", UA)
        .header("Origin", "https://connect.ubisoft.com")
        .header("Referer", "https://connect.ubisoft.com/")
        .json(&serde_json::json!({ "rememberMe": true }))
        .send()
        .await
        .map_err(net::err)?;
    if !r.status().is_success() {
        return Err("Ubisoft session expired: sign in again in Accounts".into());
    }
    let v: Value = r.json().await.map_err(net::err)?;
    let mut n = s.clone();
    n.ticket = v["ticket"].as_str().ok_or("Ubisoft didn't renew the session")?.to_string();
    n.session_id = v["sessionId"].as_str().unwrap_or(&s.session_id).to_string();
    if let Some(rm) = v["rememberMeTicket"].as_str() {
        n.remember = Some(rm.to_string());
    }
    if let Some(name) = v["nameOnPlatform"].as_str() {
        n.name = name.to_string();
    }
    save(&n)?;
    Ok(n)
}

const QUERY: &str = "query AllGames { viewer { id ownedGames: games(filterBy: {isOwned: true}) { totalCount nodes { id spaceId name coverUrl backgroundUrl viewer { meta { id ownedPlatformGroups { id name type } } } } } } }";

async fn query(s: &Session) -> Result<Value, (u16, String)> {
    let r = headers(net::client().post("https://public-ubiservices.ubi.com/v1/profiles/me/uplay/graphql"), s)
        .json(&serde_json::json!({ "operationName": "AllGames", "variables": { "owned": true }, "query": QUERY }))
        .send()
        .await
        .map_err(|e| (0, e.to_string()))?;
    let code = r.status().as_u16();
    if !r.status().is_success() {
        return Err((code, format!("Ubisoft answered {code}")));
    }
    r.json().await.map_err(|e| (0, e.to_string()))
}

/// PC games only (the list also has console purchases).
fn parse(v: &Value) -> Vec<Owned> {
    v["data"]["viewer"]["ownedGames"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|g| {
            let groups = &g["viewer"]["meta"]["ownedPlatformGroups"];
            match groups.as_array() {
                Some(gs) if !gs.is_empty() => gs.iter().flat_map(|x| x.as_array().cloned().unwrap_or_else(|| vec![x.clone()])).any(|p| p["type"] == "PC"),
                _ => true,
            }
        })
        .filter_map(|g| {
            Some(Owned {
                space_id: g["spaceId"].as_str().filter(|s| !s.is_empty())?.to_string(),
                name: g["name"].as_str()?.trim().to_string(),
                cover: g["coverUrl"].as_str().filter(|s| !s.is_empty()).map(String::from),
                hero: g["backgroundUrl"].as_str().filter(|s| !s.is_empty()).map(String::from),
            })
        })
        .collect()
}

/// Fetch the owned-games list and remember it. Returns how many games.
pub async fn refresh() -> Result<usize, String> {
    let mut s = load().ok_or("not signed in to Ubisoft")?;
    let v = match query(&s).await {
        Ok(v) => v,
        Err((401 | 403, _)) => {
            s = renew(&s).await?;
            query(&s).await.map_err(|e| e.1)?
        }
        Err((_, e)) => return Err(e),
    };
    if let Some(e) = v["errors"].as_array().and_then(|a| a.first()) {
        return Err(format!("Ubisoft: {}", e["message"].as_str().unwrap_or("query failed")));
    }
    let list = parse(&v);
    let _ = fs::create_dir_all(state::data_dir());
    fs::write(owned_file(), serde_json::to_string(&list).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(list.len())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    /// Asks Ubisoft's API which id fields a game has (prints field errors/values only, never the session).
    #[test]
    #[ignore]
    fn live_probe_ids() {
        tauri::async_runtime::block_on(async {
            let s = super::load().unwrap();
            let r = super::headers(crate::net::client().post("https://public-ubiservices.ubi.com/v1/profiles/me/uplay/graphql"), &s)
                .json(&json!({ "query": "{ __type(name: \"Game\") { fields { name } } }" })).send().await.unwrap();
            let v: serde_json::Value = r.json().await.unwrap();
            println!("fields: {:?} err={:?}", v["data"]["__type"]["fields"].as_array().map(|a| a.iter().map(|f| f["name"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>()), v["errors"][0]["message"]);
            let ids: Vec<String> = super::owned().iter().take(3).map(|o| o.space_id.clone()).collect();
            let r = super::headers(crate::net::client().get(format!("https://api-ubiservices.ubi.com/v2/applications?spaceIds={}", ids.join(","))), &s).send().await.unwrap();
            let code = r.status();
            let v: serde_json::Value = r.json().await.unwrap_or_default();
            println!("apps {code}: {}", serde_json::to_string(&v).unwrap_or_default().chars().take(1500).collect::<String>());
            for field in [] as [&str; 0] {
                let q = format!("query P {{ viewer {{ games(filterBy: {{isOwned: true}}) {{ nodes {{ name {field} }} }} }} }}");
                let r = super::headers(crate::net::client().post("https://public-ubiservices.ubi.com/v1/profiles/me/uplay/graphql"), &s)
                    .json(&json!({ "query": q })).send().await.unwrap();
                let v: serde_json::Value = r.json().await.unwrap();
                let err = v["errors"][0]["message"].as_str().unwrap_or("").chars().take(160).collect::<String>();
                let sample = v["data"]["viewer"]["games"]["nodes"].as_array().map(|a| a.iter().take(3).map(|n| format!("{}={}", n["name"], n[field])).collect::<Vec<_>>());
                println!("{field}: err={err:?} sample={sample:?}");
            }
        });
    }

    #[test]
    fn reads_login_storage_and_owned_games() {
        let data = r#"[{"ticket":"T","sessionId":"S","userId":"U","nameOnPlatform":"Robb"},{"rememberMeTicket":"R"},null]"#;
        let s = super::session_from(data).unwrap();
        assert_eq!((s.ticket.as_str(), s.session_id.as_str(), s.name.as_str(), s.remember.as_deref()), ("T", "S", "Robb", Some("R")));
        let v = json!({"data": {"viewer": {"ownedGames": {"nodes": [
            {"spaceId": "a", "name": "Far Cry 5", "coverUrl": "https://c/a.jpg", "viewer": {"meta": {"ownedPlatformGroups": [[{"type": "PC"}]]}}},
            {"spaceId": "b", "name": "Console Only", "viewer": {"meta": {"ownedPlatformGroups": [[{"type": "PS4"}]]}}},
            {"spaceId": "c", "name": "No Groups"}
        ]}}}});
        let o = super::parse(&v);
        assert_eq!(o.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Far Cry 5", "No Groups"]);
        assert_eq!(o[0].cover.as_deref(), Some("https://c/a.jpg"));
    }
}
