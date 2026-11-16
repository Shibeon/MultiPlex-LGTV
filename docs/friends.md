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
  chip on the right of the row. When it could not be read, a press on it reads it again
  (`CommunityCmd::Retry`), never writes.
- **Playing now**, for a server's owner only: `GET /status/sessions` on the servers the account owns
  (`community::owns`: the server's plex.tv description says `owned`; the browsed server is not
  taken for one because it is browsed), showing who, what, how far and whether it is paused. It
  deliberately carries NO device, address, player, platform, bandwidth or stream details
  (`plex/src/plex/live.rs` decodes none of them). Every owned server answering 401/403 hides the
  section until the captured servers or their facts change (the owner's own server coming up while
  they browse a friend's share asks again), and so does an empty answer.
- **Avatars**: a friend's plex.tv picture, and the plex.tv poster of a title none of the user's
  servers holds, go through the photo proxy of a reachable server the account OWNS
  (`community::art_server`), never through a friend's share, not even the one browsed nor the one
  holding the card's title; with no owned server reachable the card draws the friend's initial.

OK on a card opens the user's own copy when one of their servers holds the title (the Watchlist's
guid lookup), and otherwise its catalog page.

- **Watch Together** (`rust-modules/screens/src/friends/page.rs`, "Watch Together"): *Your
  friends' sessions*, one card per session a Plex friend made visible to the viewer, and *Join
  with a code* in the head row. These come from the together store (`plx_data::stores::together`),
  not from community.plex.tv: its listing asks the relays found on the servers' published public
  names (`/rooms`, then `POST /friends` with that server's own token or the owner's delegation
  token, never the plex.tv account token) when the page arrives, and every 45 s while the page is
  up or the viewer's *Friends' session alerts* setting is on (`app/together_ui.rs`, `POLL_MS`), so
  unlike the community reads below it also runs while the page is not on screen. `PRIVACY.md`, "A
  Watch Together relay", is what a user is told about it.

## Reads and the one write

The store is `rust-modules/data/src/stores/community.rs` (Sans-IO: it emits jobs, the app's runner in
`src/app/adapters/stores.rs` performs them). Nothing is read from community.plex.tv while the page
is not on screen. The
feed and the privacy setting are read when the page shows, the feed again when it shows more than
`FEED_STALE_MS` (2 min) after its last landing; the live sessions are read when the page shows and
every `LIVE_EVERY_MS` (10 s) while it stays up. A failed read of the privacy setting is asked again
once after `RETRY_MS` (a transient 5xx or timeout recovers by itself), then only on Retry or the
page's next showing.

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
