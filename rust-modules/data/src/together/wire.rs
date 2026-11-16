//! The session's messages, exactly as the web client writes them (`apps/web/src/together.ts`,
//! `Tick`, `Intent`, `Bye`; the relay's README, "Protocole de synchronisation"): a JSON object,
//! encrypted whole (`plx_base::crypto::seal`) and sent as one binary frame the relay forwards
//! untouched.
//!
//! **Writing** follows the web's key order (`JSON.stringify` keeps insertion order) and its number
//! format (an integral position is `61`, not `61.0`), so a message this client seals is the same
//! bytes the web client would seal; the interop fixtures hold that. **Reading** is as lenient as the
//! web's: a message is any object with a string `id` and a known `t`; a missing or mistyped field
//! reads as the web reads it (`Number(x) || 0`, `!!x`, a stamp only from a finite non-negative
//! number).
//!
//! Only position and play/pause circulate. There is no field for volume, subtitles, audio track or
//! quality, so nothing this module writes can carry them.

use serde::Serialize;
use serde_json::Value;

use super::sync::MAX_SAFE_CLOCK;

/// What a participant watches, found again on each side by its Plex identifier.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SharedItem {
    pub guid: String,
    /// The host's server and rating key: enough when the guest reaches the same server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
}

impl SharedItem {
    /// The web's `itemId`: the guid, else `server/key`.
    pub fn identity(&self) -> String {
        if self.guid.is_empty() {
            format!("{}/{}", self.server.as_deref().unwrap_or("undefined"), self.key.as_deref().unwrap_or("undefined"))
        } else {
            self.guid.clone()
        }
    }

    fn read(v: &Value) -> Option<SharedItem> {
        let o = v.as_object()?;
        let s = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
        Some(SharedItem {
            guid: s("guid").unwrap_or_default(),
            server: s("server"),
            key: s("key"),
            kind: s("type").unwrap_or_default(),
            title: s("title").unwrap_or_default(),
            sub: s("sub"),
        })
    }
}

/// A command's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncAction {
    Play,
    Pause,
    Seek,
}

impl SyncAction {
    fn read(v: Option<&Value>) -> Option<SyncAction> {
        match v.and_then(Value::as_str)? {
            "play" => Some(SyncAction::Play),
            "pause" => Some(SyncAction::Pause),
            "seek" => Some(SyncAction::Seek),
            _ => None,
        }
    }
}

/// Presence, every 3 s (README: `tick`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Tick {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
    pub host: bool,
    #[serde(serialize_with = "js_number")]
    pub time: f64,
    pub playing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<SharedItem>,
    pub live: bool,
    pub c: u64,
    pub by: String,
    /// Host only: the session is visible to friends. This client never makes one visible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vis: Option<bool>,
    /// Host only: when it took the hand (Lamport clock; 0 for the original host).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hc: Option<u64>,
    /// Arriving or returning: asks everyone for a tick now. Written only when true.
    #[serde(skip_serializing_if = "is_false")]
    pub want: bool,
}

/// A stamped command (README: `intent`). The stamp's author is `id`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Intent {
    pub id: String,
    pub name: String,
    pub action: SyncAction,
    #[serde(serialize_with = "js_number")]
    pub time: f64,
    pub playing: bool,
    pub c: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<SharedItem>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Bye {
    pub id: String,
}

/// One session message, written with the `t` tag first as the web writes it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Message {
    Tick(Tick),
    Intent(Intent),
    Bye(Bye),
}

/// A received tick, read the web's way (`TickIn` of `togethersync.ts` plus the peer fields).
#[derive(Clone, Debug, PartialEq)]
pub struct TickIn {
    pub id: String,
    pub name: String,
    pub thumb: Option<String>,
    pub host: bool,
    pub live: bool,
    pub time: f64,
    pub playing: bool,
    pub item: Option<SharedItem>,
    /// `None`: no usable stamp (an older client).
    pub stamp: Option<(u64, String)>,
    pub vis: Option<bool>,
    pub hc: u64,
    pub want: bool,
}

/// A received command (`IntentIn`): `c` absent is an older client, dated on arrival.
#[derive(Clone, Debug, PartialEq)]
pub struct IntentIn {
    pub id: String,
    pub name: String,
    pub action: Option<SyncAction>,
    pub time: f64,
    pub playing: bool,
    pub item: Option<SharedItem>,
    pub c: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    Tick(TickIn),
    Intent(IntentIn),
    Bye { id: String },
}

impl Message {
    /// The JSON text the web client would write for this message.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// `typeof x === 'number' && isFinite(x) ? x : 0`.
pub fn num(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(0.0)
}

/// JavaScript truthiness of a JSON value.
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

/// `stampOf`'s clock half: a finite number >= 0, floored, and no larger than a JavaScript number
/// holds exactly ([`MAX_SAFE_CLOCK`]), so counting on from it never overflows.
fn clock_of(v: Option<&Value>) -> Option<u64> {
    let x = v.and_then(Value::as_f64)?;
    (x.is_finite() && x >= 0.0).then(|| (x.floor().min(MAX_SAFE_CLOCK as f64) as u64).min(MAX_SAFE_CLOCK))
}

/// `String(x).slice(0, 60)` for the display name (UTF-16 units approximated by chars).
fn name_of(v: Option<&Value>) -> String {
    let s = match v {
        Some(Value::String(s)) => s.clone(),
        None => "undefined".to_string(),
        Some(other) => other.to_string(),
    };
    s.chars().take(60).collect()
}

/// Reads a decrypted message. `None` for anything the web would drop: not an object, no string
/// `id`, or an unknown `t`. (The caller drops its own echoes by `id`.)
pub fn read_message(plain: &[u8]) -> Option<Incoming> {
    let v: Value = serde_json::from_slice(plain).ok()?;
    let o = v.as_object()?;
    let id = o.get("id")?.as_str()?.to_string();
    match o.get("t").and_then(Value::as_str)? {
        "bye" => Some(Incoming::Bye { id }),
        "tick" => Some(Incoming::Tick(TickIn {
            name: name_of(o.get("name")),
            thumb: o.get("thumb").and_then(Value::as_str).filter(|t| t.starts_with("https://")).map(str::to_string),
            host: truthy(o.get("host")),
            live: truthy(o.get("live")),
            time: num(o.get("time")),
            playing: truthy(o.get("playing")),
            item: o.get("item").and_then(SharedItem::read),
            stamp: clock_of(o.get("c")).map(|c| (c, o.get("by").and_then(Value::as_str).unwrap_or("").to_string())),
            vis: o.get("vis").and_then(Value::as_bool),
            hc: clock_of(o.get("hc")).unwrap_or(0),
            want: truthy(o.get("want")),
            id,
        })),
        "intent" => Some(Incoming::Intent(IntentIn {
            name: name_of(o.get("name")),
            action: SyncAction::read(o.get("action")),
            time: num(o.get("time")),
            playing: truthy(o.get("playing")),
            item: o.get("item").and_then(SharedItem::read),
            c: clock_of(o.get("c")),
            id,
        })),
        _ => None,
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A number as `JSON.stringify` writes it: an integral value without a fraction (`61`, `-0` as
/// `0`), any other finite value in its shortest round-trip form, a non-finite one as `null`.
pub fn js_number<S: serde::Serializer>(x: &f64, s: S) -> Result<S::Ok, S::Error> {
    let x = *x;
    if !x.is_finite() {
        s.serialize_unit()
    } else if x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 {
        s.serialize_i64(x as i64)
    } else {
        s.serialize_f64(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie() -> SharedItem {
        SharedItem {
            guid: "plex://movie/5d776b59ad5437001f79c6f8".into(),
            server: Some("machine-a".into()),
            key: Some("1234".into()),
            kind: "movie".into(),
            title: "Le Film".into(),
            sub: Some("2019".into()),
        }
    }

    /// A clock past what a JavaScript number holds exactly is read as `Number.MAX_SAFE_INTEGER`.
    #[test]
    fn huge_clocks_are_clamped_on_read() {
        let safe = super::super::sync::MAX_SAFE_CLOCK;
        let Some(Incoming::Tick(t)) = read_message(br#"{"t":"tick","id":"x","c":1e300,"by":"x","hc":1e300,"live":true}"#) else { panic!() };
        assert_eq!((t.stamp.map(|s| s.0), t.hc), (Some(safe), safe));
        let Some(Incoming::Intent(i)) = read_message(br#"{"t":"intent","id":"x","action":"play","c":18446744073709551615}"#) else { panic!() };
        assert_eq!(i.c, Some(safe));
    }

    /// The README's `tick`, with every optional field, in the web's key order and number format.
    #[test]
    fn tick_golden() {
        let m = Message::Tick(Tick {
            id: "aBcD-_12".into(),
            name: "Hôte".into(),
            thumb: Some("https://plex.tv/users/x/avatar".into()),
            host: true,
            time: 61.5,
            playing: true,
            item: Some(movie()),
            live: true,
            c: 3,
            by: "aBcD-_12".into(),
            vis: Some(true),
            hc: Some(0),
            want: false,
        });
        assert_eq!(
            m.to_json(),
            r#"{"t":"tick","id":"aBcD-_12","name":"Hôte","thumb":"https://plex.tv/users/x/avatar","host":true,"time":61.5,"playing":true,"item":{"guid":"plex://movie/5d776b59ad5437001f79c6f8","server":"machine-a","key":"1234","type":"movie","title":"Le Film","sub":"2019"},"live":true,"c":3,"by":"aBcD-_12","vis":true,"hc":0}"#
        );
        // The arrival form: no thumb, no item, nothing host-only, `want` written only when true.
        let m = Message::Tick(Tick {
            id: "n3w-0ne_".into(),
            name: "Nouveau".into(),
            thumb: None,
            host: false,
            time: 0.0,
            playing: false,
            item: None,
            live: false,
            c: 0,
            by: String::new(),
            vis: None,
            hc: None,
            want: true,
        });
        assert_eq!(
            m.to_json(),
            r#"{"t":"tick","id":"n3w-0ne_","name":"Nouveau","host":false,"time":0,"playing":false,"live":false,"c":0,"by":"","want":true}"#
        );
    }

    #[test]
    fn intent_and_bye_golden() {
        let m = Message::Intent(Intent {
            id: "zz9_Q-x1".into(),
            name: "Ami".into(),
            action: SyncAction::Seek,
            time: 300.0,
            playing: false,
            c: 4,
            item: None,
        });
        assert_eq!(m.to_json(), r#"{"t":"intent","id":"zz9_Q-x1","name":"Ami","action":"seek","time":300,"playing":false,"c":4}"#);
        assert_eq!(Message::Bye(Bye { id: "zz9_Q-x1".into() }).to_json(), r#"{"t":"bye","id":"zz9_Q-x1"}"#);
    }

    /// Every message this module writes reads back to the same values.
    #[test]
    fn written_messages_read_back() {
        let tick = Tick {
            id: "a".into(),
            name: "A".into(),
            thumb: None,
            host: true,
            time: 12.25,
            playing: true,
            item: Some(movie()),
            live: true,
            c: 7,
            by: "b".into(),
            vis: Some(false),
            hc: Some(3),
            want: true,
        };
        match read_message(Message::Tick(tick).to_json().as_bytes()) {
            Some(Incoming::Tick(t)) => {
                assert_eq!((t.time, t.playing, t.host, t.live, t.want, t.hc), (12.25, true, true, true, true, 3));
                assert_eq!(t.stamp, Some((7, "b".into())));
                assert_eq!(t.item, Some(movie()));
                assert_eq!(t.vis, Some(false));
            }
            other => panic!("{other:?}"),
        }
    }

    /// The web's coercions: a missing stamp is an older client; odd types read as the web reads them.
    #[test]
    fn reading_is_as_lenient_as_the_web() {
        let i = read_message(br#"{"t":"intent","id":"old","action":"pause","time":10,"playing":false}"#);
        assert!(matches!(i, Some(Incoming::Intent(IntentIn { c: None, action: Some(SyncAction::Pause), .. }))));
        let t = read_message(br#"{"t":"tick","id":"x","name":42,"host":1,"time":"5","c":-1,"by":7,"thumb":"http://x"}"#);
        match t {
            Some(Incoming::Tick(t)) => {
                assert_eq!(t.name, "42");
                assert!(t.host);
                assert_eq!(t.time, 0.0);
                assert_eq!(t.stamp, None);
                assert_eq!(t.thumb, None);
            }
            other => panic!("{other:?}"),
        }
        let t = read_message(br#"{"t":"tick","id":"x","c":2.9,"by":7}"#);
        assert!(matches!(t, Some(Incoming::Tick(TickIn { stamp: Some((2, ref by)), .. })) if by.is_empty()));
        for bad in [&br#"{"t":"tick"}"#[..], br#"{"t":"tock","id":"x"}"#, br#"[1]"#, b"not json", br#"{"t":"bye","id":3}"#] {
            assert_eq!(read_message(bad), None);
        }
    }

    #[test]
    fn numbers_are_written_as_json_stringify_writes_them() {
        let w = |x: f64| {
            let mut out = Vec::new();
            js_number(&x, &mut serde_json::Serializer::new(&mut out)).unwrap();
            String::from_utf8(out).unwrap()
        };
        assert_eq!(w(61.0), "61");
        assert_eq!(w(-0.0), "0");
        assert_eq!(w(1234.567), "1234.567");
        assert_eq!(w(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(w(f64::NAN), "null");
    }
}
