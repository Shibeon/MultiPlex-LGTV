# The Friends page

The native port of the web app's *Amis* page (`apps/web/src/pages/Activity.tsx` and
`packages/core/src/community.ts` in the web client). It sits on the top bar after My list and
before the divider: Search, Home, the library types, My list, **Friends**, then Settings and the
account chip. A managed Plex Home profile has no Friends pill, no page and no read.

## What it shows

- **The feed**: community.plex.tv `activityFeed(types:[WATCH_HISTORY])`, the newest 80 entries
  (`FEED_FIRST`, as the web app; no cursor is followed, so older activity is out of reach:
  `ci/list-caps.ini` classes `MAX_FEED` as an open reach cap). The posts are grouped by the
  viewer's local day ("Today", "Yesterday", then the date), and inside a day a friend's run of
  episodes of one show reads as one post ("watched 2 episodes of …"), with the web app's rules
  (`plx_data::community`).
- **The friend filter**: "All" first, then the six most active friends of the feed. A press
  narrows the shelves at once; nothing is read for it.
- **"Share what I watch"**: the account's `userPrivacy { watchHistory }`, shown as a filled toggle
  chip on the right of the row.
- **Playing now**, for a server's owner only: `GET /status/sessions` on the servers the account owns,
  showing who, what, how far and whether it is paused. It deliberately carries NO device, address,
  player, platform, bandwidth or stream details (`plex/src/plex/live.rs` decodes none of them). A
  server answering 401/403 hides the section, and so does an empty answer.

OK on a card opens the user's own copy when one of their servers holds the title (the Watchlist's
guid lookup), and otherwise its catalog page.

## Reads and the one write

The store is `rust-modules/data/src/stores/community.rs` (Sans-IO: it emits jobs, the app's runner in
`src/app/adapters/stores.rs` performs them). Nothing is read while the page is not on screen. The
feed and the privacy setting are read when the page shows, the feed again when it shows more than
`FEED_STALE_MS` (2 min) after its last landing; the live sessions are read when the page shows and
every `LIVE_EVERY_MS` (10 s) while it stays up.

**The privacy write (`updatePrivacy(input:{watchHistory: FRIENDS|PRIVATE})`) happens only on the
toggle's explicit press**: the page emits `StoreCmd::Community(CommunityCmd::SetShare(..))`, which
the Bridge's store step hands to `CommunityStore::set_share`; no pump emits it. In a `devtriggers`
build the write goes through the same gate as the Watchlist's writes
(`plex::watchlist::account_write_gate`): it is refused unless the loopback stand-in is armed or the
owner's live switch is on (`docs/watchlist.md`, "The live switch"). Tests drive it only against the
mock.

## The mock

`tests/mock_pms.py`'s `CommunityFixture` answers `POST /api` (`activityFeed`, `userPrivacy`,
`updatePrivacy`) and `GET /status/sessions`: four friends, movies and episodes (a two-episode binge
today), spread over today, yesterday and older days, and two live sessions. `POST
/_mock/community` switches the feed (`empty`, `fail`) and the sessions (`none`, `forbidden`).
