#!/usr/bin/env python3
"""Writes link.json: addresses, relay messages, link frames and session messages, worked out
here independently of the Swift and JavaScript implementations, which both check against it."""
import hashlib, json, re, struct, unicodedata, urllib.parse

def address(secret):
    return hashlib.sha256(b"kuvo-link address v1" + secret).digest()[:16]

secrets = [bytes(range(32)), bytes([0xff] * 32), hashlib.sha256(b"kuvo").digest()]
addresses = [{"secret": s.hex(), "address": address(s).hex()} for s in secrets]
a, b = address(secrets[0]), address(secrets[1])

relay = [
    {"name": "hello", "from": "device", "bytes": (b"\x01" + secrets[0]).hex()},
    {"name": "welcome", "from": "relay", "address": a.hex(), "bytes": (b"\x01" + a).hex()},
    {"name": "send", "from": "device", "to": b.hex(), "payload": "68656c6c6f", "bytes": (b"\x02" + b + b"hello").hex()},
    {"name": "deliver", "from": "relay", "address": a.hex(), "payload": "68656c6c6f", "bytes": (b"\x02" + a + b"hello").hex()},
    {"name": "unreachable", "from": "relay", "address": b.hex(), "bytes": (b"\x03" + b).hex()},
    {"name": "pushed", "from": "relay", "address": b.hex(), "status": 2, "bytes": (b"\x06" + b + b"\x02").hex()},
]

frames = [
    {"kind": kind, "session": session, "body": body.hex(), "bytes": (bytes([kind]) + struct.pack(">I", session) + body).hex()}
    for kind, session, body in [(1, 1, b"\x00" * 3), (3, 0xDEADBEEF, b"ciphertext"), (4, 42, b""), (5, 0, b"mail")]
]

def message(head, body):
    encoded = json.dumps(head, separators=(",", ":"), sort_keys=True).encode()
    return {"head": head, "body": body.hex(), "bytes": (struct.pack(">H", len(encoded)) + encoded + body).hex()}

messages = [
    message({"t": "req", "id": 1, "m": "GET", "p": "api/session?limit=50", "h": {"x-opencode-directory": "/home/me/p"}, "end": True}, b""),
    message({"t": "res", "id": 1, "s": 200, "h": {"content-type": "application/json"}, "end": True}, b'[{"id":"ses_1"}]'),
    message({"t": "data", "id": 7, "end": False}, b"data: {}\n\n"),
    message({"t": "cancel", "id": 7}, b""),
    message({"t": "ping"}, b""),
]

# Paths a phone may send an engine, through a Mac or a machine (what each gets: below).
routes = [{"path": p, "allowed": a} for p, a in [
    ("api/session", True), ("api/session?limit=50&order=desc", True), ("api/session/ses_1/prompt", True),
    ("api/session/active", True), ("api/event", True), ("api/event?x=1", True), ("api/info", True),
    ("api/provider", True), ("api/model/default", True), ("api/permission/saved", True), ("api/form", True),
    ("api/experimental/session/ses_1/wait", True), ("api/experimental/session/ses_1/instructions/entries/k", False),
    ("api/credential", False), ("api/credential/anthropic", False), ("api/integration/github/connect/key", False),
    ("api/integration", True), ("api/integration/github-copilot", True),
    ("api/config", False), ("api/config/shell", False), ("api/mcp", False), ("api/plugin", False),
    ("api/fs/list", True), ("api/fs/list?path=Sources", True), ("api/fs/find?query=main", True),
    ("api/fs/read/Sources/main.swift", True), ("api/fs/read", False), ("api/fs/write/x", False), ("api/fs/read/../../etc/hosts", False),
    ("api/fs/list?path=%2E%2E", False), ("api/fs/list?path=..%2F", False), ("api/fs/read/%2e%2e%2fsecret", False),
    ("api/session/%2E%2E/credential", False), ("api/session/%2e%2e/credential", False), ("api/session%2f..%2fcredential", False),
    ("api/session/.\t./.\t./api/credential", False), ("api/session/.\n./.\r./api/config", False), ("api/session/x\x7f", False),
    ("api/session#/../credential", False),
    # A sign joined with a mark after it (one character to some checks): still that sign to URL parsers.
    ("api/experimental/session/import#\u0301/wait", False), ("api/session/ses_1#\u0301/../../credential", False),
    ("api/fs/read/Re\u0301sume\u0301.txt", True),
    # The same, encoded: ".." with a joining mark after it, once decoded.
    ("api/session/%2e%2e\u200d/credential", False), ("api/session/%2e%2e\u0301/credential", False),
    # A space at either end, which URL parsers drop.
    ("api/session/ses_1/prompt ", False), (" api/session", False), ("api/fs/read/a%20b.txt", True),
    ("api/fs/read/..%5c..%5csecret", False), ("api/fs/read/a%5cb", False), ("api/fs/read/bad%E0%A4%A", False),
    ("api/sessions", False), ("api/eventual", False), ("api/session/../credential", False), ("kuvo/info", False), ("", False),
    ("api/pty", True), ("api/pty?location[directory]=x", True), ("api/pty/pty_1", True), ("api/pty/pty_1/connect-token", True),
    ("api/ptys", False), ("api/pty/%2e%2e/x", False), ("api/experimental/persistent-pty/pty_1", False),
    ("api/shell", True), ("api/shell/sh_1", True), ("api/shell/sh_1/output?cursor=0&limit=1", True),
    ("api/shell/../config", False), ("api/shell/sh_1/x", False), ("api/shells", False),
]]
# Commands the engine runs for agents, by method.
routes += [{"path": p, "allowed": a, "method": m} for p, a, m in [
    ("api/shell", False, "POST"), ("api/shell/sh_1", False, "DELETE"), ("api/shell/sh_1/output", True, "GET"),
]]

# Plan accounts and their keys, by method.
routes += [{"path": p, "allowed": a, "method": m} for p, a, m in [
    ("api/integration", True, "GET"), ("api/integration?fresh=1", True, "GET"), ("api/integration/github-copilot", True, "GET"),
    ("api/integration/anthropic/connect/key", True, "POST"), ("api/integration/github-copilot/connect/oauth", True, "POST"),
    ("api/integration/github-copilot/connect/oauth/att_1", True, "GET"), ("api/integration/gitlab/connect/oauth/att_1/complete", True, "POST"),
    ("api/integration/gitlab/connect/oauth/att_1", True, "DELETE"), ("api/credential/cred_1/activate", True, "POST"),
    ("api/credential/cred_1", True, "DELETE"), ("api/integration/openai@work/connect/key", True, "POST"),
    ("api/credential", False, "GET"), ("api/credential/cred_1", False, "GET"), ("api/credential", False, "POST"),
    ("api/credential?x=1", False, "GET"), ("api/credential/cred_1/activate", False, "GET"), ("api/credential/cred_1/x", False, "DELETE"),
    ("api/integration/anthropic/update", False, "POST"), ("api/integration/anthropic/connect/external", False, "POST"),
    ("api/integration/anthropic/connect/%6bey", False, "POST"), ("api/integration/anthropic//connect/key", False, "POST"),
    ("api/integration/./connect/key", False, "POST"), ("api/integration/anthropic/connect/key/x", False, "POST"),
    ("api/integration/anthropic/connect/key?x=1", False, "POST"), ("api/integration/anthropic", False, "DELETE"),
    ("api/integration/anthropic", False, "PATCH"), ("api/integration/anthropic/connect/key", False, "GET"),
    ("api/integration/anthropic/connect/key", False, "PUT"), ("api/integration/../credential", False, "GET"),
    ("api/integration/%2e%2e/credential", False, "GET"), ("api/integration/anthropic/connect/oauth/att_1/complete ", False, "POST"),
    ("api/integration/a#/../../credential", False, "GET"), ("api/credential/.hidden", False, "DELETE"),
]]

# The sockets a phone may open (PROTOCOL.md, "Sockets"): a terminal's.
sockets = [{"path": p, "allowed": a} for p, a in [
    ("api/pty/pty_1/connect", True), ("api/pty/pty_1/connect?cursor=0", True),
    ("api/pty/pty_1", False), ("api/pty", False), ("api/event", False), ("api/pty/pty_1/connect/x", False),
    ("api/pty/a/b/connect", False), ("api/pty/%2e%2e/connect", False), ("api/pty/..%5cx/connect", False),
    ("api/experimental/persistent-pty/pty_1/connect", False), ("api/session/ses_1/connect", False),
    ("api/pty/%2e%2e\u200d/connect", False),
]]

# Paths a phone may send the coding agents' bridge, on a Mac or a machine (`kuvo/agents/api/…`,
# without the prefix), however where the engine is is written.
agent_routes = [{"path": p, "allowed": a, "method": m} for p, a, m in [
    ("api/session/ses_1/prompt", True, "POST"), ("api/session", True, "GET"), ("api/event", True, "GET"),
    ("api/integration", True, "GET"), ("api/integration/claude-code@1f2e/connect/oauth", True, "POST"),
    ("api/kuvo/auto", True, "PUT"), ("api/kuvo/auto/limits", True, "DELETE"), ("api/kuvo/accounts", True, "POST"),
    ("api/kuvo/accounts/codex-cli@9a8b", True, "PATCH"), ("api/model", True, "GET"),
    ("api/kuvo/engine", False, "PUT"), ("api/kuvo/%65ngine", False, "GET"), ("api/kuvo//engine", False, "POST"),
    ("api/integration/claude-code/update", False, "POST"), ("api/integration/claude-code/%75pdate", False, "POST"),
    ("api/integration/claude-code//update", False, "POST"), ("api/integration/claude-code/./update", False, "POST"),
    ("api/integration/claude-code/%2e/update", False, "POST"), ("api/integration//claude-code/update", False, "POST"),
    ("api/integration/claude-code/update/", False, "POST"), ("api/integration/claude-code/update#x", False, "POST"),
    ("api/kuvo/engine#", False, "POST"), ("api/fs/read/../../etc/passwd", False, "GET"), ("api/credential", False, "GET"),
    ("api/config", False, "GET"), ("api/kuvo/other", False, "GET"), ("api/shell", False, "POST"),
    ("api/integration/claude-code/update ", False, "POST"), (" api/kuvo/engine", False, "PUT"),
    ("api/kuvo/engine  ", False, "PUT"),
    ("api/integration/claude-code/update#\u0301", False, "POST"), ("api/integration/claude-code/update?\u0301", False, "POST"),
    ("api/integration/claude-code/update/\u0301", False, "POST"), ("api/kuvo/engine#\u200d", False, "PUT"),
    ("api/integration/claude-code/update%3F\u0301", False, "POST"),
]]

# A paired phone is yours: the engine's whole API, as the app has it (accounts and keys, settings,
# MCP servers, commands included), with only a path that isn't what it seems refused; and the
# same of the coding agents' bridge, but for where the engine is and its group chats' own steps.
# The paths above are kept; what each should get is worked out here, by that rule.
def percent_decoded(path):
    if re.search(r"%(?![0-9A-Fa-f]{2})", path):
        return None
    try:
        return urllib.parse.unquote_to_bytes(path).decode("utf-8")
    except UnicodeDecodeError:
        return None

def joins_a_sign(text):
    # A sign other than a letter or digit, then a mark that joins it into one character.
    extends = lambda c: unicodedata.category(c) in ("Mn", "Me", "Mc") or c == "\u200d" or 0xFE00 <= ord(c) <= 0xFE0F
    return any(ord(x) < 0x80 and not x.isalnum() and extends(y) for x, y in zip(text, text[1:]))

def is_what_it_seems(path):
    if any(ord(c) < 0x20 or ord(c) == 0x7F for c in path) or path.startswith(" ") or path.endswith(" ") or "#" in path:
        return False
    decoded = percent_decoded(path)
    return decoded is not None and ".." not in path and ".." not in decoded and "\\" not in decoded \
        and not joins_a_sign(path) and not joins_a_sign(decoded)

def routed(path):
    at = path.find("?")
    head, query = (path, "") if at < 0 else (path[:at], path[at:])
    parts = [part for part in head.split("/") if part not in ("", ".") and part.lower() != "%2e"]
    return "/".join(percent_decoded(part) or part for part in parts) + query

def phone_may(path):
    return is_what_it_seems(path) and re.match(r"api(/|\?|$)", path) is not None

def agents_may(path):
    return phone_may(path) and re.match(r"api/kuvo/(engine|groups)(/|\?|$)", routed(path)) is None

agent_routes += [{"path": p, "allowed": False, "method": m} for p, m in [
    ("api/kuvo/groups", "GET"), ("api/kuvo/groups/grp_x/release", "POST"), ("api/kuvo/%67roups", "GET"),
]]
for item in routes:
    item["allowed"] = phone_may(item["path"])
for item in agent_routes:
    item["allowed"] = agents_may(item["path"])
# What that rule says of a few, as meant.
assert phone_may("api/credential") and phone_may("api/config") and phone_may("api/shell") and phone_may("api/integration/x/update")
assert not phone_may("api/session/../credential") and not phone_may("kuvo/info") and not phone_may("api/session/%2e%2e\u200d/x")
assert not agents_may("api/kuvo/engine") and not agents_may("api/kuvo/%65ngine") and not agents_may("api/kuvo/groups/x")
assert agents_may("api/integration/claude-code/update") and agents_may("api/kuvo/auto")

# Where a sign-in's page sends the browser back to the computer (`kuvo/sign-in/return`), which
# the phone hands it to load there: only this computer's own address (127.0.0.1, localhost, [::1])
# over http, at a port, with what the page sent back (a query), written plainly and in lowercase,
# in the characters an address may have as it is (RFC 3986), so nothing re-encodes it on the way.
# What's loaded is the address without a fragment, which the browser wouldn't send.
returns = [{"url": u} for u in [
    "http://127.0.0.1:19876/mcp/oauth/callback?code=abc&state=xyz", "http://localhost:1455/auth/callback?code=a.b-c_d~e&state=1",
    "http://[::1]:8080/cb?code=x", "http://127.0.0.1:19876/?code=x", "http://127.0.0.1:19876?code=x", "http://127.0.0.1:65535/cb?error=access_denied",
    "http://127.0.0.1:19876/cb?code=x#_=_", "http://127.0.0.1:19876/cb?code=%2F%3D", "http://127.0.0.1:1/cb?code=x",
    "http://127.0.0.1:19876/cb", "http://127.0.0.1:19876/cb?", "http://127.0.0.1/cb?code=x", "http://127.0.0.1:0/cb?code=x",
    "http://127.0.0.1:65536/cb?code=x", "http://127.0.0.1:019876/cb?code=x", "https://127.0.0.1:19876/cb?code=x", "HTTP://127.0.0.1:19876/cb?code=x",
    "http://LOCALHOST:19876/cb?code=x", "http://127.0.0.2:19876/cb?code=x", "http://example.com:19876/cb?code=x",
    "http://127.0.0.1.example.com:19876/cb?code=x", "http://localhost.example.com:80/cb?code=x", "http://me@127.0.0.1:19876/cb?code=x",
    "http://127.0.0.1:19876@example.com/cb?code=x", "http://127.0.0.1:19876\\@example.com/cb?code=x", "http://127.0.0.1:19876/cb?code=a b",
    "http://127.0.0.1:19876/cb?code=x\n", " http://127.0.0.1:19876/cb?code=x", "http://127.0.0.1:19876/cb?code=x ", "http://127.0.0.1:19876/c\\b?code=x",
    "http://127.0.0.1:19876/cb?code=\u00e9", "http://0x7f000001:19876/cb?code=x", "http://2130706433:19876/cb?code=x", "http://[::ffff:127.0.0.1]:19876/cb?code=x",
    "http://127.0.0.1:19876#?code=x", "http://127.0.0.1:19876/cb#x?code=y", "//127.0.0.1:19876/cb?code=x", "127.0.0.1:19876/cb?code=x", "",
    "http://127.0.0.1:1/cb?code=x#", "http://127.0.0.1:19876/a@b?code=x", "http://127.0.0.1:19876/cb?#x",
    # Only what an address may hold as it is: a `%` with two hex digits after it, no brackets or
    # quotes past the host, which a URL parser would encode (and a `%2F` with them, again).
    "http://127.0.0.1:19876/cb?code=a%2Fb&x=[1]", "http://127.0.0.1:19876/cb?code=a%2Fb&x=1", "http://127.0.0.1:19876/cb?code=50%",
    "http://127.0.0.1:19876/cb?code=%zz", "http://127.0.0.1:19876/cb?code=a|b", "http://127.0.0.1:19876/cb?code={x}", "http://127.0.0.1:19876/cb?code=a^b",
    "http://127.0.0.1:19876/cb?code=`x`", "http://127.0.0.1:19876/cb?code=\"x\"", "http://127.0.0.1:19876/cb?code=<x>", "http://127.0.0.1:19876/c[b]?code=x",
    "http://127.0.0.1:19876/cb?code=a'b(c)*!$,;:@/?~", "http://127.0.0.1:19876/cb?code=x#frag|ment[1]", "http://127.0.0.1:19876/cb%7C?code=x",
    "http://127.0.0.1:19876/cb?code=x" + "a" * 8200,
]]

def loads(url):
    # Worked out apart from the bridge's and the Mac's pattern: split as URL parsers do, then held
    # to the rule above.
    if len(url) > 8192 or any(not ("!" <= c <= "~") or c == "\\" for c in url) or not url.startswith("http://"):
        return None
    parts = urllib.parse.urlsplit(url)
    if parts.scheme != "http" or "@" in parts.netloc or not parts.query:
        return None
    plain = lambda text: re.fullmatch(r"(?:[A-Za-z0-9\-._~!$&'()*+,;=:@/?]|%[0-9A-Fa-f]{2})*", text) is not None
    if not plain(parts.path) or not plain(parts.query):
        return None
    host, _, port = parts.netloc.rpartition(":")
    if host not in ("127.0.0.1", "localhost", "[::1]") or not re.fullmatch(r"[1-9][0-9]{0,4}", port) or int(port) > 65535:
        return None
    return urllib.parse.urlunsplit(("http", parts.netloc, parts.path, parts.query, ""))

for item in returns:
    item["loads"] = loads(item["url"])
assert loads("http://127.0.0.1:19876/cb?code=x#_=_") == "http://127.0.0.1:19876/cb?code=x"
assert loads("http://[::1]:8080/cb?code=x") and loads("http://127.0.0.1:19876?code=x") == "http://127.0.0.1:19876?code=x"
assert not loads("http://127.0.0.1:19876/cb") and not loads("http://me@127.0.0.1:19876/cb?code=x") and not loads("http://127.0.0.1/cb?code=x")
assert loads("http://127.0.0.1:19876/cb?code=a%2Fb&x=1") and not loads("http://127.0.0.1:19876/cb?code=a%2Fb&x=[1]") and not loads("http://127.0.0.1:19876/cb?code=50%")

json.dump({"addresses": addresses, "relay": relay, "frames": frames, "messages": messages, "routes": routes, "sockets": sockets,
           "agentRoutes": agent_routes, "signInReturns": returns}, open("link.json", "w"), indent=2)
