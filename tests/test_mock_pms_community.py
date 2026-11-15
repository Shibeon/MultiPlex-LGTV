"""The mock's community stand-in (the Friends page): the GraphQL wire shapes the Rust client reads
(`plex::community`), the in-memory privacy setting, and `/status/sessions`.

Run: python3 -I tests/test_mock_pms_community.py   (-I drops the script dir, so it is added below)
"""
import calendar
import json
import pathlib
import sys
import time
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from mock_pms import Library, MockPms  # noqa: E402


def make():
    return MockPms(Library(seed=3, movies=24, shows=6, seasons=1, episodes=2))


def gql(pms, query, want=200):
    status, ctype, data = pms.handle("POST", "/api", json.dumps({"query": query, "variables": {}}).encode(), {})
    assert status == want, (status, data[:200])
    assert ctype == "application/json"
    return json.loads(data)


FEED = "query { activityFeed(first: 80, types: [WATCH_HISTORY]) { nodes { id } } }"


class CommunityMock(unittest.TestCase):
    def test_the_feed_spans_today_yesterday_and_older_with_a_binge(self):
        pms = make()
        nodes = gql(pms, FEED)["data"]["activityFeed"]["nodes"]
        self.assertEqual(len(nodes), 9)
        ages = [time.time() - calendar.timegm(time.strptime(n["date"][:19], "%Y-%m-%dT%H:%M:%S")) for n in nodes]
        self.assertEqual(ages, sorted(ages), "newest first")
        self.assertTrue(any(a < 6 * 3600 for a in ages) and any(a > 3 * 86400 for a in ages))
        binge = [n for n in nodes[:2]]
        self.assertTrue(all(n["metadataItem"]["type"] == "episode" for n in binge))
        self.assertEqual({n["userV2"]["id"] for n in binge}, {"1001"})
        self.assertEqual({n["metadataItem"]["grandparent"]["title"] for n in binge}, {"The Long Meridian"})
        self.assertEqual(len({n["userV2"]["id"] for n in nodes}), 4, "several friends")
        self.assertIn(None, [n["userV2"]["displayName"] for n in nodes], "one friend has no display name")
        for n in nodes:
            item = n["metadataItem"]
            show = item["grandparent"] or item
            self.assertRegex(show["key"], r"^/library/metadata/[0-9a-f]{24}$")
            self.assertTrue(show["images"]["coverPoster"].startswith("http://"))
        self.assertEqual(len(gql(pms, FEED.replace("80", "3"))["data"]["activityFeed"]["nodes"]), 3)

    def test_feed_states(self):
        pms = make()
        status, _, _ = pms.handle("POST", "/_mock/community", json.dumps({"feed": "empty"}).encode(), {})
        self.assertEqual(status, 200)
        self.assertEqual(gql(pms, FEED)["data"]["activityFeed"]["nodes"], [])
        pms.handle("POST", "/_mock/community", json.dumps({"feed": "fail"}).encode(), {})
        gql(pms, FEED, want=503)
        status, _, _ = pms.handle("POST", "/_mock/community", json.dumps({"feed": "nope"}).encode(), {})
        self.assertEqual(status, 400)

    def test_the_privacy_setting_reads_and_writes_in_memory(self):
        pms = make()
        read = "query { userPrivacy { watchHistory } }"
        self.assertEqual(gql(pms, read)["data"]["userPrivacy"]["watchHistory"], "FRIENDS")
        self.assertEqual(pms.writes, [])
        gql(pms, "mutation { updatePrivacy(input: { watchHistory: PRIVATE }) }")
        self.assertEqual(gql(pms, read)["data"]["userPrivacy"]["watchHistory"], "PRIVATE")
        self.assertEqual([(w[1], w[2]) for w in pms.writes], [("/api", b"PRIVATE")], "the write is logged")
        gql(pms, "mutation { updatePrivacy(input: { watchHistory: EVERYONE }) }", want=400)

    def test_status_sessions_answers_the_owner_and_hides_nothing_the_app_must_drop(self):
        pms = make()
        status, _, data = pms.handle("GET", "/status/sessions", b"", {})
        self.assertEqual(status, 200)
        rows = json.loads(data)["MediaContainer"]["Metadata"]
        self.assertEqual([(r["User"]["title"], r["Player"]["state"]) for r in rows], [("Camille", "playing"), ("Léo", "paused")])
        self.assertTrue(all(0 < r["viewOffset"] < r["duration"] for r in rows))
        self.assertIn("address", rows[0]["Player"], "the device facts a real server sends ride along")
        pms.handle("POST", "/_mock/community", json.dumps({"sessions": "none"}).encode(), {})
        self.assertEqual(json.loads(pms.handle("GET", "/status/sessions", b"", {})[2])["MediaContainer"]["size"], 0)
        pms.handle("POST", "/_mock/community", json.dumps({"sessions": "forbidden"}).encode(), {})
        self.assertEqual(pms.handle("GET", "/status/sessions", b"", {})[0], 401)

    def test_the_absolute_images_are_painted_locally(self):
        pms = make()
        status, ctype, _ = pms.handle(
            "GET", "/photo/:/transcode?width=64&height=64&url=http%3A%2F%2F127.0.0.1%2F_mock%2Fcommunity%2Favatar%2Fleo.png",
            b"", {})
        self.assertEqual((status, ctype), (200, "image/png"))


if __name__ == "__main__":
    unittest.main()
