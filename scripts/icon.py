#!/usr/bin/env python3
"""Generates KuvoCode/AppIcon.icon, the app icon in Icon Composer's format.

The icon is the team: Kuvo Code's five bot shapes (round, squircle, pebble, gem and bloom, as
BotAvatar.swift draws them) standing together at the bottom of the tile, each with two small
eyes, all looking up the same way. It's one flat shape with the gaps between the bots and their
eyes cut out, so Icon Composer colours it per appearance: ink on cream in light mode, cream on
ink in dark mode, and white in the clear and tinted styles. No glass, shine or shadow. Xcode
compiles the file with the app.

It also renders the README's docs/images/icon.png with Icon Composer's ictool, and, given their
paths, writes the iPhone app's copy and the website's icon.svg and apple-touch-icon.png:

    python3 scripts/icon.py [--phone ../kuvo-code-ios/KuvoCodeMobile/AppIcon.icon] [--web ../kuvo-web]

Needs numpy and OpenCV (pip install numpy opencv-python), and Xcode for ictool.
"""
import argparse
import json
import os
import shutil
import subprocess
import tempfile

import cv2
import numpy as np

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
OUT = os.path.join(ROOT, "KuvoCode", "AppIcon.icon")
README_ICON = os.path.join(ROOT, "docs", "images", "icon.png")
ICTOOL = "/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool"

CREAM, INK, WHITE = "#F1EDE5", "#17161B", "#FFFFFF"

# The team, back to front: shape, centre, size, turn in degrees, on Icon Composer's 1024-point
# canvas. The three in front run off the bottom edge.
TEAM = [
    ("bloom", 320, 520, 360, -6),
    ("gem", 712, 510, 360, 8),
    ("pebble", 140, 830, 420, -4),
    ("squircle", 884, 830, 430, 4),
    ("round", 512, 800, 440, 0),
]
GAP = 14                      # paper between one bot and the one in front of it
EYES = 0.66                   # their faces, as a share of the app's
LOOK = (0.09, -0.11)          # where they all look: right and up, as a share of their size

# The shapes are drawn as signed distances (negative inside) on a grid twice the canvas, then
# traced into a path.
SS = 2
N = 1024 * SS
_gy, _gx = np.mgrid[0:N, 0:N].astype(np.float64)
X, Y = (_gx + 0.5) / SS, (_gy + 0.5) / SS


def frame(cx, cy, deg):
    """Coordinates around (cx, cy), turned by deg."""
    a = np.deg2rad(deg)
    dx, dy = X - cx, Y - cy
    return np.cos(a) * dx + np.sin(a) * dy, -np.sin(a) * dx + np.cos(a) * dy


def ellipse(x, y, a, b):
    k0 = np.hypot(x / a, y / b)
    k1 = np.hypot(x / (a * a), y / (b * b))
    return k0 * (k0 - 1) / np.maximum(k1, 1e-9)


def superellipse(x, y, a, b, n):
    u, v = np.abs(x) / a + 1e-9, np.abs(y) / b + 1e-9
    f = (u ** n + v ** n) ** (1 / n)
    gx, gy = f ** (1 - n) * u ** (n - 1) / a, f ** (1 - n) * v ** (n - 1) / b
    return (f - 1) / np.maximum(np.hypot(gx, gy), 1e-9)


def polygon(x, y, points):
    d = np.full(x.shape, 1e9)
    sign = np.ones(x.shape)
    for (ax, ay), (bx, by) in zip(points[-1:] + points[:-1], points):
        px, py, ex, ey = x - ax, y - ay, bx - ax, by - ay
        h = np.clip((px * ex + py * ey) / (ex * ex + ey * ey), 0, 1)
        d = np.minimum(d, np.hypot(px - ex * h, py - ey * h))
        c1, c2, c3 = y >= ay, y < by, ex * (y - ay) > ey * (x - ax)
        sign = np.where((c1 & c2 & c3) | (~c1 & ~c2 & ~c3), -sign, sign)
    return sign * d


def rounded_polygon(x, y, points, r):
    """A polygon with every corner rounded by r: shrunk by r, then grown back."""
    p = np.array(points, float)
    area = sum(p[i - 1][0] * p[i][1] - p[i][0] * p[i - 1][1] for i in range(len(p)))
    lines = []
    for i in range(len(p)):
        t = (p[(i + 1) % len(p)] - p[i]) / np.linalg.norm(p[(i + 1) % len(p)] - p[i])
        lines.append((p[i] + np.array([-t[1], t[0]]) * np.sign(area) * r, t))
    inner = []
    for (p1, t1), (p2, t2) in zip(lines[-1:] + lines[:-1], lines):
        s = np.linalg.solve(np.array([[t1[0], -t2[0]], [t1[1], -t2[1]]]), p2 - p1)
        inner.append(tuple(p1 + t1 * s[0]))
    return polygon(x, y, inner) - r


def body(shape, x, y, s):
    """A bot's shape at size s, as BotShape in BotAvatar.swift draws it."""
    if shape == "round":
        return np.hypot(x, y) - 0.5 * s
    if shape == "squircle":
        return superellipse(x, y, 0.46 * s, 0.46 * s, 3.4)
    if shape == "pebble":
        return rounded_polygon(x, y, [(0, -0.66 * s), (0.6 * s, 0.44 * s), (-0.6 * s, 0.44 * s)], 0.24 * s)
    if shape == "gem":
        return rounded_polygon(x, y, [(0, -0.58 * s), (0.58 * s, 0), (0, 0.58 * s), (-0.58 * s, 0)], 0.22 * s)
    if shape == "bloom":
        a = np.arctan2(y, x) + np.pi / 2
        return (np.hypot(x, y) - s * (0.455 + 0.035 * np.cos(8 * a))) * 0.93
    raise ValueError(shape)


def eyes(shape, x, y, s):
    """Two eyes, as BotFace's calm face places them in its box."""
    w, h = 0.5 * s, 0.3 * s
    cx, cy = 0.04 * s, (0.12 if shape == "pebble" else -0.03) * s
    a, b = w * 0.17 / 2, h * 0.62 / 2
    return np.minimum(ellipse(x - (cx - 0.23 * w), y - cy, a, b), ellipse(x - (cx + 0.23 * w), y - cy, a, b))


def team():
    """Signed distance of the whole mark: each bot clears a gap behind it, then its eyes are cut out."""
    d = np.full((N, N), 1e9)
    for shape, cx, cy, s, deg in TEAM:
        x, y = frame(cx, cy, deg)
        own = body(shape, x, y, s)
        d = np.minimum(np.maximum(d, GAP - own), own)
        d = np.maximum(d, -eyes(shape, x - LOOK[0] * s, y - LOOK[1] * s, s * EYES))
    return d


def sample(field, px, py):
    gx, gy = px * SS - 0.5, py * SS - 0.5
    x0, y0 = np.clip(np.floor(gx).astype(int), 0, N - 2), np.clip(np.floor(gy).astype(int), 0, N - 2)
    fx, fy = gx - x0, gy - y0
    return (field[y0, x0] * (1 - fx) * (1 - fy) + field[y0, x0 + 1] * fx * (1 - fy)
            + field[y0 + 1, x0] * (1 - fx) * fy + field[y0 + 1, x0 + 1] * fx * fy)


def trace(d):
    """The outlines of d < 0, moved onto the exact edge and thinned to what a curve needs.
    Where the mark runs off the canvas, its outline is pushed past the edge."""
    contours, _ = cv2.findContours((d < 0).astype(np.uint8), cv2.RETR_CCOMP, cv2.CHAIN_APPROX_NONE)
    gy, gx = np.gradient(d)
    paths = []
    for c in contours:
        if len(c) < 12:
            continue
        grid = (c[:, 0, :].astype(np.float64) + 0.5) / SS
        p = grid.copy()
        for _ in range(3):
            u, v = sample(gx, *p.T) * SS, sample(gy, *p.T) * SS
            step = np.clip(sample(d, *p.T) / np.maximum(u * u + v * v, 1e-9), -1, 1)
            p -= np.stack([step * u, step * v], 1)
        k = 3
        q = np.vstack([p[-k:], p, p[:k]])
        p = np.stack([np.convolve(q[:, i], np.ones(2 * k + 1) / (2 * k + 1), mode="valid") for i in (0, 1)], 1)
        edge = (grid < 1) | (grid > 1023)
        p = np.where(edge, np.where(grid < 512, -4.0, 1028.0), p)
        p = cv2.approxPolyDP(p.astype(np.float32).reshape(-1, 1, 2), 0.12, True).reshape(-1, 2)
        paths.append(p)
    return paths


def path_data(paths, scale=1.0):
    return " ".join("M" + " L".join(f"{x * scale:.2f},{y * scale:.2f}".replace(".00", "") for x, y in p) + "Z" for p in paths)


def color(hex_color):
    r, g, b = (int(hex_color[i:i + 2], 16) / 255 for i in (1, 3, 5))
    return f"srgb:{r:.5f},{g:.5f},{b:.5f},1.00000"


def write_icon(path, mark, platform):
    """An Icon Composer document: the cream or ink tile and the team on it, recoloured per appearance."""
    if os.path.exists(path):
        shutil.rmtree(path)
    os.makedirs(os.path.join(path, "Assets"))
    with open(os.path.join(path, "Assets", "team.svg"), "w") as f:
        f.write('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">'
                f'<path d="{mark}" fill="{INK}" fill-rule="evenodd"/></svg>\n')
    icon = {
        "fill-specializations": [
            {"value": {"solid": color(CREAM)}},
            {"appearance": "dark", "value": {"solid": color(INK)}},
        ],
        "groups": [{
            "name": "Team",
            "layers": [{
                "image-name": "team.svg",
                "name": "team",
                "glass": False,
                "fill-specializations": [
                    {"value": {"solid": color(INK)}},
                    {"appearance": "dark", "value": {"solid": color(CREAM)}},
                    {"appearance": "tinted", "value": {"solid": color(WHITE)}},
                ],
                "position": {"scale": 1, "translation-in-points": [0, 0]},
            }],
            "shadow": {"kind": "none", "opacity": 0},
            "translucency": {"enabled": False, "value": 0},
            "lighting": "combined",
            "specular": False,
        }],
        "supported-platforms": {"squares": [platform]},
    }
    with open(os.path.join(path, "icon.json"), "w") as f:
        f.write(json.dumps(icon, indent=2) + "\n")
    print("Wrote", os.path.normpath(path))


def render(icon, out, size, platform="macOS", margin=0):
    """Icon Composer's own rendering of the icon in light mode, with an empty margin round it."""
    subprocess.run([ICTOOL, icon, "--export-image", "--output-file", out, "--platform", platform,
                    "--rendition", "Default", "--width", str(size - 2 * margin), "--height", str(size - 2 * margin),
                    "--scale", "1"], check=True, capture_output=True)
    if margin:
        image = cv2.imread(out, cv2.IMREAD_UNCHANGED)
        cv2.imwrite(out, cv2.copyMakeBorder(image, margin, margin, margin, margin, cv2.BORDER_CONSTANT, value=(0, 0, 0, 0)))
    print("Wrote", os.path.normpath(out))


def write_web(web, paths, phone_icon):
    """The website's favicon (light or dark with the system, like the app's icon) and touch icon."""
    svg = os.path.join(web, "public", "code", "icon.svg")
    with open(svg, "w") as f:
        f.write('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">\n'
                f'  <style>.tile{{fill:{CREAM}}}.team{{fill:{INK}}}'
                f'@media (prefers-color-scheme: dark){{.tile{{fill:{INK}}}.team{{fill:{CREAM}}}}}</style>\n'
                '  <clipPath id="tile"><rect width="64" height="64" rx="14.5"/></clipPath>\n'
                '  <rect class="tile" width="64" height="64" rx="14.5"/>\n'
                f'  <path class="team" clip-path="url(#tile)" fill-rule="evenodd" d="{path_data(paths, 1 / 16)}"/>\n'
                '</svg>\n')
    print("Wrote", os.path.normpath(svg))
    render(phone_icon, os.path.join(web, "public", "code", "apple-touch-icon.png"), 180, platform="iOS")


if __name__ == "__main__":
    args = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    args.add_argument("--phone", help="the iPhone app's AppIcon.icon, to write its copy")
    args.add_argument("--web", help="the kuvo-web checkout, to write its icon.svg and apple-touch-icon.png")
    args = args.parse_args()
    paths = trace(team())
    mark = path_data(paths)
    write_icon(OUT, mark, "macOS")
    render(OUT, README_ICON, 256, margin=25)    # macOS's icon grid: the tile is 824 of 1024 points
    if args.phone:
        write_icon(args.phone, mark, "iOS")
    if args.web:
        with tempfile.TemporaryDirectory() as scratch:
            # the touch icon is drawn as the iPhone draws the icon
            phone = args.phone or os.path.join(scratch, "AppIcon.icon")
            if not args.phone:
                write_icon(phone, mark, "iOS")
            write_web(args.web, paths, phone)
