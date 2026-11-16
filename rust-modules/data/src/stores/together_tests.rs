use super::*;
use crate::together::relay::PlexConnection;
use std::sync::atomic::{AtomicU8, Ordering};

/// A scripted IO: counted random bytes, canned HTTP answers, and a record of what was asked.
#[derive(Default)]
struct Script {
    draws: AtomicU8,
    rooms: Mutex<Option<String>>,
    friends: Mutex<Option<String>>,
    xml: Mutex<Option<String>>,
    asked: Mutex<Vec<String>>,
}

impl Script {
    fn note(&self, s: String) {
        self.asked.lock().unwrap().push(s);
    }
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl TogetherIo for Script {
    fn random(&self, n: usize) -> Option<Vec<u8>> {
        let d = self.draws.fetch_add(1, Ordering::SeqCst);
        Some((0..n).map(|i| (i as u8).wrapping_mul(7).wrapping_add(d).max(1)).collect())
    }
    fn get(&self, url: &str, _: u32) -> Option<String> {
        self.note(format!("GET {url}"));
        self.rooms.lock().unwrap().clone()
    }
    fn post_text(&self, url: &str, body: &str, _: u32) -> Option<String> {
        self.note(format!("POST {url} {body}"));
        self.friends.lock().unwrap().clone()
    }
    fn delegation(&self, base: &str, token: &str) -> Option<String> {
        self.note(format!("DELEGATION {base} {token}"));
        Some("delegated".into())
    }
    fn friends_xml(&self, auth: &TogetherAuth) -> Option<String> {
        self.note(format!("FRIENDS {}", auth.account_token()));
        self.xml.lock().unwrap().clone()
    }
    fn exchange(&self, url: &str, first: &str, _: &mut dyn FnMut(SocketNews) -> Option<Result<GrantPayload, JoinError>>) -> Result<GrantPayload, JoinError> {
        self.note(format!("EXCHANGE {url} {first}"));
        Err(JoinError::NoHost)
    }
}

/// The held runner: work jobs run at once through the script (or are refused), sockets and
/// frames are kept for the test to read.
#[derive(Default)]
struct Held {
    sockets: Vec<(u32, String)>,
    sent: Vec<(u32, Frame)>,
    closed: Vec<u32>,
    refuse: bool,
}

impl Held {
    fn run(&mut self, store: &mut TogetherStore, io: &Script) {
        for job in store.take_jobs() {
            match job {
                TogetherJob::Work(Job { addr, req }, port) => {
                    if self.refuse {
                        port.refused(addr);
                    } else {
                        finish_together_job(&port, addr, req, io);
                    }
                }
                TogetherJob::Socket { conn, url, .. } => self.sockets.push((conn, url)),
                TogetherJob::Send { conn, frame } => self.sent.push((conn, frame)),
                TogetherJob::Close { conn } => self.closed.push(conn),
            }
        }
    }
}

struct World {
    profile: TogetherProfile,
    auth: Option<TogetherAuth>,
    guest: bool,
    managed: bool,
    writes: bool,
    settings: TogetherSettings,
    servers: Vec<TogetherServer>,
    player: Option<PlayerSnapshot>,
    now_ms: u32,
}

impl World {
    fn new() -> World {
        let info = ServerInfo { id: "srv-1".into(), connections: vec![PlexConnection { uri: "https://relais.exemple.fr:32400".into(), local: false, relay: false }] };
        World {
            profile: TogetherProfile { name: "Hôte".into(), thumb: None },
            auth: Some(TogetherAuth::new("client", Some("1"), "account-token")),
            guest: false,
            managed: false,
            writes: true,
            settings: TogetherSettings { relay: "relais.exemple.fr".into(), ..TogetherSettings::default() },
            servers: vec![TogetherServer::new(info, true, Some("http://127.0.0.1:32400".into()), Some("account-token".into()))],
            player: None,
            now_ms: 1000,
        }
    }

    fn input(&self) -> TogetherInput<'_> {
        TogetherInput {
            profile: Some(&self.profile),
            auth: self.auth.as_ref(),
            guest: self.guest,
            managed: self.managed,
            writes_permitted: self.writes,
            settings: &self.settings,
            server: Some("srv-1"),
            servers: &self.servers,
            player: self.player.as_ref(),
            now_ms: self.now_ms,
        }
    }

    /// Pump and run the jobs until nothing more lands.
    fn settle(&mut self, store: &mut TogetherStore, held: &mut Held, io: &Script) {
        for _ in 0..8 {
            store.pump(&self.input());
            held.run(store, io);
        }
    }
}

fn movie() -> SharedItem {
    SharedItem { guid: "plex://movie/1".into(), server: Some("srv-1".into()), key: Some("42".into()), kind: "movie".into(), title: "Le Film".into(), sub: None }
}

fn texts(held: &Held) -> Vec<serde_json::Value> {
    held.sent.iter().filter_map(|(_, f)| if let Frame::Text(t) = f { serde_json::from_str(t).ok() } else { None }).collect()
}

/// A host whose socket opened, its jobs run through `io`.
fn hosting(world: &mut World, io: &Script) -> (TogetherStore, Held) {
    let (mut store, mut held) = (TogetherStore::new(), Held::default());
    store.command(TogetherCmd::Host { item: Some(movie()) });
    world.settle(&mut store, &mut held, io);
    let (conn, url) = held.sockets.first().cloned().expect("a socket");
    assert!(url.starts_with("wss://relais.exemple.fr/r/"), "{url}");
    store.port().socket(conn, SocketNews::Opened);
    world.settle(&mut store, &mut held, io);
    (store, held)
}

#[test]
fn a_participant_id_is_drawn_before_any_command_and_a_host_derives_off_the_frame() {
    let (world, io, mut held) = (World::new(), Script::default(), Held::default());
    let mut store = TogetherStore::new();
    store.command(TogetherCmd::Host { item: Some(movie()) });
    store.pump(&world.input());
    let jobs = store.take_jobs();
    assert!(matches!(jobs.as_slice(), [TogetherJob::Work(Job { req: TogetherReq::Random { n: 6 }, .. }, _)]), "{jobs:?}");
    for job in jobs {
        if let TogetherJob::Work(Job { addr, req }, port) = job {
            finish_together_job(&port, addr, req, &io);
        }
    }
    store.pump(&world.input());
    let jobs = store.take_jobs();
    assert!(matches!(jobs.as_slice(), [TogetherJob::Work(Job { req: TogetherReq::Random { n: 32 }, .. }, _)]), "{jobs:?}");
    for job in jobs {
        if let TogetherJob::Work(Job { addr, req }, port) = job {
            finish_together_job(&port, addr, req, &io);
        }
    }
    store.pump(&world.input());
    let jobs = store.take_jobs();
    assert!(matches!(jobs.as_slice(), [TogetherJob::Work(Job { req: TogetherReq::Derive { iterations: 200_000, .. }, .. }, _)]), "{jobs:?}");
    for job in jobs {
        if let TogetherJob::Work(Job { addr, req }, port) = job {
            finish_together_job(&port, addr, req, &io);
        }
    }
    store.pump(&world.input());
    held.run(&mut store, &io);
    assert_eq!(held.sockets.len(), 1);
    let notices = store.take_notices();
    assert!(notices.iter().any(|n| matches!(n, Notice::Opened { host: true, .. })), "{notices:?}");
    assert_eq!(store.view().map(|v| v.host), Some(true));
}

#[test]
fn a_visible_host_announces_with_the_delegated_token_and_its_friends() {
    let mut world = World::new();
    let io = Script::default();
    *io.rooms.lock().unwrap() = Some(r#"{"multiplex":"relay","found":[],"friends":true,"friendsOnly":true,"server":"srv-1"}"#.into());
    *io.friends.lock().unwrap() = Some(r#"{"multiplex":"relay","server":"srv-1","friendsOnly":true,"ticket":"T","ttl":120,"sessions":[]}"#.into());
    *io.xml.lock().unwrap() = Some(r#"<MediaContainer><User id="2" title="Ami"/><User id="1" title="Moi"/></MediaContainer>"#.into());
    let (_store, held) = hosting(&mut world, &io);
    let asked = io.asked();
    // The owner's account token goes to its own server and to plex.tv, never to the relay.
    assert!(asked.contains(&"DELEGATION http://127.0.0.1:32400 account-token".to_string()), "{asked:?}");
    assert!(asked.contains(&"FRIENDS account-token".to_string()), "{asked:?}");
    let post = asked.iter().find(|a| a.starts_with("POST ")).expect("a /friends post");
    assert_eq!(post, r#"POST https://relais.exemple.fr/friends {"token":"delegated"}"#);
    assert!(!asked.iter().any(|a| (a.starts_with("POST") || a.starts_with("GET")) && a.contains("account-token")), "{asked:?}");
    let announce = texts(&held).into_iter().find(|t| t["t"] == "announce").expect("an announce");
    assert_eq!(announce["friends"], serde_json::json!(["2"]));
    assert_eq!(announce["ticket"], "T");
}

#[test]
fn a_guest_a_managed_profile_or_a_gated_build_never_announces_nor_asks_a_token() {
    for case in 0..3 {
        let mut world = World::new();
        match case {
            0 => world.guest = true,
            1 => world.managed = true,
            _ => world.writes = false,
        }
        let io = Script::default();
        *io.rooms.lock().unwrap() = Some(r#"{"multiplex":"relay","found":[],"friends":true,"friendsOnly":true,"server":"srv-1"}"#.into());
        let (mut store, mut held) = hosting(&mut world, &io);
        store.command(TogetherCmd::SetVisible(true));
        world.settle(&mut store, &mut held, &io);
        let asked = io.asked();
        assert!(!asked.iter().any(|a| a.starts_with("POST") || a.starts_with("DELEGATION") || a.starts_with("FRIENDS")), "case {case}: {asked:?}");
        assert!(!texts(&held).iter().any(|t| t["t"] == "announce"), "case {case}");
        // The code session itself is open.
        assert_eq!(store.view().map(|v| v.host), Some(true), "case {case}");
    }
}

#[test]
fn a_shared_server_is_shown_its_own_token_and_never_the_account_token() {
    let mut world = World::new();
    world.servers[0] = TogetherServer::new(world.servers[0].info.clone(), false, None, Some("share-token".into()));
    assert_eq!(world.input().credential("srv-1"), Some(Credential::Share("share-token".into())));
    world.servers[0] = TogetherServer::new(world.servers[0].info.clone(), false, None, Some("account-token".into()));
    assert_eq!(world.input().credential("srv-1"), None);
    world.servers[0] = TogetherServer::new(world.servers[0].info.clone(), true, Some("http://pms".into()), Some("account-token".into()));
    assert_eq!(world.input().credential("srv-1"), Some(Credential::Delegation { base: "http://pms".into(), token: "account-token".into() }));
    world.writes = false;
    assert_eq!(world.input().credential("srv-1"), None);
    assert_eq!(world.input().credential("srv-2"), None);
    assert!(!format!("{:?}", Credential::Share("share-token".into())).contains("share-token"));
    assert!(!format!("{:?}", world.auth).contains("account-token"));
}

#[test]
fn a_refused_job_lands_as_its_negative_answer() {
    let mut world = World::new();
    let io = Script::default();
    let (mut store, mut held) = (TogetherStore::new(), Held::default());
    world.settle(&mut store, &mut held, &io);
    held.refuse = true;
    store.command(TogetherCmd::Join { code: "MPX-K7Q2-9XFD@relais.exemple.fr".into() });
    world.settle(&mut store, &mut held, &io);
    let notices = store.take_notices();
    assert!(notices.contains(&Notice::HostFailed) || notices.iter().any(|n| matches!(n, Notice::JoinFailed(_))), "{notices:?}");
    assert!(store.view().is_none());
    assert!(store.inflight.is_empty());
}

#[test]
fn the_player_is_attached_observed_every_half_second_and_detached() {
    let mut world = World::new();
    let io = Script::default();
    let (mut store, mut held) = hosting(&mut world, &io);
    held.sent.clear();
    world.player = Some(PlayerSnapshot { item: movie(), obs: PlayerObs { time: 10.0, paused: false, ready: true, seeking: false } });
    world.now_ms += 16;
    store.pump(&world.input());
    assert_eq!(store.attached, Some(movie()));
    world.now_ms += 600;
    store.pump(&world.input());
    assert_eq!(store.since_obs_us, 0);
    world.player = None;
    world.now_ms += 16;
    store.pump(&world.input());
    assert_eq!(store.attached, None);
}

#[test]
fn another_identity_leaves_the_session() {
    let mut world = World::new();
    let io = Script::default();
    let (mut store, mut held) = hosting(&mut world, &io);
    world.profile.name = "Autre".into();
    world.settle(&mut store, &mut held, &io);
    assert!(store.view().is_none());
    assert!(!held.closed.is_empty());
}

#[test]
fn the_notify_setting_silences_the_others_gestures() {
    let mut world = World::new();
    world.settings.notify = false;
    let mut store = TogetherStore::new();
    store.notice(Notice::Action { name: "Ami".into(), action: None }, &world.input());
    assert!(store.take_notices().is_empty());
    world.settings.notify = true;
    store.notice(Notice::Action { name: "Ami".into(), action: None }, &world.input());
    assert_eq!(store.take_notices().len(), 1);
    store.notice(Notice::Item(movie()), &world.input());
    assert_eq!(store.take_player_requests(), [TogetherPlayerRequest::Follow { item: movie(), position: None }]);
}

#[test]
fn a_friend_reads_the_hosts_grant_and_its_errors() {
    let host_key = [9u8; 32];
    let friend = KeyExchange::new(&[5u8; 32]).unwrap();
    let request: serde_json::Value = serde_json::from_str(&friend.request_text("T")).unwrap();
    assert_eq!(request["t"], "key-request");
    assert_eq!(request["ticket"], "T");
    let payload = GrantPayload { secret: "K7Q29XFD".into(), server: "srv-1".into(), state: None };
    let (public, sealed) = seal_grant(request["pub"].as_str().unwrap(), &payload, &host_key, &[1; 12]).unwrap();
    let grant = format!(r#"{{"t":"key-grant","pub":"{public}","box":"{sealed}"}}"#);
    assert_eq!(friend.read(SocketNews::Text(grant)), Some(Ok(payload)));
    assert_eq!(friend.read(SocketNews::Text(r#"{"t":"error","error":"nohost"}"#.into())), Some(Err(JoinError::NoHost)));
    assert_eq!(friend.read(SocketNews::Text(r#"{"t":"error","error":"busy"}"#.into())), Some(Err(JoinError::Failed)));
    assert_eq!(friend.read(SocketNews::Closed), Some(Err(JoinError::NotFound)));
    assert_eq!(friend.read(SocketNews::Opened), None);
    assert_eq!(room_socket_url("https://relais.exemple.fr", "R"), "wss://relais.exemple.fr/r/R");
    assert_eq!(room_socket_url("wss://relais.exemple.fr", "R"), "wss://relais.exemple.fr/r/R");
}
