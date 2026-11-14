# Nightly builds

A nightly is the newest MultiPlex build from `main`, published on most days the code changes. It
installs **beside** the regular app as **MultiPlex Nightly** - its own launcher tile and its own
sign-in - so trying it never touches the release you already trust.

**A nightly is not tested on a television.** It passes the same automated build and packaging
checks a release does, but nobody has watched it play. Keep the regular MultiPlex installed, and
expect rough edges. New work reaches a nightly before it reaches a release, which is the point.

Each nightly is a [prerelease](https://github.com/Shibeon/MultiPlex-LGTV/releases?q=nightly) on
GitHub. This fork runs no Homebrew Channel repository of its own, so a nightly is installed by hand
and does not update itself.

## What you need

- A TV that already runs the regular MultiPlex. The [installation guide](install-and-verify.md)
  covers LG Developer Mode and webOS Dev Manager; nothing extra is needed for nightlies.
- The TV's Developer Mode session kept alive, as for any app installed through it. It
  [expires and removes the apps](install-and-verify.md#important-developer-mode-expires) unless you
  renew it - the nightly is no exception.

## Install

1. Open the newest [nightly prerelease](https://github.com/Shibeon/MultiPlex-LGTV/releases?q=nightly).
   Its notes name the `.ipk` and its SHA-256 checksum.
2. Download the `.ipk` and [verify the checksum](install-and-verify.md#verifying-the-package).
3. Install it with webOS Dev Manager, as in the [installation guide](install-and-verify.md).
4. Open **MultiPlex Nightly** from the launcher and sign in. It does not share your sign-in or
   settings with the regular app.

A package installed this way does not update itself: install the next nightly the same way, over
the old one. There is at most one nightly a day, and none on days when nothing outside the docs
changed.

## Going back

- To stop following nightlies, uninstall **MultiPlex Nightly** from the TV.
- The regular MultiPlex is separate and is not affected either way.
- Nightly builds are deleted from the releases page 30 days after they are published. The newest
  one is always kept. A copy already installed on your TV keeps working.

## What is different about a nightly

- **Its own app.** Its id is `app.multiplex.native.nightly`: separate tile, separate sign-in,
  separate saved state. Nothing is shared with the regular app.
- **Untested on a TV**, as above.
- **No reports.** Like every MultiPlex build so far, a nightly carries no crash-report or analytics
  destination, so it sends neither.
- **It reports a dated version**, which is what to quote when you report a problem.

## Versions

The regular app has one version, `X.Y.Z`. A nightly has two, because the TV's installer and the
app itself need different things from a version.

| What | Looks like | Where you see it | What it is for |
| --- | --- | --- | --- |
| Reported version | `0.8.0-nightly-20261006` | About screen, diagnostics, bug reports | Telling people which build you ran. **Quote this.** |
| Package version | `0.8.20261006` | The TV's installer | Telling the installer a build is different and newer. |

The reported version reads **`<release it is heading to>-nightly-<date it was cut>`**. The package
version keeps the same `major.minor` and uses the **date as its patch number**. A real patch
number is small, so a long one always means a nightly. Because the date only goes up, each nightly
is a newer package than the one before.

How releases are told apart:

| Release | Cut from | Version moves | The nightlies after it |
| --- | --- | --- | --- |
| Minor (the normal case) | `main` | `0.7.0` to `0.8.0` | head for the next minor: `0.9.<date>` |
| Patch | a maintenance branch (`release/vX.Y`) | `0.7.0` to `0.7.1` | `main` is unaffected; nightlies still come only from `main` |
| Major | `main`, bumped by hand | `0.x.y` to `1.0.0` | head for `1.1.<date>` |

Some consequences:

- **No downgrade across a release.** A release raises the version the next nightly heads for, so
  the package version keeps rising straight through it.
- **Nightlies come only from `main`.** A build from a maintenance branch would carry a lower
  version than one you already installed, so the pipeline refuses to publish one.
- **At most one nightly a day**, because the date is the whole build number.
- **The package version is not the commit.** The commit a build came from is in its release notes.

## If something goes wrong

- **Install fails.** Note the exact message, the package version and your TV model and webOS
  version, and [open an issue](https://github.com/Shibeon/MultiPlex-LGTV/issues/new/choose).
- **The app is gone after a while.** That is the Developer Mode session expiring, not the nightly.
  See the [installation guide](install-and-verify.md#important-developer-mode-expires).

## Reporting problems

[Open an issue](https://github.com/Shibeon/MultiPlex-LGTV/issues/new/choose) and include the **reported
version** from the About screen, for example `0.8.0-nightly-20261006`, with your TV model and webOS
version. Do not post passwords, Plex tokens or screenshots that show them.
