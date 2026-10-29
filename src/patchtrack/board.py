"""A static HTML board: one card per family, in a column per stage."""

import html

from . import __version__, api, lore

COLUMNS = (
    ("Waiting", ("unfetched", "no-reply", "bot-only")),
    ("In review", ("discussed", "commented", "reviewed")),
    ("Applied", ("partially-applied", "applied", "queued", "in-next")),
    ("Merged", ("merged",)),
)

CSS = """
:root { color-scheme: light dark; --bg: #f6f7f9; --card: #fff; --fg: #1d2129; --muted: #5c6370;
  --line: #d9dde3; --accent: #2457c5; --warn: #9a5b00; }
@media (prefers-color-scheme: dark) { :root { --bg: #15171c; --card: #1e2128; --fg: #e3e6eb;
  --muted: #9aa1ad; --line: #333844; --accent: #7aa2f7; --warn: #e0a84e; } }
body { margin: 0; padding: 1.5rem; background: var(--bg); color: var(--fg);
  font: 14px/1.45 system-ui, sans-serif; }
h1 { font-size: 1.25rem; margin: 0 0 .25rem; }
header p { margin: 0 0 1.25rem; color: var(--muted); }
.board { display: grid; grid-template-columns: repeat(4, minmax(16rem, 1fr)); gap: 1rem; align-items: start; }
.column h2 { font-size: .85rem; text-transform: uppercase; letter-spacing: .05em; color: var(--muted);
  margin: 0 0 .5rem; }
.card { background: var(--card); border: 1px solid var(--line); border-radius: 6px; padding: .6rem .75rem;
  margin-bottom: .6rem; }
.card a { color: var(--accent); text-decoration: none; font-weight: 600; }
.card a:hover { text-decoration: underline; }
.card a.mail { font-weight: 400; font-size: .75rem; }
.meta { color: var(--muted); font-size: .8rem; margin-top: .25rem; }
.state { display: inline-block; border: 1px solid var(--line); border-radius: 999px; padding: 0 .45rem;
  font-size: .75rem; margin-right: .35rem; }
.flag { color: var(--warn); font-weight: 600; }
.evidence { font-size: .8rem; margin-top: .35rem; overflow-wrap: anywhere; }
@media (max-width: 70rem) { .board { grid-template-columns: 1fr 1fr; } }
"""


def _url(base, msgid):
    return html.escape(lore.message_url(base, msgid))


def _card(fam, base):
    s, e = fam.head, html.escape
    versions = f"{len(fam.versions)} versions" if len(fam.versions) > 1 else "1 version"
    flag = ' <span class="flag">unanswered</span>' if s.unanswered else ""
    as_of = s.as_of_text
    return (f'<div class="card"><a href="{_url(base, s.root)}">{e(s.label())}</a>'
            f'<div class="meta"><span class="state">{e(fam.state)}</span>{flag} '
            f'last activity {e(s.last_activity[:10])} · {versions} · {e(as_of)}</div>'
            + (f'<div class="evidence">{_evidence(fam.evidence, base)}</div>' if fam.evidence else "")
            + "</div>")


def _evidence(text, base):
    out, last = [], 0
    for m in api.EVIDENCE_MSGID_RE.finditer(text):
        out.append(html.escape(text[last:m.start()]))
        out.append(f' <a class="mail" href="{_url(base, m.group(1))}">mail</a>')
        last = m.end()
    return "".join(out) + html.escape(text[last:])


def render(families, title, generated, base=lore.DEFAULT_BASE):
    cols = []
    for name, states in COLUMNS:
        cards = [_card(f, base) for f in families if f.state in states]
        cols.append(f'<section class="column"><h2>{html.escape(name)} ({len(cards)})</h2>'
                    + "".join(cards) + "</section>")
    return (f'<!doctype html><html lang="en"><head><meta charset="utf-8">'
            f'<meta name="viewport" content="width=device-width, initial-scale=1">'
            f"<title>{html.escape(title)}</title><style>{CSS}</style></head><body>"
            f"<header><h1>{html.escape(title)}</h1><p>Generated {html.escape(generated)} UTC by "
            f"patchtrack {__version__}. Each state is as of its thread's last fetch.</p></header>"
            f'<main class="board">{"".join(cols)}</main></body></html>\n')
