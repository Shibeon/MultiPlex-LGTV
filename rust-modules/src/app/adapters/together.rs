//! **The Watch Together runner** (`plx_data::stores::together`): the IO of the together store and
//! nothing else. [`TogetherRunner::start`] executes the store's jobs: work jobs on a small worker
//! through [`LiveTogetherIo`] (randomness from `/dev/urandom`, PBKDF2, seal and open, the relay's
//! `/rooms` and `/friends`, the plex.tv friends read, the owner's delegation token, a friend's key
//! exchange), each socket on its own off-frame worker running `plx_net::ws::run`, and the sends and
//! closes on that socket's outbox, in the order the store pushed them. Every answer and socket event
//! lands in the store's mailbox (which wakes the frame loop); a job that could not start is refused
//! through its port, a socket that could not start reports `Closed`. A socket's entry leaves the
//! runner's table on its `Close`, or at the next job once its worker has ended by itself.
//!
//! **Relay TLS** (owner approved, kept narrow): each relay host this runner dials, and only those, is
//! admitted to the bundled public roots (`keypin::admit_relay_host`), so a 2020 firmware whose store
//! lacks the relay certificate's issuer retries once against `pkg/le-roots.pem` — the HTTP requests
//! through `keypin`'s ladder, a socket by dialling again with `Tls::CaBundle` after a refused
//! handshake (libcurl 60). Chain, dates and name are always checked.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use plx_data::stores::together::{
    finish_together_job, Frame, SocketNews, TogetherAuth, TogetherIo, TogetherJob, TogetherPort, KEY_WAIT_MS,
};
use plx_data::together::friends::GrantPayload;
use plx_data::together::session::JoinError;
use plx_machine::job::{Job, Port};
use plx_net::net::connect_only::OpenError;
use plx_net::net::{keypin, Timeouts, Tls};
use plx_net::ws::{self, Dial, End, Event, Outbox, Refusal};

/// A relay message is small (a sealed session message, a control message); past this the
/// connection is failed rather than buffered.
const MESSAGE_LIMIT: usize = 1 << 20;
/// A session socket with nothing heard for this long is ended (the session ticks every 3 s).
const SOCKET_IDLE: Duration = Duration::from_secs(45);
/// A relay answer (`/rooms`, `/friends`) is a few kilobytes.
const RELAY_BODY_LIMIT: usize = 1 << 20;
/// libcurl's 60: the peer's certificate was not verified.
const TLS_VERIFY_FAILED: i32 = 60;

/// The host of a `ws(s)://` or `http(s)://` URL, for the roots admission.
fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') || authority.starts_with('[') {
        return None;
    }
    Some(authority.rsplit_once(':').map_or(authority, |(h, _)| h))
}

/// Admit the relay host of `url` to the bundled roots (`https`/`wss` only).
fn admit(url: &str) {
    if url.starts_with("https://") || url.starts_with("wss://") {
        if let Some(host) = host_of(url) {
            keypin::admit_relay_host(host);
        }
    }
}

/// Dial `url`, and once more with the bundled roots when an admitted relay's handshake was refused
/// for its certificate. `install` receives each attempt's outbox before it dials.
fn dial(off: &plx_base::task::OffFrame, url: &str, idle: Duration, cancel: &AtomicBool, install: &mut dyn FnMut(Outbox), on: &mut dyn FnMut(Event)) -> End {
    let attempt = |trust: Tls<'_>, install: &mut dyn FnMut(Outbox), on: &mut dyn FnMut(Event)| -> End {
        let Some((outbox, inbox)) = ws::channel() else { return End::NoEntropy };
        install(outbox);
        ws::run(off, Dial { url, trust, limit: MESSAGE_LIMIT, idle }, cancel, inbox, on)
    };
    let end = attempt(Tls::Ca, install, on);
    let refused_cert = matches!(&end, End::Refused(Refusal::Open(OpenError::Connect { rc, .. })) if *rc == TLS_VERIFY_FAILED);
    if refused_cert && url.starts_with("wss://") {
        if let Some(bundle) = host_of(url).and_then(keypin::relay_roots_bundle) {
            plx_base::eventlog::log("together: relay certificate refused by the device store — retrying against the bundled public roots");
            return attempt(Tls::CaBundle(&bundle), install, on);
        }
    }
    end
}

/// The production IO of a together job. `off` is the worker's proof (a key exchange dials).
pub(crate) struct LiveTogetherIo<'a> {
    pub off: &'a plx_base::task::OffFrame,
}

fn timeouts(total_ms: u32) -> Timeouts {
    // `c_long` is 32 bits on the armv7 target: no lossless `From<u32>` there, so
    // clamp to the widest value both widths hold (as curlio does).
    Timeouts {
        connect_s: 4,
        total_s: 0,
        total_ms: total_ms.min(i32::MAX as u32) as std::ffi::c_long,
        ..plx_net::net::API
    }
}

fn body_of(resp: Result<plx_net::net::Resp, plx_net::net::RequestFailure>) -> Option<String> {
    let resp = resp.ok().filter(|r| r.status == 200)?;
    String::from_utf8(resp.body).ok()
}

impl TogetherIo for LiveTogetherIo<'_> {
    fn random(&self, n: usize) -> Option<Vec<u8>> {
        let mut out = vec![0u8; n];
        std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut out)).ok()?;
        Some(out)
    }

    fn get(&self, url: &str, timeout_ms: u32) -> Option<String> {
        admit(url);
        body_of(plx_net::net::request_evidence(url, &[], "GET", None, timeouts(timeout_ms), false, Some(RELAY_BODY_LIMIT), None))
    }

    fn post_text(&self, url: &str, body: &str, timeout_ms: u32) -> Option<String> {
        admit(url);
        let headers = ["Content-Type: text/plain;charset=UTF-8".to_string()];
        body_of(plx_net::net::request_evidence(url, &headers, "POST", Some(body.as_bytes()), timeouts(timeout_ms), false, Some(RELAY_BODY_LIMIT), None))
    }

    fn delegation(&self, base: &str, token: &str) -> Option<String> {
        let client_id = plx_plex::plex::session::peek().client_id.clone();
        plx_plex::plex::together::delegation_token(base, &client_id, token)
    }

    fn friends_xml(&self, auth: &TogetherAuth) -> Option<String> {
        plx_plex::plex::account::AccountClient::new(&auth.client_id, Some(auth.account_token())).plex_friends_xml()
    }

    fn exchange(&self, url: &str, first: &str, read: &mut dyn FnMut(SocketNews) -> Option<Result<GrantPayload, JoinError>>) -> Result<GrantPayload, JoinError> {
        admit(url);
        let cancel = AtomicBool::new(false);
        let outbox = std::cell::RefCell::<Option<Outbox>>::new(None);
        let mut answer = None;
        let mut on = |event: Event| {
            if answer.is_some() {
                return;
            }
            let news = match event {
                Event::Opened => {
                    if let Some(o) = &*outbox.borrow() {
                        o.text(first.to_string());
                    }
                    SocketNews::Opened
                }
                Event::Text(t) => SocketNews::Text(t),
                Event::Binary(b) => SocketNews::Binary(b),
                Event::Closed { .. } | Event::Failed(_) => SocketNews::Closed,
            };
            answer = read(news);
            if answer.is_some() {
                cancel.store(true, Ordering::Release);
            }
        };
        let end = dial(self.off, url, Duration::from_millis(KEY_WAIT_MS.into()), &cancel, &mut |o| *outbox.borrow_mut() = Some(o), &mut on);
        match answer {
            Some(a) => a,
            None => Err(match end {
                End::Idle | End::TimedOut => JoinError::NoHost,
                End::Refused(_) | End::Failed(_) | End::NoEntropy => JoinError::Failed,
                End::Closed { .. } | End::Lost | End::Cancelled => JoinError::NotFound,
            }),
        }
    }
}

/// A socket the runner keeps: the outbox of its current attempt, its cancel flag, and whether its
/// worker has ended (set by the worker, last).
pub(crate) struct Live {
    outbox: Arc<Mutex<Option<Outbox>>>,
    cancel: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
}

impl Live {
    fn ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }
}

/// The runner. `Held` (tests) parks the jobs instead of starting them.
pub(crate) enum TogetherRunner {
    Live { sockets: HashMap<u32, Live> },
    #[cfg(test)]
    Held { parked: Vec<TogetherJob> },
}

impl Default for TogetherRunner {
    fn default() -> Self {
        TogetherRunner::Live { sockets: HashMap::new() }
    }
}

impl TogetherRunner {
    pub(crate) fn start(&mut self, jobs: Vec<TogetherJob>) {
        for job in jobs {
            self.start_one(job);
        }
    }

    fn start_one(&mut self, job: TogetherJob) {
        let sockets = match self {
            TogetherRunner::Live { sockets } => sockets,
            #[cfg(test)]
            TogetherRunner::Held { parked } => {
                parked.push(job);
                return;
            }
        };
        // A socket that ended by itself (the relay closed it, the network failed) never sees a
        // `Close`: its entry goes with the next job.
        sockets.retain(|_, live| !live.ended());
        match job {
            TogetherJob::Work(Job { addr, req }, port) => {
                let refusal = port.clone();
                let started = plx_base::task::spawn_off_frame("together", move |off| {
                    finish_together_job(&port, addr, req, &LiveTogetherIo { off });
                });
                if !started {
                    refusal.refused(addr);
                }
            }
            TogetherJob::Socket { conn, url, port } => {
                let live = Live { outbox: Arc::default(), cancel: Arc::default(), ended: Arc::default() };
                if open_socket(conn, url, port.clone(), Arc::clone(&live.outbox), Arc::clone(&live.cancel), Arc::clone(&live.ended)) {
                    sockets.insert(conn, live);
                } else {
                    port.socket(conn, SocketNews::Closed);
                }
            }
            TogetherJob::Send { conn, frame } => {
                let outbox = sockets.get(&conn).and_then(|s| s.outbox.lock().unwrap_or_else(|e| e.into_inner()).clone());
                if let Some(outbox) = outbox {
                    // A refused send means the connection is gone; its `Closed` is on its way.
                    let _ = match frame {
                        Frame::Text(t) => outbox.text(t),
                        Frame::Binary(b) => outbox.binary(b),
                    };
                }
            }
            TogetherJob::Close { conn } => {
                if let Some(live) = sockets.remove(&conn) {
                    let outbox = live.outbox.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    let closed = outbox.is_some_and(|o| o.close(1000, String::new()));
                    if !closed {
                        live.cancel.store(true, Ordering::Release);
                    }
                }
            }
        }
    }
}

/// One socket's worker: `ws::run` (and the roots retry), its events to the store, `Closed` last.
fn open_socket(conn: u32, url: String, port: TogetherPort, slot: Arc<Mutex<Option<Outbox>>>, cancel: Arc<AtomicBool>, ended: Arc<AtomicBool>) -> bool {
    admit(&url);
    plx_base::task::spawn_off_frame("together-ws", move |off| {
        let mut on = |event: Event| match event {
            Event::Opened => port.socket(conn, SocketNews::Opened),
            Event::Text(t) => port.socket(conn, SocketNews::Text(t)),
            Event::Binary(b) => port.socket(conn, SocketNews::Binary(b)),
            Event::Closed { .. } | Event::Failed(_) => {}
        };
        let mut install = |o: Outbox| *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(o);
        let end = dial(off, &url, SOCKET_IDLE, &cancel, &mut install, &mut on);
        if !matches!(end, End::Closed { .. } | End::Cancelled) {
            plx_base::eventlog::log(&format!("together: socket {conn} ended: {}", end_word(&end)));
        }
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
        ended.store(true, Ordering::Release);
        port.socket(conn, SocketNews::Closed);
    })
}

/// A word for the log (no address, no message).
fn end_word(end: &End) -> String {
    let word = match end {
        End::Refused(Refusal::Url) => "refused (address)",
        End::Refused(Refusal::Entropy) => "refused (entropy)",
        End::Refused(Refusal::Open(OpenError::Connect { rc, .. })) => return format!("refused (libcurl {rc})"),
        End::Refused(Refusal::Open(OpenError::Unavailable)) => "refused (no libcurl)",
        End::Refused(Refusal::Open(OpenError::SerialTls)) => "refused (serial TLS only)",
        End::Refused(Refusal::Open(OpenError::Address)) => "refused (address)",
        End::Refused(Refusal::Open(OpenError::Refused(why))) => return format!("refused ({why})"),
        End::Refused(Refusal::Open(OpenError::NoSocket)) => "refused (no socket)",
        End::Refused(Refusal::Open(OpenError::Cancelled)) => "cancelled",
        End::Closed { .. } => "closed",
        End::Failed(_) => "failed",
        End::Lost => "lost",
        End::TimedOut => "timed out",
        End::Idle => "idle",
        End::NoEntropy => "no entropy",
        End::Cancelled => "cancelled",
    };
    word.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relay_host_is_read_from_its_url_and_nothing_else() {
        assert_eq!(host_of("wss://relais.exemple.fr/r/abc"), Some("relais.exemple.fr"));
        assert_eq!(host_of("https://relais.exemple.fr:8443/rooms?ids=a"), Some("relais.exemple.fr"));
        assert_eq!(host_of("wss://x.plex.direct@evil.example/r"), None);
        assert_eq!(host_of("wss://[::1]:9/r"), None);
        assert_eq!(host_of("relais"), None);
    }

    /// A socket that ends by itself (here: an address the socket refuses at once) leaves the
    /// runner's table by the next job, without waiting for a `Close` that may never come.
    #[test]
    fn a_socket_that_ended_by_itself_is_pruned() {
        let store = plx_data::stores::together::TogetherStore::new();
        let mut runner = TogetherRunner::default();
        runner.start(vec![TogetherJob::Socket { conn: 1, url: "pas-une-adresse".into(), port: store.port() }]);
        let start = std::time::Instant::now();
        loop {
            let TogetherRunner::Live { sockets } = &runner else { panic!() };
            if sockets.get(&1).is_none_or(Live::ended) {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "the socket worker ends");
            std::thread::sleep(Duration::from_millis(5));
        }
        runner.start(vec![TogetherJob::Send { conn: 2, frame: Frame::Text("t".into()) }]);
        let TogetherRunner::Live { sockets } = &runner else { panic!() };
        assert!(sockets.is_empty(), "the ended socket's entry is gone");
    }

    #[test]
    fn a_held_runner_parks_every_job_in_order() {
        let store = plx_data::stores::together::TogetherStore::new();
        let mut runner = TogetherRunner::Held { parked: Vec::new() };
        runner.start(vec![
            TogetherJob::Socket { conn: 1, url: "wss://r/r/a".into(), port: store.port() },
            TogetherJob::Send { conn: 1, frame: Frame::Text("t".into()) },
            TogetherJob::Close { conn: 1 },
        ]);
        let TogetherRunner::Held { parked } = runner else { panic!() };
        assert!(matches!(parked.as_slice(), [TogetherJob::Socket { .. }, TogetherJob::Send { .. }, TogetherJob::Close { .. }]));
    }
}

#[cfg(test)]
#[path = "together_e2e_tests.rs"]
mod e2e_tests;
