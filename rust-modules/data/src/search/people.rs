//! people — the person a search names, and the people it finds.
//!
//! Two sources speak about people, and this module is where they become one answer:
//!
//! - **the servers**: `/hubs/search`'s `actor`, `director`, `writer` and `producer` hubs, already
//!   folded into the Person shelf's [`TagHit`]s by `search::project` (one per person, `tagKey` first);
//! - **plex.tv**: its people search (`plx_plex::plex::discover::people_search_url`), whose answer
//!   [`parse_catalog`] reads. Its shape is known from ONE published sample (python-plexapi issue
//!   #1158): `MediaContainer.SearchResults[].SearchResult[]`, each `{Directory:{metadataId, key:
//!   "/library/people/<id>", title, thumb, type:"person", CreditType:[{type, count, title}]}, score}`.
//!   So it is read tolerantly: anything that does not look like a person costs that entry, never the
//!   answer, and never an error that could hide the servers' results.
//!
//! [`rank`] merges the two on the person's key (the `tagKey` a server sends is the id plex.tv
//! answers to, `plex/discover.rs`), keeps the people whose NAME the query names, and orders them:
//! [`match_strength`], then in the user's libraries, then plex.tv's score, then the title count. The
//! first is FEATURED (a card at the top of the results) only when the query really names them
//! (strength [`FEATURE_STRENGTH`] or more: the whole name, or two whole words of it); a first name
//! alone never puts a stranger ahead of the films.
//!
//! The ranking is the web client's (`packages/core/src/people.ts`), ported so both clients agree on
//! who a query names. Pure: no IO, no clock, no registry.
use super::TagHit;
use plx_plex::plex::ServerId;

/// From this [`match_strength`] up, the first person is featured at the top of the results.
pub const FEATURE_STRENGTH: i8 = 2;

/// The departments a person's jobs line can name, in the order it names them; bit `i` of a jobs
/// mask ([`TagHit::jobs`]) is `JOBS[i]`. Both sources spell them this way: a server's hub `type` and
/// plex.tv's `CreditType.type`. A department outside these (`music`, `costume-makeup`) is the person
/// page's to show, not the card's.
pub const JOBS: [&str; 4] = ["actor", "director", "writer", "producer"];

/// The bit of [`JOBS`] that `kind` names, 0 for a department the card does not name.
pub fn job_bit(kind: &str) -> u8 {
    JOBS.iter().position(|j| *j == kind).map_or(0, |i| 1 << i)
}

/// Is `key` a person key the person page can open? It goes into a URL PATH on plex.tv and on a
/// server (`/library/people/<key>`), so it is a bare id: 8 to 64 letters, digits, `-` or `_` (a real
/// guid is 24 hex digits; the mock's synthetic ones are `s` + 8 hex and `demo-person-N`). Anything
/// else (`../x`, an empty string, a full URL) is dropped rather than shown as a card that opens
/// nothing.
pub fn valid_key(key: &str) -> bool {
    (8..=64).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// One person as plex.tv's people search returns them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CatalogPerson {
    /// The person's plex.tv id: `metadataId`, else the id at the end of `key`.
    pub key: String,
    pub name: String,
    /// An `https://` headshot, else empty (a plain `http` URL is not shown).
    pub thumb: String,
    /// [`JOBS`] mask of the person's departments.
    pub jobs: u8,
    /// The largest credit count of any department: the person's career, in titles.
    pub credits: i64,
    /// plex.tv's relevance (0 to 1), when it sent one.
    pub score: Option<f64>,
}

type Json = serde_json::Value;

/// `v` as a list: an array's elements, one object as itself, anything else as nothing.
fn list(v: Option<&Json>) -> Vec<&Json> {
    match v {
        Some(Json::Array(items)) => items.iter().filter(|i| i.is_object()).collect(),
        Some(o @ Json::Object(_)) => vec![o],
        _ => Vec::new(),
    }
}

/// A non-blank string field, trimmed.
fn text<'a>(o: &'a Json, k: &str) -> Option<&'a str> {
    o.get(k).and_then(Json::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// A number field, or a string holding one (PMS and plex.tv string-encode numbers at will).
fn number(o: &Json, k: &str) -> Option<f64> {
    match o.get(k)? {
        Json::Number(n) => n.as_f64(),
        Json::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// The people of plex.tv's people search answer (its `MediaContainer`), in the order it sent them,
/// each once. Never fails: a body of another shape is no people.
pub fn parse_catalog(container: &Json) -> Vec<CatalogPerson> {
    let mut out: Vec<CatalogPerson> = Vec::new();
    for group in list(container.get("SearchResults")) {
        for result in list(group.get("SearchResult")) {
            let Some(d) = list(result.get("Directory")).into_iter().next()
                .or_else(|| list(result.get("Metadata")).into_iter().next()) else { continue };
            if d.get("type").is_some_and(|t| t.as_str() != Some("person")) {
                continue;
            }
            let Some(name) = text(d, "title") else { continue };
            let from_path = text(d, "key").and_then(|k| k.strip_prefix("/library/people/"));
            let Some(key) = [text(d, "metadataId"), from_path].into_iter().flatten().find(|k| valid_key(k)) else { continue };
            if out.iter().any(|p| p.key == key) {
                continue;
            }
            let mut jobs = 0;
            let mut credits = 0i64;
            for c in list(d.get("CreditType")) {
                jobs |= text(c, "type").map_or(0, job_bit);
                credits = credits.max(number(c, "count").unwrap_or(0.0) as i64);
            }
            let thumb = text(d, "thumb").filter(|t| t.starts_with("https://")).unwrap_or("");
            out.push(CatalogPerson { key: key.to_string(), name: name.to_string(), thumb: thumb.to_string(),
                jobs, credits, score: number(result, "score") });
        }
    }
    out
}

/// The base letter of an accented Latin (or Cyrillic й/ё) letter, the letter itself otherwise: the
/// fold Unicode's canonical decomposition followed by dropping the marks gives for these, without a
/// normalization table this crate does not carry.
fn base_letter(c: char) -> char {
    const FOLDS: &[(&str, char)] = &[
        ("ÀÁÂÃÄÅàáâãäåĀāĂăĄą", 'a'), ("ÇçĆćĈĉĊċČč", 'c'), ("ĎďĐđ", 'd'),
        ("ÈÉÊËèéêëĒēĔĕĖėĘęĚě", 'e'), ("ĜĝĞğĠġĢģ", 'g'), ("ĤĥĦħ", 'h'),
        ("ÌÍÎÏìíîïĨĩĪīĬĭĮįİı", 'i'), ("Ĵĵ", 'j'), ("Ķķ", 'k'), ("ĹĺĻļĽľĿŀŁł", 'l'),
        ("ÑñŃńŅņŇň", 'n'), ("ÒÓÔÕÖØòóôõöøŌōŎŏŐő", 'o'), ("ŔŕŖŗŘř", 'r'), ("ŚśŜŝŞşŠšȘș", 's'),
        ("ŢţŤťŦŧȚț", 't'), ("ÙÚÛÜùúûüŨũŪūŬŭŮůŰűŲų", 'u'), ("Ŵŵ", 'w'), ("ÝýÿŶŷŸ", 'y'),
        ("ŹźŻżŽž", 'z'), ("Йй", 'и'), ("Ёё", 'е'),
    ];
    if c.is_ascii() {
        return c;
    }
    FOLDS.iter().find(|(from, _)| from.contains(c)).map_or(c, |(_, to)| *to)
}

/// The words of a name or a query: lower case, accents and punctuation gone ("Inès MOREL-Duval" is
/// `ines morel duval`).
pub fn name_words(s: &str) -> Vec<String> {
    let folded: String = s.chars()
        // a combining mark (a decomposed accent) belongs to the letter before it
        .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
        .map(base_letter)
        .flat_map(char::to_lowercase)
        .collect();
    folded.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// How strongly `query` names `name`:
///
/// - **3**: the whole name ("ines morel" for "Inès Morel");
/// - **2**: two or more words, each a whole word of the name, in any order ("bale christian");
/// - **1**: one word, a whole word of the name ("christian", "bale");
/// - **0**: every word begins a word of the name (typing in progress: "christian ba");
/// - **-1**: none of these (a server's spelling correction, a match on another field).
///
/// Each query word must find a DIFFERENT word of the name, so "christian christian" names nobody
/// called Christian once.
pub fn match_strength(query: &str, name: &str) -> i8 {
    let q = name_words(query);
    let n = name_words(name);
    if q.is_empty() || n.is_empty() {
        return -1;
    }
    if q == n {
        return 3;
    }
    let fits = |whole: bool| {
        let mut left: Vec<&str> = n.iter().map(String::as_str).collect();
        q.iter().all(|w| match left.iter().position(|x| if whole { x == w } else { x.starts_with(w.as_str()) }) {
            Some(i) => { left.remove(i); true }
            None => false,
        })
    };
    if fits(true) {
        if q.len() > 1 { 2 } else { 1 }
    } else if fits(false) {
        0
    } else {
        -1
    }
}

/// One person after the merge, what a card draws and what OK opens: the [`TagHit`] the Person shelf
/// and the person page already speak (a plex.tv-only person is one with `catalog` set, no local id,
/// and the server that draws its headshot as its `sid`).
#[derive(Clone)]
pub struct Person {
    pub hit: TagHit,
    /// plex.tv's credit count for them, 0 when plex.tv did not answer about them.
    pub credits: i64,
    score: Option<f64>,
    strength: i8,
}

impl Person {
    /// The titles that make up their career as far as anyone knows: plex.tv's count, else the
    /// libraries'.
    fn titles(&self) -> i64 {
        self.credits.max(self.hit.count)
    }
    fn in_library(&self) -> bool {
        !self.hit.catalog
    }
}

/// [`rank`]'s answer.
#[derive(Default)]
pub struct Ranked {
    /// The person the query names, for the card at the top.
    pub featured: Option<Person>,
    /// Everyone else the query names, best first: the People row.
    pub people: Vec<Person>,
    /// The servers' people whose name the query does not name (a spelling correction, a match on
    /// another field). A server's answer is never dropped: they follow, in the servers' order.
    pub unmatched: Vec<TagHit>,
}

/// Merge the servers' people (`library`, the Person shelf's hits) with plex.tv's (`catalog`) on the
/// person's key, keep the ones `query` names, and rank them ([module doc](self)). `art` is the server
/// that draws a plex.tv-only person's headshot (its `sid`: the headshot is an `https` URL a server's
/// photo transcoder fetches, `TagHit::thumb`).
pub fn rank(query: &str, library: &[TagHit], catalog: &[CatalogPerson], art: ServerId) -> Ranked {
    let mut merged: Vec<Person> = library.iter()
        .map(|hit| Person { hit: hit.clone(), credits: 0, score: None, strength: -1 })
        .collect();
    for c in catalog {
        match merged.iter_mut().find(|p| !p.hit.tag_key.is_empty() && p.hit.tag_key == c.key) {
            Some(p) => {
                // plex.tv knows the whole career; the library keeps its own face and count
                p.hit.jobs |= c.jobs;
                p.credits = c.credits;
                p.score = c.score;
                if p.hit.thumb.is_empty() {
                    p.hit.thumb.clone_from(&c.thumb);
                }
            }
            None => merged.push(Person {
                hit: TagHit { sid: art, name: c.name.clone(), tag_key: c.key.clone(), thumb: c.thumb.clone(),
                    jobs: c.jobs, catalog: true, ..Default::default() },
                credits: c.credits, score: c.score, strength: -1,
            }),
        }
    }
    let mut out = Ranked::default();
    let mut named = Vec::new();
    for mut p in merged {
        p.strength = match_strength(query, &p.hit.name);
        if p.strength >= 0 {
            named.push(p);
        } else if p.in_library() {
            out.unmatched.push(p.hit);
        }
    }
    // stable: equal people keep the servers' order, then plex.tv's
    named.sort_by(|a, b| b.strength.cmp(&a.strength)
        .then(b.in_library().cmp(&a.in_library()))
        .then(b.score.unwrap_or(0.0).total_cmp(&a.score.unwrap_or(0.0)))
        .then(b.titles().cmp(&a.titles())));
    let mut named = named.into_iter();
    out.featured = named.next();
    if out.featured.as_ref().is_some_and(|p| p.strength < FEATURE_STRENGTH) {
        out.people.extend(out.featured.take());
    }
    out.people.extend(named);
    out
}

/// The person featured at the top of a result set, as the screen draws it: the merged hit OK opens
/// (always an [`super::Item::Tag`], the shape the Person shelf's cards have, so the application's
/// open path checks it the same way) and plex.tv's credit count.
#[derive(Clone)]
pub struct Featured {
    /// Always a tag hit: only [`Featured::of`] builds one.
    item: super::Item,
    /// plex.tv's credit count for them, 0 when plex.tv did not answer about them.
    pub credits: i64,
}

impl Featured {
    pub fn of(person: Person) -> Self {
        Self::new(person.hit, person.credits)
    }
    /// The featured card of `hit`, with plex.tv's `credits` for them.
    pub fn new(hit: TagHit, credits: i64) -> Self {
        Self { item: super::Item::Tag(hit), credits }
    }
    pub fn item(&self) -> &super::Item {
        &self.item
    }
    /// The hit [`Featured::item`] holds.
    pub fn hit(&self) -> Option<&TagHit> {
        match &self.item {
            super::Item::Tag(t) => Some(t),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "people_tests.rs"]
mod tests;
