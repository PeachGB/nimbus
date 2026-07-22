use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, List, ListItem, ListState, Paragraph},
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
        _ => 3,
    }
}

pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(step_area_height(app)),
            Constraint::Length(1),
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
    let footer = Span::styled(footer_text, footer_style);
    let footer = Paragraph::new(Line::from(footer)).alignment(Alignment::Center);
    frame.render_widget(footer, chunks[3]);
}
