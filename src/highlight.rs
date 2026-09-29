use gpui::{rgb, HighlightStyle, StyledText};
use std::path::Path;
use tree_sitter::{Node, Parser};

pub struct HighlightedLine {
    text: String,
    highlights: Vec<(std::ops::Range<usize>, u32)>,
}

impl HighlightedLine {
    pub fn render_editor(
        &self,
        selection: Option<std::ops::Range<usize>>,
        search_matches: &[std::ops::Range<usize>],
        changed: Option<(std::ops::Range<usize>, u32)>,
    ) -> StyledText {
        if self.text.is_empty() && selection.as_ref().is_some_and(|range| range.is_empty()) {
            let mut style = HighlightStyle::default();
            style.background_color = Some(rgb(0x3e4451).into());
            return StyledText::new(" ".to_owned()).with_highlights(vec![(0..1, style)]);
        }
        let mut boundaries = vec![0, self.text.len()];
        for (range, _) in &self.highlights {
            boundaries.push(range.start);
            boundaries.push(range.end);
        }
        if let Some(selection) = &selection {
            boundaries.push(selection.start);
            boundaries.push(selection.end);
        }
        for range in search_matches {
            boundaries.push(range.start);
            boundaries.push(range.end);
        }
        if let Some((range, _)) = &changed {
            boundaries.push(range.start);
            boundaries.push(range.end);
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        let highlights = boundaries
            .windows(2)
            .filter_map(|window| {
                let start = window[0];
                let end = window[1];
                if start == end {
                    return None;
                }
                let syntax_color = self.highlights.iter().find_map(|(range, color)| {
                    (range.start < end && range.end > start).then_some(*color)
                });
                let selected = selection
                    .as_ref()
                    .is_some_and(|selection| selection.start < end && selection.end > start);
                let found = search_matches
                    .iter()
                    .any(|range| range.start < end && range.end > start);
                let changed_color = changed.as_ref().and_then(|(range, color)| {
                    (range.start < end && range.end > start).then_some(*color)
                });
                if syntax_color.is_none() && !selected && !found && changed_color.is_none() {
                    return None;
                }
                let mut style = HighlightStyle::default();
                if let Some(color) = syntax_color {
                    style.color = Some(rgb(color).into());
                }
                if selected {
                    style.background_color = Some(rgb(0x3e4451).into());
                }
                if found && !selected {
                    style.background_color = Some(rgb(0x524b32).into());
                } else if !selected {
                    if let Some(color) = changed_color {
                        style.background_color = Some(rgb(color).into());
                    }
                }
                Some((start..end, style))
            })
            .collect::<Vec<_>>();

        StyledText::new(self.text.clone()).with_highlights(highlights)
    }
}

pub fn line(text: &str, path: &Path) -> Vec<HighlightedLine> {
    let language = match path.extension().and_then(|x| x.to_str()) {
        Some("rs") => tree_sitter_rust::LANGUAGE.into(),
        Some("json") => tree_sitter_json::LANGUAGE.into(),
        Some("html" | "htm") => tree_sitter_html::LANGUAGE.into(),
        Some("css") => tree_sitter_css::LANGUAGE.into(),
        Some("less") => tree_sitter_less::language(),
        Some("js" | "mjs" | "cjs") => tree_sitter_javascript::LANGUAGE.into(),
        _ => {
            return text
                .split('\n')
                .map(|s| HighlightedLine {
                    text: s.to_owned(),
                    highlights: Vec::new(),
                })
                .collect()
        }
    };
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(text, None) else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    fn visit(node: Node, out: &mut Vec<(std::ops::Range<usize>, u32)>) {
        if node.child_count() == 0 {
            let color = color(node.kind(), node.parent().map(|parent| parent.kind()));
            if let Some(color) = color {
                out.push((node.byte_range(), color));
            }
        } else {
            if matches!(
                node.kind(),
                "string"
                    | "template_string"
                    | "attribute_value"
                    | "string_value"
                    | "comment"
                    | "line_comment"
                    | "block_comment"
                    | "regex"
            ) {
                if let Some(color) = color(node.kind(), None) {
                    out.push((node.byte_range(), color));
                }
                return;
            }
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    visit(child, out);
                }
            }
        }
    }
    // One Dark Pro token colors: https://github.com/Binaryify/OneDark-Pro
    fn color(kind: &str, parent: Option<&str>) -> Option<u32> {
        match kind {
            "string_content" | "string" | "string_fragment" | "template_string"
            | "attribute_value" | "string_value" => Some(0x98c379),
            "regex" => Some(0x56b6c2),
            "line_comment" | "block_comment" | "comment" => Some(0x7f848e),
            "integer_literal" | "float_literal" | "integer_value" | "float_value" | "number"
            | "color_value" => Some(0xd19a66),
            "true" | "false" | "null" | "undefined" => Some(0xd19a66),
            "tag_name" => Some(0xe06c75),
            "attribute_name" => Some(0xd19a66),
            "property_name" | "property_identifier" => Some(0xe06c75),
            "function_name" => Some(0x61afef),
            "class_name" | "type_identifier" => Some(0xe5c07b),
            "id_name" => Some(0x61afef),
            "identifier"
                if matches!(
                    parent,
                    Some("call_expression" | "function_declaration" | "method_definition")
                ) =>
            {
                Some(0x61afef)
            }
            "variable" => Some(0xe06c75),
            "fn" | "let" | "const" | "var" | "function" | "pub" | "use" | "mod" | "struct"
            | "enum" | "impl" | "match" | "if" | "else" | "return" | "import" | "export"
            | "async" | "await" | "class" | "new" | "from" | "default" | "throw" | "try"
            | "catch" | "for" | "while" | "of" | "in" => Some(0xc678dd),
            _ => None,
        }
    }
    visit(tree.root_node(), &mut ranges);
    let mut offset = 0;
    let mut first = 0;
    text.split('\n')
        .map(|s| {
            let end = offset + s.len();
            while first < ranges.len() && ranges[first].0.end <= offset {
                first += 1;
            }
            let highlights = ranges
                .iter()
                .skip(first)
                .take_while(|(range, _)| range.start < end)
                .filter_map(|(range, color)| {
                    let start = range.start.max(offset);
                    let stop = range.end.min(end);
                    (start < stop).then(|| (start - offset..stop - offset, *color))
                })
                .collect::<Vec<_>>();
            offset = end + 1;
            HighlightedLine {
                text: s.to_owned(),
                highlights,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_requested_languages() {
        for (path, source) in [
            ("view.html", "<div class=\"card\">hola</div>"),
            ("site.css", ".card { color: red; }"),
            ("site.less", "@accent: #abc;\n.card { color: @accent; }"),
            ("app.js", "const greeting = 'hola';"),
        ] {
            let lines = line(source, Path::new(path));
            assert!(
                lines.iter().any(|line| !line.highlights.is_empty()),
                "{path} sin resaltado"
            );
        }
    }
}
