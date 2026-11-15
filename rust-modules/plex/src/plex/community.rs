//! Plex's **community** service (`community.plex.tv`, one GraphQL endpoint): what the account's
//! Plex friends watched (the Friends page's feed), and the account's own privacy switch that says
//! whether its friends see what IT watches ("Share what I watch").
//!
//! Written as an `impl AccountClient` block like `watchlist.rs`, on the base [`community`]
//! (`account.rs`), which follows the Discover stand-in: a dev build that armed the loopback mock
//! never reaches the real service. Every call is blocking (a worker thread runs it) and goes through
//! an injectable transport ([`CommunityTransport`]); production passes
//! [`AccountClient::community_transport`], tests a canned closure. The token rides in the headers
//! the live transport builds, never in a [`CommunityRequest`].
//!
//! **Reads** are gated as the Watchlist's are (`discover_read_allowed`: a dev-token boot with no
//! stand-in sends nothing). **The one write**, `updatePrivacy`, is a write to the household's real
//! plex.tv account: it answers to the same failsafe as a Watchlist `PUT`
//! (`watchlist::account_write_gate`, the owner's `multiplex-watchlist-live` switch), so a
//! `devtriggers` build refuses it unless the stand-in takes it or the owner switched live writes on.
//!
//! The wire shapes are the web app's (`packages/core/src/community.ts`): every field of a node may
//! be missing or `null`, and drift costs the field, never the feed. A node with no title or no
//! user is dropped (nothing could be drawn for it).
use super::account::{community, discover_read_allowed, endpoint_shape, note_response_contact, AccountClient};
use super::watchlist::{WatchResponse, WatchlistError};
use serde_json::Value;

/// The most entries one feed read asks for (the service's own page bound; the web app's clamp).
pub const MAX_FEED: u32 = 100;

/// The `activityFeed` query, the web app's fields exactly. `first` is clamped to 1..=[`MAX_FEED`].
pub fn feed_query(first: u32) -> String {
    let first = first.clamp(1, MAX_FEED);
    format!(
        "query {{ activityFeed(first: {first}, types: [WATCH_HISTORY]) {{ nodes {{ id date \
         metadataItem {{ id title type year index key images {{ coverPoster coverArt }} parent {{ index }} \
         grandparent {{ id title key images {{ coverPoster coverArt }} }} }} \
         userV2 {{ id username displayName avatar }} }} }} }}"
    )
}

/// The privacy read.
pub const PRIVACY_QUERY: &str = "query { userPrivacy { watchHistory } }";

/// The privacy write: `FRIENDS` shares the account's watch history with its friends, `PRIVATE`
/// keeps it to itself. The ONLY mutation this module sends.
pub fn privacy_mutation(share: bool) -> String {
    format!("mutation {{ updatePrivacy(input: {{ watchHistory: {} }}) }}", if share { "FRIENDS" } else { "PRIVATE" })
}

/// What kind of title a feed entry is, from the node's `type` (any case).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActivityKind {
    Movie,
    Episode,
    Show,
    Other,
}

/// The friend an entry is by.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Friend {
    /// `userV2.id`; empty when the service sent none.
    pub id: String,
    /// `displayName`, else `username`, else `"?"`.
    pub name: String,
    pub username: Option<String>,
    /// An absolute avatar URL (the photo transcoder of a reachable server renders it).
    pub avatar: Option<String>,
}

/// One entry of the feed, as the page shows it (the web app's `FriendActivity`).
#[derive(Clone, Debug, PartialEq)]
pub struct FriendActivity {
    pub id: String,
    /// The node's `date`, verbatim (ISO 8601).
    pub date: String,
    /// `date` as unix seconds; `None` when it does not parse (the entry then sorts into no day).
    pub at: Option<i64>,
    pub user: Friend,
    pub kind: ActivityKind,
    /// The item's own title (an episode's title for an episode).
    pub title: String,
    /// For an episode: its show's title.
    pub show: Option<String>,
    /// For an episode: its season and episode numbers, when both were sent.
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub year: Option<i32>,
    /// The show's poster for an episode, else the item's own (absolute URLs).
    pub poster: Option<String>,
    /// The show's wide art for an episode, else the item's own.
    pub art: Option<String>,
    /// The catalog id (24 hex) of the title to open: the SHOW for an episode, else the item.
    pub catalog_hex: Option<String>,
}

impl FriendActivity {
    /// The `plex://movie|show/<hex>` guid of the title a press opens, `None` when the entry names
    /// no catalog id or is of a kind no page shows.
    pub fn guid(&self) -> Option<String> {
        let hex = self.catalog_hex.as_deref()?;
        match self.kind {
            ActivityKind::Movie => Some(format!("plex://movie/{hex}")),
            ActivityKind::Episode | ActivityKind::Show => Some(format!("plex://show/{hex}")),
            ActivityKind::Other => None,
        }
    }
}

/// A request as the transport sees it: the GraphQL document, and whether it is the write. Carries
/// no credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommunityRequest {
    pub query: String,
    pub write: bool,
}

/// The injectable request closure every call takes.
pub type CommunityTransport<'a> = &'a mut dyn FnMut(&CommunityRequest) -> Result<WatchResponse, WatchlistError>;

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn int(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// An object field, `None` for a missing key, `null` or a non-object.
fn obj<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.get(key).filter(|o| o.is_object())
}

fn is_hex_id(s: &str) -> bool {
    s.len() == 24 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The catalog id an item names: the 24-hex tail of its `key`, else of its `id` (the web app's
/// `idOf`, narrowed to the one id form the catalog page takes).
fn hex_of(item: Option<&Value>) -> Option<String> {
    let item = item?;
    [text(item, "key"), text(item, "id")].into_iter().flatten().find_map(|s| {
        let tail = s.rsplit('/').next().unwrap_or(&s);
        is_hex_id(tail).then(|| tail.to_ascii_lowercase())
    })
}

fn image(item: Option<&Value>, key: &str) -> Option<String> {
    obj(item?, "images").and_then(|i| text(i, key)).filter(|u| u.starts_with("https://") || u.starts_with("http://"))
}

/// `YYYY-MM-DDTHH:MM[:SS[.fff]][Z|±HH:MM]` as unix seconds. A string with no zone is read as UTC.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |a: usize, b: usize| -> Option<i64> {
        let part = s.get(a..b)?;
        part.bytes().all(|c| c.is_ascii_digit()).then(|| part.parse().ok()).flatten()
    };
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    if s.get(4..5)? != "-" || s.get(7..8)? != "-" || !matches!(s.get(10..11)?, "T" | "t" | " ") {
        return None;
    }
    let (h, mi) = (num(11, 13)?, num(14, 16)?);
    let mut rest = s.get(16..)?;
    let mut sec = 0;
    if let Some(r) = rest.strip_prefix(':') {
        sec = r.get(..2).and_then(|p| p.parse().ok())?;
        rest = &r[2..];
        if let Some(frac) = rest.strip_prefix('.') {
            rest = frac.trim_start_matches(|c: char| c.is_ascii_digit());
        }
    }
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        zone => {
            let sign = match zone.get(..1)? { "+" => 1, "-" => -1, _ => return None };
            let digits: String = zone[1..].chars().filter(|c| *c != ':').collect();
            if digits.len() != 4 || !digits.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            sign * (digits[..2].parse::<i64>().ok()? * 3600 + digits[2..].parse::<i64>().ok()? * 60)
        }
    };
    Some(plx_base::civil::days_from_civil([y as i32, mo as i32, d as i32]) * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

/// One feed node as the page shows it (the web app's `toFriendActivity`); `None` when it has no
/// title or no user.
pub fn activity_of(node: &Value) -> Option<FriendActivity> {
    let m = obj(node, "metadataItem")?;
    let title = text(m, "title")?;
    let user = obj(node, "userV2")?;
    let kind = match text(m, "type").unwrap_or_default().to_ascii_lowercase().as_str() {
        "movie" => ActivityKind::Movie,
        "episode" => ActivityKind::Episode,
        "show" => ActivityKind::Show,
        _ => ActivityKind::Other,
    };
    let episode = kind == ActivityKind::Episode;
    let show = if episode { obj(m, "grandparent") } else { None };
    let (season, number) = if episode {
        match (obj(m, "parent").and_then(|p| int(p, "index")), int(m, "index")) {
            (Some(s), Some(e)) => (Some(s), Some(e)),
            _ => (None, None),
        }
    } else {
        (None, None)
    };
    let date = text(node, "date").unwrap_or_default();
    let username = text(user, "username");
    Some(FriendActivity {
        id: text(node, "id").unwrap_or_default(),
        at: parse_iso8601(&date),
        date,
        user: Friend {
            id: text(user, "id").unwrap_or_default(),
            name: text(user, "displayName").or_else(|| username.clone()).unwrap_or_else(|| "?".into()),
            username,
            avatar: text(user, "avatar").filter(|u| u.starts_with("https://") || u.starts_with("http://")),
        },
        kind,
        title,
        show: show.and_then(|s| text(s, "title")),
        season,
        episode: number,
        year: int(m, "year").and_then(|y| i32::try_from(y).ok()),
        poster: image(show, "coverPoster").or_else(|| image(Some(m), "coverPoster")),
        art: image(show, "coverArt").or_else(|| image(Some(m), "coverArt")),
        catalog_hex: if episode { hex_of(show) } else { hex_of(Some(m)) },
    })
}

/// A GraphQL answer's `data`, or why there is none. A 2xx with no `data` (an `errors` array, a
/// drifted body) is [`WatchlistError::Unreadable`].
fn data_of(response: &WatchResponse) -> Result<Value, WatchlistError> {
    let body: Value = serde_json::from_slice(&response.body).map_err(|_| WatchlistError::Unreadable)?;
    match body.get("data") {
        Some(data) if data.is_object() => Ok(data.clone()),
        _ => Err(WatchlistError::Unreadable),
    }
}

/// Run one request and map the status: 2xx yields the response, anything else its error. A
/// failure is logged by endpoint SHAPE and status only.
fn call(req: CommunityTransport, query: String, write: bool) -> Result<WatchResponse, WatchlistError> {
    let request = CommunityRequest { query, write };
    let what = if write { "write" } else { "read" };
    match req(&request) {
        Ok(response) if (200..300).contains(&response.status) => Ok(response),
        Ok(response) => {
            plx_base::eventlog::log(&format!("community: {what} -> HTTP {}", response.status));
            Err(match response.status {
                400 => WatchlistError::BadRequest,
                401 | 403 => WatchlistError::Unauthorized,
                404 => WatchlistError::NotFound,
                other => WatchlistError::Other(other),
            })
        }
        Err(e) => {
            if e != WatchlistError::Refused {
                plx_base::eventlog::log(&format!("community: {what} -> no answer"));
            }
            Err(e)
        }
    }
}

/// The feed's entries from a decoded `data` (`activityFeed.nodes`), unusable nodes dropped.
pub fn feed_of(data: &Value) -> Result<Vec<FriendActivity>, WatchlistError> {
    let nodes = obj(data, "activityFeed").and_then(|f| f.get("nodes")).ok_or(WatchlistError::Unreadable)?;
    Ok(nodes.as_array().map_or(&[][..], Vec::as_slice).iter().filter_map(activity_of).collect())
}

/// Does `data` (`userPrivacy.watchHistory`) say the account shares what it watches? Anything but
/// `PRIVATE` shares, as the web app reads it; a missing value is unreadable, never "private".
pub fn shares_of(data: &Value) -> Result<bool, WatchlistError> {
    let value = obj(data, "userPrivacy").and_then(|p| text(p, "watchHistory")).ok_or(WatchlistError::Unreadable)?;
    Ok(!value.eq_ignore_ascii_case("PRIVATE"))
}

impl AccountClient {
    /// The live transport: one HTTPS `POST <community>/api` with the account headers and token and
    /// the request's document as a JSON body. A read is refused on a dev-token boot with no
    /// stand-in (`discover_read_allowed`); the write is refused by the account-write failsafe
    /// (`watchlist::account_write_gate`) and, when it goes out, logged with its status (`LIVE` when
    /// only the owner's switch let a debug build send it). Never the body or a header.
    pub fn community_transport(&self) -> impl FnMut(&CommunityRequest) -> Result<WatchResponse, WatchlistError> + '_ {
        move |req: &CommunityRequest| -> Result<WatchResponse, WatchlistError> {
            let url = format!("{}/api", community());
            let mut live = false;
            if req.write {
                match super::watchlist::account_write_gate() {
                    Ok(l) => live = l,
                    Err(e) => {
                        plx_base::eventlog::log(&format!(
                            "community: privacy write {} refused (dev build: neither the stand-in nor the live switch is armed)",
                            endpoint_shape(&url)));
                        return Err(e);
                    }
                }
            } else if !discover_read_allowed(&url) {
                return Err(WatchlistError::Refused);
            }
            let mut headers = self.headers();
            headers.push("Content-Type: application/json".to_string());
            let body = serde_json::json!({ "query": req.query, "variables": {} }).to_string().into_bytes();
            let response = plx_net::net::request_evidence(&url, &headers, "POST", Some(&body),
                plx_net::net::API, false, None, None);
            note_response_contact(&url, &response);
            if req.write {
                let tag = if live { "LIVE " } else { "" };
                match &response {
                    Ok(r) => plx_base::eventlog::log(&format!("community: {tag}privacy write {} -> HTTP {}", endpoint_shape(&url), r.status)),
                    Err(f) => plx_base::eventlog::log(&format!("community: {tag}privacy write {} -> {}", endpoint_shape(&url),
                        f.status.map_or_else(|| "no answer".to_string(), |s| format!("HTTP {s}")))),
                }
            }
            match response {
                Ok(r) => Ok(WatchResponse { status: r.status, body: r.body, etag: None }),
                Err(f) => match f.status {
                    Some(status) if !(200..300).contains(&status) => Ok(WatchResponse { status, body: Vec::new(), etag: None }),
                    _ => Err(WatchlistError::Transport),
                },
            }
        }
    }

    /// The friends' feed, newest first, at most `first` (clamped to [`MAX_FEED`]) entries.
    pub fn activity_feed(&self, first: u32, req: CommunityTransport) -> Result<Vec<FriendActivity>, WatchlistError> {
        let response = call(req, feed_query(first), false)?;
        feed_of(&data_of(&response)?)
    }

    /// Does the account share what it watches with its friends?
    pub fn shares_watching(&self, req: CommunityTransport) -> Result<bool, WatchlistError> {
        let response = call(req, PRIVACY_QUERY.to_string(), false)?;
        shares_of(&data_of(&response)?)
    }

    /// **WRITE**: set whether the account shares what it watches. Only ever called on the user's
    /// explicit press of the Friends page's toggle.
    pub fn set_shares_watching(&self, share: bool, req: CommunityTransport) -> Result<(), WatchlistError> {
        let response = call(req, privacy_mutation(share), true)?;
        data_of(&response).map(|_| ())
    }
}

#[cfg(test)]
#[path = "community_tests.rs"]
mod community_tests;
