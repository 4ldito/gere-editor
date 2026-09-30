use gpui::{px, rgb, HighlightStyle, StyledText, UnderlineStyle};
use std::path::Path;
use tree_sitter::{Node, Parser};

pub struct HighlightedLine {
    text: String,
    highlights: Vec<(std::ops::Range<usize>, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

impl DiagnosticSeverity {
    pub const fn color(self) -> u32 {
        match self {
            Self::Error => 0xe06c75,
            Self::Warning => 0xe5c07b,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Advertencia",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub range: std::ops::Range<usize>,
    pub message: String,
    pub severity: DiagnosticSeverity,
}

impl HighlightedLine {
    pub fn aligned(&self, row: &crate::csv::Row) -> Self {
        Self {
            text: row.display.clone(),
            highlights: self
                .highlights
                .iter()
                .map(|(range, color)| {
                    (
                        row.display_byte(range.start, &self.text)
                            ..row.display_byte(range.end, &self.text),
                        *color,
                    )
                })
                .collect(),
        }
    }
    pub fn render_editor(
        &self,
        selection: Option<std::ops::Range<usize>>,
        search_matches: &[std::ops::Range<usize>],
        changed: Option<(std::ops::Range<usize>, u32)>,
        diagnostics: &[Diagnostic],
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
        for diagnostic in diagnostics {
            boundaries.push(diagnostic.range.start);
            boundaries.push(diagnostic.range.end);
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
                let diagnostic_color = diagnostics
                    .iter()
                    .find_map(|diagnostic| {
                        (diagnostic.severity == DiagnosticSeverity::Error
                            && diagnostic.range.start < end
                            && diagnostic.range.end > start)
                            .then_some(diagnostic.severity.color())
                    })
                    .or_else(|| {
                        diagnostics.iter().find_map(|diagnostic| {
                            (diagnostic.severity == DiagnosticSeverity::Warning
                                && diagnostic.range.start < end
                                && diagnostic.range.end > start)
                                .then_some(diagnostic.severity.color())
                        })
                    });
                if syntax_color.is_none()
                    && !selected
                    && !found
                    && changed_color.is_none()
                    && diagnostic_color.is_none()
                {
                    return None;
                }
                let mut style = HighlightStyle::default();
                if let Some(color) = syntax_color {
                    style.color = Some(rgb(color).into());
                }
                if let Some(color) = diagnostic_color {
                    style.underline = Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(rgb(color).into()),
                        wavy: true,
                    });
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

        let (display, byte_columns) = expand_tabs(&self.text);
        StyledText::new(display).with_highlights(
            highlights
                .into_iter()
                .map(|(range, style)| (byte_columns[range.start]..byte_columns[range.end], style))
                .collect::<Vec<_>>(),
        )
    }
}

fn expand_tabs(text: &str) -> (String, Vec<usize>) {
    let mut display = String::with_capacity(text.len());
    let mut byte_columns = vec![0; text.len() + 1];
    let mut column = 0;
    for (at, ch) in text.char_indices() {
        byte_columns[at] = display.len();
        if ch == '\t' {
            let spaces = crate::buffer::TAB_WIDTH - column % crate::buffer::TAB_WIDTH;
            display.push_str(&" ".repeat(spaces));
            column += spaces;
        } else {
            display.push(ch);
            column += 1;
        }
    }
    byte_columns[text.len()] = display.len();
    (display, byte_columns)
}

fn language(path: &Path) -> Option<tree_sitter::Language> {
    let extension = path
        .extension()
        .and_then(|x| x.to_str())?
        .to_ascii_lowercase();
    Some(match extension.as_str() {
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "json" => tree_sitter_json::LANGUAGE.into(),
        "html" | "htm" | "svg" => tree_sitter_html::LANGUAGE.into(),
        "css" => tree_sitter_css::LANGUAGE.into(),
        "less" => tree_sitter_less::language(),
        "js" | "mjs" | "cjs" => tree_sitter_javascript::LANGUAGE.into(),
        _ => return None,
    })
}

pub fn diagnostics(text: &str, path: &Path) -> Vec<Diagnostic> {
    let Some(language) = language(path) else {
        return Vec::new();
    };
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(text, None) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    fn visit(node: Node, text: &str, result: &mut Vec<Diagnostic>) {
        if !node.has_error() && !node.is_missing() {
            return;
        }
        if node.is_error() || node.is_missing() {
            let line = node.start_position().row;
            let line_start = text[..node.start_byte()].rfind('\n').map_or(0, |at| at + 1);
            let line_end = text[node.start_byte()..]
                .find('\n')
                .map_or(text.len(), |at| node.start_byte() + at);
            let start = node
                .start_byte()
                .saturating_sub(line_start)
                .min(line_end - line_start);
            let end = if node.end_position().row == line {
                node.end_byte()
                    .saturating_sub(line_start)
                    .min(line_end - line_start)
            } else {
                line_end - line_start
            };
            let start = if start == end && start > 0 {
                start - 1
            } else {
                start
            };
            result.push(Diagnostic {
                line,
                range: start..end.max(start + 1).min(line_end - line_start),
                message: if node.is_missing() {
                    format!("Falta {}", node.kind())
                } else {
                    "Error de sintaxis".into()
                },
                severity: DiagnosticSeverity::Error,
            });
            return;
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                visit(child, text, result);
            }
        }
    }
    visit(tree.root_node(), text, &mut result);
    result
}

pub fn line(text: &str, path: &Path) -> Vec<HighlightedLine> {
    lines_and_folds(text, path).0
}

/// Returns syntax-colored lines and the last (inclusive) line of each foldable block.
pub fn lines_and_folds(text: &str, path: &Path) -> (Vec<HighlightedLine>, Vec<Option<usize>>) {
    let language = match language(path) {
        Some(language) => language,
        None => {
            return (
                text.split('\n')
                    .map(|s| HighlightedLine {
                        text: s.to_owned(),
                        highlights: Vec::new(),
                    })
                    .collect(),
                vec![None; text.split('\n').count()],
            )
        }
    };
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return (Vec::new(), Vec::new());
    }
    let Some(tree) = parser.parse(text, None) else {
        return (Vec::new(), Vec::new());
    };
    let mut folds = vec![None; text.split('\n').count()];
    fn collect_folds(node: Node, folds: &mut [Option<usize>]) {
        if matches!(
            node.kind(),
            "function_item"
                | "impl_item"
                | "mod_item"
                | "struct_item"
                | "enum_item"
                | "trait_item"
                | "function_declaration"
                | "generator_function_declaration"
                | "class_declaration"
                | "method_definition"
                | "function_expression"
                | "arrow_function"
                | "class"
                | "object"
                | "array"
                | "rule_set"
                | "media_statement"
                | "element"
                | "script_element"
                | "style_element"
        ) {
            let start = node.start_position().row;
            let end = node
                .end_position()
                .row
                .saturating_sub(usize::from(node.end_position().column == 0));
            if end > start && end < folds.len() {
                folds[start] = Some(folds[start].map_or(end, |previous: usize| previous.max(end)));
            }
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                collect_folds(child, folds);
            }
        }
    }
    collect_folds(tree.root_node(), &mut folds);
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
    let lines = text
        .split('\n')
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
        .collect();
    (lines, folds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expanding_tabs_keeps_highlight_byte_offsets_valid_after_unicode() {
        let (display, offsets) = expand_tabs("é\tx");
        assert_eq!(display, "é   x");
        assert_eq!(&display[offsets[2]..offsets[3]], "   ");
        assert_eq!(&display[offsets[3]..offsets[4]], "x");
    }

    #[test]
    fn highlights_requested_languages() {
        for (path, source) in [
            ("view.html", "<div class=\"card\">hola</div>"),
            ("graphic.SVG", "<svg><path d=\"M0 0\" /></svg>"),
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

    #[test]
    fn fold_ranges_follow_syntax_and_do_not_eat_the_next_line() {
        let (_, rust) = lines_and_folds(
            "fn outer() {\n    fn inner() {\n        work();\n    }\n}\nnext();",
            Path::new("test.rs"),
        );
        assert_eq!(rust[0], Some(4));
        assert_eq!(rust[1], Some(3));
        assert_eq!(rust[5], None);

        let (_, js) = lines_and_folds(
            "class Demo {\n  run() {\n    return 1;\n  }\n}\nconst x = 1;",
            Path::new("test.js"),
        );
        assert_eq!(js[0], Some(4));
        assert_eq!(js[1], Some(3));
        let (_, plain) = lines_and_folds("first\nsecond", Path::new("notes.txt"));
        assert_eq!(plain, [None, None]);
    }

    #[test]
    fn syntax_errors_have_a_line_and_an_underline_range() {
        let errors = diagnostics("const value = ;", Path::new("app.js"));
        assert!(!errors.is_empty());
        assert_eq!(errors[0].line, 0);
        assert!(errors[0].range.start < errors[0].range.end);
        assert_eq!(errors[0].severity, DiagnosticSeverity::Error);
        assert!(diagnostics("hello", Path::new("notes.txt")).is_empty());
    }
}
