use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, List, ListItem, ListState, Paragraph, Widget, Wrap},
};

use crate::{
    app::{App, Step},
    builder::OriginKind,
};

fn summary_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if !app.name.is_empty() {
        lines.push(Line::from(format!("name: {}", app.name)));
    }
    if matches!(
        app.step,
        Step::SelectOrigin | Step::Field(_) | Step::SavePath | Step::Confirm
    ) && !app.root_id.is_empty()
    {
        lines.push(Line::from(format!("root_id: {}", app.root_id)));
    }
    if let Some(kind) = app.origin_kind {
        lines.push(Line::from(format!("origin: {}", kind.label())));
        for field in kind.fields() {
            if let Some(value) = app.values.get(field.key) {
                lines.push(Line::from(format!("  {}: {}", field.key, value)));
            }
        }
    }
    if app.encrypt {
        lines.push(Line::from(format!(
            "encrypted at rest, key file: {}",
            app.key_path
        )));
    }
    if app.step == Step::Confirm && !app.save_path.is_empty() {
        lines.push(Line::from(format!("save to: {}", app.save_path)));
    }
    lines
}

fn render_prompt(frame: &mut Frame, area: Rect, prompt: &str, input: &str) {
    let block = Block::bordered()
        .title(prompt)
        .border_type(BorderType::Rounded);
    let paragraph = Paragraph::new(format!("{input}_")).block(block);
    frame.render_widget(paragraph, area);
}

/// Height needed for the step area: the select list needs one row per origin kind (plus
/// borders) so every entry is visible without scrolling; other steps are a single input line.
fn step_area_height(app: &App) -> u16 {
    match app.step {
        Step::SelectOrigin => OriginKind::ALL.len() as u16 + 2,
        Step::Encrypt => 4,
        Step::KeyBackup => key_backup_lines(app).len() as u16 + 2,
        _ => 3,
    }
}

/// The key-backup screen: what losing the key means, where it will be written, the key itself
/// (shown this once), and what to type to go on. This is the only place the key's text is
/// displayed.
///
/// The returned lines and the ratatui frame buffers they are drawn into hold copies of the key
/// text that are not zeroized: ratatui owns those buffers. The key itself, and the text this
/// function asks it for, are wiped when dropped.
fn key_backup_lines(app: &App) -> Vec<Line<'static>> {
    let key = app.key.as_ref().map(|key| key.to_text());
    let key = key.as_deref().map(String::as_str).unwrap_or_default();
    let destination = app.key_file().display().to_string();
    vec![
        Line::from(Span::styled(
            "Losing this key loses the vault's data: no one, you included, can decrypt it without the key.",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "It will be written to {destination} (readable by you only) once you confirm below."
        )),
        Line::from(
            "Copy it now to a password manager or another safe place; it isn't shown again:",
        ),
        Line::from(
            "The key is shown only here. Don't run the wizard under a terminal recorder \
             (script, asciinema, tmux logging): it would capture the key.",
        ),
        Line::from(""),
        Line::from(Span::styled(
            key.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!(
            "Then type \"{}\" and press enter: {}_",
            crate::app::BACKUP_PHRASE,
            app.input
        )),
    ]
}

/// The most lines the footer may take, so a long error can't squeeze out the step area.
const FOOTER_MAX_LINES: u16 = 4;

/// The footer: the current error, else the tab suggestions, else the key hints. It wraps, so a
/// long error (one naming a long path, say) is shown whole rather than cut at the edge.
fn footer(app: &App) -> Paragraph<'static> {
    let path_completable = app
        .current_field()
        .map(|f| f.path_completable)
        .unwrap_or(false);
    let footer_text = if let Some(e) = &app.error {
        format!("error: {e}")
    } else if !app.suggestions.is_empty() {
        format!("tab: {}", app.suggestions.join("  "))
    } else if path_completable {
        "enter confirm \u{b7} esc cancel \u{b7} tab complete path".to_string()
    } else {
        "enter confirm \u{b7} esc cancel".to_string()
    };
    let footer_style = if app.error.is_some() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Paragraph::new(Line::from(Span::styled(footer_text, footer_style)))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false })
}

/// How many lines `footer` takes once wrapped at `width`, up to [`FOOTER_MAX_LINES`]: it is
/// rendered into a scratch buffer and the last row with anything in it counted, which matches
/// ratatui's own wrapping exactly.
fn footer_height(footer: &Paragraph, width: u16) -> u16 {
    let area = Rect::new(0, 0, width, FOOTER_MAX_LINES);
    let mut buffer = Buffer::empty(area);
    footer.clone().render(area, &mut buffer);
    (0..FOOTER_MAX_LINES)
        .rev()
        .find(|&y| (0..width).any(|x| buffer[(x, y)].symbol() != " "))
        .map_or(1, |y| y + 1)
}

pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let footer = footer(app);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(step_area_height(app)),
            Constraint::Length(footer_height(&footer, area.width)),
        ])
        .split(area);

    let header = Paragraph::new("nimbus vault creator")
        .block(Block::bordered().border_type(BorderType::Rounded))
        .alignment(Alignment::Center)
        .fg(Color::Cyan);
    frame.render_widget(header, chunks[0]);

    let summary = Paragraph::new(summary_lines(app)).block(
        Block::bordered()
            .title("summary")
            .border_type(BorderType::Rounded),
    );
    frame.render_widget(summary, chunks[1]);

    match app.step {
        Step::Name => render_prompt(frame, chunks[2], "vault name", &app.input),
        Step::RootId => render_prompt(
            frame,
            chunks[2],
            "root id (optional, default '/')",
            &app.input,
        ),
        Step::SelectOrigin => {
            let items: Vec<ListItem> = OriginKind::ALL
                .iter()
                .map(|kind| ListItem::new(kind.label()))
                .collect();
            let list = List::new(items)
                .block(
                    Block::bordered()
                        .title("origin type (\u{2191}/\u{2193}, enter to select)")
                        .border_type(BorderType::Rounded),
                )
                .highlight_style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("\u{25b8} ");
            let mut state = ListState::default().with_selected(Some(app.origin_idx));
            frame.render_stateful_widget(list, chunks[2], &mut state);
        }
        Step::Field(_) => {
            let label = app.current_field().map(|f| f.label).unwrap_or("field");
            render_prompt(frame, chunks[2], label, &app.input);
        }
        Step::Encrypt => {
            let paragraph = Paragraph::new(vec![
                Line::from("y: encrypt; the storage then only ever holds ciphertext, and the key stays here"),
                Line::from("n: store the data as it is"),
            ])
            .block(
                Block::bordered()
                    .title("encrypt this vault at rest? (y / n)")
                    .border_type(BorderType::Rounded),
            );
            frame.render_widget(paragraph, chunks[2]);
        }
        Step::KeyPath => render_prompt(
            frame,
            chunks[2],
            "key file to create (never overwrites one)",
            &app.input,
        ),
        Step::KeyBackup => {
            let paragraph = Paragraph::new(key_backup_lines(app))
                .wrap(Wrap { trim: false })
                .block(
                    Block::bordered()
                        .title("back up the key")
                        .border_type(BorderType::Rounded),
                );
            frame.render_widget(paragraph, chunks[2]);
        }
        Step::SavePath => render_prompt(frame, chunks[2], "save config to", &app.input),
        Step::Confirm => {
            let paragraph = Paragraph::new("press enter to save, esc to cancel").block(
                Block::bordered()
                    .title("confirm")
                    .border_type(BorderType::Rounded),
            );
            frame.render_widget(paragraph, chunks[2]);
        }
    }

    frame.render_widget(footer, chunks[3]);
}
