# MultiPlex Privacy Policy

Applies to MultiPlex 0.11.0. Last updated 15 November 2026.

## Who is responsible for MultiPlex data

Shibeon, who maintains MultiPlex, is responsible only for the data MultiPlex stores on your
television. MultiPlex sends nothing to its maintainer: it has no account system, no server and no
crash-report or analytics service of its own.

## No crash reports and no analytics

MultiPlex is based on PlxNative, which could send optional crash reports to Sentry and optional
product analytics to PostHog. MultiPlex releases are built without either destination: no Sentry or
PostHog address or key is in the app, so it cannot send a crash report, an error report, a sign-in
problem report or an analytics event, and it never asks whether you want to. Settings → Privacy &
data shows this policy, in the app's language, and Delete all local data, and no reporting switch;
the sign-in screen offers no report to send. The release workflow checks the packaged app for a
Sentry or PostHog key before publishing it.

When MultiPlex crashes or stops responding, it writes what happened to a crash log on the
television, where it stays (see below).

## What MultiPlex connects to

Every connection the app makes goes to one of the places below. Each request to a Plex service
carries the app's identity: the product name MultiPlex and its version, the platform webOS and the
television's webOS version, the device name "MultiPlex (LG TV)", the vendor LG, the app's client
identifier (a random identifier created on this television, which Plex uses to list it among your
devices), the app's language and, once you are signed in, the Plex account token of the profile in
use. Plex processes what its services receive under [Plex’s own Privacy
Policy](https://www.plex.tv/about/privacy-legal/). MultiPlex’s developer does not receive any of it.

**plex.tv.** Signing in with the code shown on screen; your account details and Plex features (for
example whether Plex Pass offers automatic intro skipping); the list of your servers and how to
reach them; the Home profiles on your account, switching between them, and their pictures.

**clients.plex.tv.** Your account's audio and subtitle preferences (languages and automatic
selection), read and, when you change them in MultiPlex, saved.

**discover.provider.plex.tv** (Plex's catalog). Your Watchlist (reading it, and adding or removing a
title when you ask), a title's catalog details and your watched state for it, a person's page and
credits, and Continue Watching artwork as described below.

Continue Watching artwork. For each title in a Continue Watching row, MultiPlex asks Plex’s catalog
(discover.provider.plex.tv) for the title’s own cover artwork. The request carries the title’s Plex
catalog identifier (which your Plex Media Server lists with the title), the language of the app, the
app’s client identifier and, only when the profile has one, its Plex account token, so it tells
Plex which titles are in the Continue Watching row of the profile in use. Your Plex Media Server then fetches
the picture itself from the image host that Plex’s answer names. The request is made when a
title’s card is first shown and this television has no saved answer for it; again when the saved
answer is more than a day old and the card is shown; again on a later start or after a profile
switch when an earlier attempt saved nothing, and when the card is shown again after Plex’s catalog
could not be reached (at most once every 45 seconds while it cannot be reached); and again after the saved artwork was removed to make room, by signing out, or
by changing the app’s language. MultiPlex’s developer does not receive these requests.

**metadata.provider.plex.tv** (Plex's metadata service), read only:

- *Upcoming episodes.* When you open a season of a show, the app asks for that season's episodes as
  Plex announces them, so it can show the ones not yet on your server. The request carries the
  season's Plex catalog identifier and the app's language, with the identity above. It is made once
  per visit to the season, and again after you sign in.
- *People in search.* When you search, the app also sends what you typed to this service to find
  people by name, with the identity above, so that a person can be shown at the top of the results.

**Your Plex Media Servers.** To browse and play media, update watch progress and use server
features, MultiPlex communicates directly with the Plex Media Servers you select: on your home
network, over the internet, or through Plex's relay when a server can only be reached that way.
Those requests are handled by the selected server and its operator. Artwork from outside your
server (posters, cast photos) is fetched by your server, not by the television.

**An address you type for a player HUD.** If you install a player HUD package from an address, the
app downloads the package from that address (a GitHub repository name you type is fetched from
github.com). Nothing else is sent to it.

MultiPlex makes no other connection: no advertising, no update check, no analytics.

## Data stored on this television

MultiPlex has a developer-only input/frame recorder used to reproduce and test bugs. It is not
part of the app you installed: a release build has the developer-trigger inputs, including the
recorder, compiled out — there is no code path in a release binary that can open, write or read a
recording, on this television or off it. The unconditional `multiplex-*.log` files are create-only
diagnostics, not trigger inputs. The recorder exists only in development builds used to build and
test MultiPlex itself, is started only by explicitly arming it on that build, and its recordings
never leave the device it was made on.

MultiPlex stores your Plex account token and a separate token for each server you use. For every
profile on your account you have switched to on this television, it also keeps that profile's own
server access token(s), so a later switch still works with no internet, and, for a PIN-protected
profile, a one-way check computed from that PIN rather than the PIN itself. It also keeps the app's
client identifier, the addresses and identifiers of those servers, the profile names and pictures on
your account — pictures and other downloaded artwork (posters, backdrops and cast images) are cached
as bounded files for reuse across restarts — your Home library choices, your recent searches, your
settings, any player HUD packages you installed, and local technical logs: an event log and an
error-output log, each emptied every time the app launches, a crash log that is kept until the
television restarts, a marker recording how much of the crash log has already been read, and a
bounded storage status snapshot. For a server you were asked about, it keeps whether you allowed it
to be reached without encryption on your home network (the answer only, for the Plex account that
gave it: the permission itself is never stored, and ends when a fresh check of the server no longer
reaches the same address, at a sign-in or sign-out, at a profile switch unless the new profile
reaches that server at the same address, or when the app returns from the background). It keeps no
bookmark of its own for where you stopped watching: playback position is held by your Plex Media
Server.

Those lifetimes differ. Signing out removes the sign-in, the servers registered with it and their
tokens, every profile's own cached server access token(s) and PIN check, the cached profile pictures
and other downloaded artwork, and your answers about unencrypted connections. Switching between the
profiles of one Plex account is not a sign-out and keeps all of it, including the server access
token(s) and PIN check cached for a profile you are not currently using, so that profile can be
switched to again with no internet. The event log and the error-output log are emptied every time
the app launches, the crash log is kept until the television restarts (the television clears its
temporary storage then), and the storage snapshot is replaced when its bounded status changes.
**webOS gives an application no way to run code as it is removed**, so the sign-in can survive an
uninstall — use Delete all local data in Settings → Privacy & data before uninstalling if you want
nothing of MultiPlex left on the television.

## Security reports and non-affiliation

Security vulnerabilities may be reported privately through GitHub Security Advisories for
`Shibeon/MultiPlex-LGTV`.

MultiPlex is an independent, unofficial application. It is not produced by, endorsed by, or
affiliated with Plex, Inc. or LG Electronics Inc.
