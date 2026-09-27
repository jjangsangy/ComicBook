#!/usr/bin/env python3
"""Turn a macOS `sample` textual call graph into an SVG flame chart.

Usage: flame.py <sample.txt> <out.svg> [title]
"""
import html
import re
import subprocess
import sys
from collections import OrderedDict


def demangle(names):
    """Batch-demangle Rust/C++ symbols with the system c++filt."""
    unique = list(dict.fromkeys(names))
    try:
        out = subprocess.run(
            ["c++filt"], input="\n".join(unique), capture_output=True, text=True,
            check=True,
        ).stdout.splitlines()
    except Exception:
        return {n: n for n in unique}
    if len(out) != len(unique):
        return {n: n for n in unique}
    return dict(zip(unique, out))


def clean_symbol(rest):
    # Strip the trailing " (in module) + offset  [0xaddr]" suffix.
    rest = re.sub(r"\s*\(in [^)]*\).*$", "", rest)
    rest = re.sub(r"\s*\+\s*\d+\s*$", "", rest)
    rest = re.sub(r"\s*\[0x[0-9a-fA-F]+\]\s*$", "", rest)
    return rest.strip()


def parse(path):
    lines = open(path, encoding="utf-8", errors="replace").read().splitlines()
    start = next(i for i, l in enumerate(lines) if l.strip() == "Call graph:")
    nodes = []  # (depth, count, name)
    for line in lines[start + 1:]:
        if not line.strip():
            continue
        if line.strip().startswith(("Total number", "Binary Images",
                                    "Sort by top")):
            break
        m = re.match(r"^(.*?)(\d+)\s+(.*)$", line)
        if not m:
            continue
        prefix, count, rest = m.group(1), int(m.group(2)), m.group(3)
        # Each depth level is two columns; the base indent is four spaces.
        depth = max(0, (len(prefix) - 4) // 2)
        nodes.append((depth, count, clean_symbol(rest)))
    return nodes


def build_tree(nodes):
    demangled = demangle([n[2] for n in nodes])
    root = {"name": "all", "self": 0, "children": OrderedDict(), "count": 0}
    stack = [(-1, root)]
    for depth, count, name in nodes:
        name = demangled.get(name, name)
        while stack and stack[-1][0] >= depth:
            stack.pop()
        parent = stack[-1][1]
        if name not in parent["children"]:
            parent["children"][name] = {
                "name": name, "self": 0, "children": OrderedDict(), "count": 0,
            }
        node = parent["children"][name]
        node["count"] += count
        stack.append((depth, node))
    # Self time = inclusive minus children inclusive.
    def compute(node):
        child_sum = 0
        for child in node["children"].values():
            child_sum += compute(child)
        node["sub"] = node["count"] if node["count"] >= child_sum else child_sum
        node["self"] = node["sub"] - child_sum
        return node["sub"]
    compute(root)
    return root


def layout(node, x, depth, pos, max_depth):
    width = node["sub"]
    if width <= 0:
        return x
    pos.append((node, x, width, depth))
    max_depth[0] = max(max_depth[0], depth)
    children = sorted(node["children"].values(), key=lambda c: -c["sub"])
    cx = x
    for child in children:
        if child["sub"] <= 0:
            continue
        layout(child, cx, depth + 1, pos, max_depth)
        cx += child["sub"]
    return cx


def colour(name):
    h = 0
    for ch in name:
        h = (h * 31 + ord(ch)) & 0xFFFFFFFF
    hue = h % 360
    return f"hsl({hue},65%,58%)"


def render(root, out, title):
    pos = []
    max_depth = [0]
    layout(root, 0, 0, pos, max_depth)
    total = max(1, root["sub"])
    scale = 1500.0 / total
    row_h = 16
    height = (max_depth[0] + 2) * row_h
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="1600" '
        f'height="{height}" font-family="monospace" font-size="11">',
        f'<rect width="1600" height="{height}" fill="#1a1a1a"/>',
        f'<text x="6" y="{height - 4}" fill="#eee">{html.escape(title)} '
        f'({total} samples)</text>',
    ]
    for node, x, width, depth in pos:
        px = x * scale
        pw = width * scale
        if pw < 1:
            continue
        y = depth * row_h
        fill = colour(node["name"])
        pct = width * 100.0 / total
        tip = f'{node["name"]}  ({width} samples, {pct:.1f}%)'
        parts.append(
            f'<g><rect x="{px:.1f}" y="{y}" width="{pw:.1f}" '
            f'height="{row_h - 1}" fill="{fill}" fill-opacity="0.9">'
            f'<title>{html.escape(tip)}</title></rect>'
        )
        if pw > 40:
            label = node["name"]
            if len(label) * 6.2 > pw - 4:
                label = label[: max(1, int((pw - 4) / 6.2))]
            parts.append(
                f'<text x="{px + 3:.1f}" y="{y + 12}" fill="#000">'
                f'{html.escape(label)}</text>'
            )
        parts.append("</g>")
    parts.append("</svg>")
    open(out, "w", encoding="utf-8").write("\n".join(parts))
    print(f"wrote {out} ({len(pos)} frames, {total} samples)")


def main():
    src, out = sys.argv[1], sys.argv[2]
    title = sys.argv[3] if len(sys.argv) > 3 else src
    nodes = parse(src)
    root = build_tree(nodes)
    render(root, out, title)


if __name__ == "__main__":
    main()
