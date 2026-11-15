//! Watch Together, the web client's (`MultiPlex` 03f5f10, `packages/core/src/together*.ts` and
//! `apps/web/src/together.ts`) on this client, wire-compatible with it: same invitation codes, same
//! session keys and room names, same sealed JSON. Nothing here performs IO.
//!
//! - [`invite`]: invitation codes (`MPX-K7Q2-9XFD`, and the older v1/v2), the relay address rule.
//! - [`keys`]: room, host proof and AES key of a session, the sealing of its messages.
//! - [`wire`]: the `tick` / `intent` / `bye` messages, written as `JSON.stringify` writes them.
//! - [`relay`]: relay candidates from the Plex servers' addresses, and `findRoom` as a fold.
//! - [`sync`]: the Lamport rule that keeps players together, with a keyframe-slack guard.
//! - [`session`]: [`session::TogetherSession`], the machine of one session.
//! - [`friends`]: PROVISIONAL pure pieces of the friends listing (being redesigned; not wired).
//!
//! `interop.json` was produced by the web client's own code under Node's WebCrypto; the tests below
//! hold this implementation to it byte for byte, in both directions.

pub mod friends;
pub mod invite;
pub mod keys;
pub mod relay;
pub mod session;
pub mod sync;
pub mod wire;

#[cfg(test)]
mod interop_tests {
    use serde_json::Value;

    use super::friends::{open_grant, seal_grant, GrantPayload, GrantState};
    use super::invite::{invite_text, parse_invite, relay_url, Invite};
    use super::keys::{derive_legacy, derive_session, open_message, seal_message};
    use super::relay::{http_base, relay_candidates, rooms_url, PlexConnection, ServerInfo};
    use super::wire::{read_message, Bye, Incoming, Intent, Message, Tick};

    fn fixture() -> Value {
        serde_json::from_str(include_str!("together/interop.json")).expect("interop.json")
    }

    fn s<'a>(v: &'a Value, k: &str) -> &'a str {
        v[k].as_str().unwrap_or_else(|| panic!("{k} in {v}"))
    }

    fn hex(t: &str) -> Vec<u8> {
        (0..t.len()).step_by(2).map(|i| u8::from_str_radix(&t[i..i + 2], 16).unwrap()).collect()
    }

    fn arr<const N: usize>(t: &str) -> [u8; N] {
        hex(t).try_into().unwrap()
    }

    /// The message this client would write for what it read.
    fn rewrite(m: Incoming) -> Message {
        match m {
            Incoming::Bye { id } => Message::Bye(Bye { id }),
            Incoming::Tick(t) => {
                let (c, by) = t.stamp.unwrap_or_default();
                Message::Tick(Tick {
                    id: t.id,
                    name: t.name,
                    thumb: t.thumb,
                    host: t.host,
                    time: t.time,
                    playing: t.playing,
                    item: t.item,
                    live: t.live,
                    c,
                    by,
                    vis: t.vis,
                    hc: t.host.then_some(t.hc),
                    want: t.want,
                })
            }
            Incoming::Intent(i) => Message::Intent(Intent {
                id: i.id,
                name: i.name,
                action: i.action.expect("action"),
                time: i.time,
                playing: i.playing,
                c: i.c.expect("stamped"),
                item: i.item,
            }),
        }
    }

    #[test]
    fn sessions_derive_and_seal_as_the_web_does() {
        let f = fixture();
        let cases = f["sessions"].as_array().unwrap();
        assert!(cases.iter().any(|c| c["iterations"] == 200_000), "one case at the real round count");
        for c in cases {
            let keys = derive_session(s(c, "secret"), s(c, "server"), c["iterations"].as_u64().unwrap() as u32);
            assert_eq!(keys.room, s(c, "room"), "{c}");
            assert_eq!(keys.owner.as_deref(), Some(s(c, "owner")), "{c}");
            let sealed = hex(s(c, "box"));
            // Web -> here: the web's box opens to the web's text.
            assert_eq!(open_message(&keys.key, &sealed).as_deref(), Some(s(c, "plain").as_bytes()), "{c}");
            // Here -> web: the same IV gives the same box, byte for byte.
            assert_eq!(seal_message(&keys.key, &arr(s(c, "iv")), s(c, "plain")), sealed, "{c}");
        }
    }

    #[test]
    fn legacy_v2_secrets_split_as_the_web_does() {
        let f = fixture();
        for c in f["legacy"].as_array().unwrap() {
            let keys = derive_legacy(&Invite::V2 { relay: "wss://r".into(), secret: s(c, "secret").into() }).unwrap();
            assert_eq!(keys.room, s(c, "room"));
            assert_eq!(keys.owner.as_deref(), Some(s(c, "owner")));
            let sealed = hex(s(c, "box"));
            assert_eq!(open_message(&keys.key, &sealed).as_deref(), Some(s(c, "plain").as_bytes()));
            assert_eq!(seal_message(&keys.key, &arr(s(c, "iv")), s(c, "plain")), sealed);
        }
    }

    /// The messages the web writes, sealed under one session key: they open, they read, and the
    /// message this client writes for the same content is the same text (so the same box).
    #[test]
    fn messages_read_and_write_as_the_web_does() {
        let f = fixture();
        let keys = derive_session("K7Q29XFD", "machine-a", 1000);
        let mut labels = Vec::new();
        for c in f["messages"].as_array().unwrap() {
            let sealed = hex(s(c, "box"));
            let plain = open_message(&keys.key, &sealed).expect("opens");
            assert_eq!(plain, s(c, "json").as_bytes());
            assert_eq!(seal_message(&keys.key, &arr(s(c, "iv")), s(c, "json")), sealed);
            let m = read_message(&plain).unwrap_or_else(|| panic!("reads {}", s(c, "json")));
            assert_eq!(rewrite(m).to_json(), s(c, "json"), "rewritten the web's way");
            labels.push(s(c, "label").to_string());
        }
        assert_eq!(labels, ["tick", "intent", "bye", "want"]);
    }

    #[test]
    fn grants_seal_and_open_as_the_web_does() {
        let f = fixture();
        for c in f["grants"].as_array().unwrap() {
            let opened: Value = serde_json::from_str(s(c, "opened_json")).unwrap();
            let state = opened.get("state").map(|st| GrantState {
                time: st["time"].as_f64().unwrap(),
                playing: st["playing"].as_bool().unwrap(),
                c: st["c"].as_u64().unwrap(),
                by: st["by"].as_str().unwrap().into(),
            });
            let payload = GrantPayload { secret: s(&opened, "secret").into(), server: s(&opened, "server").into(), state };
            // Web -> here.
            let got = open_grant(&arr(s(c, "req_d")), s(c, "req_pub"), s(c, "grant_pub"), s(c, "box"));
            assert_eq!(got.as_ref(), Some(&payload), "{}", s(c, "label"));
            assert_eq!(serde_json::to_string(&payload).unwrap(), s(c, "payload_json"));
            // Here -> web: the web's ephemeral key and IV give its public key and box.
            let (grant_pub, sealed) = seal_grant(s(c, "req_pub"), &payload, &arr(s(c, "grant_d")), &arr(s(c, "iv"))).unwrap();
            assert_eq!((grant_pub.as_str(), sealed.as_str()), (s(c, "grant_pub"), s(c, "box")), "{}", s(c, "label"));
        }
    }

    #[test]
    fn invites_parse_and_print_as_the_web_does() {
        let f = fixture();
        for c in f["invites"].as_array().unwrap() {
            let text = s(c, "text");
            let got = parse_invite(text);
            let p = &c["parsed"];
            let want = match p["v"].as_u64() {
                None => None,
                Some(3) => Some(Invite::V3 { secret: s(p, "secret").into(), relay: p["relay"].as_str().map(str::to_string) }),
                Some(2) => Some(Invite::V2 { relay: s(p, "relay").into(), secret: s(p, "secret").into() }),
                Some(1) => Some(Invite::V1 { relay: s(p, "relay").into(), room: s(p, "room").into(), key: s(p, "key").into() }),
                Some(v) => panic!("v{v}"),
            };
            assert_eq!(got, want, "{text:?}");
            assert_eq!(got.as_ref().map(invite_text).as_deref(), c["share"].as_str(), "{text:?}");
        }
    }

    #[test]
    fn relay_addresses_as_the_web_does() {
        let f = fixture();
        for c in f["relay_urls"].as_array().unwrap() {
            assert_eq!(relay_url(s(c, "input")), s(c, "url"), "{c}");
        }
        for c in f["candidates"].as_array().unwrap() {
            let servers: Vec<ServerInfo> = c["servers"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, sv)| ServerInfo {
                    id: format!("s{i}"),
                    connections: sv["connections"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|k| PlexConnection { uri: s(k, "uri").into(), local: k["local"] == true, relay: k["relay"] == true })
                        .collect(),
                })
                .collect();
            let want: Vec<String> = c["out"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
            assert_eq!(relay_candidates(&servers), want);
        }
        for c in f["rooms_urls"].as_array().unwrap() {
            let rooms: Vec<String> = c["rooms"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
            assert_eq!(rooms_url(s(c, "relay"), &rooms), s(c, "url"));
        }
        for c in f["friends_urls"].as_array().unwrap() {
            assert_eq!(format!("{}/friends", http_base(s(c, "relay"))), s(c, "url"));
        }
    }
}
