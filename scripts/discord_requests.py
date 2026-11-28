#!/usr/bin/env python3
"""Checks the #device-requests forum on Discord and handles new posts. Run by hand.

  python scripts/discord_requests.py                      show the plan, then ask about each post
  python scripts/discord_requests.py --apply              do the whole plan without asking
  python scripts/discord_requests.py --skip TEXT          leave out posts whose title contains TEXT
                                                          (repeatable)
  python scripts/discord_requests.py --unsupported TEXT   plan "Unsupported" for the new posts whose
                                                          title contains TEXT (repeatable)
  python scripts/discord_requests.py --bug TEXT           the same for "Device bug fix" (a bug report)
  python scripts/discord_requests.py --selftest

A post is new while it has no tag, or only "New device". Its VID:PID is looked up in the
"Supported devices" table of README.md as released (git tags):
  in the release that was current when it was posted -> "Supported" + reply
  only in a later release                            -> "Added" + "Awaiting confirm" + reply
  anything else                                      -> left as it is, only listed
An "Unsupported" post (planned with --unsupported, or tagged by hand and still open) always gets
the reply saying how to object (a new post explaining how to add it) and is then closed.
No reply is sent where the server owner or the bot has already written; for "Added" only a
message naming the new version counts.

A post tagged "Awaiting confirm" waits 24 hours for its author, counted from when the bot tagged it
or first saw the tag (kept in ~/.batteryhub-discord-awaiting.json). When someone other than the
owner or a bot has written since then, the tag is taken off; otherwise the post is closed. Nothing
happens until the script is run.

The bot token is read from ~/.batteryhub-discord-token (never put it in the repository). The bot
needs the Message Content intent.
"""
import json
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timedelta, timezone
from pathlib import Path

API = "https://discord.com/api/v10"
REPO_DIR = Path(__file__).resolve().parent.parent
TOKEN_FILE = Path.home() / ".batteryhub-discord-token"
STATE_FILE = Path.home() / ".batteryhub-discord-awaiting.json"  # {post id: since when it waits}
WAIT = timedelta(hours=24)
FORUM_NAME = "device-requests"
DOWNLOAD = "https://github.com/ferloom/Wireless-Device-Battery-Monitor/releases/latest"
# View channel, read history, manage threads (tags), send messages in threads.
PERMISSIONS = (1 << 10) | (1 << 16) | (1 << 34) | (1 << 38)
FORUM_TYPE, PINNED = 15, 1 << 1

NEW, SUPPORTED, UNSUPPORTED = "new device", "supported", "unsupported"
ADDED, AWAITING = "added", "awaiting confirm"
BUG = "device bug fix"
TAGS = {SUPPORTED: [SUPPORTED], ADDED: [ADDED, AWAITING], UNSUPPORTED: [UNSUPPORTED], BUG: [BUG]}
REPLIES = {
    SUPPORTED: "Hi! This device (`{id}`) is already supported by BatteryHub. Please install the latest version "
               "({version}) and check that the level shows: <{url}>\nIf it doesn't, reply here.",
    ADDED: "Hi! Support for this device (`{id}`) has been added. Please update to BatteryHub {version}: <{url}>\n"
           "Then tell us here whether the level shows and matches the maker's app.",
    UNSUPPORTED: "Hi! This device can't be added to BatteryHub for now: I don't know a documented way to read its "
                 "battery, and commands are never guessed. (Over Bluetooth, BatteryHub shows the level whenever "
                 "Windows shows one.)\nIf you think I'm wrong, please create a new post, say right at the start that "
                 "it can be added, and explain how (for example, link a project or a protocol description that reads "
                 "its battery). This post is now closed.",
}


def table_ids(readme, heading):
    """{VID:PID: rows naming it} of a README table. A bare `PID` takes the VID written before it."""
    parts = readme.split(f"\n## {heading}\n", 1)
    if len(parts) < 2:
        return {}
    ids = {}
    for row in parts[1].split("\n## ", 1)[0].splitlines():
        if not row.startswith("|"):
            continue
        name, vid = row.split("|")[1].strip(), None
        for m in re.finditer(r"`(?:([0-9A-F]{4}):)?([0-9A-F]{4})`", row, re.I):
            vid = m[1] or vid
            if vid and not vid.upper().startswith("FF"):  # FFxx:yyyy is a HID usage, not a device
                ids.setdefault(f"{vid}:{m[2]}".upper(), set()).add(name)
    return ids


def post_id(title, body):
    """The one VID:PID a request is about, or None if there is none or several."""
    body = re.sub(r"\bif\d+ [0-9A-F]{4}:[0-9A-F]{4}", "", body, flags=re.I)  # HID collections
    ids_lines = "\n".join(re.findall(r"^.*\bIDs:.*$", body, re.M))
    for text in (title, ids_lines, body):
        ids = {i.upper() for i in re.findall(r"\b[0-9A-F]{4}:[0-9A-F]{4}\b", text, re.I)}
        if ids:
            return ids.pop() if len(ids) == 1 else None
    return None


def verdict(device, created, releases):
    """releases: [(tag, date, supported table_ids)], newest first."""
    now = releases[0][2].get(device)
    if not now:
        return None
    then = next((ids for _, date, ids in releases if date <= created), {})
    # A receiver id shared by several families counts as added when a new row starts naming it.
    return SUPPORTED if now <= then.get(device, set()) else ADDED


def answered(messages, ours, kind, version):
    """Whether the owner or the bot (ids in `ours`) already said what the reply would say."""
    said = [m["content"] for m in messages if m["author"]["id"] in ours]
    if kind == ADDED:  # "coming in the next version" does not count: the release is news
        return any(version.lstrip("v") in text for text in said)
    return bool(said)


def awaiting_step(messages, ours, since, now):
    """For a post waiting for its author: ("untag", their messages) once someone else wrote after
    `since`, ("close", []) after 24 hours of silence, else (None, [])."""
    theirs = [m for m in messages if m["author"]["id"] not in ours and not m["author"].get("bot")
              and datetime.fromisoformat(m["timestamp"]) > since]
    if theirs:
        return "untag", theirs
    return ("close" if now - since >= WAIT else None), []


def load_state():
    return json.loads(STATE_FILE.read_text(encoding="utf-8")) if STATE_FILE.is_file() else {}


def save_state(state):
    STATE_FILE.write_text(json.dumps(state, indent=1), encoding="utf-8")


def git(*args):
    return subprocess.run(["git", "-C", str(REPO_DIR), *args], capture_output=True, encoding="utf-8", check=True).stdout


def load_releases():
    try:
        git("fetch", "--tags", "--quiet")
    except subprocess.CalledProcessError:
        print("Uwaga: nie udało się pobrać tagów, używam lokalnych.")
    out = git("for-each-ref", "refs/tags/v*", "--sort=-creatordate", "--format=%(refname:short) %(creatordate:iso-strict)")
    releases = []
    for line in out.splitlines():
        tag, date = line.split()
        readme = git("show", f"{tag}:README.md")
        releases.append((tag, datetime.fromisoformat(date), table_ids(readme, "Supported devices")))
    if not releases:
        sys.exit("Brak tagów wydań w repozytorium.")
    return releases


class Discord:
    def __init__(self, token):
        self.token = token

    def call(self, method, path, body=None):
        request = urllib.request.Request(
            API + path,
            data=None if body is None else json.dumps(body).encode(),
            method=method,
            headers={
                "Authorization": f"Bot {self.token}",
                "Content-Type": "application/json",
                "User-Agent": "DiscordBot (https://discord.com, 1.0)",
            },
        )
        for _ in range(5):
            try:
                with urllib.request.urlopen(request, timeout=30) as response:
                    return json.loads(response.read() or "null")
            except urllib.error.HTTPError as e:
                text = e.read().decode(errors="replace")
                if e.code != 429:
                    raise RuntimeError(f"{method} {path}: HTTP {e.code} {text}") from None
                time.sleep(float(json.loads(text).get("retry_after", 1)) + 0.1)
        raise RuntimeError(f"{method} {path}: rate limited")

    def forum(self):
        for guild in self.call("GET", "/users/@me/guilds"):
            for channel in self.call("GET", f"/guilds/{guild['id']}/channels"):
                if channel["type"] == FORUM_TYPE and channel["name"] == FORUM_NAME:
                    return self.call("GET", f"/guilds/{guild['id']}"), channel
        app = self.call("GET", "/oauth2/applications/@me")["id"]
        sys.exit(f"Bot nie widzi forum #{FORUM_NAME}. Zaproś go na serwer:\n"
                 f"https://discord.com/oauth2/authorize?client_id={app}&scope=bot&permissions={PERMISSIONS}")

    def threads(self, guild_id, forum_id):
        active = self.call("GET", f"/guilds/{guild_id}/threads/active")["threads"]
        found = [t for t in active if t["parent_id"] == forum_id]
        before = ""
        while True:
            page = self.call("GET", f"/channels/{forum_id}/threads/archived/public?limit=100{before}")
            found += page["threads"]
            if not page.get("has_more") or not page["threads"]:
                return found
            stamp = page["threads"][-1]["thread_metadata"]["archive_timestamp"]
            before = "&before=" + urllib.parse.quote(stamp)


def main(argv):
    def values(flag):
        return [argv[i + 1].lower() for i, a in enumerate(argv[:-1]) if a == flag]

    unsupported, bug, skip = values("--unsupported"), values("--bug"), values("--skip")
    if not TOKEN_FILE.is_file():
        sys.exit(f"Brak tokenu bota. Zapisz go w pliku {TOKEN_FILE}")
    discord = Discord(TOKEN_FILE.read_text(encoding="utf-8-sig").strip())
    me = discord.call("GET", "/users/@me")["id"]
    guild, forum = discord.forum()
    tag_id = {t["name"].lower(): t["id"] for t in forum["available_tags"]}
    tag_name = {v: k for k, v in tag_id.items()}
    missing = {NEW, SUPPORTED, UNSUPPORTED, ADDED, AWAITING, BUG} - tag_id.keys()
    if missing:
        sys.exit(f"Na forum brakuje tagów: {', '.join(sorted(missing))}")

    releases = load_releases()
    version = releases[0][0]
    listed_unsupported = table_ids(git("show", f"{version}:README.md"), "Unsupported devices")
    actions = []  # (post id, title, what it does, reply, edits of the post)
    state, now = load_state(), datetime.now(timezone.utc)
    for thread in discord.threads(guild["id"], forum["id"]):
        applied = thread.get("applied_tags", [])
        tags = {tag_name.get(t) for t in applied}
        tagged_unsupported = tags == {UNSUPPORTED}
        if thread.get("flags", 0) & PINNED or thread["thread_metadata"].get("locked"):
            continue
        if not (tags <= {NEW} or tagged_unsupported or AWAITING in tags):
            continue
        closed = thread["thread_metadata"].get("archived")
        tid, title = thread["id"], thread["name"]
        if any(text in title.lower() for text in skip):
            continue
        try:
            if AWAITING in tags:
                since = datetime.fromisoformat(state.setdefault(tid, now.isoformat()))
                messages = discord.call("GET", f"/channels/{tid}/messages?limit=100")
                step, theirs = awaiting_step(messages, (me, guild["owner_id"]), since, now)
                if step == "untag":
                    rest = [t for t in applied if t != tag_id[AWAITING]]
                    actions.append((tid, title, "zdjęcie Awaiting confirm", None,
                                    [{"archived": False, "applied_tags": rest}]))
                    print(f"[ktoś odpisał -> zdjęcie Awaiting confirm] {title}")
                    for m in reversed(theirs):
                        print(f"    {m['author']['username']}: {m['content']}")
                elif step == "close" and not closed:
                    actions.append((tid, title, "zamknięcie", None, [{"archived": True}]))
                    print(f"[24 h bez odpowiedzi -> zamknięcie] {title}")
                elif not closed:
                    print(f"[czeka do {(since + WAIT).astimezone():%d.%m %H:%M}] {title}")
                continue
            starter = discord.call("GET", f"/channels/{tid}/messages/{tid}")
            device = post_id(title, starter["content"])
            if tagged_unsupported or any(text in title.lower() for text in unsupported):
                kind = UNSUPPORTED
            elif any(text in title.lower() for text in bug):
                kind = BUG
            else:
                created = datetime.fromtimestamp(((int(tid) >> 22) + 1420070400000) / 1000, timezone.utc)
                kind = device and verdict(device, created, releases)
            if not kind:
                if not starter["content"]:
                    why = "pusta treść (włącz Message Content Intent?)"
                elif not device:
                    why = "brak jednego VID:PID"
                elif device in listed_unsupported:
                    why = f"{device} jest w README jako nieobsługiwane"
                else:
                    why = f"{device} nie ma na liście obsługiwanych w {version}"
                print(f"[bez zmian] {title}: {why}")
                continue
            reply = None
            if kind in REPLIES:
                messages = discord.call("GET", f"/channels/{tid}/messages?limit=100")
                if not answered(messages, (me, guild["owner_id"]), kind, version):
                    reply = REPLIES[kind].format(id=device, version=version, url=DOWNLOAD)
        except RuntimeError as e:
            print(f"[błąd] {title}: {e}")
            continue
        if tagged_unsupported and closed and not reply:
            continue  # already dealt with
        edits = [{"archived": False, "applied_tags": [tag_id[t] for t in TAGS[kind]]}]
        if kind == UNSUPPORTED:  # a separate call: a closed thread takes no other edit
            edits.append({"archived": True})
        names = " + ".join(TAGS[kind])
        what = names + (" i odpowiedź" if reply else "") + (" i zamknięcie" if kind == UNSUPPORTED else "")
        actions.append((tid, title, what, reply, edits))
        extra = (", z odpowiedzią" if reply else "") + (", zamknięcie" if kind == UNSUPPORTED else "")
        print(f"[{names}] {title} ({device or 'bez VID:PID'}){extra}")
        if reply:
            print("    " + reply.replace("\n", "\n    "))

    save_state(state)
    if not actions:
        print("Nic do zrobienia.")
        return
    ask = "--apply" not in argv
    if ask and not sys.stdin.isatty():
        print("To był podgląd. Dodaj --apply, żeby wykonać.")
        return
    for tid, title, what, reply, edits in actions:
        if ask and input(f"{title}: {what}? [t/N] ").strip().lower() != "t":
            continue
        try:
            # Reply first: if tagging then fails, the next run sees the reply and only tags.
            if reply:
                discord.call("POST", f"/channels/{tid}/messages", {"content": reply, "allowed_mentions": {"parse": []}})
            for edit in edits:
                discord.call("PATCH", f"/channels/{tid}", edit)
            tags_now = edits[0].get("applied_tags")
            if tags_now is not None:  # tagged: the wait starts now; untagged: forget it
                if tag_id[AWAITING] in tags_now:
                    state[tid] = datetime.now(timezone.utc).isoformat()
                else:
                    state.pop(tid, None)
                save_state(state)
            print(f"Gotowe: {title}")
        except RuntimeError as e:
            print(f"[błąd] {title}: {e}")


def selftest():
    readme = ("x\n## Supported devices\n\n| A | `3554:F507` / `F508`, command `B4` |\n| B | `FF00` |\n"
              "## Unsupported devices\n| C | `040B:0897` |\n")
    assert table_ids(readme, "Supported devices") == {"3554:F507": {"A"}, "3554:F508": {"A"}}
    assert list(table_ids(readme, "Unsupported devices")) == ["040B:0897"]
    body = "- IDs: `1e7d:3a3f`\n- HID: `if3 FF00:0001 64/64/0`, `if4 000C:0001 3/0/0`\n  other 1234:5678"
    assert post_id("my mouse", body) == "1E7D:3A3F"
    assert post_id("mouse (3554:F507)", body) == "3554:F507"
    assert post_id("x", "HID: `if3 FF00:0001 64/64/0`") is None
    assert post_id("x", "1111:2222 or 3333:4444") is None
    day = lambda d: datetime(2026, 11, d, tzinfo=timezone.utc)
    old, new = {"A": {"x"}, "S": {"keyboards"}}, {"A": {"x"}, "B": {"y"}, "S": {"keyboards", "mice"}}
    releases = [("v2", day(20), new), ("v1", day(10), old)]
    assert verdict("A", day(15), releases) == SUPPORTED
    assert verdict("B", day(15), releases) == ADDED
    assert verdict("B", day(25), releases) == SUPPORTED
    assert verdict("S", day(15), releases) == ADDED
    assert verdict("C", day(15), releases) is None
    said = [{"author": {"id": "bot"}, "content": "it will be in the next version"}]
    assert answered(said, ("bot", "owner"), UNSUPPORTED, "v1.9.2")
    assert not answered(said, ("bot", "owner"), ADDED, "v1.9.2")
    assert answered([{"author": {"id": "owner"}, "content": "update to 1.9.2"}], ("bot", "owner"), ADDED, "v1.9.2")
    assert not answered([{"author": {"id": "user"}, "content": "+1"}], ("bot", "owner"), UNSUPPORTED, "v1.9.2")
    since = day(2)

    def wrote(who, when, is_bot=False):
        return [{"author": {"id": who, "bot": is_bot}, "timestamp": when.isoformat(), "content": "x"}]

    assert awaiting_step([], ("me",), since, since + WAIT / 2)[0] is None
    assert awaiting_step([], ("me",), since, since + WAIT)[0] == "close"
    assert awaiting_step(wrote("user", day(3)), ("me",), since, day(5))[0] == "untag"
    assert awaiting_step(wrote("user", day(1)), ("me",), since, day(5))[0] == "close"  # written before
    assert awaiting_step(wrote("me", day(3)) + wrote("tickets", day(3), True), ("me",), since, day(5))[0] == "close"
    print("ok")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")  # titles can hold emoji
    selftest() if "--selftest" in sys.argv else main(sys.argv[1:])
