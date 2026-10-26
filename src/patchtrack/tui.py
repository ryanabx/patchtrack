"""An interactive browser: the list, a series' conversation, its version diff.

All behaviour is in Model, driven by key names and free of curses, so it
can be tested; draw() and run() are the thin curses layer.
"""

import curses
import urllib.parse
from dataclasses import dataclass, field

from . import pretty, thread, vdiff

HELP = """patchtrack: keys

  List                            Reading a series
  j / k, ↓ / ↑    move            j / k, ↓ / ↑     scroll
  PgDn / PgUp     page            space / b        page
  g / G           first / last    g / G            top / bottom
  Enter           read            q / ←            back to the list
  d               version diff    o                open on the archive
  o               open on the archive
  /               filter by title or state (Enter or Esc ends)
  f               open series only, or everything
  ?               this help
  q               quit
"""

OPEN = {"unfetched", "no-reply", "bot-only", "discussed", "commented", "reviewed", "partially-applied"}

# ANSI colour codes in pretty.STYLE -> curses colours
COLOURS = {"\033[35m": curses.COLOR_MAGENTA, "\033[32m": curses.COLOR_GREEN, "\033[33m": curses.COLOR_YELLOW,
           "\033[36m": curses.COLOR_CYAN, "\033[34m": curses.COLOR_BLUE, "\033[31m": curses.COLOR_RED,
           "\033[90m": curses.COLOR_WHITE}


@dataclass
class Model:
    families: list
    found: dict  # every series, by root
    messages: dict  # stored message dicts, by message-id
    lore_base: str
    height: int = 24
    view: str = "list"  # list, read, diff, help
    sel: int = 0
    top: int = 0
    filter: str = ""
    typing: bool = False  # entering a filter
    only_open: bool = False
    lines: list = field(default_factory=list)  # of the read/diff/help view
    status: str = ""

    @property
    def visible(self):
        f = self.filter.lower()
        return [fam for fam in self.families
                if (not self.only_open or fam.state in OPEN)
                and (not f or f in fam.head.title.lower() or f in fam.state)]

    @property
    def page(self):
        return max(self.height - 3, 1)

    def selected(self):
        v = self.visible
        return v[self.sel] if v else None

    def url(self):
        fam = self.selected()
        if fam is None:
            return None
        return f"{self.lore_base}/all/{urllib.parse.quote(fam.head.root, safe='@+-._~=')}/T/#u"

    def key(self, k):
        """Handle one key; returns "quit", ("open", url) or None."""
        self.status = ""
        if self.typing:
            return self._type(k)
        if self.view == "list":
            return self._list_key(k)
        return self._pager_key(k)

    def _type(self, k):
        if k in ("\n", "KEY_ENTER", "\x1b"):
            self.typing = False
        elif k in ("KEY_BACKSPACE", "\x7f", "\b"):
            self.filter = self.filter[:-1]
        elif len(k) == 1 and k.isprintable():
            self.filter += k
        self.sel = self.top = 0

    def _list_key(self, k):
        n = len(self.visible)
        if k == "q":
            return "quit"
        if k in ("j", "KEY_DOWN"):
            self.sel = min(self.sel + 1, max(n - 1, 0))
        elif k in ("k", "KEY_UP"):
            self.sel = max(self.sel - 1, 0)
        elif k == "KEY_NPAGE":
            self.sel = min(self.sel + self.page, max(n - 1, 0))
        elif k == "KEY_PPAGE":
            self.sel = max(self.sel - self.page, 0)
        elif k == "g":
            self.sel = 0
        elif k == "G":
            self.sel = max(n - 1, 0)
        elif k == "/":
            self.typing, self.filter = True, ""
        elif k == "f":
            self.only_open, self.sel, self.top = not self.only_open, 0, 0
        elif k == "?":
            self._show("help", HELP.splitlines())
        elif k in ("\n", "KEY_ENTER", "l", "KEY_RIGHT") and n:
            s = self.selected().head
            self._show("read", [f"{s.label()}  ·  {self.selected().state}  ·  {s.as_of_text}", ""]
                       + thread.render(s, self.messages))
        elif k == "d" and n:
            fam = self.selected()
            by_v = vdiff.versions(self.found, (fam.author, fam.key))
            if len(by_v) < 2:
                self.status = "only one version is known"
            else:
                vs = sorted(by_v)
                self._show("diff", vdiff.render(by_v[vs[-2]], by_v[vs[-1]]))
        elif k == "o" and n:
            return ("open", self.url())
        # keep the selection on screen
        if self.sel < self.top:
            self.top = self.sel
        elif self.sel >= self.top + self.page:
            self.top = self.sel - self.page + 1
        return None

    def _show(self, view, lines):
        self.view, self.lines, self.top_saved, self.top = view, lines, self.top, 0

    def _pager_key(self, k):
        last = max(len(self.lines) - self.page, 0)
        if k in ("q", "h", "KEY_LEFT", "\x1b"):
            self.view, self.top = "list", getattr(self, "top_saved", 0)
        elif k in ("j", "KEY_DOWN"):
            self.top = min(self.top + 1, last)
        elif k in ("k", "KEY_UP"):
            self.top = max(self.top - 1, 0)
        elif k in (" ", "KEY_NPAGE"):
            self.top = min(self.top + self.page, last)
        elif k in ("b", "KEY_PPAGE"):
            self.top = max(self.top - self.page, 0)
        elif k == "g":
            self.top = 0
        elif k == "G":
            self.top = last
        elif k == "o" and self.view != "help":
            return ("open", self.url())
        return None


def _attrs():
    """Colour pair numbers by ANSI code, after curses.start_color()."""
    pairs = {}
    if curses.has_colors():
        curses.use_default_colors()
        for n, (code, colour) in enumerate(COLOURS.items(), 1):
            curses.init_pair(n, colour, -1)
            pairs[code] = curses.color_pair(n)
    return pairs


def draw(win, m, pairs=None):
    """Draw the model on a curses-like window (addnstr, getmaxyx, erase, refresh)."""
    pairs = pairs or {}
    height, width = win.getmaxyx()
    m.height = height
    win.erase()

    def put(y, x, text, attr=0):
        if 0 <= y < height and x < width:
            win.addnstr(y, x, text, max(width - x - (1 if y == height - 1 else 0), 0), attr)

    if m.view == "list":
        vis = m.visible
        title = f"patchtrack · {len(vis)} series" + (" · open only" if m.only_open else "")
        if m.filter or m.typing:
            title += f" · filter: {m.filter}" + ("▏" if m.typing else "")
        put(0, 0, title, curses.A_BOLD)
        shown = vis[m.top:m.top + m.page]
        state_w = max([len(f.state) for f in shown] + [8])
        meta = lambda f: f"v{f.head.version} · {f.head.last_activity[:10]}"
        meta_w = max([len(meta(f)) for f in shown] + [0])
        flag_w = len("  unanswered") if any(f.head.unanswered for f in shown) else 0
        room = max(width - 1 - (4 + state_w) - (2 + meta_w) - flag_w, 10)
        for row, fam in enumerate(shown):
            y = row + 1
            shape, code = pretty.STYLE.get(fam.state, ("●", ""))
            attr = curses.A_REVERSE if m.top + row == m.sel else 0
            put(y, 0, " " * (width - 1), attr)
            put(y, 0, shape, attr | pairs.get(code, 0))
            put(y, 2, fam.state.ljust(state_w), attr | pairs.get(code, 0))
            title = fam.head.title if len(fam.head.title) <= room else fam.head.title[:room - 1] + "…"
            put(y, 4 + state_w, title.ljust(room) + "  " + meta(fam).rjust(meta_w), attr)
            if fam.head.unanswered:
                put(y, 4 + state_w + room + 2 + meta_w, "  unanswered", attr | pairs.get("\033[33m", 0))
        hints = "Enter read · d diff · o open · / filter · f open only · ? help · q quit"
    else:
        for row, line in enumerate(m.lines[m.top:m.top + m.page + 1]):
            attr = curses.A_DIM if line.lstrip().startswith((">", "[…", "[diff")) else 0
            put(row, 0, line.expandtabs(8), attr)
        hints = "j/k scroll · space/b page · o open · q back"
    put(height - 1, 0, (m.status or hints)[:width - 1], curses.A_DIM)
    win.refresh()


def run(model, open_url):
    def main(stdscr):
        curses.curs_set(0)
        pairs = _attrs()
        while True:
            draw(stdscr, model, pairs)
            try:
                k = stdscr.getkey()
            except curses.error:
                continue
            action = model.key(k)
            if action == "quit":
                return
            if isinstance(action, tuple) and action[1]:
                open_url(action[1])
                model.status = f"opened {action[1]}"
    curses.wrapper(main)
