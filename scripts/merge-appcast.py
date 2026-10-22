#!/usr/bin/env python3
"""Prints the update feed installed copies read, from a new release's (scripts/appcast.sh) and the
one that's out now:

    scripts/merge-appcast.py NEW.xml [CURRENT.xml] > appcast.xml

It holds the newest release, for everyone, and the newest beta (in the "beta" channel, for
copies that get betas) when that's newer than the release. Each item keeps its own signature, so
items carried over from the current feed stay as they were published. A CURRENT that's missing
or empty (the first release with a feed) leaves the new item alone."""
import os
import re
import sys

ITEM = re.compile(r"[ \t]*<item>.*?</item>\n?", re.S)


def build(item):
    """What Sparkle compares, as a key that orders 0.7.0b1 < 0.7.0b2 < 0.7.0 < 0.7.1 as it does."""
    version = re.search(r"<sparkle:version>([^<]+)</sparkle:version>", item).group(1).strip()
    match = re.fullmatch(r"(\d+(?:\.\d+)*)(?:b(\d+))?", version)
    if not match:
        sys.exit(f"An update's version isn't one this feed orders: {version}")
    numbers = [int(part) for part in match.group(1).split(".")]
    numbers += [0] * (4 - len(numbers))
    return (numbers, (0, int(match.group(2))) if match.group(2) else (1, 0))


def is_beta(item):
    return "<sparkle:channel>beta</sparkle:channel>" in item


def main():
    new_feed = open(sys.argv[1]).read()
    new_items = ITEM.findall(new_feed)
    if len(new_items) != 1:
        sys.exit("The new feed should have one item.")
    current = sys.argv[2] if len(sys.argv) > 2 else None
    items = ITEM.findall(open(current).read()) if current and os.path.exists(current) else []
    # Built again (a release run once more): the new one.
    items = [item for item in items if build(item) != build(new_items[0])] + new_items

    releases = [item for item in items if not is_beta(item)]
    betas = [item for item in items if is_beta(item)]
    release = max(releases, key=build, default=None)
    beta = max(betas, key=build, default=None)
    kept = [item for item in (beta, release) if item is not None]
    if release is not None and beta is not None and build(beta) < build(release):
        kept = [release]

    # The new feed around them, newest first.
    head = new_feed[: new_feed.index(new_items[0])]
    tail = new_feed[new_feed.index(new_items[0]) + len(new_items[0]) :]
    body = "".join(item if item.endswith("\n") else item + "\n" for item in sorted(kept, key=build, reverse=True))
    sys.stdout.write(head + body + tail)


if __name__ == "__main__":
    main()
