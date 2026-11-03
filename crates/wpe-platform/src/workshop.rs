//! Steam public API request/response contract. Native NSURLSession transports bytes;
//! authentication-dependent UGC mutation belongs to an authenticated Steamworks provider.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize)]
pub struct PublicRequest {
    pub method: String,
    pub url: String,
    pub content_type: String,
    pub body: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkshopItem {
    pub id: u64,
    pub title: String,
    pub description: String,
    pub creator: String,
    pub consumer_app_id: u64,
    pub preview_url: Option<String>,
    pub file_url: Option<String>,
    pub tags: Vec<String>,
}
pub fn item_uri(id: u64) -> Result<String> {
    if id == 0 {
        return Err(Error("Workshop ID must be positive".into()));
    }
    Ok(format!("steam://url/CommunityFilePage/{id}"))
}
pub fn browse_uri() -> String {
    "steam://url/SteamWorkshopPage/431960".into()
}
pub fn details_request(ids: &[u64]) -> Result<PublicRequest> {
    if ids.is_empty() || ids.len() > 100 || ids.contains(&0) {
        return Err(Error("Provide1..100 positive Workshop IDs".into()));
    }
    let mut body = format!("itemcount={}", ids.len());
    for (i, id) in ids.iter().enumerate() {
        body.push_str(&format!("&publishedfileids%5B{i}%5D={id}"));
    }
    Ok(PublicRequest {
        method: "POST".into(),
        url: "https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/".into(),
        content_type: "application/x-www-form-urlencoded".into(),
        body: body.into_bytes(),
    })
}
pub fn parse_details(bytes: &[u8]) -> Result<Vec<WorkshopItem>> {
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(Error("Workshop response exceeds16MiB".into()));
    }
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    let rows = v["response"]["publishedfiledetails"]
        .as_array()
        .ok_or_else(|| Error("Missing publishedfiledetails".into()))?;
    let mut items = vec![];
    for row in rows {
        if row["result"].as_u64() != Some(1) {
            continue;
        }
        let id = row["publishedfileid"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| row["publishedfileid"].as_u64())
            .ok_or_else(|| Error("Invalid Workshop ID".into()))?;
        let app = row["consumer_app_id"].as_u64().unwrap_or(0);
        if app != 431960 {
            continue;
        }
        items.push(WorkshopItem {
            id,
            title: row["title"].as_str().unwrap_or("").into(),
            description: row["description"].as_str().unwrap_or("").into(),
            creator: row["creator"].as_str().unwrap_or("").into(),
            consumer_app_id: app,
            preview_url: row["preview_url"].as_str().map(str::to_owned),
            file_url: row["file_url"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            tags: row["tags"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t["tag"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    Ok(items)
}
