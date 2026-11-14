//! Privacy & data in a build with no reporting destination compiled in
//! (`ScreenInputs::telemetry_absent`), against a keyed build: the switches, the example reports
//! and the identifier documents are offered only where they can change something, and the page
//! that is left still walks and opens what it lists.

use super::*;
use super::test_support::*;
use plx_machine::machine::FocusKey;
use plx_platform::i18n::msg;
use plx_ui::focus::{FocusEngine, Outcome};
use plx_ui::screen::{By, Dir};

fn inputs(telemetry_absent: bool) -> crate::registry::ScreenInputs {
    crate::registry::ScreenInputs { telemetry_absent, ..crate::registry::ScreenInputs::default() }
}

fn page_for(telemetry_absent: bool) -> ConsentPage {
    let m = FixtureMeasure;
    let inputs = inputs(telemetry_absent);
    let c = test_cx_with(&m, &inputs);
    let (mut out, mut present) = sink();
    ConsentPage::settings(EntryId(1), &c, &mut mk_fx(&mut out, &mut present))
}

/// A keyed build keeps every row: the opt-out has to stay reachable wherever sending can happen.
#[test]
fn a_keyed_build_lists_the_switches_the_reports_and_the_identifier_documents() {
    let _guard = plx_base::testlock::serial();
    let page = page_for(false);
    assert_eq!(page.row_ids(), vec![
        RowId::Errors, RowId::Usage,
        RowId::PreviewCrash, RowId::PreviewUsage, RowId::Policy, RowId::ErrorsId, RowId::AnalyticsId,
        RowId::Delete,
    ]);
    assert_eq!(page.body(), msg::settings_consent_settings_copy());
    assert_eq!(page.follow.text(), msg::settings_consent_crash_detail(), "opens on the Errors switch");
}

/// With nothing to send, the page lists the policy and Delete all local data alone. Neither
/// identifier document (which print the upstream contact address) is a row, the rows keep their
/// own keys, and the page's sentence does not speak of reporting.
#[test]
fn a_build_without_a_destination_lists_only_the_policy_and_delete() {
    let _guard = plx_base::testlock::serial();
    let page = page_for(true);
    assert_eq!(page.row_ids(), vec![RowId::Policy, RowId::Delete]);
    assert_eq!(page.key_of(RowId::Policy), RowId::Policy.key().0, "focus is by identity, not position");
    assert_eq!(page.key_of(RowId::Delete), RowId::Delete.key().0);
    let sections: Vec<usize> = page.form.table.sections.iter().map(|s| s.rows.len()).filter(|n| *n > 0).collect();
    assert_eq!(sections, vec![1, 1], "Information (the policy) and On this TV (Delete)");
    assert_eq!(page.body(), msg::settings_consent_settings_copy_local());
    assert_eq!(page.follow.text(), msg::settings_consent_preview_policy(), "opens on the policy's sentence");
    assert!(!page.uncommitted() && page.band_labels().is_empty(), "no draft, so no Done");
}

/// The rows that are left still walk through the real engine: DOWN from the policy reaches
/// Delete, UP comes back, and nothing past either end is a hidden row.
#[test]
fn focus_walks_the_two_rows_that_are_left() {
    let _guard = plx_base::testlock::serial();
    let m = FixtureMeasure;
    let inputs = inputs(true);
    let c = test_cx_with(&m, &inputs);
    let (mut out, mut present) = sink();
    let page = ConsentPage::settings(EntryId(1), &c, &mut mk_fx(&mut out, &mut present));
    let mut engine: FocusEngine<u32> = FocusEngine::new();
    let owner = InputOwner::Entry(EntryId(1));
    let key = |id: RowId| FocusKey { entry: EntryId(1), elem: page.key_of(id) };
    engine.set(owner, key(RowId::Policy), Some(TABLE_GROUP), By::Dir);

    let outcome = engine.move_dir(owner, &page.view(), &[], Dir::Down, &c);
    assert!(matches!(outcome, Outcome::Moved { .. }), "DOWN from the policy: {outcome:?}");
    assert_eq!(engine.current(owner), Some(key(RowId::Delete)));
    let _ = engine.move_dir(owner, &page.view(), &[], Dir::Down, &c);
    assert_eq!(engine.current(owner), Some(key(RowId::Delete)), "nothing below Delete");
    let outcome = engine.move_dir(owner, &page.view(), &[], Dir::Up, &c);
    assert!(matches!(outcome, Outcome::Moved { .. }), "UP from Delete: {outcome:?}");
    assert_eq!(engine.current(owner), Some(key(RowId::Policy)));
    let _ = engine.move_dir(owner, &page.view(), &[], Dir::Up, &c);
    assert_eq!(engine.current(owner), Some(key(RowId::Policy)), "nothing above the policy");
}

/// The policy row still opens the policy, and Delete still asks before deleting.
#[test]
fn the_rows_that_are_left_still_act() {
    let _guard = plx_base::testlock::serial();
    let mut page = page_for(true);
    let (mut out, mut present) = sink();
    page.row_commit(page.key_of(RowId::Policy), &mut mk_fx(&mut out, &mut present));
    let policy = PreviewKind::ALL.iter().position(|k| *k == PreviewKind::Policy).unwrap() as u8;
    assert!(out.iter().any(|s| matches!(s.fx, Fx::Nav(NavOp::Push(SettingsPage::Preview(i))) if i == policy)),
        "OK on the policy pushes its preview");
    out.clear();
    page.row_commit(page.key_of(RowId::Delete), &mut mk_fx(&mut out, &mut present));
    assert!(page.alert.is_open(), "OK on Delete opens the confirmation");
}

/// A route that names a telemetry document (a dev `--screen` boot, a stale recording) opens the
/// policy in a build without a destination, so the identifier documents are unreachable; a keyed
/// build opens what was named. The first-run crumb flags survive the mapping.
#[test]
fn a_preview_route_to_a_telemetry_document_opens_the_policy_without_a_destination() {
    let at = |kind: PreviewKind| PreviewKind::ALL.iter().position(|k| *k == kind).unwrap() as u8;
    let policy = at(PreviewKind::Policy);
    for kind in PreviewKind::ALL {
        let which = at(kind);
        assert_eq!(offered_preview(which, false), which, "{kind:?}: a keyed build opens what was named");
        assert_eq!(offered_preview(which, true), policy, "{kind:?}: no destination, the policy");
        assert_eq!(offered_preview(which | FIRST_RUN_CRASH, true), policy | FIRST_RUN_CRASH, "{kind:?}: the crumb flag is kept");
    }
}
