#!/usr/bin/env python3
"""PreToolUse guard: an agent may not turn on the owner's live Watchlist switch.

`multiplex-watchlist-live` in a debug install's runtime root (on the television
`/tmp/app.multiplex.native.debug/`) lets a `devtriggers` build send its plex.tv ACCOUNT writes to the
REAL account: the Watchlist's add/remove and the Friends page's "Share what I watch" privacy mutation
(`updatePrivacy`), both behind the one gate (`rust-modules/plex/src/plex/watchlist.rs`,
`account_write_gate` / `live_armed`). Without it a debug build refuses every such write (the Q11
failsafe), which is what keeps an agent's TV run from changing a household's Watchlist or its
viewing-privacy setting. The owner creates the file by hand, from their own terminal, when they want to
check real writes; AGENTS.md: no agent creates it unless the owner asks for it in that same session.

What is refused is a Bash command that would CREATE the file: `touch`, `tee`, `cp`/`mv`/`install`/
`ln`/`dd`/`rsync`/`scp`, a `>`/`>>` redirection onto it, a script that opens it for writing, or
`tools/tv-session.sh up --arm watchlist-live`, also when the command is wrapped in
`tools/tv-ssh ssh tv "..."` or `ssh root@... "..."` (the inner string is part of the same command
line). Reading, grepping, listing, testing for and REMOVING it stay allowed, and so does an Edit or
Write of source or docs that merely name it (this hook sees Bash only).

THE ESCAPE HATCH is a prefix on the command itself — `PLX_WATCHLIST_LIVE_OWNER=1 …` — for the
owner, or for an agent the owner told IN THAT SESSION to turn the switch on. An agent reaching for it
on its own is deciding, on the household's behalf, to let a test build write to their real account.

FAIL-OPEN ON ITS OWN BUGS, fail-closed on the answer, like `outbound-guard.py`: exit 0 allows, exit 2
blocks and feeds stderr back to the model.
"""
import json
import re
import sys

# `plxnative-` is the name a build from before the plxnative -> multiplex rename reads, and an older
# debug build may still be deployed on the set, so creating either is refused.
NAME = re.compile(r"(?:multiplex|plxnative)-watchlist-live\b")
BYPASS = re.compile(r"\bPLX_WATCHLIST_LIVE_OWNER=1\b")
# `--arm watchlist-live` / `--arm=watchlist-live[=…]` on tv-session.sh writes multiplex-<name>.
ARM = re.compile(r"--arm(?:=|\s+)['\"]?watchlist-live\b")
# Commands that write a file named among their arguments.
WRITERS = re.compile(r"(?:^|[\s;&|('\"`])(?:touch|tee|cp|mv|install|ln|dd|rsync|scp|truncate)(?=\s)")
# A redirection whose target is the trigger: `> …multiplex-watchlist-live`, `>>`, `>|`, `&>`.
REDIRECT = re.compile(r"(?:\d?>>?|>\||&>)\s*['\"]?[^\s;&|'\"]*(?:multiplex|plxnative)-watchlist-live\b")
# A script that opens it for writing: python `open(…, 'w'|'a'|'x')`, `.write_text(`, `.touch(`.
SCRIPTED = re.compile(r"open\([^)]*(?:multiplex|plxnative)-watchlist-live[^)]*,\s*['\"][wax]|"
                      r"(?:multiplex|plxnative)-watchlist-live[^;\n]*\.(?:write_text|write_bytes|touch)\(")
# Statement separators. A pipe stays inside the segment on purpose: `echo 1 | tee <path>`.
SPLIT = re.compile(r"&&|\|\||;|\n")


def creates(cmd):
    """Does `cmd` create the live trigger? Returns the offending segment, or None."""
    if BYPASS.search(cmd):
        return None
    if ARM.search(cmd):
        return ARM.search(cmd).group(0)
    if not NAME.search(cmd):
        return None
    if REDIRECT.search(cmd) or SCRIPTED.search(cmd):
        return cmd
    for seg in SPLIT.split(cmd):
        if NAME.search(seg) and WRITERS.search(seg):
            return seg.strip()
    return None


def main():
    raw = sys.stdin.read()
    try:
        payload = json.loads(raw)
    except Exception:
        return 0
    if not isinstance(payload, dict) or payload.get("tool_name") != "Bash":
        return 0
    ti = payload.get("tool_input")
    cmd = ti.get("command") if isinstance(ti, dict) else None
    if not isinstance(cmd, str) or not cmd.strip():
        return 0
    seg = creates(cmd)
    if seg is None:
        return 0
    sys.stderr.write(
        "BLOCKED: this command would turn on the owner's live Watchlist switch "
        "(multiplex-watchlist-live).\n"
        "  the command: %s\n\n"
        "With that file present a debug build writes Watchlist add/remove and the Friends page's\n"
        "privacy setting (updatePrivacy) to the REAL plex.tv account.\n"
        "AGENTS.md: no agent creates it unless the owner asks for it in this same session. Test writes\n"
        "against the mock instead (`tools/tv-session.sh up --mock-plextv=PORT` arms the discover\n"
        "stand-in). If the owner did ask, prefix the command with PLX_WATCHLIST_LIVE_OWNER=1.\n"
        % seg[:160])
    return 2


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as e:      # a bug in the guard must not wedge every Bash call
        sys.stderr.write("watchlist-live-guard: internal error, allowing (%s)\n" % e)
        sys.exit(0)
