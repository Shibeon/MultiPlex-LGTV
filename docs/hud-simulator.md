# HUD authoring simulator

HUD authors lint, pack and preview a package with the app's own compiler, schema and real text
renderer through an immutable Linux authoring bundle. This fork has no HUD registry (the upstream
project's registry and its in-app gallery were removed; `docs/hud-plugins.md`, "Registry"), so the
bundle is a tool for authors only and nothing outside this repository consumes it.

`hud-simulator.yml` builds an optimized `multiplex-sim` with the existing `site-video-sim`
target and the repository's host FFmpeg. `tools/hud_bundle.py` packages explicit tracked fonts,
licenses, native dependencies, both document schemas, an indexed source/archive fixture corpus,
synthetic mock and capture tools. The
archive and its manifest have SHA-256 pins, the full source commit and the reported app version.
An unreleased build reports `-dev` and is explicitly marked development. A manual CI run with
`publish_development=true` publishes a prerelease utility under `hud-simulator-<full source SHA>`
for trying an unreleased build; it does not create a `v*` app release tag. A normal app release
attaches its own simulator once; uploads do not overwrite existing assets. The Linux bundle
needs Ubuntu 24.04-compatible glibc, the system Mesa driver, Xvfb and ffmpeg/Pillow for previews.
It relocates independently of the source/build checkout. No real Plex configuration is included.

Headless commands are `--hud-info`, `--hud-text-metrics`, `--hud-inspect FILE_OR_DIRECTORY`,
`--hud-schema manifest|hud --version 1.0`, `--hud-export builtin:classic|builtin:multiplex --out FILE`,
`--hud-pack DIRECTORY --out FILE`, and `--hud-lint FILE_OR_DIRECTORY --format json --lang all`.
The native packer freezes manifest.json, hud.json and declared resource/notice files into a
canonical `.plxhud` archive. Preview tools pack a directory once, then use the same archive bytes
and complete SHA-256 for inspection and every capture. Inspection expands
the compiler's collapsed states through its actual lookup into content, transport and section
valuations, with native control and scrub hit rectangles. Preview tools click those hit areas
through the shared frame writer's input queue. `tools/screenshots.py` owns the mock, runtime,
process, refusal checks and PNG output for both normal stills and virtual-clock selections.

`--hud-lint ... --text-skew PROFILE` applies explicitly supplied headroom at each
supported type rung. The default and shipping TV fit pass keep zero margin. `--hud-text-metrics`
measures the lint's exact app-text candidates in every language/rung/weight with real SDL_ttf,
including chapter fallbacks, episode prefixes and clock-width envelopes. Font hashes include
all installed fallback faces. On the TV, the `hud-text-metrics` dev trigger writes the same
report after text initialization into its runtime root. Capture that under the TV lock using
a synthetic guest session and compare it with the Linux report. Neither report is a substitute
for the other renderer's actual measurements.

After a successful stable app release, `release.yml` dispatches the simulator workflow on the
released tag, which builds the bundle from that tag and attaches it to the release once. Nothing
else is notified.

The initial native measurement contains 12,276 samples across all eleven shipped languages,
nine type rungs and both weights; the TV-minus-simulator differences it found ranged from 4 to 9
pixels. Reports retain exact font hashes and host/TV source identities.
