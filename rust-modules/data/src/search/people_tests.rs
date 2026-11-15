//! The people a search names: reading plex.tv's answer, and the ranking that picks the featured
//! person. Mirrors the web client's `packages/core/test/people.test.ts`, case for case.
use super::*;
use serde_json::json;

/// plex.tv's answer, in the shape of the one published sample (python-plexapi issue #1158).
fn catalog_reply() -> Json {
    json!({
        "size": 1,
        "SearchResults": [{
            "id": "people", "title": "People", "size": 2,
            "SearchResult": [
                { "Directory": {
                    "bornAt": "1996-09-01",
                    "key": "/library/people/5d7768d2ebdf2200209c916d",
                    "metadataId": "5d7768d2ebdf2200209c916d",
                    "slug": "zendaya",
                    "thumb": "https://metadata-static.plex.tv/8/people/868e9505c5a89efc4477e2d4eb1617fb.jpg",
                    "title": "Zendaya", "type": "person",
                    "source": "provider://tv.plex.provider.metadata",
                    "CreditType": [
                        { "type": "actor", "count": 98, "title": "Actor" },
                        { "type": "producer", "count": 23, "title": "Producer" },
                        { "type": "music", "count": 18, "title": "Composer" },
                        { "type": "writer", "count": "1", "title": "Writer" }
                    ] },
                  "score": 1 },
                // no `metadataId`: the id is read from `key`
                { "Directory": { "key": "/library/people/5f3fe1b2a3c4d5e6f7a8b9c0", "title": "Zendaya Coleman",
                    "type": "person" }, "score": "0.4" }
            ]
        }]
    })
}

#[test]
fn plex_tv_people_are_read_from_their_directory_entries() {
    let people = parse_catalog(&catalog_reply());
    assert_eq!(people.iter().map(|p| p.key.as_str()).collect::<Vec<_>>(),
        ["5d7768d2ebdf2200209c916d", "5f3fe1b2a3c4d5e6f7a8b9c0"]);
    assert_eq!(people[0], CatalogPerson {
        key: "5d7768d2ebdf2200209c916d".into(), name: "Zendaya".into(),
        thumb: "https://metadata-static.plex.tv/8/people/868e9505c5a89efc4477e2d4eb1617fb.jpg".into(),
        jobs: job_bit("actor") | job_bit("producer") | job_bit("writer"), credits: 98, score: Some(1.0),
    });
    assert_eq!(people[1].score, Some(0.4), "a string-encoded score is a score");
    assert_eq!((people[1].jobs, people[1].credits, people[1].thumb.as_str()), (0, 0, ""));
}

#[test]
fn anything_that_is_not_a_person_is_skipped_without_an_error() {
    for odd in [Json::Null, json!(42), json!("x"), json!([]), json!({}), json!({"SearchResults": "x"}),
        json!({"SearchResults": [{"SearchResult": [null, 3, {}]}]})] {
        assert!(parse_catalog(&odd).is_empty(), "{odd}");
    }
    let strange = json!({"SearchResults": [{"SearchResult": [
        { "Metadata": { "type": "movie", "title": "Dune", "key": "/library/metadata/5d776830880197001ec967c4",
            "ratingKey": "5d776830880197001ec967c4" } },
        { "Directory": { "type": "person", "title": "No key" } },
        { "Directory": { "type": "person", "title": "Odd key", "key": "/library/people/../../x" } },
        { "Directory": { "type": "person", "key": "/library/people/5d7768d2ebdf2200209c916d" } },
        // a list instead of one object, and a plain-http headshot (not shown)
        { "Directory": [{ "type": "person", "title": "Listed", "metadataId": "5d7768d2ebdf2200209c9170",
            "thumb": "http://x/y.jpg" }] },
        // the same person twice: once
        { "Directory": { "type": "person", "title": "Listed", "metadataId": "5d7768d2ebdf2200209c9170" } }
    ]}]});
    assert_eq!(parse_catalog(&strange), [CatalogPerson { key: "5d7768d2ebdf2200209c9170".into(),
        name: "Listed".into(), ..Default::default() }]);
}

#[test]
fn a_person_key_is_a_bare_path_safe_id() {
    for good in ["5d7768d2ebdf2200209c916d", "s0123abcd", "demo-person-12"] {
        assert!(valid_key(good), "{good}");
    }
    for bad in ["", "short", "../../x", "/library/people/5d7768d2ebdf2200209c916d", "a b c d e f g h", &"x".repeat(65)] {
        assert!(!valid_key(bad), "{bad}");
    }
}

#[test]
fn names_compare_without_case_or_accents() {
    assert_eq!(name_words("  Inès MOREL-Duval "), ["ines", "morel", "duval"]);
    assert_eq!(name_words("Ине\u{0300}с Пётр Йорк"), ["инес", "петр", "иорк"]);
    assert_eq!(match_strength("ines morel", "Inès Morel"), 3);
    assert_eq!(match_strength("INÈS   MOREL", "Inès Morel"), 3);
    assert_eq!(match_strength("bale christian", "Christian Bale"), 2);
    assert_eq!(match_strength("christian", "Christian Bale"), 1);
    assert_eq!(match_strength("bale", "Christian Bale"), 1);
    assert_eq!(match_strength("christian ba", "Christian Bale"), 0);
    assert_eq!(match_strength("chris", "Christian Bale"), 0);
    assert_eq!(match_strength("batman", "Christian Bale"), -1);
    assert_eq!(match_strength("christian christian", "Christian Bale"), -1);
    assert_eq!(match_strength("", "Christian Bale"), -1);
}

fn library(name: &str, key: &str, count: i64) -> TagHit {
    TagHit { sid: ServerId::from_raw(0), name: name.into(), tag_key: key.into(), id: "7".into(),
        thumb: format!("/library/people/{key}/thumb"), count, jobs: job_bit("actor"), ..Default::default() }
}

fn catalog(name: &str, key: &str, score: f64, credits: i64) -> CatalogPerson {
    CatalogPerson { key: key.into(), name: name.into(), thumb: format!("https://img/{key}.jpg"),
        jobs: job_bit("actor"), credits, score: Some(score) }
}

fn names(people: &[Person]) -> Vec<&str> {
    people.iter().map(|p| p.hit.name.as_str()).collect()
}

const ART: ServerId = ServerId::from_raw(3);

#[test]
fn the_whole_name_is_featured_ahead_of_the_films() {
    let r = rank("christian bale", &[], &[catalog("Christian Slater", "k0000000000000002", 0.95, 10),
        catalog("Christian Bale", "k0000000000000001", 0.9, 80), catalog("Bale Christianson", "k0000000000000003", 0.1, 1)], ART);
    let featured = r.featured.expect("featured");
    assert_eq!(featured.hit.name, "Christian Bale");
    assert!(featured.hit.catalog && featured.hit.sid == ART && featured.hit.id.is_empty(),
        "a plex.tv-only person opens by its key, its headshot drawn through `art`");
    // "Christian Slater" is not named; "Bale Christianson" only by word starts
    assert_eq!(names(&r.catalog), ["Bale Christianson"]);
}

#[test]
fn a_first_name_alone_features_nobody_and_the_people_stay_in_their_row() {
    let r = rank("christian", &[], &[catalog("Christian Slater", "k0000000000000002", 0.95, 10),
        catalog("Christian Bale", "k0000000000000001", 0.9, 80), catalog("Bale Christianson", "k0000000000000003", 0.1, 1)], ART);
    assert!(r.featured.is_none());
    // whole words first, then plex.tv's score; "Christianson" (a word start) after
    assert_eq!(names(&r.catalog), ["Christian Slater", "Christian Bale", "Bale Christianson"]);
}

#[test]
fn people_the_query_does_not_name_are_left_out_unless_a_server_sent_them() {
    let r = rank("dune", &[library("Dee Wallace", "k00000000000000d1", 3)],
        &[catalog("Christian Bale", "k0000000000000001", 0.9, 80)], ART);
    assert!(r.featured.is_none() && r.people.is_empty() && r.catalog.is_empty());
    assert_eq!(r.unmatched.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Dee Wallace"],
        "a server's answer is never dropped, only ranked last");
}

#[test]
fn both_sources_merge_on_the_key_and_the_library_wins_a_tie() {
    let local = library("Christian Bale", "k0000000000000001", 3);
    let namesake = catalog("Christian Bale", "k0000000000000009", 1.0, 200);
    let mut same = catalog("Christian Bale", "k0000000000000001", 0.9, 80);
    same.jobs |= job_bit("director");
    let r = rank("Christian Bale", &[local.clone()], &[namesake, same], ART);
    let featured = r.featured.expect("featured");
    assert_eq!((featured.hit.tag_key.as_str(), featured.hit.id.as_str(), featured.hit.sid), ("k0000000000000001", "7", local.sid),
        "the library's person, opened on its own server");
    assert!(!featured.hit.catalog);
    assert_eq!(featured.hit.thumb, local.thumb, "the library keeps its own face");
    assert_eq!((featured.hit.count, featured.credits), (3, 80), "its titles here, its career on plex.tv");
    assert_eq!(featured.hit.jobs, job_bit("actor") | job_bit("director"));
    assert_eq!(r.catalog.iter().map(|p| p.hit.tag_key.as_str()).collect::<Vec<_>>(), ["k0000000000000009"]);
}

#[test]
fn equal_people_rank_by_score_then_titles() {
    let r = rank("morel", &[], &[catalog("Ines Morel", "k0000000000000001", 0.5, 2),
        catalog("Paul Morel", "k0000000000000002", 0.5, 40), catalog("Anne Morel", "k0000000000000003", 0.7, 1)], ART);
    assert!(r.featured.is_none());
    assert_eq!(names(&r.catalog), ["Anne Morel", "Paul Morel", "Ines Morel"]);
}

/// The row as the screen draws it after the featured card: the servers' people the query names,
/// then the ones it does not, then plex.tv's own.
fn row(r: &Ranked) -> Vec<&str> {
    r.row().map(|t| t.name.as_str()).collect()
}

#[test]
fn the_servers_order_decides_between_their_equally_named_people() {
    // the merge put the favourite library's person first; their title count must not undo it
    let r = rank("morel", &[library("Ines Morel", "k00000000000000a1", 1), library("Paul Morel", "k00000000000000a2", 9)],
        &[], ART);
    assert_eq!(row(&r), ["Ines Morel", "Paul Morel"]);
}

#[test]
fn plex_tv_landing_late_moves_none_of_the_servers_people() {
    let library = [library("Ines Morel", "k00000000000000a1", 1), library("Dee Wallace", "k00000000000000d1", 3),
        library("Paul Morel", "k00000000000000a2", 9)];
    let before = rank("morel", &library, &[], ART);
    assert_eq!(row(&before), ["Ines Morel", "Paul Morel", "Dee Wallace"]);
    let after = rank("morel", &library, &[catalog("Anne Morel", "k0000000000000003", 1.0, 50),
        catalog("Paul Morel", "k00000000000000a2", 0.99, 40)], ART);
    assert_eq!(row(&after), ["Ines Morel", "Paul Morel", "Dee Wallace", "Anne Morel"],
        "plex.tv's people follow the servers'; its scores place only them");
}
