use chrono::{DateTime, Local, Utc};
use nimbus_vault::object::Object;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, List, ListItem, StatefulWidget},
};

use crate::app::App;

/// Width of the size column, wide enough for `1023.9G`.
const SIZE_WIDTH: usize = 8;
/// Width of the modified column, sized for `2026-07-25 14:03`.
const MODIFIED_WIDTH: usize = 16;
/// Below this, the columns are dropped entirely rather than squeezing names to nothing.
const MIN_NAME_WIDTH: usize = 16;

pub fn render_vault_list(app: &mut App, area: Rect, buf: &mut Buffer) {
    let block = Block::bordered()
        .title("vaults")
        .border_type(BorderType::Rounded)
        .fg(Color::Cyan);

    let items: Vec<ListItem> = app
        .vaults
        .iter()
        .map(|name| ListItem::new(Line::from(format!("  {}", name))))
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .highlight_symbol("> ");

    StatefulWidget::render(list, area, buf, &mut app.vault_state);
}

pub fn render_object_list(app: &mut App, area: Rect, buf: &mut Buffer) {
    let title = match (&app.filter, app.marked.len()) {
        (Some(filter), 0) => format!("objects — filter: {filter}"),
        (Some(filter), n) => format!("objects — filter: {filter} · {n} marked"),
        (None, 0) => "objects".to_string(),
        (None, n) => format!("objects — {n} marked"),
    };

    let block = Block::bordered()
        .title(title)
        .border_type(BorderType::Rounded)
        .fg(Color::Cyan);

    // `inner` accounts for the border; the leading 2 is the highlight symbol the List reserves.
    let inner_width = block.inner(area).width as usize;
    let name_width = inner_width.saturating_sub(2 + 1 + SIZE_WIDTH + 2 + MODIFIED_WIDTH);
    let columns = name_width >= MIN_NAME_WIDTH;

    let items: Vec<ListItem> = app
        .visible
        .iter()
        .filter_map(|&i| app.objects.get(i))
        .map(|object| {
            let marked = app.marked.contains(&object.get_name());
            ListItem::new(entry_line(object, marked, columns, name_width))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .highlight_symbol("> ");

    StatefulWidget::render(list, area, buf, &mut app.object_state);
}

/// One row of the object list: a mark column, the name, and — when there's room — right-aligned
/// size and modified time.
fn entry_line(object: &Object, marked: bool, columns: bool, name_width: usize) -> Line<'static> {
    let is_dir = matches!(object, Object::Branch { .. } | Object::Root { .. });
    let name = match object {
        Object::Root { .. } => "/".to_string(),
        other if is_dir => format!("{}/", other.get_name()),
        other => other.get_name(),
    };

    let mark = Span::styled(
        if marked { "*" } else { " " }.to_string(),
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    );
    // Directories carry the accent so the eye can group them without reading the trailing `/`.
    let name_style = match (marked, is_dir) {
        (true, _) => Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
        (false, true) => Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD),
        (false, false) => Style::default().fg(Color::White),
    };

    if !columns {
        return Line::from(vec![mark, Span::styled(name, name_style)]);
    }

    let meta = object.get_meta();
    let size = if is_dir {
        // A directory's own byte count says nothing about what's in it, so showing it would be
        // actively misleading.
        "—".to_string()
    } else {
        meta.as_ref()
            .and_then(|meta| meta.size)
            .map(human_size)
            .unwrap_or_else(|| "?".to_string())
    };
    let modified = meta
        .as_ref()
        .and_then(|meta| meta.modified)
        .map(human_time)
        .unwrap_or_else(|| "—".to_string());

    Line::from(vec![
        mark,
        Span::styled(pad_or_truncate(&name, name_width), name_style),
        Span::styled(
            format!("{size:>SIZE_WIDTH$}  "),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(
            format!("{modified:>MODIFIED_WIDTH$}"),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

/// Fits `text` to exactly `width` columns, eliding the middle of anything too long — the tail of
/// a filename (its extension, a trailing version) is usually as informative as the head.
pub fn pad_or_truncate(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return format!("{text:<width$}");
    }
    if width <= 1 {
        return "…".repeat(width);
    }
    let keep = width - 1;
    let head_len = keep.div_ceil(2);
    let tail_len = keep - head_len;
    let chars: Vec<char> = text.chars().collect();
    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[count - tail_len..].iter().collect();
    format!("{head}…{tail}")
}

/// Renders a byte count in the largest unit that keeps it short.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{bytes}B");
    }
    // One decimal only while it adds information; `1.0K` reads worse than `1K`.
    if value < 10.0 {
        format!("{value:.1}{}", UNITS[unit])
    } else {
        format!("{value:.0}{}", UNITS[unit])
    }
}

/// Renders a timestamp in the viewer's own timezone — origins report UTC, but "when did I last
/// touch this" is a local-time question.
fn human_time(time: DateTime<Utc>) -> String {
    time.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

#[cfg(test)]
#[path = "../../tests/widgets.rs"]
mod tests;
