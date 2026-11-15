# MultiPlex for LG webOS

The native LG webOS television app of **MultiPlex**, an unofficial [Plex](https://www.plex.tv/)
client. The interface is drawn straight on the GPU and video plays on the television's own
decoder: no web view, no Chromium.

This repository is a fork of [PlxNative](https://github.com/GLinnik21/plx-native) by Gleb Linnik,
distributed under the same licence (GPL-3.0-or-later, see [LICENSE](LICENSE)). The copyright
notices of the original work are kept. MultiPlex is an independent project: it is not affiliated
with, endorsed by or supported by the PlxNative project, Plex, Inc. or LG. Please report problems
with this app here, not to the original project.

### What is different from the original project

- The app's identity: name, application id (`app.multiplex.native`), icons and splash screen. It
  installs beside the original app rather than replacing it.
- The device name Plex sees: "MultiPlex (LG TV)".
- No optional reporting: this fork's builds carry no crash-report or analytics destination, so
  nothing is sent, the first-run question about reports is not asked, and Settings → Privacy &
  data lists only the privacy policy and Delete all local data.

MultiPlex features (themes, My list, Watch together, ...) are being brought over step by step.

### Installing

The television must be rooted, with Homebrew Channel. Every push to `main` builds a debug
package (`.ipk`) in this repository's GitHub Actions (CI run, artifact `multiplex-<commit>`).
Install it with Homebrew Channel or `ares-install`. The detailed steps are in
[docs/install-and-verify.md](docs/install-and-verify.md).

---

*Much of what follows comes from the original project's README, kept for its technical content.
Where it speaks in the first person, it describes the original author's set-up.*

## Why this exists

The official Plex app on my old LG is slow. Scrolling a shelf stutters, opening a poster takes a
beat too long. It behaves like a web page because it is one: a web app running in the television's
Chromium. Patching it doesn't help — the ceiling isn't the code, it's the browser.

So I threw the browser away.

MultiPlex draws straight on the GPU and hands video to the same silicon the built-in apps use. No
Chromium, no JavaScript, no web view. It just draws. Almost all of it is Rust, and I use it every
day to watch things off my server in the next room.

## What it looks like

Real captures of the app, not mockups: the same code that runs on the television, on the desktop
simulator, browsing a demo library of openly licensed films
([credits](docs/screenshots/CREDITS.md)). `make screenshots` regenerates them.

![Home](docs/screenshots/home.jpg)

**Home** — a rotating hero from what you're partway through and what's just landed, shelves under it.

![Library](docs/screenshots/library.jpg)

**Library** — a library opens on its full poster grid, with an A–Z rail down the side and one
header row for sort & filter (including unwatched-only), a Collections page and a Recommended
page. The shelves live on Recommended: what you're partway through, with the time left under
the focused card, and what just landed
([pictured here](docs/screenshots/ux-library-grid.jpg)).
Settings → Libraries → Under posters also has a **Sort value** switch (on by default, per
profile). In a movie or TV show library sorted by something other than Title or Random (its movies, shows, seasons or
episodes), every card shows the value it is sorted by on one line under it (a score, a length, a date as Today,
Yesterday, 27 Oct or Oct 2024), in place of the title and details line; a sort with no value to show, such as
the Show order of Seasons and Episodes, keeps the usual captions. It uses the existing sort menu; other poster
surfaces keep their usual captions.

![Search](docs/screenshots/search.jpg)

**Search** — one query across every server you can reach, results grouped by kind.

![Player](docs/screenshots/player.jpg)

**Player** — the transport with chapters and track menus, drawn over video the television decodes
itself. (In this capture the frame under it was decoded by the simulator.)

## What it does

- **An interface that keeps up with the remote.** 60 fps on the 2019 set I develop on, with
  frame-rate regression scenes that measure it on the television rather than trusting it.
- **Sign in on the TV** with an on-screen QR code and pick a Plex Home profile.
- **Browse and search your libraries** — on your own servers and on ones shared with you. Your
  servers' libraries, that is, plus the plex.tv Watchlist as its own tab, where any title can be added or
  removed; search covers servers only.
- **Direct play** H.264 and HEVC — 4K, 10-bit, Dolby Vision profiles 5 and 8, E-AC-3 Atmos, and AV1 or VP9 (10-bit VP9 only on webOS 4 sets)
  on televisions whose codec table lists that decoder — decided against your television's own
  codec table where it publishes one. Anything else the server transcodes, and you can switch a
  playback to Auto to follow a link whose speed changes.
- **Everything you expect while watching**: resume, seek and scrub, chapters, audio and subtitle
  tracks (including image subtitles), Skip Intro, Skip Credits, Up Next with auto-advance, and
  progress reported back to your server.

## Will it work on my television?

**It needs webOS 4.0 or newer**, with two tested exceptions: a 2016 set on **webOS 3.4.3** signs
in, browses, searches and plays (direct play, transcodes, subtitles and seeking) through Developer
Mode, and a 2018 set on **webOS 3.9.3** plays direct play the same way when built from source at
or after [`60f99c7`](https://github.com/Shibeon/MultiPlex-LGTV/commit/60f99c7) — the catalogue's
prebuilt v0.8.0 predates that fix (see [Known issues](#known-issues)). Other 3.x releases are
untested, and anything older than 3.4 is unknown. Past that, it most
likely will. I develop and test on a 2019 set, and opt-in usage reports show
video playing on sets from 2018 through the newest, on webOS 11 — both direct play and server
transcodes, Developer Mode installs included.

Owners' reports, set by set, are in the [compatibility list](docs/compatibility.md).

### Known issues

| Where | What happens |
|---|---|
| **Some 2019 sets on LG's k5lp or k3lp chip**, installed through Developer Mode | Video won't play. On these sets LG's Developer Mode sandbox can withhold a device the video path needs — Kodi and Moonlight hit the same wall. The app checks for it and says so instead of crashing; other sets on the same chip play normally. On a rooted TV with Homebrew Channel, the failure screen offers **Repair**, which applies the Homebrew Channel fix to the sandbox (one owner fixed it this way from a root shell; the button itself hasn't been run on an affected set yet). Without root there is no fix. |
| **2018 sets on firmware that reports platform release 3.9.3** | Sign-in and browsing work. The prebuilt **v0.8.0** holds a strong ELF reference to the cxx11 `StarfishMediaAPIs::Feed` symbol, which 3.x `libplayerAPIs` does not export, so on the prebuilt video never starts and no error is reported: [#249](https://github.com/Shibeon/MultiPlex-LGTV/issues/249). A source build at or after [`60f99c7`](https://github.com/Shibeon/MultiPlex-LGTV/commit/60f99c7) resolves Feed with a dual-ABI `dlsym` and plays direct play on a 2018 C7 (W17H chassis, 3.9.3). |
| **webOS 3.x** | Seeking jumps in place through LG's public `Seek`, as on webOS 5 and newer (tested on platform release 3.9.3 only). The in-place path webOS 4 uses depends on the internal layout of LG's 4.x media player, which differs on 3.x, so it is not used there. A set that refuses `Seek` falls back to restarting the stream at the new position, as all 3.x sets did before. Menus can drop frames at 1920x1080 on these sets; **Settings → System → Interface resolution** trades sharper text for smoother menus. |
| **Sets on LG's k6hp chip** | A crash seen in opt-in crash reports and not reproduced here, because I have no such set: [#174](https://github.com/Shibeon/MultiPlex-LGTV/issues/174). |

### Rooted or not

**No root is needed.** A regular TV in Developer Mode runs everything except the k5lp/k3lp Repair
above. What Developer Mode costs you is renewal: if the session lapses, LG removes the apps
installed through it ([how to keep them](docs/install-and-verify.md#important-developer-mode-expires)).
A rooted TV with Homebrew Channel has no expiry.

If something goes wrong, check [Troubleshooting](docs/troubleshooting.md) first, then
[tell me what happened](https://github.com/Shibeon/MultiPlex-LGTV/issues/new/choose) — and if you own one of the
sets above, it working is as useful a report as it failing: post it under
[Discussions > Compatibility](https://github.com/Shibeon/MultiPlex-LGTV/discussions/categories/compatibility).

## Installing

**First time installing an app outside the LG Content Store?** Follow the
[**step-by-step installation guide**](docs/install-and-verify.md). It starts with a regular LG
webOS TV, a computer on the same network, and a Plex account with access to your own or a shared
server. **No root is required.**

Set up LG Developer Mode and connect with webOS Dev Manager, then choose:

- **Install MultiPlex directly:** add only MultiPlex to the TV; install future `.ipk` updates from
  your computer.
- **Install Homebrew Channel first:** get an app catalogue on the TV, then install MultiPlex and
  its updates with the remote.

**Already have Homebrew Channel?** Find [MultiPlex in its catalogue](https://repo.webosbrew.org/apps/app.multiplex.native/)
and select **Install**. Skip the computer setup.

**Developer Mode needs periodic renewal.** If it expires and LG disables Developer Mode, apps
installed through it are removed. Installing Homebrew Channel through Developer Mode does not
remove that requirement. The guide explains [how to renew the session](docs/install-and-verify.md#important-developer-mode-expires).

For manual `.ipk` downloads, [verify the release checksum](docs/install-and-verify.md#verifying-the-package)
before installing. Homebrew Channel verifies catalogue downloads for you.

## Also for LG webOS

Want a cleaner home screen for your LG TV? [**Clean Your TV**](https://github.com/Razdva122/clean-your-tv-lg-webos),
a separate project by another developer, lets you rearrange your apps, customize the dock and pick a
theme. The launcher has no sponsored rows, and on a regular TV it installs through LG Developer Mode.

[Explore Clean Your TV →](https://github.com/Razdva122/clean-your-tv-lg-webos)

## Nightly builds

Every day `main` moves, CI cuts a nightly `.ipk` from wherever it stands. It installs beside the
regular app as **"MultiPlex Nightly"** — its own tile, its own sign-in — so trying it never touches
the release you already trust.

Each build is a [prerelease](https://github.com/Shibeon/MultiPlex-LGTV/releases?q=nightly). Install
it with Dev Manager; a package installed that way does not update itself. The
[nightly builds guide](docs/nightly-builds.md) has the steps, how the versions work and what to do
if something goes wrong.

**It is not tested on a television.** It passes the same automated build and packaging checks a
release does, but nobody has watched it play. Each nightly is deleted 30 days after it is
published, so link to a specific `.ipk` at your own risk.

## Release candidates

Before a release, CI may publish a release candidate as a
[prerelease](https://github.com/Shibeon/MultiPlex-LGTV/releases?q=rc) named like `MultiPlex 0.8.0-rc.1`.
It is the release build itself, so it **installs as the regular MultiPlex app**, replacing the one
you have the way an update would. It is not in the Homebrew Channel, and the Channel will not offer
the final release over it either: reinstall the final by hand once it is out.

## Privacy

**Your library data never reaches me.** The app talks to your Plex server, to `plex.tv` to sign in,
to `community.plex.tv` when the Friends page is shown (your friends' watch activity, and your "Share
what I watch" setting, which changes only when you press its switch), and to
`discover.provider.plex.tv` for cast biographies, for the Watchlist (reading it, and adding or
removing a title when you ask, with the active profile's plex.tv token) and for the cover artwork of
each title in Continue Watching (which tells Plex which titles are in that row for the profile in use).

**Crash reports and usage analytics are off until you turn them on** — two separate first-run
questions, each answerable with Don't Share, both reversible later under Account → Settings →
Privacy & data. A build compiled without a reporting destination (every MultiPlex build so far)
cannot send either, so it asks neither question and that page shows no reporting switch at all. No title, search term, subtitle line, server name or address can appear in any
report. [`PRIVACY.md`](PRIVACY.md) is the whole statement, including the schemas.

ASS/SSA subtitles render natively during direct play, including styles, positioning, overlapping
signs and dialogue, karaoke, and embedded fonts. External ASS files retain their original scripts.
A re-encode still burns the selected subtitle on the server; over a remux the app draws a sidecar, and an embedded track when the link is local and the Part is 1..=30000 kbps, and otherwise the server burns it.

## The honest scope

I built this for how *I* watch, so it's narrower than Plex's:

- **Movies and TV shows.** No music, no photos, no live TV or DVR.
- **No typing in server addresses.** Servers come from your Plex account; set them up on a phone or
  PC and choose from what's there. Servers reached through Plex's relay, or that require an
  encrypted connection, are supported but haven't been watched end to end.
- **One person's spare time.** There will be bugs I haven't hit, because I don't watch the way you do.

If that fits, it's genuinely nice to use. If it doesn't, the official app will serve you better.

## Contributing

Issues and pull requests are welcome, especially from anyone whose television or library differs
from mine — [**docs/building.md**](docs/building.md) is the build and the test loop, and says what
hardware I most need help with. Feature ideas go to [Discussions >
Ideas](https://github.com/Shibeon/MultiPlex-LGTV/discussions/categories/ideas). Security issues go
through [`SECURITY.md`](SECURITY.md) rather than a public issue.

## Licence

[GPL-3.0-or-later](LICENSE), © 2026 Gleb Linnik. The PlxNative name and its brand artwork are excluded — see
[`TRADEMARKS.md`](TRADEMARKS.md), which also carries the Plex and LG non-affiliation statements.
Third-party components and their licences are in
[`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md) and `licenses/` — notably the app ships its own
LGPL build of FFmpeg. Those notices and licence texts ship inside the `.ipk` too, so they travel
with the binary and not only with this repository.

This is an unofficial client, not affiliated with, endorsed by, or sponsored by Plex GmbH or LG
Electronics. "Plex", "Rotten Tomatoes", "IMDb", "TMDB", "LG" and "webOS" are trademarks of their
respective owners; where they appear in the app, they identify whose service or score is being shown.
