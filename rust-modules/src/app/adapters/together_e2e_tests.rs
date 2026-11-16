//! **End to end against the real relay**: native together stores, their production jobs
//! (`finish_together_job` over [`LiveTogetherIo`]) and their production runner ([`TogetherRunner`],
//! `ws::run` on libcurl), talking to the web repository's relay (`apps/relay/server.mjs`'s
//! `createRelay`) on loopback, itself in front of a stand-in Plex server and a stand-in plex.tv
//! (`tests/together_relay_harness.mjs`). Nothing reaches a real server or account.
//!
//! Ignored by default: it needs Node.js and the relay's source. Run it with
//! `TOGETHER_RELAY=<web repo>/apps/relay/server.mjs [TOGETHER_NODE=<node>] cargo test --lib
//! together_e2e -- --ignored`.
//!
//! The friends-only half needs the relay at a PUBLIC https name (a host only finds, and announces
//! to, a relay on its server's public names, and so does a friend), which loopback is not: its relay
//! is `relais.test`, and the test's IO rewrites that one name to the loopback relay (`https`/`wss` to
//! `http`/`ws`) before the production IO dials. The plex.tv friends read is the one canned answer
//! (the account's friends, as plex.tv `/api/users/` lists them); the rest is the real wire.
use super::*;
use plx_data::stores::together::{
    PlayerSnapshot, TogetherCmd, TogetherInput, TogetherPlayerRequest, TogetherProfile, TogetherServer, TogetherSettings, TogetherStore,
};
use plx_data::together::relay::{PlexConnection, ServerInfo};
use plx_data::together::session::{Listed, Notice, Status};
use plx_data::together::sync::PlayerObs;
use plx_data::together::wire::SharedItem;
use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::time::Instant;

const PUBLIC_HOST: &str = "relais.test";
const SERVER_ID: &str = "mock-machine-identifier-0001";

struct Relay {
    child: Child,
    port: u16,
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_relay() -> Relay {
    // What the boot does before any request: libcurl's process-wide init.
    assert!(plx_net::net::curl_ready(), "libcurl is needed");
    let relay = std::env::var("TOGETHER_RELAY").expect("TOGETHER_RELAY=<web repo>/apps/relay/server.mjs");
    let node = std::env::var("TOGETHER_NODE").unwrap_or_else(|_| "node".into());
    let harness = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/together_relay_harness.mjs");
    let mut child = Command::new(node).arg(harness).arg(relay).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("node starts");
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().expect("stdout")).read_line(&mut line).expect("the harness prints its port");
    let v: serde_json::Value = serde_json::from_str(&line).expect("one JSON line");
    let port = v["port"].as_u64().expect("port") as u16;
    Relay { child, port }
}

/// `https`/`wss` at [`PUBLIC_HOST`] (any port) -> `http`/`ws` at the loopback relay.
fn local(url: &str, port: u16) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let host = authority.split(':').next().unwrap_or("");
    if host != PUBLIC_HOST {
        return url.to_string();
    }
    let scheme = if scheme.starts_with("ws") { "ws" } else { "http" };
    format!("{scheme}://127.0.0.1:{port}{path}")
}

/// The production IO with the one public name rewritten, and the account's friends canned.
struct E2eIo<'a> {
    live: LiveTogetherIo<'a>,
    port: u16,
    friends_xml: String,
}

impl TogetherIo for E2eIo<'_> {
    fn random(&self, n: usize) -> Option<Vec<u8>> {
        self.live.random(n)
    }
    fn get(&self, url: &str, timeout_ms: u32) -> Option<String> {
        self.live.get(&local(url, self.port), timeout_ms)
    }
    fn post_text(&self, url: &str, body: &str, timeout_ms: u32) -> Option<String> {
        self.live.post_text(&local(url, self.port), body, timeout_ms)
    }
    fn delegation(&self, _: &str, _: &str) -> Option<String> {
        panic!("a shared user's credential is its own token: no delegation is asked");
    }
    fn friends_xml(&self, _: &plx_data::stores::together::TogetherAuth) -> Option<String> {
        Some(self.friends_xml.clone())
    }
    fn exchange(&self, url: &str, first: &str, read: &mut dyn FnMut(SocketNews) -> Option<Result<GrantPayload, JoinError>>) -> Result<GrantPayload, JoinError> {
        self.live.exchange(&local(url, self.port), first, read)
    }
}

/// A device: its store and runner, what the Bridge would hand it, and a simulated player that obeys.
struct Device {
    store: TogetherStore,
    runner: TogetherRunner,
    profile: TogetherProfile,
    auth: plx_data::stores::together::TogetherAuth,
    settings: TogetherSettings,
    servers: Vec<TogetherServer>,
    player: Option<PlayerSnapshot>,
    notices: Vec<Notice>,
    requests: Vec<TogetherPlayerRequest>,
    friends_xml: String,
    port: u16,
}

fn movie() -> SharedItem {
    SharedItem { guid: "plex://movie/5d776825880197001ec967c4".into(), server: Some(SERVER_ID.into()), key: Some("42".into()), kind: "movie".into(), title: "Le Film".into(), sub: None }
}

impl Device {
    fn new(name: &str, account_id: &str, server_token: &str, relay: &str, port: u16) -> Device {
        let info = ServerInfo { id: SERVER_ID.into(), connections: vec![PlexConnection { uri: format!("https://{PUBLIC_HOST}:32400"), local: false, relay: false }] };
        Device {
            store: TogetherStore::new(),
            runner: TogetherRunner::default(),
            profile: TogetherProfile { name: name.into(), thumb: None },
            auth: plx_data::stores::together::TogetherAuth::new("e2e-client", Some(account_id), &format!("plextv-account-{account_id}")),
            settings: TogetherSettings { relay: relay.into(), ..TogetherSettings::default() },
            servers: vec![TogetherServer::new(info, false, None, Some(server_token.into()))],
            player: None,
            notices: Vec::new(),
            requests: Vec::new(),
            friends_xml: String::new(),
            port,
        }
    }

    fn pump(&mut self, now_ms: u32, dt_s: f64) {
        if let Some(p) = &mut self.player {
            if !p.obs.paused {
                p.obs.time += dt_s;
            }
        }
        let input = TogetherInput {
            profile: Some(&self.profile),
            auth: Some(&self.auth),
            guest: false,
            managed: false,
            writes_permitted: true,
            settings: &self.settings,
            server: Some(SERVER_ID),
            servers: &self.servers,
            player: self.player.as_ref(),
            now_ms,
        };
        self.store.pump(&input);
        for job in self.store.take_jobs() {
            match job {
                TogetherJob::Work(Job { addr, req }, port) => {
                    let (relay, xml) = (self.port, self.friends_xml.clone());
                    assert!(plx_base::task::spawn_off_frame("together-e2e", move |off| {
                        finish_together_job(&port, addr, req, &E2eIo { live: LiveTogetherIo { off }, port: relay, friends_xml: xml });
                    }));
                }
                TogetherJob::Socket { conn, url, port } => self.runner.start(vec![TogetherJob::Socket { conn, url: local(&url, self.port), port }]),
                other @ (TogetherJob::Send { .. } | TogetherJob::Close { .. }) => self.runner.start(vec![other]),
            }
        }
        self.notices.extend(self.store.take_notices());
        for r in self.store.take_player_requests() {
            if let Some(p) = &mut self.player {
                match r {
                    TogetherPlayerRequest::Seek(t) => p.obs.time = t,
                    TogetherPlayerRequest::Play => p.obs.paused = false,
                    TogetherPlayerRequest::Pause => p.obs.paused = true,
                    TogetherPlayerRequest::Follow { .. } => {}
                }
            }
            self.requests.push(r);
        }
    }

    fn open(&self) -> bool {
        self.store.view().is_some_and(|v| v.status == Status::Open)
    }
}

/// Pump every device each 10 ms on the real clock until `done` or `secs` pass.
fn spin(devices: &mut [&mut Device], start: Instant, secs: u64, what: &str, done: impl Fn(&[&mut Device]) -> bool) {
    let until = Instant::now() + Duration::from_secs(secs);
    let mut last = Instant::now();
    while !done(devices) {
        if Instant::now() >= until {
            for d in devices.iter() {
                eprintln!("{}: notices {:?} view {:?} busy {}", d.profile.name, d.notices, d.store.view(), d.store.busy());
            }
            panic!("timed out waiting for: {what}");
        }
        let dt = last.elapsed().as_secs_f64();
        last = Instant::now();
        let now_ms = start.elapsed().as_millis() as u32;
        for d in devices.iter_mut() {
            d.pump(now_ms, dt);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn playing(time: f64, paused: bool) -> Option<PlayerSnapshot> {
    Some(PlayerSnapshot { item: movie(), obs: PlayerObs { time, paused, ready: true, seeking: false } })
}

#[test]
#[ignore = "needs Node.js and the web repository's relay: TOGETHER_RELAY=<web repo>/apps/relay/server.mjs"]
fn together_e2e_code_session_and_intents_through_the_real_relay() {
    let relay = start_relay();
    let start = Instant::now();
    let base = format!("http://127.0.0.1:{}/multiplex-relay", relay.port);
    let mut a = Device::new("Hôte", "2002", "host-server-token", &base, relay.port);
    let mut b = Device::new("Invité", "2001", "friend-server-token", &base, relay.port);
    a.settings.visible_default = false;

    a.store.command(TogetherCmd::Host { item: Some(movie()) });
    spin(&mut [&mut a], start, 30, "the host's session opens", |d| d[0].open());
    let code = a.notices.iter().find_map(|n| if let Notice::Opened { full_code, host: true, .. } = n { full_code.clone() } else { None }).expect("a code with its relay");

    b.store.command(TogetherCmd::Join { code });
    spin(&mut [&mut a, &mut b], start, 30, "the guest joins and both see each other", |d| {
        d[1].open() && d[0].store.view().is_some_and(|v| v.peers.len() == 1) && d[1].store.view().is_some_and(|v| v.peers.len() == 1)
    });
    assert_eq!(b.store.view().map(|v| v.host), Some(false));

    // Both players on the title; the host pauses, seeks, plays: the guest's player is asked each.
    a.player = playing(10.0, false);
    b.player = playing(10.0, false);
    spin(&mut [&mut a, &mut b], start, 5, "both attached", |d| d.iter().all(|x| x.store.view().is_some_and(|v| v.peers.iter().all(|p| p.live))));
    b.requests.clear();
    a.player.as_mut().unwrap().obs.paused = true;
    spin(&mut [&mut a, &mut b], start, 10, "the guest is asked to pause", |d| d[1].requests.contains(&TogetherPlayerRequest::Pause));
    a.player.as_mut().unwrap().obs.time = 600.0;
    spin(&mut [&mut a, &mut b], start, 10, "the guest is asked to seek", |d| {
        d[1].requests.iter().any(|r| matches!(r, TogetherPlayerRequest::Seek(t) if (t - 600.0).abs() < 2.0))
    });
    a.player.as_mut().unwrap().obs.paused = false;
    spin(&mut [&mut a, &mut b], start, 10, "the guest is asked to play", |d| d[1].requests.contains(&TogetherPlayerRequest::Play));
    assert!(b.notices.iter().any(|n| matches!(n, Notice::Action { .. })), "{:?}", b.notices);

    b.store.command(TogetherCmd::Leave);
    spin(&mut [&mut a, &mut b], start, 15, "the host sees the guest leave", |d| d[1].store.view().is_none() && d[0].store.view().is_some_and(|v| v.peers.is_empty()));
}

#[test]
#[ignore = "needs Node.js and the web repository's relay: TOGETHER_RELAY=<web repo>/apps/relay/server.mjs"]
fn together_e2e_friends_only_announce_list_and_join_through_the_real_relay() {
    let relay = start_relay();
    let start = Instant::now();
    let public = format!("{PUBLIC_HOST}/multiplex-relay");
    // The host (plex.tv 2002) is friends with 2001 only; 2003 sees the same server but is not a friend.
    let mut host = Device::new("Hôte", "2002", "host-server-token", &public, relay.port);
    host.friends_xml = r#"<MediaContainer size="1"><User id="2001" title="Camille" username="camille"/></MediaContainer>"#.into();
    let mut friend = Device::new("Camille", "2001", "friend-server-token", &public, relay.port);
    let mut other = Device::new("Eli", "2003", "other-server-user-token", &public, relay.port);

    host.store.command(TogetherCmd::Host { item: Some(movie()) });
    host.player = playing(42.0, false);
    spin(&mut [&mut host], start, 40, "the session is listed to the host's friends", |d| d[0].notices.contains(&Notice::Listed(Some(Listed::Yes))));

    friend.store.command(TogetherCmd::RefreshFriends);
    other.store.command(TogetherCmd::RefreshFriends);
    spin(&mut [&mut host, &mut friend, &mut other], start, 20, "both listings answered", |d| {
        d[1..].iter().all(|x| x.notices.iter().any(|n| matches!(n, Notice::FriendSessions(_))))
    });
    assert!(other.store.friend_sessions().is_empty(), "not a friend: nothing listed");
    let listed = friend.store.friend_sessions().to_vec();
    assert_eq!(listed.len(), 1, "{listed:?}");
    // The relay names the host from plex.tv (`displayName`: title and username), never as typed.
    assert_eq!(listed[0].card.name, "Dominique (dom)");
    assert_eq!(listed[0].card.title, "Le Film");

    friend.store.command(TogetherCmd::JoinFriend { relay: listed[0].relay.clone(), room: listed[0].room.clone() });
    spin(&mut [&mut host, &mut friend], start, 40, "the friend is in the host's session", |d| {
        d[1].open() && d[0].store.view().is_some_and(|v| v.peers.len() == 1)
    });
    let (hv, fv) = (host.store.view().unwrap(), friend.store.view().unwrap());
    assert_eq!(hv.code, fv.code, "the same session");
    assert!(!fv.host);
}
