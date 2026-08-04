use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph, Widget},
};

pub mod widgets;

use crate::{
    app::{App, AppMode},
    command,
};

impl Widget for &mut App {
    /// Renders the user interface widgets.
    fn render(self, area: Rect, buf: &mut Buffer) {
        // The footer grows for a long message rather than truncating it — origin errors are
        // routinely longer than a terminal is wide, and a half-shown error is a useless one.
        let footer_height = footer_lines(self, area.width).len().clamp(1, 4) as u16;
        let [header_area, main_area, footer_area] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(footer_height),
        ])
        .areas(area);

        render_header(self, header_area, buf);

        match &self.mode {
            AppMode::Root => widgets::render_vault_list(self, main_area, buf),
            AppMode::Vault(_) => widgets::render_object_list(self, main_area, buf),
            _ => {}
        }

        render_footer(self, footer_area, buf);

        // Drawn last so it sits on top of whichever list is underneath.
        if let Some(scroll) = self.help {
            render_help(area, scroll, buf);
        }
    }
}

fn render_header(app: &App, area: Rect, buf: &mut Buffer) {
    let title = match &app.mode {
        AppMode::Root => "nimbus — vaults".to_string(),
        AppMode::Vault(name) => format!("nimbus — {}:{}", name, app.nimbus.pwd()),
        _ => "nimbus".to_string(),
    };

    let block = Block::bordered()
        .title(title)
        .border_type(BorderType::Rounded)
        .fg(Color::Cyan);

    Paragraph::new("").block(block).render(area, buf);
}

fn render_footer(app: &App, area: Rect, buf: &mut Buffer) {
    Paragraph::new(footer_lines(app, area.width)).render(area, buf);
}

/// Builds the footer's content, pre-wrapped to `width`. Split out from rendering so the layout
/// can ask how tall it needs to be before handing it an area.
fn footer_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let width = width as usize;

    // Outranks everything else in the bar: it's a question blocking every other key, and it's
    // about to destroy something, so it gets the line to itself in a colour nothing else uses.
    if let Some(confirm) = &app.confirm {
        return styled_lines(
            &confirm.prompt,
            width,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        );
    }

    if let Some(buffer) = &app.command {
        return vec![Line::from(Span::styled(
            format!(":{buffer}"),
            Style::default().fg(Color::Yellow),
        ))];
    }

    if app.filtering {
        return vec![Line::from(Span::styled(
            format!("/{}", app.filter.clone().unwrap_or_default()),
            Style::default().fg(Color::Yellow),
        ))];
    }

    if let Some(prompt) = &app.rename {
        // The old name is kept on screen next to the field being edited, so it stays clear
        // which object is being renamed once the input has been edited away from it.
        return vec![Line::from(vec![
            Span::styled(
                format!("rename {} to: ", prompt.original),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{}_", prompt.input),
                Style::default().fg(Color::Yellow),
            ),
        ])];
    }

    // What's on the clipboard outranks the keybinding hints — it's transient state the user
    // needs to see to know a `p` is pending, and it survives navigating between vaults.
    let pending = app
        .clipboard
        .as_ref()
        .map(|clipboard| {
            format!(
                "[{}: {}] ",
                if clipboard.cut { "cut" } else { "copy" },
                clipboard.label()
            )
        })
        .unwrap_or_default();

    let hints = match &app.mode {
        AppMode::Root => "↑/↓ move  →/enter open  n new vault  x forget  : command  ? help  q quit",
        AppMode::Vault(_) => {
            "↑/↓ move  →/enter open  e edit  r run  ←/esc back  space mark  y copy  d cut  p paste  c rename  x delete  / filter  ? help"
        }
        _ => "",
    };

    let Some(status) = &app.status else {
        let mut spans = Vec::new();
        if !pending.is_empty() {
            spans.push(Span::styled(
                pending,
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        spans.push(Span::styled(
            hints.to_string(),
            Style::default().fg(Color::DarkGray),
        ));
        return vec![Line::from(spans)];
    };

    // The clipboard indicator is dropped when the status message had to wrap; it's ambient
    // state, and the message is the thing that just happened.
    let mut lines = styled_lines(status, width, Style::default().fg(Color::Yellow));
    if !pending.is_empty() && lines.len() == 1 {
        lines[0].spans.insert(
            0,
            Span::styled(
                pending,
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
        );
    }
    lines
}

fn styled_lines(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    wrap_indented(text, 0, width)
        .into_iter()
        .map(|line| Line::from(Span::styled(line, style)))
        .collect()
}

fn render_help(area: Rect, scroll: u16, buf: &mut Buffer) {
    let width = area.width.saturating_mul(9).saturating_div(10).min(96);
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([Constraint::Percentage(90)])
        .flex(Flex::Center)
        .areas(area);

    // Text is pre-wrapped to the popup's inner width rather than leaning on `Paragraph`'s own
    // wrapping, which would restart continuation lines at column 0 and break the two-column read.
    let text_width = area.width.saturating_sub(4) as usize;

    let heading = |text: &str| {
        Line::from(Span::styled(
            text.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let entry = |key: &str, description: &str| {
        Line::from(vec![
            Span::styled(format!("  {key:<22}"), Style::default().fg(Color::White)),
            Span::styled(description.to_string(), Style::default().fg(Color::Gray)),
        ])
    };
    let note = |text: &str| {
        wrap_indented(text, 2, text_width)
            .into_iter()
            .map(|line| Line::from(Span::styled(line, Style::default().fg(Color::DarkGray))))
            .collect::<Vec<_>>()
    };

    let mut lines = vec![heading("NAVIGATION")];
    lines.extend([
        entry("↑/↓  or  k/j", "move the selection"),
        entry(
            "→/enter  or  l",
            "enter a vault or directory, or open a file",
        ),
        entry(
            "←/esc  or  h",
            "go up a directory, or back to the vault list",
        ),
        entry("q  /  ctrl-c", "quit"),
    ]);

    lines.push(Line::default());
    lines.push(heading("OPENING FILES"));
    lines.extend([
        entry("→/enter  or  l", "open with the OS's default application"),
        entry("e", "open in $EDITOR, whatever the OS would have picked"),
        entry("r", "run the file as a program"),
    ]);
    lines.extend(note(
        "Enter and `e` save your edits back to the vault when the program you edited in exits.",
    ));
    lines.extend(note(
        "`r` runs a shebang script or a binary this machine can execute, holding its output on screen until you press a key. Running a program changes nothing in the vault.",
    ));

    lines.push(Line::default());
    lines.push(heading("SELECTING"));
    lines.extend([
        entry("space", "mark/unmark, and step down"),
        entry("/", "filter this listing as you type"),
        entry("esc", "clear the filter (then: go up a directory)"),
    ]);
    lines.extend(note(
        "Copy, cut and delete act on every marked object at once, or on the cursor when nothing is marked. Marks and filters are dropped when you leave the directory.",
    ));

    lines.push(Line::default());
    lines.push(heading("COPY & MOVE"));
    lines.extend([
        entry("y", "yank (copy) the marked objects, or the cursor"),
        entry("d", "cut the marked objects, or the cursor"),
        entry("p", "paste into the directory you're browsing"),
    ]);
    lines.extend(note(
        "Navigate to another vault before pasting to copy/move across vaults.",
    ));

    lines.push(Line::default());
    lines.push(heading("VIEW"));
    lines.extend([
        entry("s  /  S", "cycle sort key / reverse the order"),
        entry(".", "show or hide dot-prefixed names"),
        entry("R", "reload the listing from the origin"),
    ]);
    lines.extend(note(
        "Directories always sort before files, whichever key and direction you pick.",
    ));

    lines.push(Line::default());
    lines.push(heading("CREATE, RENAME & DELETE"));
    lines.extend([
        entry("a", "add a directory here"),
        entry("t", "add an empty file here"),
        entry("c  /  F2", "rename the selected object, in place"),
        entry(
            "x  or  del",
            "delete marked objects, or the cursor — asks first",
        ),
    ]);
    lines.extend(note(
        "`c` opens a prompt pre-filled with the current name, so you edit it rather than retype it. Enter renames, esc abandons.",
    ));
    lines.extend(note(
        "Deleting a directory takes everything inside it, and nothing here is undoable — there is no trash.",
    ));
    lines.extend(note(
        "Opening a file and editing it saves your changes back to the vault on exit.",
    ));

    lines.push(Line::default());
    lines.push(heading("OTHER"));
    lines.extend([
        entry("n", "create a vault with the setup wizard"),
        entry("x", "on the vault list: stop tracking a vault"),
        entry(":", "open the command line"),
        entry("?", "toggle this help"),
    ]);
    lines.extend(note(
        "Forgetting a vault only unregisters it — its config file and everything in its origin are left where they are.",
    ));

    lines.push(Line::default());
    lines.push(heading("COMMANDS  (press : first)"));
    for (usage, about) in command::command_summaries() {
        // Usage on its own line: argument lists are long enough that a shared column would
        // either collide with the description or squeeze it to nothing.
        lines.push(Line::from(Span::styled(
            format!("  {usage}"),
            Style::default().fg(Color::White),
        )));
        for line in wrap_indented(&about, 6, text_width) {
            lines.push(Line::from(Span::styled(
                line,
                Style::default().fg(Color::Gray),
            )));
        }
    }

    lines.push(Line::default());
    lines.push(heading("PATHS"));
    lines.extend([
        entry("notes.txt", "relative to the directory you're in"),
        entry("/docs/notes.txt", "absolute within the current vault"),
        entry("backup:/inbox", "the vault named `backup`, from its root"),
    ]);
    lines.extend(note(
        "A `vault:` prefix is what tells a vault apart from a directory of the same name.",
    ));
    lines.extend(note(
        "cp/mv take either an existing directory or a new path — `cp a.txt backup:/inbox` keeps the name, `cp a.txt backup:/inbox/copy.txt` renames as it copies.",
    ));

    let block = Block::bordered()
        .title(" help — ↑/↓ scroll · esc close ")
        .border_type(BorderType::Rounded)
        .fg(Color::Cyan);

    Clear.render(area, buf);
    Paragraph::new(lines)
        .block(block)
        .scroll((scroll, 0))
        .render(area, buf);
}

/// Greedily wraps `text` to `width` columns, prefixing every resulting line with `indent`
/// spaces so continuations stay visually attached to what they belong to.
///
/// A word too long to fit on a line of its own is broken across lines rather than left to
/// overflow — error messages routinely carry an unbroken path or id longer than the terminal
/// is wide, and silently clipping one hides the part that identifies what went wrong.
pub fn wrap_indented(text: &str, indent: usize, width: usize) -> Vec<String> {
    let available = width.saturating_sub(indent).max(1);
    let prefix = " ".repeat(indent);

    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        for chunk in split_to_width(word, available) {
            let extra = if current.is_empty() {
                chunk.chars().count()
            } else {
                chunk.chars().count() + 1
            };
            if !current.is_empty() && current.chars().count() + extra > available {
                lines.push(format!("{prefix}{current}"));
                current = String::new();
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(&chunk);
        }
    }
    if !current.is_empty() {
        lines.push(format!("{prefix}{current}"));
    }
    lines
}

/// Splits `word` into pieces that each fit within `width` columns. Anything already short
/// enough comes back as a single piece, so ordinary prose is untouched.
fn split_to_width(word: &str, width: usize) -> Vec<String> {
    if word.chars().count() <= width {
        return vec![word.to_string()];
    }
    word.chars()
        .collect::<Vec<_>>()
        .chunks(width)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

#[cfg(test)]
#[path = "tests/ui.rs"]
mod tests;
