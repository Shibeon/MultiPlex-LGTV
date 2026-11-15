//! The Bridge's half of plex.tv's people search: the account the Search store may ask plex.tv as
//! ([`SearchInput::catalog`](plx_data::search::SearchInput)) is withheld from every run whose
//! answer plex.tv must not supply — a controlled one, a tape replay, a boot whose people search
//! would be refused — and handed over only to a live, settled session.

use std::sync::Arc;

use plx_plex::plex::session::Session;

use super::StoreSessionCapture;

fn settled() -> StoreSessionCapture {
    let session = Session { client_id: "synthetic-client".into(), account_token: "synthetic-token".into(), ..Default::default() };
    StoreSessionCapture { session: Some(Arc::new(session)), ..Default::default() }
}

fn asks_plex_tv(capture: &StoreSessionCapture) -> bool {
    capture.search_input(&plx_plex::plex::ClientKeys::default()).catalog.is_some()
}

#[test]
fn a_live_settled_session_lends_search_its_account() {
    assert!(asks_plex_tv(&settled()));
}

#[test]
fn no_unsettled_session_is_asked_as() {
    assert!(!asks_plex_tv(&StoreSessionCapture::default()));
}

#[test]
fn a_controlled_run_never_asks_plex_tv_for_people() {
    assert!(!asks_plex_tv(&StoreSessionCapture { controlled: true, ..settled() }));
}

/// A tape replay supplies no plex.tv answer, so the guard holds on the tape alone, whatever
/// `controlled` was computed from.
#[test]
fn a_tape_replay_never_asks_plex_tv_for_people() {
    assert!(!asks_plex_tv(&StoreSessionCapture { tape_active: true, ..settled() }));
}

#[test]
fn a_refused_people_search_is_never_asked() {
    assert!(!asks_plex_tv(&StoreSessionCapture { people_refused: true, ..settled() }));
}
