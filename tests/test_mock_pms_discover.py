"""The mock's Discover stand-in (docs/watchlist-plan.md W45): wire shapes the Rust client reads
(`plex::watchlist`), the fixture's tile states, and in-memory add/remove.

Run: python3 -I tests/test_mock_pms_discover.py   (-I drops the script dir, so it is added below)
"""
import json
import pathlib
import sys
import unittest
import urllib.parse

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from mock_pms import (  # noqa: E402
    DiscoverFixture, Library, MockPms, add_cw_art_fixture, discover_stand_in_problem)

LIST = "/library/sections/watchlist/all?includeCollections=1&includeExternalMedia=1&sort=watchlistedAt:desc"


def make():
    return MockPms(Library(seed=3, movies=24, shows=6, seasons=1, episodes=2))


def call(pms, method, path):
    status, ctype, data = pms.handle(method, path, b"", {})
    return status, ctype, data


def jcall(pms, method, path, want=200):
    status, ctype, data = call(pms, method, path)
    assert status == want, (path, status, data[:200])
    assert ctype == "application/json"
    return json.loads(data)["MediaContainer"]


def page(pms, variant="all", start=0, size=100):
    return jcall(pms, "GET", f"/library/sections/watchlist/{variant}?X-Plex-Container-Start={start}"
                             f"&X-Plex-Container-Size={size}")


class DiscoverMock(unittest.TestCase):
    def test_list_pages_and_caps_size_at_100(self):
        pms = make()
        full = page(pms)
        self.assertEqual(full["totalSize"], 14)
        self.assertEqual(full["size"], 14)
        self.assertEqual(full["librarySectionID"], "watchlist")
        self.assertTrue(all("watchlistedAt" not in m for m in full["Metadata"]))
        second = page(pms, start=5, size=5)
        self.assertEqual([m["ratingKey"] for m in second["Metadata"]],
                         [m["ratingKey"] for m in full["Metadata"][5:10]])
        self.assertEqual(second["offset"], 5)
        self.assertEqual(call(pms, "GET", LIST + "&X-Plex-Container-Start=0&X-Plex-Container-Size=101")[0], 400)

    def test_a_held_shows_season_lists_its_episodes_and_the_announced_ones(self):
        pms = make()
        show = next(it for it in pms.lib.items.values()
                    if it["type"] == "show" and it["guid"].startswith("plex://show/"))
        season = pms.lib.children(int(show["ratingKey"]))[0]
        self.assertRegex(season["guid"], r"^plex://season/[0-9a-f]{24}$")
        hx = season["guid"].rsplit("/", 1)[1]
        path = f"/library/metadata/{hx}/children?X-Plex-Language=fr"
        self.assertEqual(call(pms, "GET", path)[0], 401, "the provider wants a token")
        status, _, data = pms.handle("GET", path, b"", {"X-Plex-Token": "t"})
        self.assertEqual(status, 200)
        rows = json.loads(data)["MediaContainer"]["Metadata"]
        held = pms.lib.children(int(season["ratingKey"]))
        self.assertEqual([r["index"] for r in rows],
                         [e["index"] for e in held] + [len(held) + 1, len(held) + 2])
        self.assertEqual(rows[-2]["originallyAvailableAt"], "2031-04-04")
        self.assertNotIn("originallyAvailableAt", rows[-1])
        self.assertTrue(all("thumb" not in r for r in rows[-2:]))
        unknown = "/library/metadata/" + "0" * 24 + "/children"
        self.assertEqual(pms.handle("GET", unknown, b"", {"X-Plex-Token": "t"})[0], 404)

    def test_ids_are_24_hex_and_guids_carry_them(self):
        for m in page(make())["Metadata"]:
            self.assertRegex(m["ratingKey"], r"^[0-9a-f]{24}$")
            self.assertEqual(m["guid"], f"plex://{m['type']}/{m['ratingKey']}")

    def test_released_variant_drops_the_unreleased(self):
        pms = make()
        released = page(pms, "released")
        unreleased = {c["hex"] for c in pms.discover.catalog if not c["released"]}
        self.assertEqual(len(unreleased), 3)
        self.assertEqual(released["totalSize"], 11)
        self.assertFalse(unreleased & {m["ratingKey"] for m in released["Metadata"]})

    def test_fixture_covers_the_states_one_mock_can_produce(self):
        pms = make()
        held, errs = {}, set()
        for c in pms.discover.catalog[:14]:
            kind = {"movie": "1", "show": "2"}[c["kind"]]
            guid = urllib.parse.quote(c["guid"], safe="")
            rows = jcall(pms, "GET", f"/library/all?type={kind}&guid={guid}")["Metadata"]
            held[c["hex"]] = bool(rows) and rows[0]["ratingKey"].isdigit()
            if rows:
                self.assertEqual(rows[0]["guid"], c["guid"])
                if not rows[0]["ratingKey"].isdigit():
                    errs.add(c["hex"])
        states = set()
        for c in pms.discover.catalog[:14]:
            if c["hex"] in errs:
                states.add("Unknown")
                continue
            states.add(("Available" if c["released"] else "ComingSoonMatched") if held[c["hex"]]
                       else ("NotOnAnyServer" if c["released"] else "ComingSoon"))
        self.assertEqual(states, {"Available", "ComingSoonMatched", "ComingSoon", "NotOnAnyServer", "Unknown"})
        self.assertEqual(sum(held.values()), 7)
        self.assertEqual(errs, {pms.discover.catalog[pms.discover.GARBLED_LOOKUP]["hex"]},
                         "exactly one title's lookup errs (the availability badge's To check)")
        self.assertGreaterEqual(sum(1 for c in pms.discover.catalog[:14] if c["kind"] == "show"), 4)

    def test_a_guid_batch_is_one_request(self):
        pms = make()
        cs = [c for c in pms.discover.catalog[:14] if c["kind"] == "movie" and c["held"]]
        joined = ",".join(urllib.parse.quote(c["guid"], safe="") for c in cs)
        rows = jcall(pms, "GET", f"/library/all?type=1&guid={joined}")["Metadata"]
        self.assertEqual({r["guid"] for r in rows}, {c["guid"] for c in cs})

    def test_future_titles_have_a_future_date_and_released_a_past_one(self):
        for c in make().discover.catalog:
            self.assertEqual(c["date"] > "2027-01-01", not c["released"], c["title"])

    def test_user_state_is_an_array_with_watchlisted_at_only_while_listed(self):
        pms = make()
        listed = pms.discover.catalog[0]["hex"]
        unlisted = pms.discover.catalog[15]["hex"]
        state = jcall(pms, "GET", f"/library/metadata/{listed}/userState")["UserState"]
        self.assertIsInstance(state, list)
        self.assertIsInstance(state[0]["watchlistedAt"], int)
        state = jcall(pms, "GET", f"/library/metadata/{unlisted}/userState")["UserState"]
        self.assertIsInstance(state, list)
        self.assertNotIn("watchlistedAt", state[0])

    def test_metadata_answers_a_catalog_item_and_404s_the_unknown(self):
        pms = make()
        c = pms.discover.catalog[12]
        m = jcall(pms, "GET", f"/library/metadata/{c['hex']}")["Metadata"][0]
        self.assertEqual((m["ratingKey"], m["type"], m["title"]), (c["hex"], "show", c["title"]))
        self.assertTrue(m["Genre"])
        self.assertEqual(call(pms, "GET", "/library/metadata/" + "0" * 24)[0], 404)
        status, ctype, data = call(pms, "GET", f"/library/metadata/{c['hex']}/thumb/1")
        self.assertEqual((status, ctype), (200, "image/png"))
        self.assertTrue(data.startswith(b"\x89PNG"))

    def test_remove_and_add_change_later_lists_in_memory(self):
        pms = make()
        target = page(pms)["Metadata"][2]["ratingKey"]
        self.assertEqual(jcall(pms, "PUT", f"/actions/removeFromWatchlist?ratingKey={target}"), {"size": 0})
        after = page(pms)
        self.assertEqual(after["totalSize"], 13)
        self.assertNotIn(target, [m["ratingKey"] for m in after["Metadata"]])
        self.assertNotIn("watchlistedAt",
                         jcall(pms, "GET", f"/library/metadata/{target}/userState")["UserState"][0])
        jcall(pms, "PUT", f"/actions/removeFromWatchlist?ratingKey={target}")  # idempotent
        self.assertEqual(page(pms)["totalSize"], 13)
        jcall(pms, "PUT", f"/actions/addToWatchlist?ratingKey={target}")
        jcall(pms, "PUT", f"/actions/addToWatchlist?ratingKey={target}")  # idempotent
        again = page(pms)
        self.assertEqual(again["totalSize"], 14)
        self.assertEqual(again["Metadata"][0]["ratingKey"], target)  # newest first
        added = pms.discover.catalog[14]["hex"]
        jcall(pms, "PUT", f"/actions/addToWatchlist?ratingKey={added}")
        self.assertEqual(page(pms)["totalSize"], 15)
        self.assertIn(added, [m["ratingKey"] for m in page(pms, "released")["Metadata"]])
        self.assertEqual([w[0] for w in pms.writes], ["PUT"] * 5)
        self.assertTrue(all("ratingKey=" in w[1] for w in pms.writes))

    def test_bad_writes_are_refused(self):
        pms = make()
        self.assertEqual(call(pms, "PUT", "/actions/addToWatchlist?ratingKey=nothex")[0], 400)
        self.assertEqual(call(pms, "PUT", "/actions/addToWatchlist?ratingKey=" + "a" * 24)[0], 404)
        self.assertEqual(call(pms, "GET", "/actions/addToWatchlist?ratingKey=" + "a" * 24)[0], 405)

    def test_state_is_per_process_object(self):
        a, b = make(), make()
        jcall(a, "PUT", f"/actions/removeFromWatchlist?ratingKey={a.discover.catalog[0]['hex']}")
        self.assertEqual(page(a)["totalSize"], 13)
        self.assertEqual(page(b)["totalSize"], 14)

    def test_guid_lookup_can_be_made_to_fail(self):
        pms = make()
        status, obj = pms.discover.configure({"guid_lookup": "fail"})
        self.assertEqual(status, 200)
        guid = urllib.parse.quote(pms.discover.catalog[0]["guid"], safe="")
        self.assertEqual(call(pms, "GET", f"/library/all?type=1&guid={guid}")[0], 503)
        pms.discover.configure({"guid_lookup": "ok"})
        self.assertEqual(call(pms, "GET", f"/library/all?type=1&guid={guid}")[0], 200)
        self.assertEqual(pms.discover.configure({"x": 1})[0], 400)

    def test_list_modes_reset_the_list_over_the_control(self):
        pms = make()
        status, ctype, data = pms.handle("POST", "/_mock/discover", json.dumps({"list": "empty"}).encode(), {})
        self.assertEqual(status, 200)
        empty = page(pms)
        self.assertEqual((empty["totalSize"], empty.get("Metadata", [])), (0, []))
        self.assertEqual(page(pms, "released")["totalSize"], 0)
        pms.discover.configure({"list": "movies_only"})
        movies = page(pms)
        self.assertEqual({m["type"] for m in movies["Metadata"]}, {"movie"})
        listed_movies = [c for c in pms.discover.catalog[:DiscoverFixture.LISTED] if c["kind"] == "movie"]
        self.assertEqual(movies["totalSize"], len(listed_movies))
        self.assertLess(movies["totalSize"], 14)
        self.assertGreater(movies["totalSize"], 0)
        pms.discover.configure({"list": "default"})
        self.assertEqual(page(pms)["totalSize"], 14)
        # Add/remove still work from a reset state.
        pms.discover.configure({"list": "empty"})
        jcall(pms, "PUT", f"/actions/addToWatchlist?ratingKey={pms.discover.catalog[0]['hex']}")
        self.assertEqual(page(pms)["totalSize"], 1)

    def test_list_mode_validation(self):
        pms = make()
        self.assertEqual(pms.discover.configure({"list": "bogus"})[0], 400)
        self.assertEqual(pms.discover.configure({})[0], 400)
        self.assertEqual(page(pms)["totalSize"], 14)  # a refused patch changes nothing
        self.assertEqual(pms.discover.configure({"list": "empty", "x": 1})[0], 400)
        self.assertEqual(page(pms)["totalSize"], 14)

    def test_a_title_bound_by_guids_all_can_be_added_and_listed(self):
        pms = make()
        before = page(pms)["totalSize"]
        status, _ = pms.discover.configure({"guids": "all"}, pms.lib)
        self.assertEqual(status, 200)
        it = next(i for i in pms.lib.items.values() if i.get("type") == "movie"
                  and i["guid"] not in {c["guid"] for c in pms.discover.catalog[:DiscoverFixture.LISTED]})
        hx = it["guid"].rsplit("/", 1)[1]
        self.assertEqual(page(pms)["totalSize"], before, "known, but not on the list")
        jcall(pms, "PUT", f"/actions/addToWatchlist?ratingKey={hx}")
        listed = page(pms)
        self.assertEqual(listed["totalSize"], before + 1)
        self.assertEqual(listed["Metadata"][0]["title"], it["title"])
        jcall(pms, "PUT", f"/actions/removeFromWatchlist?ratingKey={hx}")
        self.assertEqual(page(pms)["totalSize"], before)
        self.assertEqual(call(pms, "PUT", f"/actions/addToWatchlist?ratingKey={'0' * 24}")[0], 404,
                         "a guid nothing bound is still unknown")

    def test_scenario_modes_are_all_known_to_the_mock(self):
        scenario = json.loads((pathlib.Path(__file__).resolve().parent / "scenarios"
                               / "watchlist_captures.json").read_text())
        modes = {c["mock"] for c in scenario["captures"]}
        self.assertLessEqual(modes, set(DiscoverFixture.LIST_MODES))

    def _credited(self, pms):
        """A person the synthetic library credits as an actor, with the titles that credit names."""
        who = next(t for t in pms.lib.people.values() if pms.lib.person_media(t["id"]))
        return who, pms.lib.person_media(who["id"])

    def test_the_stand_in_answers_the_person_profile_without_a_token(self):
        # The person page's biography read (`discover.rs::person_profile`): the tagKey addresses the
        # person, the record needs no token, an unknown person is a 200 with nothing in it.
        pms = make()
        who, _ = self._credited(pms)
        c = jcall(pms, "GET", f"/library/people/{who['tagKey']}")
        record = c["Metadata"][0]
        self.assertEqual(record["title"], who["tag"])
        self.assertTrue(record["summary"] and record["bornAt"] and record["birthPlace"])
        self.assertRegex(record["bornAt"], r"^\d{4}-\d{2}-\d{2}$")
        self.assertIn("actor", {t["type"] for t in record["CreditType"]})
        self.assertEqual(record, jcall(pms, "GET", f"/library/people/{who['tagKey']}")["Metadata"][0],
                         "a pure function of the tagKey")
        self.assertNotIn("Metadata", jcall(pms, "GET", "/library/people/not-a-person"))

    def test_the_stand_in_answers_the_person_credits_only_with_a_token(self):
        pms = make()
        who, held = self._credited(pms)
        path = f"/library/people/{who['tagKey']}/credits"
        self.assertEqual(call(pms, "GET", path)[0], 401, "the real provider answers 401 without a token")
        status, ctype, data = pms.handle("GET", path, b"", {"X-Plex-Token": "t"})
        self.assertEqual((status, ctype), (200, "application/json"))
        groups = json.loads(data)["MediaContainer"]["CreditGroup"]
        self.assertTrue(groups and all(g["type"] and g["title"] and g["Credit"] for g in groups), groups)
        rows = [c["Metadata"] for g in groups for c in g["Credit"]]
        for item in rows:
            self.assertNotIn("guid", item, "the credits wire carries no guid on the item")
            self.assertRegex(item["ratingKey"], r"^[0-9a-f]{24}$")
        # The availability join: a held title's ratingKey is the TAIL of its library guid.
        tails = {it["guid"].rsplit("/", 1)[-1] for it in held}
        self.assertTrue(tails & {i["ratingKey"] for i in rows}, "the filmography has a row a server holds")
        self.assertTrue({i["ratingKey"] for i in rows} - tails, "and rows no server holds")
        self.assertEqual(jcall(pms, "GET", f"/library/people/{who['tagKey']}/credits?X-Plex-Token=t"),
                         json.loads(data)["MediaContainer"], "a token in the query is a token")
        self.assertEqual(pms.handle("GET", "/library/people/not-a-person/credits", b"", {"X-Plex-Token": "t"})[0], 200)

    def test_a_real_actors_record_invents_no_fact_about_them(self):
        # The demo library's actors carry real names (`demo-person-N`): their record is the name and
        # the synthetic departments, with no birth date, place or biography made up for them.
        pms = make()
        pms.lib.people[9999] = {"id": 9999, "tag": "Some Real Actor", "tagKey": "demo-person-9"}
        record = jcall(pms, "GET", "/library/people/demo-person-9")["Metadata"][0]
        self.assertEqual(record["title"], "Some Real Actor")
        for invented in ("summary", "bornAt", "birthPlace", "knownFor"):
            self.assertNotIn(invented, record)
        # A synthetic person (`s%08x`) still gets the plainly-labelled synthetic record.
        who, _ = self._credited(pms)
        synthetic = jcall(pms, "GET", f"/library/people/{who['tagKey']}")["Metadata"][0]
        self.assertIn("synthetic person", synthetic["summary"])

    def test_a_harness_requires_the_stand_in_boot_line(self):
        # The line `account::announce_discover` writes; a harness that boots against this mock fails
        # the run unless it reads `base=stand-in`, and fails closed on a log with no line at all.
        self.assertIsNone(discover_stand_in_problem("[0.1] boot\n[0.2] discover: base=stand-in\n"))
        self.assertIn("base=real", discover_stand_in_problem("[0.2] discover: base=real\n"))
        self.assertIn("no `discover: base=` line", discover_stand_in_problem("[0.1] boot\n"))
        self.assertIn("base=real", discover_stand_in_problem(
            "discover: base=stand-in\ndiscover: base=real\n"), "any real line fails the run")

    def test_the_pms_people_media_route_is_not_the_stand_ins(self):
        pms = make()
        who, held = self._credited(pms)
        self.assertEqual(len(jcall(pms, "GET", f"/library/people/{who['id']}/media")["Metadata"]), len(held))
        self.assertEqual(call(pms, "PUT", f"/library/people/{who['tagKey']}")[0], 405)

    def test_ordinary_pms_paths_are_untouched(self):
        pms = make()
        self.assertGreater(len(jcall(pms, "GET", "/library/sections/1/all")["Metadata"]), 0)
        self.assertEqual(len(DiscoverFixture.TITLES), 16)


class PeopleSearch(unittest.TestCase):
    """The metadata provider's people search (`discover.rs::search_people`, the Search screen's
    featured person): the library's people under their `tagKey`, plus one person only it knows."""

    @staticmethod
    def search(pms, query, extra="&searchTypes=people&includeMetadata=1&filterPeople=1"):
        return jcall(pms, "GET", "/library/search?" + urllib.parse.urlencode({"query": query, "limit": 10}) + extra)

    @staticmethod
    def people(mc):
        return [r["Directory"] for g in mc["SearchResults"] for r in g["SearchResult"]]

    def test_a_library_person_is_found_by_both_searches_under_one_key(self):
        pms = make()
        who = next(t for t in sorted(pms.lib.people.values(), key=lambda t: t["id"])
                   if any(t["id"] == r["id"] for it in pms.lib.items.values() for r in it.get("Role", [])))
        found = self.people(self.search(pms, who["tag"]))
        self.assertEqual([p["metadataId"] for p in found], [who["tagKey"]])
        self.assertEqual(found[0]["type"], "person")
        self.assertEqual(found[0]["key"], f"/library/people/{who['tagKey']}")
        self.assertIn("actor", [c["type"] for c in found[0]["CreditType"]])
        self.assertTrue(all(c["count"] > 0 for c in found[0]["CreditType"]))
        hubs = jcall(pms, "GET", "/hubs/search?" + urllib.parse.urlencode({"query": who["tag"], "limit": 10}))["Hub"]
        actors = next(h for h in hubs if h["type"] == "actor")["Directory"]
        self.assertEqual([a["tagKey"] for a in actors], [who["tagKey"]], "the server's search finds the same key")

    def test_a_person_only_the_provider_knows_has_a_headshot_and_a_profile(self):
        pms = make()
        found = self.people(self.search(pms, DiscoverFixture.CATALOG_PERSON["tag"]))
        self.assertEqual([p["metadataId"] for p in found], ["sfeedc0de"])
        self.assertTrue(found[0]["thumb"].startswith("https://"))
        self.assertEqual(found[0]["CreditType"], [])
        profile = jcall(pms, "GET", "/library/people/sfeedc0de")
        self.assertEqual(profile["Metadata"][0]["title"], "sfeedc0de")

    def test_nothing_matches_nothing_and_the_pms_search_is_untouched(self):
        pms = make()
        self.assertEqual(self.people(self.search(pms, "zzzz")), [])
        mc = jcall(pms, "GET", "/library/search?query=sfeedc0de&searchTypes=movies")
        self.assertNotIn("SearchResults", mc, "without filterPeople the PMS's own typed search answers")
        self.assertEqual(mc["SearchResult"], [])


class CwArtDiscover(unittest.TestCase):
    """`--cw-art-fixture`'s half of the stand-in: Continue Watching titles it knows by catalog id, off the
    list, with a `coverArt` where the fixture says so. Without the flag nothing is added."""

    def test_the_fixture_titles_are_known_but_never_listed_and_the_list_does_not_move(self):
        plain, pms = make(), make()
        add_cw_art_fixture(pms)
        self.assertEqual(len(plain.discover.catalog), 16)
        self.assertEqual(len(pms.discover.catalog), 16 + 4)  # the guid-less item has no record
        self.assertEqual(page(pms), page(plain))
        self.assertEqual(pms.discover.order, plain.discover.order)
        for c in pms.discover.catalog[16:]:
            self.assertEqual(len(c["hex"]), 24)
            self.assertEqual(c["guid"], f"plex://{c['kind']}/{c['hex']}")
            self.assertNotIn(c["hex"], [h for h, _ in pms.discover.order])
            self.assertEqual(jcall(pms, "GET", f"/library/metadata/{c['hex']}")["Metadata"][0]["title"], c["title"])

    def test_only_the_cover_titles_carry_a_cover_art_url(self):
        pms = make()
        add_cw_art_fixture(pms)
        pms.discover.art_base = "http://127.0.0.1:1"
        shapes = {}
        for c in pms.discover.catalog[16:]:
            record = jcall(pms, "GET", f"/library/metadata/{c['hex']}")["Metadata"][0]
            shapes[c["title"]] = sorted(i["type"] for i in record["Image"]) if "Image" in record else None
        self.assertEqual(shapes, {"North Ridge": ["coverArt", "coverPoster"], "Lumen Drift": ["coverArt", "coverPoster"],
                                  "The Quiet Harbour": ["coverPoster"], "Brass Hours": None})


if __name__ == "__main__":
    unittest.main()
