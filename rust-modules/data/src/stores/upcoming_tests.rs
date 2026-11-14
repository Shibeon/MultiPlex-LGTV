//! The upcoming store against a scripted read: the job it emits, its one answer, the strip it
//! computes, and the rules that forget (another show, another profile, another language). No
//! network: the seam is a fake.

use super::*;
use crate::metadata::{Episode, EpisodeList};
use crate::watchlist::decode_hex;
use std::sync::Mutex;

fn hex(n: u8) -> [u8; 12] {
    let mut h = decode_hex("5e5eac000000000000000000").unwrap();
    h[11] = n;
    h
}

struct Fake {
    answer: Mutex<Result<Vec<ProviderEpisode>, WatchlistError>>,
    asked: Mutex<Vec<(String, Option<String>)>>,
}

impl Fake {
    fn new(answer: Result<Vec<ProviderEpisode>, WatchlistError>) -> Arc<Fake> {
        Arc::new(Fake { answer: Mutex::new(answer), asked: Mutex::new(Vec::new()) })
    }
}

impl UpcomingIo for Fake {
    fn read(&self, _: &WatchlistAuth, hex: &str, language: Option<&str>) -> Result<Vec<ProviderEpisode>, WatchlistError> {
        self.asked.lock().unwrap().push((hex.to_string(), language.map(str::to_string)));
        self.answer.lock().unwrap().clone()
    }
}

fn announced() -> Vec<ProviderEpisode> {
    vec![
        ProviderEpisode { index: 1, title: Some("Pilot".into()), aired: Some("2026-01-01".into()), ..Default::default() },
        ProviderEpisode { index: 3, title: Some("TBA".into()), aired: Some("2031-02-03".into()), ..Default::default() },
        ProviderEpisode { index: 2, title: Some("Next".into()), aired: None, ..Default::default() },
    ]
}

fn server(indices: &[i64]) -> EpisodeList {
    indices.iter().map(|&index| Episode { rk: format!("e{index}"), index, ..Default::default() }).collect()
}

/// Run everything the store emitted, as an inline runner would, and return how many ran.
fn run_emitted(s: &mut UpcomingStore) -> usize {
    let jobs = s.take_jobs();
    let n = jobs.len();
    for job in jobs {
        let StoreJob::Upcoming(job, port) = job else { panic!("not an upcoming job") };
        finish_upcoming_job(&port, job.addr, job.req);
    }
    n
}

#[test]
fn a_season_is_read_once_and_its_strip_lists_what_the_server_lacks() {
    let fake = Fake::new(Ok(announced()));
    let mut s = UpcomingStore::with_io(fake.clone());
    let input = WatchlistInput::for_test(1);
    let show = [hex(1), hex(2)];
    assert!(s.wants(hex(1), "fr"));
    s.want(hex(1), &show, &input, "fr");
    let jobs = s.take_jobs();
    assert_eq!(jobs.len(), 1, "one read for the season");
    let StoreJob::Upcoming(job, port) = &jobs[0] else { panic!("not an upcoming job") };
    assert_eq!(job.addr, Addr { to: MachineId::Store(StoreId::Upcoming.ord()), req: RequestId(1) });
    assert_eq!(job.req.hex(), hex(1));
    assert_eq!(jobs[0].store(), StoreId::Upcoming);
    assert_eq!(jobs[0].admission(), super::super::Admission::Direct);
    finish_upcoming_job(port, job.addr, job.req.clone());
    assert_eq!(*fake.asked.lock().unwrap(), vec![("5e5eac000000000000000001".to_string(), Some("fr".to_string()))]);

    // Asked: the same season is not asked again while the read is out or after it landed.
    s.want(hex(1), &show, &input, "fr");
    assert!(!s.has_jobs());
    let before = s.gen();
    assert!(s.pump(&WatchlistInput::profile_for_test(1)));
    assert_ne!(s.gen(), before);
    assert!(!s.wants(hex(1), "fr"));
    s.want(hex(1), &show, &input, "fr");
    assert!(!s.has_jobs());

    let strip = s.strip(hex(1), &server(&[1]), Some("2026-11-14"));
    assert_eq!(strip.iter().map(|e| e.index).collect::<Vec<_>>(), vec![2, 3]);
    assert_eq!(strip[1].title, "TBA", "a placeholder is kept as sent; the card words it");
    let again = s.strip(hex(1), &server(&[1]), Some("2026-11-14"));
    assert_eq!(&*again, &*strip);
    assert!(s.strip(hex(2), &server(&[1]), Some("2026-11-14")).is_empty(), "a season not read shows nothing");
}

#[test]
fn a_refused_job_is_asked_again_after_the_wait_and_a_failure_is_not() {
    let fake = Fake::new(Err(WatchlistError::Transport));
    let mut s = UpcomingStore::with_io(fake.clone());
    let input = WatchlistInput::for_test(1);
    let profile = WatchlistInput::profile_for_test(1);
    s.want(hex(1), &[hex(1)], &input, "en");
    let jobs = s.take_jobs();
    let StoreJob::Upcoming(job, port) = &jobs[0] else { panic!("not an upcoming job") };
    port.refused(job.addr);
    s.pump(&profile);
    for _ in 1..RETRY_FRAMES {
        s.pump(&profile);
        assert!(!s.has_jobs(), "the retry waits out its frames");
    }
    s.pump(&profile);
    assert_eq!(run_emitted(&mut s), 1, "asked again");
    assert!(!s.pump(&profile), "a failure changes nothing on the strip");
    for _ in 0..=RETRY_FRAMES {
        s.pump(&profile);
    }
    assert!(!s.has_jobs(), "a failed read is not retried within the show visit");
    assert!(!s.wants(hex(1), "en"));
}

#[test]
fn another_show_profile_or_language_forgets_what_was_read() {
    let fake = Fake::new(Ok(announced()));
    let mut s = UpcomingStore::with_io(fake.clone());
    let input = WatchlistInput::for_test(1);
    s.want(hex(1), &[hex(1)], &input, "en");
    run_emitted(&mut s);
    s.pump(&WatchlistInput::profile_for_test(1));
    assert!(s.announced(hex(1)).is_some());

    // Another show: its seasons do not include hex(1), which is forgotten.
    s.want(hex(9), &[hex(9)], &input, "en");
    assert!(s.announced(hex(1)).is_none());
    run_emitted(&mut s);

    // Another language: everything is read again in it.
    s.want(hex(9), &[hex(9)], &input, "de");
    assert_eq!(run_emitted(&mut s), 1);

    // Another profile seated: the pump forgets the identity and its seasons.
    s.pump(&WatchlistInput::profile_for_test(1));
    assert!(s.announced(hex(9)).is_some());
    assert!(s.pump(&WatchlistInput::profile_for_test(2)));
    assert!(s.announced(hex(9)).is_none());
    assert!(s.wants(hex(9), "de"), "the new profile reads it again");
}

#[test]
fn without_a_credential_nothing_is_asked_and_the_season_settles() {
    let fake = Fake::new(Ok(announced()));
    let mut s = UpcomingStore::with_io(fake.clone());
    s.want(hex(1), &[hex(1)], &WatchlistInput::signed_out_for_test(1), "en");
    assert!(!s.has_jobs());
    assert!(!s.wants(hex(1), "en"), "the caller stops offering it");
    assert!(fake.asked.lock().unwrap().is_empty());
    // The same profile publication keeps the settled season; the next one (a sign-in that seats a
    // credential) forgets it, and the season is asked under the new identity.
    assert!(!s.pump(&WatchlistInput::profile_for_test(1)));
    assert!(!s.wants(hex(1), "en"));
    assert!(s.pump(&WatchlistInput::profile_for_test(2)), "a new identity forgets the no-credential state");
    assert!(s.wants(hex(1), "en"));
    s.want(hex(1), &[hex(1)], &WatchlistInput::for_test(2), "en");
    assert_eq!(run_emitted(&mut s), 1, "asked once the credential is there");
    assert!(s.pump(&WatchlistInput::profile_for_test(2)));
    assert!(s.announced(hex(1)).is_some());
}

#[test]
fn a_not_found_season_reads_empty_and_a_reset_drops_a_job_not_yet_taken() {
    let fake = Fake::new(Err(WatchlistError::NotFound));
    let mut s = UpcomingStore::with_io(fake.clone());
    let input = WatchlistInput::for_test(1);
    s.want(hex(1), &[hex(1)], &input, "en");
    run_emitted(&mut s);
    assert!(s.pump(&WatchlistInput::profile_for_test(1)));
    assert_eq!(s.announced(hex(1)), Some(&[][..]));

    s.want(hex(2), &[hex(1), hex(2)], &input, "en");
    assert!(s.has_jobs());
    s.reset();
    assert!(!s.has_jobs(), "the credential goes with the reset");
    assert!(s.announced(hex(1)).is_none());
}
