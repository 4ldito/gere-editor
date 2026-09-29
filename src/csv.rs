// A visual-only layout: padding is never written back to the file.
#[derive(Clone, Debug)]
pub struct Layout {
    pub lines: Vec<Row>,
    pub max_chars: usize,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub display: String,
    pub columns: Vec<usize>, // source character boundary -> displayed character boundary
}

impl Row {
    pub fn source_column(&self, displayed: usize) -> usize {
        self.columns
            .partition_point(|&column| column < displayed)
            .min(self.columns.len() - 1)
    }

    pub fn display_byte(&self, source_byte: usize, source: &str) -> usize {
        let column = source[..source_byte].chars().count();
        self.display
            .char_indices()
            .nth(self.columns[column])
            .map_or(self.display.len(), |(at, _)| at)
    }
}

fn fields(line: &str) -> (Vec<usize>, bool) {
    let mut commas = Vec::new();
    let mut quoted = false;
    let mut chars = line.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        if ch == '"' {
            if quoted && chars.peek().is_some_and(|(_, next)| *next == '"') {
                chars.next();
            } else {
                quoted = !quoted;
            }
        } else if ch == ',' && !quoted {
            commas.push(at);
        }
    }
    (commas, quoted)
}

pub fn layout(text: &str) -> Option<Layout> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut widths = Vec::<usize>::new();
    let mut separators = Vec::new();
    for line in &lines {
        let (commas, quoted) = fields(line);
        // Multiline quoted fields require a grid editor; leave them unchanged.
        if quoted {
            return None;
        }
        let mut start = 0;
        for (index, end) in commas
            .iter()
            .copied()
            .chain(std::iter::once(line.len()))
            .enumerate()
        {
            let width = line[start..end].chars().count();
            if widths.len() <= index {
                widths.push(width);
            } else {
                widths[index] = widths[index].max(width);
            }
            start = end + 1;
        }
        separators.push(commas);
    }
    let mut max_chars = 0;
    let rows = lines
        .iter()
        .zip(separators)
        .map(|(line, commas)| {
            let mut display = String::new();
            let mut columns = Vec::with_capacity(line.chars().count() + 1);
            let mut source_chars = 0;
            let mut shown = 0;
            let mut field_start = 0;
            let mut previous = 0;
            for (index, end) in commas
                .into_iter()
                .chain(std::iter::once(line.len()))
                .enumerate()
            {
                for ch in line[previous..end].chars() {
                    columns.push(shown);
                    display.push(ch);
                    shown += 1;
                    source_chars += 1;
                }
                if end < line.len() {
                    let pad = widths[index].saturating_sub(line[field_start..end].chars().count());
                    display.extend(std::iter::repeat(' ').take(pad));
                    shown += pad;
                    columns.push(shown);
                    display.push(',');
                    shown += 1;
                    source_chars += 1;
                    display.push(' ');
                    shown += 1;
                    previous = end + 1;
                    field_start = previous;
                }
            }
            debug_assert_eq!(columns.len(), source_chars);
            columns.push(shown);
            max_chars = max_chars.max(shown);
            Row { display, columns }
        })
        .collect();
    Some(Layout {
        lines: rows,
        max_chars,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aligns_without_touching_quoted_commas_or_utf8() {
        let layout = layout("nombre,edad\ná,12\n\"x,y\",3").unwrap();
        assert_eq!(layout.lines[1].display, "á     , 12");
        assert_eq!(layout.lines[2].display, "\"x,y\" , 3");
        assert_eq!(layout.lines[1].source_column(8), 2);
        assert!(super::layout("\"multi\nline\",2").is_none());
    }
}
