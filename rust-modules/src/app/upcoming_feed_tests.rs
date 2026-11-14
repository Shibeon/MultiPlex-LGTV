//! `Bridge::upcoming_feed` end to end on a mounted Detail page: the season's catalog id is read off
//! its `plex://season/` guid, the read is emitted and started in the same call (parked by the test
//! runner, then run on this thread), and the next feed hands the page the episodes the server lacks
//! that have not aired as of the published day. The read goes through a scripted seam: nothing here
//! reaches a network or an account.

use super::*;
use super::test_support::tick;
use plx_data::metadata::{Detail, Episode, Season};
use plx_data::stores::upcoming::UpcomingStore;
use plx_data::stores::StoreId;
use plx_data::upcoming::UpcomingIo;
use plx_data::watchlist::WatchlistAuth;
use plx_plex::plex::watchlist::{ProviderEpisode, WatchlistError};
use plx_screens::registry::ContentArg;
use std::sync::{Arc, Mutex};

const SEASON_HEX: &str = "5e5eac00000000000000002a";

/// A provider that announces episodes 1-4 of the season: 3 aired before "today", 4 after.
struct Provider {
    asked: Mutex<Vec<String>>,
}

impl UpcomingIo for Provider {
    fn read(&self, _: &WatchlistAuth, hex: &str, _: Option<&str>) -> Result<Vec<ProviderEpisode>, WatchlistError> {
        self.asked.lock().unwrap().push(hex.to_string());
        let ep = |index: i64, title: &str, aired: &str| ProviderEpisode {
            index,
            title: Some(title.into()),
            aired: Some(aired.into()),
            ..Default::default()
        };
        Ok(vec![ep(1, "One", "2026-01-01"), ep(2, "Two", "2026-01-08"), ep(3, "Missed", "2026-11-01"), ep(4, "TBA", "2026-12-24")])
    }
}

fn detail_page(d: &Dispatcher<AppHost>) -> &plx_screens::detail::DetailScreen {
    d.top_screen()
        .and_then(|s| s.as_any())
        .and_then(|s| s.downcast_ref::<plx_screens::detail::DetailScreen>())
        .expect("the Detail page is the top page")
}

#[test]
fn a_season_page_is_handed_the_announced_episodes_the_server_lacks_that_have_not_aired() {
    let _guard = plx_base::testlock::serial();
    plx_plex::plex::reset_servers_for_test();
    let sid = plx_plex::plex::register_for_test("upcoming-feed", "127.0.0.1", 9, "synthetic", "fixture");
    let mut d = Dispatcher::<AppHost>::new();
    let mut rig = Bridge::for_test(|| 0);
    rig.seat_watchlist_account_for_test(true);
    let provider = Arc::new(Provider { asked: Mutex::new(Vec::new()) });
    rig.stores.upcoming = UpcomingStore::with_io(provider.clone());
    rig.publish_today(Some([2026, 11, 14]));
    let episode = |index: i64| Episode { rk: format!("e{index}"), index, title: format!("Episode {index}"), ..Default::default() };
    plx_data::metadata::set_current_for_test(rig.stores.metadata.state_mut(), Some(Detail {
        sid,
        rk: "show".into(),
        kind: "show".into(),
        is_show: true,
        seasons: vec![Season {
            rk: "sk1".into(), index: 1, title: "Season 1".into(), leaf_count: 2, viewed_leaf_count: 0,
            guid: format!("plex://season/{SEASON_HEX}"),
        }],
        cur_season: 0,
        episodes: vec![episode(1), episode(2)].into(),
        ..Default::default()
    }));
    d.request(MachineId::Nav, NavOp::Root(AppArg::Content(ContentArg::Detail { sid, rk: "show".into() })));
    for now in 0..4 {
        d.frame_with(&mut rig, tick(now), Vec::new(), Vec::new(), &mut NoTap, false);
    }
    assert!(detail_page(&d).upcoming_for_test().1.is_empty(), "nothing before the read");

    rig.upcoming_feed(&mut d);
    assert_eq!(rig.run_held_store_jobs_of_for_test(StoreId::Upcoming), 1, "one read, started in the feed");
    assert_eq!(*provider.asked.lock().unwrap(), [SEASON_HEX], "asked by the season's catalog id");
    rig.upcoming_feed(&mut d);
    rig.assert_no_store_jobs_left_for_test();

    let list = rig.metadata_view().current().map(|d| d.episodes.id()).unwrap();
    let (for_list, cells) = detail_page(&d).upcoming_for_test();
    assert_eq!(for_list, list, "computed against the list the page shows");
    // 1 and 2 are on the server, 3 aired before today: only 4 is coming, its placeholder title worded.
    assert_eq!(cells.len(), 1, "{cells:?}");
    assert_eq!(cells[0].0, 4);
    assert_eq!(cells[0].1, plx_platform::i18n::msg::browse_detail_upcoming_title_tba().to_string());
    assert!(!cells[0].2.is_empty(), "an air line");

    // The season is read once per show visit: later feeds ask nothing again.
    rig.upcoming_feed(&mut d);
    assert_eq!(rig.run_held_store_jobs_of_for_test(StoreId::Upcoming), 0);
    assert_eq!(provider.asked.lock().unwrap().len(), 1);
    plx_data::metadata::set_current_for_test(rig.stores.metadata.state_mut(), None);
    plx_plex::plex::reset_servers_for_test();
}
