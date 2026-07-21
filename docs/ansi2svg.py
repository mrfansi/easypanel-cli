#!/usr/bin/env python3
"""Turn a `tmux capture-pane -e -p` dump into a self-contained colored SVG.

Why this exists: this is a terminal TUI, so a real "screenshot" is the terminal's
own output. GitHub does not render ANSI colour in code fences, but it DOES render
SVG images — so the READMEs screenshots are the actual bytes the running binary
drew, converted here to SVG rather than mocked up by hand.

Usage:
    tmux capture-pane -t <session> -e -p | python3 docs/ansi2svg.py "Title" > out.svg

Handles the SGR subset ratatui emits: 256-colour fg/bg (38;5;N / 48;5;N), the
16 base colours, bold, and the resets. No dependencies.
"""
import sys
import html

CELL_W = 8.4
CELL_H = 18.0
FONT = 14
PAD = 16
TITLE_H = 34

BG_DEFAULT = "#0d1117"
FG_DEFAULT = "#c9d1d9"

BASE16 = [
    "#0d1117", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc",
    "#11a8cd", "#cccccc", "#767676", "#f14c4c", "#23d18b", "#f5f543",
    "#3b8eea", "#d670d6", "#29b8db", "#ffffff",
]


def xterm(n):
    """RGB hex for an xterm-256 colour index."""
    if n < 16:
        return BASE16[n]
    if n < 232:
        n -= 16
        levels = [0, 95, 135, 175, 215, 255]
        r, g, b = levels[n // 36], levels[(n // 6) % 6], levels[n % 6]
        return f"#{r:02x}{g:02x}{b:02x}"
    v = 8 + 10 * (n - 232)
    return f"#{v:02x}{v:02x}{v:02x}"


class Style:
    __slots__ = ("fg", "bg", "bold")

    def __init__(self):
        self.fg = FG_DEFAULT
        self.bg = None
        self.bold = False

    def copy(self):
        s = Style()
        s.fg, s.bg, s.bold = self.fg, self.bg, self.bold
        return s


def apply_sgr(style, params):
    i = 0
    while i < len(params):
        p = params[i]
        if p in (0, None):
            style.fg, style.bg, style.bold = FG_DEFAULT, None, False
        elif p == 1:
            style.bold = True
        elif p == 22:
            style.bold = False
        elif p == 39:
            style.fg = FG_DEFAULT
        elif p == 49:
            style.bg = None
        elif 30 <= p <= 37:
            style.fg = BASE16[p - 30]
        elif 90 <= p <= 97:
            style.fg = BASE16[p - 90 + 8]
        elif 40 <= p <= 47:
            style.bg = BASE16[p - 40]
        elif 100 <= p <= 107:
            style.bg = BASE16[p - 100 + 8]
        elif p == 38 and i + 2 < len(params) and params[i + 1] == 5:
            style.fg = xterm(params[i + 2])
            i += 2
        elif p == 48 and i + 2 < len(params) and params[i + 1] == 5:
            style.bg = xterm(params[i + 2])
            i += 2
        i += 1


def parse_line(line):
    """Yield (text, Style) runs for one ANSI line."""
    runs = []
    style = Style()
    buf = []
    i = 0
    while i < len(line):
        c = line[i]
        if c == "\x1b" and i + 1 < len(line) and line[i + 1] == "[":
            j = line.index("m", i) if "m" in line[i:] else len(line)
            if buf:
                runs.append(("".join(buf), style.copy()))
                buf = []
            seq = line[i + 2:j]
            params = [int(x) if x else 0 for x in seq.split(";")] if seq else [0]
            apply_sgr(style, params)
            i = j + 1
        else:
            buf.append(c)
            i += 1
    if buf:
        runs.append(("".join(buf), style.copy()))
    return runs


def main():
    title = sys.argv[1] if len(sys.argv) > 1 else ""
    raw = sys.stdin.read().rstrip("\n")
    lines = raw.split("\n")
    cols = max((len(strip_ansi(l)) for l in lines), default=80)
    rows = len(lines)

    width = int(cols * CELL_W + 2 * PAD)
    height = int(rows * CELL_H + 2 * PAD + (TITLE_H if title else 0))
    y0 = PAD + (TITLE_H if title else 0)

    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}" font-family="ui-monospace,SFMono-Regular,Menlo,Consolas,monospace" font-size="{FONT}">',
        f'<rect width="{width}" height="{height}" rx="8" fill="{BG_DEFAULT}"/>',
    ]
    if title:
        out.append(
            f'<text x="{PAD}" y="{PAD + 16}" fill="#8b949e" font-size="13" '
            f'font-weight="600">{html.escape(title)}</text>'
        )
    for r, line in enumerate(lines):
        y = y0 + r * CELL_H
        col = 0
        for text, st in parse_line(line):
            if not text:
                continue
            w = len(text)
            x = PAD + col * CELL_W
            if st.bg:
                out.append(
                    f'<rect x="{x:.1f}" y="{y:.1f}" width="{w * CELL_W:.1f}" '
                    f'height="{CELL_H:.1f}" fill="{st.bg}"/>'
                )
            weight = ' font-weight="700"' if st.bold else ""
            out.append(
                f'<text x="{x:.1f}" y="{y + 13:.1f}" fill="{st.fg}"{weight} '
                f'xml:space="preserve">{html.escape(text)}</text>'
            )
            col += w
    out.append("</svg>")
    sys.stdout.write("\n".join(out) + "\n")


def strip_ansi(line):
    out = []
    i = 0
    while i < len(line):
        if line[i] == "\x1b" and i + 1 < len(line) and line[i + 1] == "[":
            j = line.index("m", i) if "m" in line[i:] else len(line)
            i = j + 1
        else:
            out.append(line[i])
            i += 1
    return "".join(out)


if __name__ == "__main__":
    main()
