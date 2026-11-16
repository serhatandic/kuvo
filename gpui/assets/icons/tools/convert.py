#!/usr/bin/env python3
"""Convert lucide-static SVGs to Kuvo's 16-unit grid (Python standard library only).

Usage: python3 convert.py /path/to/unpacked/package
Download/unpack lucide-static separately into a new empty directory; never run its code.
The source is read only. Coordinates map as x/2+2, y/2+2; radii/lengths halve.
"""
import json
from pathlib import Path
import re
import sys
import xml.etree.ElementTree as ET

HERE = Path(__file__).resolve().parent
MAP = {name: name for name in '''archive arrow-down arrow-left arrow-right arrow-up bell bot check chevron-down chevron-left chevron-right chevron-up clock copy eye eye-off file folder folder-open globe hand image info link mic minus moon panel-right paperclip pencil pin plus search settings shield sun terminal trash undo user'''.split()}
MAP.update(dict(bolt='zap', branch='git-branch', close='x', compose='square-pen', computer='monitor', drive='hard-drive', external='external-link', message='message-square', more='ellipsis', question='circle-help', queue='list-todo', **{'return': 'corner-down-left'}, sidebar='panel-left', sliders='sliders-horizontal', sparkle='sparkles', spinner='loader-circle', stop='square', warning='triangle-alert'))

def fmt(n):
    return f'{n:.6f}'.rstrip('0').rstrip('.') if n else '0'

def path(d):
    result, cmd, i, first = [], None, 0, True
    sizes = dict(M=2, L=2, H=1, V=1, C=6, S=4, Q=4, T=2, A=7)
    number = re.compile(r'[-+]?(?:\d*\.\d+|\d+\.?\d*)(?:[eE][-+]?\d+)?')
    while i < len(d):
        while i < len(d) and d[i] in ' ,\t\r\n':
            i += 1
        if i == len(d):
            break
        if d[i].isalpha():
            cmd = d[i]
            result.append(cmd)
            i += 1
            if cmd.upper() == 'Z':
                continue
        values = []
        for j in range(sizes[cmd.upper()]):
            while i < len(d) and d[i] in ' ,\t\r\n':
                i += 1
            if cmd.upper() == 'A' and j in (3, 4):
                assert d[i] in '01', d[i:]
                value = float(d[i])
                i += 1
            else:
                match = number.match(d, i)
                if not match:
                    raise ValueError(d[i:])
                value = float(match.group())
                i = match.end()
            offset = 2 if cmd.isupper() or (first and cmd == 'm') else 0
            if cmd.upper() == 'A':
                value = value / 2 if j in (0, 1) else value if j in (2, 3, 4) else value / 2 + offset
            else:
                value = value / 2 + offset
            values.append(value)
        result.extend(fmt(v) for v in values)
        first = False
        if cmd == 'M':
            cmd = 'L'
        elif cmd == 'm':
            cmd = 'l'
    return ' '.join(result)

def convert(source, *, filled=False):
    root = ET.parse(source).getroot()
    children = []
    for element in root:
        tag = element.tag.split('}')[-1]
        a = element.attrib
        if tag == 'path':
            children.append(f'<path d="{path(a["d"])}"/>')
        elif tag in ('circle', 'ellipse', 'rect', 'line'):
            attrs = []
            for key, value in a.items():
                assert key in ('x', 'y', 'cx', 'cy', 'x1', 'x2', 'y1', 'y2', 'r', 'rx', 'ry', 'width', 'height'), key
                n = float(value) / 2 + (2 if key in ('x', 'y', 'cx', 'cy', 'x1', 'x2', 'y1', 'y2') else 0)
                attrs.append(f'{key}="{fmt(n)}"')
            children.append(f'<{tag} {" ".join(attrs)}/>')
        elif tag in ('polyline', 'polygon'):
            points = re.findall(r'[-+]?(?:\d*\.\d+|\d+\.?\d*)', a['points'])
            children.append(f'<{tag} points="{" ".join(fmt(float(n)/2+2) for n in points)}"/>')
        else:
            raise ValueError(f'Unsupported SVG element: {tag}')
    fill = "#000" if filled else "none"
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="{fill}" stroke="#000" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round">\n  ' + '\n  '.join(children) + '\n</svg>\n'

def main():
    package = Path(sys.argv[1]).resolve()
    for ours, lucide in sorted(MAP.items()):
        (HERE.parent / f'{ours}.svg').write_text(convert(package / 'icons' / f'{lucide}.svg', filled=ours == 'stop'))
    (HERE.parent / 'LICENSES').write_bytes((package / 'LICENSE').read_bytes())
    version = json.loads((package / 'package.json').read_text())['version']
    (HERE.parent / 'MAPPING.md').write_text(f'# Icon sources\n\nLucide-static {version}, converted with `tools/convert.py`. No Zed SVGs used.\nAll icons use Lucide; none were kept/redrawn. SidebarLeft aliases sidebar.svg. Stop is the filled variant of square, preserving Lucide rounded corners.\n\n| Ours | Lucide |\n| --- | --- |\n' + ''.join(f'| {ours} | {lucide}{" (filled)" if ours == "stop" else ""} |\n' for ours, lucide in sorted(MAP.items())) + '| sidebar-left (alias of sidebar) | panel-left |\n')

if __name__ == '__main__':
    main()
