//! The community module's pure halves: the GraphQL documents, the node parser (missing and `null`
//! fields included), the date parser, and the request mapping over a canned transport. No test here
//! reaches the network: every call takes the closure.
use super::*;
use serde_json::json;

fn node(v: Value) -> Option<FriendActivity> {
    activity_of(&v)
}

/// The web app's own case (`core.test.ts`, "met en forme une lecture d'ami"): an episode names its
/// show, its S·E numbers, the show's poster and the SHOW's catalog id.
#[test]
fn an_episode_node_reads_as_the_web_app_reads_it() {
    let a = node(json!({
        "id": "x", "date": "2026-11-12T10:00:00Z",
        "userV2": { "id": "1", "username": "cam", "displayName": "Camille" },
        "metadataItem": { "title": "Pilote", "type": "EPISODE", "index": 1,
            "key": "/library/metadata/5d9c086c7d06d9001ffd27ab", "parent": { "index": 2 },
            "grandparent": { "title": "Archipel", "key": "/library/metadata/5d9c086c7d06d9001ffd27aa",
                "images": { "coverPoster": "https://exemple/p.jpg" } } }
    })).expect("a usable node");
    assert_eq!(a.user.name, "Camille");
    assert_eq!(a.kind, ActivityKind::Episode);
    assert_eq!((a.title.as_str(), a.show.as_deref()), ("Pilote", Some("Archipel")));
    assert_eq!((a.season, a.episode), (Some(2), Some(1)));
    assert_eq!(a.poster.as_deref(), Some("https://exemple/p.jpg"));
    assert_eq!(a.catalog_hex.as_deref(), Some("5d9c086c7d06d9001ffd27aa"), "an episode opens its show");
    assert_eq!(a.guid().as_deref(), Some("plex://show/5d9c086c7d06d9001ffd27aa"));
    assert_eq!(a.at, Some(parse_iso8601("2026-11-12T10:00:00Z").unwrap()));
}

#[test]
fn a_node_without_a_title_or_a_user_is_dropped() {
    assert_eq!(node(json!({ "id": "y", "date": "", "metadataItem": null })), None);
    assert_eq!(node(json!({ "id": "y", "metadataItem": { "title": "T" } })), None, "no user");
    assert_eq!(node(json!({ "id": "y", "metadataItem": { "title": "T" }, "userV2": null })), None);
    assert_eq!(node(json!({ "id": "y", "metadataItem": { "title": "" }, "userV2": {} })), None, "an empty title");
    assert_eq!(node(json!("not an object")), None);
}

/// Every other field tolerates absence or `null`: the entry keeps what it has.
#[test]
fn null_and_missing_fields_cost_only_the_field() {
    let a = node(json!({
        "id": null, "date": null,
        "metadataItem": { "title": "Un été", "type": null, "year": "2023", "index": null, "key": null,
            "images": null, "parent": null, "grandparent": null },
        "userV2": { "id": null, "username": "", "displayName": null, "avatar": null }
    })).expect("a title and a user are enough");
    assert_eq!(a.kind, ActivityKind::Other);
    assert_eq!(a.user.name, "?", "no name at all reads as the web app's '?'");
    assert_eq!((a.id.as_str(), a.at, a.year), ("", None, Some(2023)));
    assert_eq!(a.guid(), None);
    assert_eq!((a.poster, a.art, a.catalog_hex, a.show), (None, None, None, None));
    let m = node(json!({ "metadataItem": { "title": "Film", "type": "movie", "id": "5D9C086C7D06D9001FFD27AB",
        "images": { "coverPoster": "/relative/path.jpg", "coverArt": "https://a/art.jpg" } },
        "userV2": { "username": "yanis", "avatar": "https://plex.tv/users/1/avatar" } })).unwrap();
    assert_eq!(m.user.name, "yanis", "the username stands in for a missing display name");
    assert_eq!(m.poster, None, "a relative image names no host the transcoder could fetch");
    assert_eq!(m.art.as_deref(), Some("https://a/art.jpg"));
    assert_eq!(m.catalog_hex.as_deref(), Some("5d9c086c7d06d9001ffd27ab"), "the id when no key, lowercased");
    assert_eq!(m.guid().as_deref(), Some("plex://movie/5d9c086c7d06d9001ffd27ab"));
    let half = node(json!({ "metadataItem": { "title": "E", "type": "episode", "index": 4, "parent": {} },
        "userV2": { "id": "2" } })).unwrap();
    assert_eq!((half.season, half.episode), (None, None), "S·E needs both numbers");
    let long = node(json!({ "metadataItem": { "title": "M", "type": "movie",
        "key": "/library/metadata/0123456789abcdef0123456789abcdef01234567" }, "userV2": {} })).unwrap();
    assert_eq!(long.catalog_hex, None, "only the catalog's 24-hex form opens a page");
}

#[test]
fn the_feed_and_privacy_answers_parse_and_drift_is_unreadable() {
    let data = json!({ "activityFeed": { "nodes": [
        { "id": "1", "metadataItem": { "title": "A" }, "userV2": { "id": "u" } },
        { "id": "2", "metadataItem": null },
        null
    ] } });
    let feed = feed_of(&data).unwrap();
    assert_eq!(feed.len(), 1);
    assert_eq!(feed_of(&json!({ "activityFeed": { "nodes": null } })).unwrap(), vec![], "null nodes: an empty feed");
    assert_eq!(feed_of(&json!({})), Err(WatchlistError::Unreadable));
    assert_eq!(shares_of(&json!({ "userPrivacy": { "watchHistory": "FRIENDS" } })), Ok(true));
    assert_eq!(shares_of(&json!({ "userPrivacy": { "watchHistory": "ANYONE" } })), Ok(true));
    assert_eq!(shares_of(&json!({ "userPrivacy": { "watchHistory": "PRIVATE" } })), Ok(false));
    assert_eq!(shares_of(&json!({ "userPrivacy": null })), Err(WatchlistError::Unreadable),
        "an unread value is never taken for private");
}

#[test]
fn iso_dates_parse_with_and_without_zones() {
    assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_iso8601("2026-11-12T10:00:00.123Z"), parse_iso8601("2026-11-12T10:00:00Z"));
    assert_eq!(parse_iso8601("2026-11-12T11:00:00+01:00"), parse_iso8601("2026-11-12T10:00:00Z"));
    assert_eq!(parse_iso8601("2026-11-12T05:30-0430"), parse_iso8601("2026-11-12T10:00:00Z"));
    assert_eq!(parse_iso8601("2026-11-12T10:00"), parse_iso8601("2026-11-12T10:00:00Z"));
    assert_eq!(parse_iso8601("2024-02-29T00:00:00Z"), Some(1_709_164_800));
    for bad in ["", "yesterday", "2026-13-01T00:00:00Z", "2026-11-12", "2026-11-12T10:00:00+1"] {
        assert_eq!(parse_iso8601(bad), None, "{bad}");
    }
}

#[test]
fn the_documents_are_the_web_apps() {
    let q = feed_query(40);
    assert!(q.contains("activityFeed(first: 40, types: [WATCH_HISTORY])"), "{q}");
    for field in ["metadataItem", "coverPoster", "coverArt", "grandparent", "userV2", "displayName", "avatar"] {
        assert!(q.contains(field), "{field}");
    }
    assert!(feed_query(0).contains("first: 1,") && feed_query(500).contains("first: 100,"), "clamped");
    assert_eq!(privacy_mutation(true), "mutation { updatePrivacy(input: { watchHistory: FRIENDS }) }");
    assert_eq!(privacy_mutation(false), "mutation { updatePrivacy(input: { watchHistory: PRIVATE }) }");
}

/// The calls over a canned transport: only `set_shares_watching` marks its request a write, and a
/// status maps to its error.
#[test]
fn only_the_privacy_setter_is_a_write_and_statuses_map() {
    let client = AccountClient::new("test-client", Some("test-token"));
    let mut seen = Vec::new();
    let mut ok = |r: &CommunityRequest| {
        seen.push(r.clone());
        let body = if r.write { json!({ "data": { "updatePrivacy": true } }) }
            else if r.query.contains("userPrivacy") { json!({ "data": { "userPrivacy": { "watchHistory": "PRIVATE" } } }) }
            else { json!({ "data": { "activityFeed": { "nodes": [] } } }) };
        Ok(WatchResponse { status: 200, body: body.to_string().into_bytes(), etag: None })
    };
    assert_eq!(client.activity_feed(80, &mut ok), Ok(vec![]));
    assert_eq!(client.shares_watching(&mut ok), Ok(false));
    assert_eq!(client.set_shares_watching(true, &mut ok), Ok(()));
    assert_eq!(seen.iter().map(|r| r.write).collect::<Vec<_>>(), [false, false, true]);
    assert!(seen[2].query.contains("FRIENDS"));
    for (status, err) in [(401, WatchlistError::Unauthorized), (403, WatchlistError::Unauthorized),
        (500, WatchlistError::Other(500))] {
        let mut fail = |_: &CommunityRequest| Ok(WatchResponse { status, body: Vec::new(), etag: None });
        assert_eq!(client.activity_feed(10, &mut fail), Err(err));
    }
    let mut errors = |_: &CommunityRequest| Ok(WatchResponse { status: 200,
        body: br#"{"errors":[{"message":"nope"}]}"#.to_vec(), etag: None });
    assert_eq!(client.set_shares_watching(false, &mut errors), Err(WatchlistError::Unreadable),
        "a 200 with no data is not a write that landed");
}

/// **The privacy write answers to the Watchlist's account-write failsafe.** In a `devtriggers`
/// build with neither the loopback stand-in nor the owner's live switch armed, the LIVE transport
/// refuses the mutation before anything is built into a request (no network: the refusal is
/// decided first); with the stand-in armed it is permitted, and the base it would go to is the
/// stand-in's. A release build has no failsafe (`Ok`).
#[test]
fn the_privacy_write_is_refused_by_the_account_write_failsafe() {
    if !plx_base::devtrig::ENABLED {
        assert_eq!(super::super::watchlist::account_write_gate(), Ok(false), "a release build writes");
        return;
    }
    plx_base::devtrig::with_private_triggers(|| {
        let client = AccountClient::new("test-client", Some("test-token"));
        let mut transport = client.community_transport();
        assert_eq!(client.set_shares_watching(true, &mut transport), Err(WatchlistError::Refused),
            "no stand-in, no live switch: the write never leaves");
    });
    plx_base::devtrig::with_private_triggers(|| {
        std::fs::write(plx_base::devtrig::path("plextv"), "http://127.0.0.1:41997").unwrap();
        assert_eq!(super::super::watchlist::account_write_gate(), Ok(false), "the stand-in takes it");
        assert_eq!(community(), "http://127.0.0.1:41997");
    });
}
