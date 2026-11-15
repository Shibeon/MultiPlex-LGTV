//! plex.tv's people search as a job of the Search store ([`ask_catalog`], [`take_catalog`],
//! [`finish_people`]): asked once per settled query as the account the step captured, answered
//! into its own mailbox, merged into the Person shelf by the rebuild, and featured when the query
//! names the person. The runner side is pinned in `app/adapters/stores.rs`.

use super::test_support::*;
use super::*;
use crate::stores::StoreJob;
use plx_machine::job::Port;
use serde_json::json;

fn account() -> plx_plex::plex::session::Session {
    plx_plex::plex::session::Session { client_id: "synthetic-client".into(),
        account_token: "synthetic-token".into(), ..Default::default() }
}

/// A pump as the Bridge runs it with a settled account (`catalog`), or without one (`None`).
fn pump_as(owner: &mut Owner, dt: f32, catalog: Option<&plx_plex::plex::session::Session>) -> bool {
    let fixture = fixture_input();
    let input = SearchInput { catalog, ..fixture.input() };
    pump_with_optional_directory(&mut owner.state, &owner.adapter, dt, None,
        plx_machine::landgate::fixture_gate(), &input)
}

/// The plex.tv people jobs on the outbox; the servers' jobs are dropped (nothing answers them).
fn people_jobs(owner: &mut Owner) -> Vec<(Job<SearchReq>, SearchPort)> {
    std::mem::take(&mut owner.state.jobs).into_iter().filter_map(|job| match job {
        StoreJob::Search(job, port) if matches!(job.req.work, SearchWork::People(_)) => Some((job, port)),
        StoreJob::Search(..) => None,
        _ => panic!("not a search job"),
    }).collect()
}

/// plex.tv's answer naming one synthetic person (its `MediaContainer`).
fn reply(name: &str, key: &str) -> serde_json::Value {
    json!({ "SearchResults": [{ "SearchResult": [{ "score": 0.9, "Directory": {
        "type": "person", "title": name, "metadataId": key,
        "thumb": format!("https://img.example/{key}.jpg"),
        "CreditType": [{ "type": "actor", "count": 12 }] } }] }] })
}

fn settled(q: &str) -> (Fresh, Owner) {
    let fresh = fresh();
    let mut owner = Owner::default();
    register(&mut owner, 2);
    owner.set_query(q);
    (fresh, owner)
}

#[test]
fn without_an_account_plex_tv_is_never_asked() {
    let (_fresh, mut owner) = settled("ines morel");
    pump_as(&mut owner, 0.3, None);
    assert!(people_jobs(&mut owner).is_empty());
    assert!(!owner.state.catalog_asked);
}

#[test]
fn a_settled_query_asks_plex_tv_once_as_the_account_it_captured() {
    let (_fresh, mut owner) = settled("ines morel");
    let account = account();
    pump_as(&mut owner, 0.01, Some(&account));
    assert!(people_jobs(&mut owner).is_empty(), "the debounce holds plex.tv back like the servers");

    pump_as(&mut owner, 0.3, Some(&account));
    let jobs = people_jobs(&mut owner);
    assert_eq!(jobs.len(), 1);
    let (job, port) = &jobs[0];
    assert_eq!(job.req.server(), None, "it asks no server");
    assert_eq!(job.addr, search_addr(owner.state.gen), "addressed by the query's generation");
    assert_eq!(*port, SearchPort::catalog(&owner.adapter));
    assert_eq!(job.req.q, "ines morel");
    assert!(matches!(&job.req.work, SearchWork::People(a)
        if a.client_id == "synthetic-client" && a.token == "synthetic-token"));
    assert!(owner.adapter.catalog.busy(), "the claim is raised at the emit");

    pump_as(&mut owner, 0.016, Some(&account));
    assert!(people_jobs(&mut owner).is_empty(), "once per query");
}

#[test]
fn the_answer_features_the_person_the_query_names() {
    let (_fresh, mut owner) = settled("ines morel");
    let account = account();
    pump_as(&mut owner, 0.3, Some(&account));
    let (job, port) = people_jobs(&mut owner).pop().expect("asked");
    finish_people(&port, job.addr, &job.req.q, || Some(reply("Inès Morel", "5d7768d2ebdf2200209c916d")));
    assert!(owner.state.snapshot().view().featured().is_none(), "nothing lands outside a pump");

    pump_as(&mut owner, 0.016, Some(&account));
    let snapshot = owner.state.snapshot();
    let featured = snapshot.view().featured().expect("featured");
    let hit = featured.hit().expect("a person");
    assert_eq!((hit.name.as_str(), hit.tag_key.as_str()), ("Inès Morel", "5d7768d2ebdf2200209c916d"));
    assert!(hit.catalog && hit.id.is_empty());
    assert_eq!(hit.sid, ServerId::from_raw(fixture_slots()[0] as u16),
        "a plex.tv headshot is drawn through the first live server");
    assert_eq!(featured.credits, 12);
    assert!(snapshot.view().shelves().iter().all(|s| s.kind != Kind::Person),
        "the featured person is not drawn twice");
    assert!(!owner.adapter.catalog.busy(), "the take released the claim");
}

#[test]
fn a_first_name_alone_puts_plex_tv_people_in_the_row_and_features_nobody() {
    let (_fresh, mut owner) = settled("ines");
    let account = account();
    pump_as(&mut owner, 0.3, Some(&account));
    let (job, port) = people_jobs(&mut owner).pop().expect("asked");
    finish_people(&port, job.addr, &job.req.q, || Some(reply("Inès Morel", "5d7768d2ebdf2200209c916d")));
    pump_as(&mut owner, 0.016, Some(&account));
    let snapshot = owner.state.snapshot();
    assert!(snapshot.view().featured().is_none());
    let row = snapshot.view().shelves().iter().find(|s| s.kind == Kind::Person).expect("the people row");
    assert_eq!(titles(row), ["Inès Morel"]);
}

#[test]
fn a_failed_or_panicking_read_answers_no_people() {
    for read in [None, Some(())] {
        let (_fresh, mut owner) = settled("ines morel");
        let account = account();
        pump_as(&mut owner, 0.3, Some(&account));
        let (job, port) = people_jobs(&mut owner).pop().expect("asked");
        match read {
            None => finish_people(&port, job.addr, &job.req.q, || None),
            Some(()) => finish_people(&port, job.addr, &job.req.q, || panic!("the transport blew up")),
        }
        pump_as(&mut owner, 0.016, Some(&account));
        assert_eq!(owner.state.catalog.as_deref(), Some(&[][..]), "settled, as no people");
        assert!(owner.state.snapshot().view().featured().is_none());
        pump_as(&mut owner, 0.016, Some(&account));
        assert!(people_jobs(&mut owner).is_empty(), "and not asked again");
    }
}

#[test]
fn a_refused_launch_settles_as_no_people_and_is_not_retried() {
    let (_fresh, mut owner) = settled("ines morel");
    let account = account();
    pump_as(&mut owner, 0.3, Some(&account));
    let (job, port) = people_jobs(&mut owner).pop().expect("asked");
    port.refused(job.addr);
    assert!(!owner.adapter.catalog.busy(), "a refusal gives the claim back");
    pump_as(&mut owner, 0.016, Some(&account));
    assert_eq!(owner.state.catalog.as_deref(), Some(&[][..]));
    pump_as(&mut owner, 0.016, Some(&account));
    assert!(people_jobs(&mut owner).is_empty());
}

#[test]
fn a_superseded_answer_is_dropped_and_the_new_query_asks_again() {
    let (_fresh, mut owner) = settled("ines morel");
    let account = account();
    pump_as(&mut owner, 0.3, Some(&account));
    let (old, port) = people_jobs(&mut owner).pop().expect("asked");
    owner.set_query("paul morel");
    assert!(owner.state.snapshot().view().featured().is_none());
    finish_people(&port, old.addr, &old.req.q, || Some(reply("Inès Morel", "5d7768d2ebdf2200209c916d")));
    pump_as(&mut owner, 0.3, Some(&account));
    assert!(owner.state.catalog.is_none(), "the old query's people never land on the new one");
    let jobs = people_jobs(&mut owner);
    assert_eq!(jobs.len(), 1, "the new query is asked");
    assert_eq!(jobs[0].0.req.q, "paul morel");
}

/// The servers' people hubs: writers and producers join the one Person shelf, each person's
/// departments are OR'd at the fold, and a person a hub sends as a `Metadata` row is a tag like any
/// other (one with no usable plex.tv id is dropped: nothing could open it).
#[test]
fn every_people_hub_feeds_one_shelf_and_a_person_keeps_all_their_departments() {
    use plx_plex::plex::{MediaContainer, Tag};
    let tag = |name: &str, key: &str| Tag { tag: name.into(), tag_key: key.into(), id: 7, ..Default::default() };
    let mut actor = hub("actor", "actor");
    actor.directory = vec![tag("Nick Park", "5d776825880197001ec90b6a")];
    let mut director = hub("director", "director");
    director.directory = vec![tag("Nick Park", "5d776825880197001ec90b6a")];
    let mut writer = hub("writer", "writer");
    writer.metadata = vec![
        plx_plex::plex::Metadata { kind: "person".into(), title: "Peter Lord".into(),
            guid: "plex://person/5d776825880197001ec90b6b".into(), rating_key: "8".into(), ..Default::default() },
        plx_plex::plex::Metadata { kind: "person".into(), title: "No Guid".into(), ..Default::default() },
    ];
    let mut producer = hub("producer", "producer");
    producer.directory = vec![tag("Peter Lord", "5d776825880197001ec90b6b")];
    let mc = MediaContainer { hub: vec![actor, director, writer, producer], ..Default::default() };

    let p = project(&mc, ServerId::from_raw(0), NO_FAVS);
    let people: Vec<&TagHit> = p[3].iter().filter_map(|i| match i { Item::Tag(t) => Some(t), _ => None }).collect();
    assert_eq!(people.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Nick Park", "Peter Lord"]);
    assert_eq!(people[0].jobs, people::job_bit("actor") | people::job_bit("director"));
    assert_eq!(people[1].jobs, people::job_bit("writer") | people::job_bit("producer"));
    assert_eq!((people[1].tag_key.as_str(), people[1].id.as_str()), ("5d776825880197001ec90b6b", "8"));
}
