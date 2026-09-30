use gpui::{div, prelude::*, px, rgb, HighlightStyle, StyledText};

use crate::{FG, MUTED, PANEL};

pub(super) struct Row {
    pub text: String,
    pub kind: Kind,
}

pub(super) enum Kind {
    Heading(u8),
    Paragraph,
    Quote,
    List,
    Code,
    Gap,
}

fn inline(source: &str) -> StyledText {
    let mut text = String::new();
    let mut spans = Vec::new();
    let mut rest = source;
    while !rest.is_empty() {
        let marker = ["**", "*", "`", "["]
            .into_iter()
            .find(|marker| rest.starts_with(marker));
        if let Some(marker) = marker {
            let close = if marker == "[" { "](" } else { marker };
            let start = marker.len();
            if let Some(end) = rest[start..].find(close).map(|at| at + start) {
                let label = &rest[start..end];
                let from = text.len();
                text.push_str(label);
                let mut style = HighlightStyle::default();
                match marker {
                    "**" => style.font_weight = Some(gpui::FontWeight::BOLD),
                    "*" => style.font_style = Some(gpui::FontStyle::Italic),
                    "`" => style.color = Some(rgb(0xe5c07b).into()),
                    _ => style.color = Some(rgb(0x61afef).into()),
                }
                spans.push((from..text.len(), style));
                rest = &rest[end + close.len()..];
                if marker == "[" {
                    if let Some(end) = rest.find(')') {
                        rest = &rest[end + 1..];
                    }
                }
                continue;
            }
        }
        let ch = rest.chars().next().unwrap();
        text.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    StyledText::new(text).with_highlights(spans)
}

pub(super) fn parse(source: &str) -> Vec<Row> {
    let mut code = false;
    source
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                code = !code;
                return Row {
                    text: String::new(),
                    kind: Kind::Gap,
                };
            }
            let (kind, content) = if code {
                (Kind::Code, line)
            } else if let Some(title) = trimmed.strip_prefix('#') {
                let depth = trimmed.bytes().take_while(|byte| *byte == b'#').count();
                if depth <= 6 && title.starts_with([' ', '#']) {
                    (Kind::Heading(depth as u8), trimmed[depth..].trim_start())
                } else {
                    (Kind::Paragraph, line)
                }
            } else if let Some(quote) = trimmed.strip_prefix("> ") {
                (Kind::Quote, quote)
            } else if let Some(item) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
            {
                (Kind::List, item)
            } else if trimmed.is_empty() {
                (Kind::Gap, "")
            } else {
                (Kind::Paragraph, line)
            };
            Row {
                text: content.to_owned(),
                kind,
            }
        })
        .collect()
}

pub(super) fn view(row: &Row) -> impl gpui::IntoElement {
    let (height, size, color) = match row.kind {
        Kind::Heading(1) => (44., 25., FG),
        Kind::Heading(2) => (38., 21., FG),
        Kind::Heading(_) => (32., 17., FG),
        Kind::Code => (27., 14., 0xe5c07b),
        Kind::Quote => (29., 14., MUTED),
        Kind::Gap => (14., 14., FG),
        _ => (29., 14., FG),
    };
    div()
        .min_h(px(height))
        .w_full()
        .flex_shrink_0()
        .px_4()
        .py_1()
        .text_size(px(size))
        .text_color(rgb(color))
        .when(matches!(row.kind, Kind::Code), |v| v.bg(rgb(PANEL)))
        .when(matches!(row.kind, Kind::Quote), |v| {
            v.border_l_2().border_color(rgb(0x61afef))
        })
        .child(if matches!(row.kind, Kind::List) {
            inline(&format!("•  {}", row.text))
        } else {
            inline(&row.text)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markdown_rows_preserve_unicode_and_structure() {
        let rows = parse("# Árbol\n\n- **acción**\n```rust\nlet x = 1;\n```");
        assert!(matches!(rows[0].kind, Kind::Heading(1)));
        assert!(matches!(rows[2].kind, Kind::List));
        assert!(matches!(rows[4].kind, Kind::Code));
    }
}
