//! An interactive browser: the list, a series' conversation, its version diff.
//!
//! All behaviour is in [`Model`], driven by key names and free of any
//! terminal, so it can be tested; [`draw`] renders it with ratatui onto any
//! backend, and [`run`] is the thin crossterm loop.

use std::collections::HashMap;

use patchtrack_core::series::{Family, State};
use patchtrack_core::{Message, sync, vdiff};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::{pretty, read};

pub const HELP: &str = "patchtrack: keys

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
  q               quit";

/// States still waiting on someone.
const OPEN: [State; 7] = [
    State::Unfetched,
    State::NoReply,
    State::BotOnly,
    State::Discussed,
    State::Commented,
    State::Reviewed,
    State::PartiallyApplied,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    List,
    Read,
    Diff,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    Open(String),
}

pub struct Model {
    pub families: Vec<Family>,
    messages: Vec<Message>,
    archive: String,
    /// Rows on the screen, set by `draw`.
    pub height: usize,
    pub view: View,
    pub sel: usize,
    pub top: usize,
    list_top: usize,
    pub filter: String,
    /// Typing a filter.
    pub typing: bool,
    pub only_open: bool,
    /// Of the read, diff or help view.
    pub lines: Vec<String>,
    pub status: String,
}

impl Model {
    pub fn new(families: Vec<Family>, messages: Vec<Message>, archive: &str) -> Model {
        Model {
            families,
            messages,
            archive: archive.to_string(),
            height: 24,
            view: View::List,
            sel: 0,
            top: 0,
            list_top: 0,
            filter: String::new(),
            typing: false,
            only_open: false,
            lines: vec![],
            status: String::new(),
        }
    }

    pub fn visible(&self) -> Vec<&Family> {
        let f = self.filter.to_lowercase();
        self.families
            .iter()
            .filter(|fam| !self.only_open || OPEN.contains(&fam.state))
            .filter(|fam| {
                f.is_empty() || fam.head().title.to_lowercase().contains(&f) || fam.state.as_str().contains(&f)
            })
            .collect()
    }

    fn page(&self) -> usize {
        self.height.saturating_sub(3).max(1)
    }

    fn selected(&self) -> Option<&Family> {
        self.visible().get(self.sel).copied()
    }

    fn url(&self) -> Option<String> {
        self.selected().map(|f| format!("{}/all/{}/T/#u", self.archive, sync::path_quote(&f.head().root)))
    }

    /// Handle one key: a character, or Enter, Esc, Backspace, Up, Down,
    /// Left, Right, PageUp, PageDown.
    pub fn key(&mut self, k: &str) -> Option<Action> {
        self.status.clear();
        if self.typing {
            match k {
                "Enter" | "Esc" => self.typing = false,
                "Backspace" => {
                    self.filter.pop();
                }
                _ if k.chars().count() == 1 && !k.chars().all(char::is_control) => self.filter.push_str(k),
                _ => {}
            }
            (self.sel, self.top) = (0, 0);
            return None;
        }
        if self.view == View::List { self.list_key(k) } else { self.pager_key(k) }
    }

    fn list_key(&mut self, k: &str) -> Option<Action> {
        let n = self.visible().len();
        let last = n.saturating_sub(1);
        match k {
            "q" => return Some(Action::Quit),
            "j" | "Down" => self.sel = (self.sel + 1).min(last),
            "k" | "Up" => self.sel = self.sel.saturating_sub(1),
            "PageDown" => self.sel = (self.sel + self.page()).min(last),
            "PageUp" => self.sel = self.sel.saturating_sub(self.page()),
            "g" => self.sel = 0,
            "G" => self.sel = last,
            "/" => (self.typing, self.filter) = (true, String::new()),
            "f" => (self.only_open, self.sel, self.top) = (!self.only_open, 0, 0),
            "?" => self.show(View::Help, HELP.lines().map(str::to_string).collect()),
            "Enter" | "l" | "Right" if n > 0 => {
                let lines = self.conversation();
                self.show(View::Read, lines);
            }
            "d" if n > 0 => match self.version_diff() {
                Some(lines) => self.show(View::Diff, lines),
                None => self.status = "only one version is known".into(),
            },
            "o" if n > 0 => return self.url().map(Action::Open),
            _ => {}
        }
        // Keep the selection on screen.
        if self.view == View::List {
            if self.sel < self.top {
                self.top = self.sel;
            } else if self.sel >= self.top + self.page() {
                self.top = self.sel + 1 - self.page();
            }
        }
        None
    }

    fn conversation(&self) -> Vec<String> {
        let Some(f) = self.selected() else { return vec![] };
        let s = f.head();
        let by_id: HashMap<&str, &Message> = self.messages.iter().map(|m| (m.msgid.as_str(), m)).collect();
        let mut lines = vec![format!("{}  ·  {}  ·  {}", s.label(), f.state, s.as_of_text()), String::new()];
        lines.extend(read::conversation(s, &by_id, false, false));
        lines
    }

    fn version_diff(&self) -> Option<Vec<String>> {
        let by_v = vdiff::versions(self.selected()?);
        let mut latest = by_v.values().rev();
        let (new, old) = (latest.next()?, latest.next()?);
        Some(read::version_diff(old, new, false))
    }

    fn show(&mut self, view: View, lines: Vec<String>) {
        (self.view, self.lines, self.list_top, self.top) = (view, lines, self.top, 0);
    }

    fn pager_key(&mut self, k: &str) -> Option<Action> {
        let last = self.lines.len().saturating_sub(self.page());
        match k {
            "q" | "h" | "Left" | "Esc" => (self.view, self.top) = (View::List, self.list_top),
            "j" | "Down" => self.top = (self.top + 1).min(last),
            "k" | "Up" => self.top = self.top.saturating_sub(1),
            " " | "PageDown" => self.top = (self.top + self.page()).min(last),
            "b" | "PageUp" => self.top = self.top.saturating_sub(self.page()),
            "g" => self.top = 0,
            "G" => self.top = last,
            "o" if self.view != View::Help => return self.url().map(Action::Open),
            _ => {}
        }
        None
    }
}

fn colour(ansi: &str) -> Option<Color> {
    Some(match ansi {
        "\x1b[35m" => Color::Magenta,
        "\x1b[32m" => Color::Green,
        "\x1b[33m" => Color::Yellow,
        "\x1b[36m" => Color::Cyan,
        "\x1b[34m" => Color::Blue,
        "\x1b[31m" => Color::Red,
        "\x1b[90m" => Color::DarkGray,
        _ => return None,
    })
}

fn fit(text: &str, w: usize) -> String {
    let n = text.chars().count();
    if n <= w {
        format!("{text}{}", " ".repeat(w - n))
    } else {
        text.chars().take(w.saturating_sub(1)).collect::<String>() + "…"
    }
}

/// Draw the model on a frame; it learns the screen's height from it.
pub fn draw(frame: &mut Frame, m: &mut Model) {
    let area = frame.area();
    let (width, height) = (usize::from(area.width), usize::from(area.height));
    m.height = height;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines: Vec<Line> = vec![];
    let hints = if m.view == View::List {
        let vis = m.visible();
        let mut title = format!("patchtrack · {} series{}", vis.len(), if m.only_open { " · open only" } else { "" });
        if !m.filter.is_empty() || m.typing {
            title += &format!(" · filter: {}{}", m.filter, if m.typing { "▏" } else { "" });
        }
        lines.push(Line::styled(title, Style::default().add_modifier(Modifier::BOLD)));
        let shown: Vec<&Family> = vis.iter().skip(m.top).take(m.page()).copied().collect();
        let state_w = shown.iter().map(|f| f.state.as_str().len()).chain([8]).max().unwrap_or(8);
        let meta = |f: &Family| format!("v{} · {}", f.head().version, f.head().last_activity().get(..10).unwrap_or(""));
        let meta_w = shown.iter().map(|f| meta(f).chars().count()).max().unwrap_or(0);
        let flag_w = if shown.iter().any(|f| f.head().unanswered) { "  unanswered".len() } else { 0 };
        let room = width.saturating_sub(1 + 4 + state_w + 2 + meta_w + flag_w).max(10);
        for (row, f) in shown.iter().enumerate() {
            let (shape, ansi) = pretty::style(f.state);
            let mut state_style = Style::default();
            if let Some(c) = colour(ansi) {
                state_style = state_style.fg(c);
            }
            let base =
                if m.top + row == m.sel { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
            let mut spans = vec![
                Span::styled(format!("{shape} "), base.patch(state_style)),
                Span::styled(format!("{:<state_w$}  ", f.state.as_str()), base.patch(state_style)),
                Span::styled(format!("{}  {:>meta_w$}", fit(&f.head().title, room), meta(f)), base),
            ];
            if f.head().unanswered {
                spans.push(Span::styled("  unanswered", base.fg(Color::Yellow)));
            }
            lines.push(Line::from(spans));
        }
        "Enter read · d diff · o open · / filter · f open only · ? help · q quit"
    } else {
        for line in m.lines.iter().skip(m.top).take(m.page() + 1) {
            let quoted =
                ["> ", "[…", "[diff"].iter().any(|p| line.trim_start().starts_with(p)) || line.trim_start() == ">";
            let text = line.replace('\t', "        ");
            lines.push(if quoted { Line::styled(text, dim) } else { Line::raw(text) });
        }
        "j/k scroll · space/b page · o open · q back"
    };
    frame.render_widget(Paragraph::new(lines), area);
    if height > 0 {
        let status = if m.status.is_empty() { hints.to_string() } else { m.status.clone() };
        let last = ratatui::layout::Rect { y: area.y + area.height - 1, height: 1, ..area };
        frame.render_widget(Paragraph::new(Line::styled(fit(&status, width.saturating_sub(1)), dim)), last);
    }
}

/// A key as `Model::key` names it.
fn key_name(k: ratatui::crossterm::event::KeyEvent) -> Option<String> {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    Some(match k.code {
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => "q".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::PageUp => "PageUp".into(),
        KeyCode::PageDown => "PageDown".into(),
        _ => return None,
    })
}

/// Run the browser until it is quit; `open` opens a URL.
pub fn run(mut model: Model, open: impl Fn(&str) -> bool) -> std::io::Result<()> {
    use ratatui::crossterm::event::{self, Event, KeyEventKind};
    let mut terminal = ratatui::init();
    let result = (|| loop {
        terminal.draw(|f| draw(f, &mut model))?;
        // Windows reports releases too; act on presses only.
        let Event::Key(k) = event::read()? else { continue };
        if k.kind != KeyEventKind::Press {
            continue;
        }
        let Some(name) = key_name(k) else { continue };
        match model.key(&name) {
            Some(Action::Quit) => return Ok(()),
            Some(Action::Open(url)) => {
                model.status = if open(&url) { format!("opened {url}") } else { url };
            }
            None => {}
        }
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use patchtrack_core::rules::Rules;
    use patchtrack_core::series::{self, Facts};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn mail(id: &str, subject: &str, from: &str, irt: Option<&str>, date: &str, body: &str) -> Message {
        let irt = irt.map(|r| format!("In-Reply-To: <{r}>\nReferences: <{r}>\n")).unwrap_or_default();
        let raw = format!("Message-ID: <{id}>\nSubject: {subject}\nFrom: {from}\n{irt}Date: {date}\n\n{body}");
        Message::parse(raw.as_bytes()).unwrap()
    }

    fn posting(tag: &str, v: u32, title: &str, date: &str) -> Vec<Message> {
        let pre = if v > 1 { format!(" v{v}") } else { String::new() };
        let root = format!("{tag}-v{v}-0@x");
        vec![
            mail(&root, &format!("[PATCH{pre} 0/1] {title}"), "Dev <dev@x>", None, date, "Cover.\n"),
            mail(
                &format!("{tag}-v{v}-1@x"),
                &format!("[PATCH{pre} 1/1] x: part 1"),
                "Dev <dev@x>",
                Some(&root),
                date,
                "One.\n",
            ),
        ]
    }

    fn model() -> Model {
        let mut msgs = posting("ta", 1, "foo: do the thing", "Mon, 05 Oct 2026 10:00:00 +0000");
        msgs.extend(posting("ta", 2, "foo: do the thing", "Mon, 12 Oct 2026 10:00:00 +0000"));
        // v10: a wider column.
        msgs.extend(posting("tb", 10, "other: a second series", "Mon, 05 Oct 2026 11:00:00 +0000"));
        msgs.push(mail(
            "q@x",
            "Re: [PATCH v10 0/1] other",
            "R <r@x>",
            Some("tb-v10-0@x"),
            "Tue, 13 Oct 2026 10:00:00 +0000",
            "> +x\nWhy?\n",
        ));
        let mut facts = Facts::default();
        for r in ["ta-v1-0@x", "ta-v2-0@x", "tb-v10-0@x"] {
            facts.fetched.insert(r.into(), "2026-10-14T00:00:00Z".into());
        }
        let families = series::families(series::build(&msgs, &facts, &Rules::default()), &facts);
        Model::new(families, msgs, "https://lore.example")
    }

    fn typed(m: &mut Model, keys: &str) {
        for c in keys.chars() {
            m.key(&c.to_string());
        }
    }

    #[test]
    fn navigation_filter_and_views() {
        let mut m = model();
        assert_eq!(m.visible().len(), 2);
        m.key("j");
        m.key("j"); // stops at the end
        assert_eq!(m.sel, 1);
        typed(&mut m, "/second");
        m.key("Enter");
        assert_eq!(m.visible().iter().map(|f| f.head().title.as_str()).collect::<Vec<_>>(), ["other: a second series"]);
        m.key("Enter");
        assert_eq!(m.view, View::Read);
        assert!(m.lines.iter().any(|l| l.contains("Why?")), "{:?}", m.lines);
        m.key("q");
        assert_eq!(m.view, View::List);
        m.key("d");
        assert_eq!(m.status, "only one version is known");
        let root = m.visible()[0].head().root.clone();
        assert_eq!(m.key("o"), Some(Action::Open(format!("https://lore.example/all/{root}/T/#u"))));
        assert_eq!(m.key("q"), Some(Action::Quit));
    }

    #[test]
    fn open_only_and_diff() {
        let mut m = model();
        m.key("f");
        assert!(m.visible().iter().all(|f| OPEN.contains(&f.state)));
        m.key("f");
        m.sel = m.visible().iter().position(|f| f.head().title == "foo: do the thing").unwrap();
        m.key("d");
        assert_eq!(m.view, View::Diff);
        assert!(m.lines[0].contains("v1 → v2"), "{:?}", m.lines);
        m.key("?");
        m.key("Esc");
        assert_eq!(m.view, View::List);
        m.key("?");
        assert_eq!(m.view, View::Help);
    }

    fn screen(m: &mut Model, w: u16, h: u16) -> Vec<String> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, m)).unwrap();
        let buf = t.backend().buffer();
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>().trim_end().to_string()).collect()
    }

    #[test]
    fn draw_aligns_columns() {
        let mut m = model();
        let rows = screen(&mut m, 80, 10);
        let listed: Vec<&String> = rows[1..].iter().filter(|r| r.starts_with(['●', '○', '◐'])).collect();
        assert_eq!(listed.len(), 2, "{rows:?}");
        let at: Vec<usize> = listed.iter().map(|r| r[..r.find("2026-10-").unwrap()].chars().count()).collect();
        assert_eq!(at[0], at[1], "{rows:?}");
        assert!(rows[9].starts_with("Enter read"), "{rows:?}");
        assert!(rows[0].starts_with("patchtrack · 2 series"));
        m.key("Enter");
        let read = screen(&mut m, 80, 10);
        assert!(read[0].contains("[PATCH"), "{read:?}");
        assert!(read[9].starts_with("j/k scroll"));
    }
}
