#!/usr/bin/env python3
"""Generates Resources/AppIcon.icon, the app icon in Icon Composer's format.

The icon is a box seen corner-on: a teal hexagon, the outline of a cube, with a
white dot inside where it holds something, on a dark teal background. Each is
its own layer, so macOS 26 and later render them as Liquid Glass and adapt them
to dark, clear and tinted icon styles.
After changing anything here, run scripts/build-icon.sh.
"""
import json
import os

OUT = os.path.join(os.path.dirname(__file__), "..", "Resources", "AppIcon.icon")

# Coordinates are on Icon Composer's 1024-point canvas.
WIDTH = 104                                  # stroke width of the outline
DOT = 150                                    # diameter of the dot
OUTLINE = ((512, 252), (737, 382), (737, 642), (512, 772), (287, 642), (287, 382))
CENTER = (512, 512)
WHITE, TEAL = "#FFFFFF", "#7FEBDD"
BACKGROUND = ("0.075,0.115,0.140", "0.018,0.032,0.042")  # top and bottom, Display P3


def svg(body):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">{body}</svg>\n'


def outline(points, color):
    path = "M" + " L".join(f"{x},{y}" for x, y in points) + " Z"
    return (f'<path d="{path}" fill="none" stroke="{color}" stroke-width="{WIDTH}" '
            f'stroke-linecap="round" stroke-linejoin="round"/>')


def dot(center, color):
    return f'<circle cx="{center[0]}" cy="{center[1]}" r="{DOT / 2}" fill="{color}"/>'


def group(name, translucency):
    return {
        "name": name.capitalize(),
        "layers": [{
            "image-name": f"{name}.svg",
            "name": name,
            "glass": True,
            "position": {"scale": 1, "translation-in-points": [0, 0]},
        }],
        "shadow": {"kind": "neutral", "opacity": 0.5},
        "translucency": {"enabled": translucency > 0, "value": translucency},
        "lighting": "individual",
        "specular": True,
    }


os.makedirs(os.path.join(OUT, "Assets"), exist_ok=True)
for name, body in (("dot", dot(CENTER, WHITE)), ("outline", outline(OUTLINE, TEAL))):
    with open(os.path.join(OUT, "Assets", f"{name}.svg"), "w") as f:
        f.write(svg(body))

icon = {
    "fill": {
        "linear-gradient": [f"display-p3:{BACKGROUND[0]},1.00000", f"display-p3:{BACKGROUND[1]},1.00000"],
        "orientation": {"start": {"x": 0.5, "y": 0}, "stop": {"x": 0.5, "y": 1}},
    },
    "groups": [group("dot", 0.0), group("outline", 0.35)],  # front first
    "supported-platforms": {"squares": ["macOS"]},
}
with open(os.path.join(OUT, "icon.json"), "w") as f:
    f.write(json.dumps(icon, indent=2) + "\n")
print("Wrote", os.path.normpath(OUT))
