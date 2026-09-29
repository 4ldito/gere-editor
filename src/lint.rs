use crate::highlight::Diagnostic;
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

pub fn eslint(root: &Path, path: &Path, source: &str) -> Vec<Diagnostic> {
    if !matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx")
    ) {
        return Vec::new();
    }
    let local = root.join("node_modules/.bin/eslint");
    let binary = if local.is_file() {
        local.as_os_str()
    } else {
        std::ffi::OsStr::new("eslint")
    };
    let Ok(mut child) = Command::new(binary)
        .current_dir(root)
        .arg("--stdin")
        .arg("--stdin-filename")
        .arg(root.join(path))
        .arg("--format")
        .arg("json")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    if child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(source.as_bytes()).is_err())
    {
        let _ = child.kill();
        let _ = child.wait();
        return Vec::new();
    }
    let Ok(output) = child.wait_with_output() else {
        return Vec::new();
    };
    parse(&output.stdout, source)
}

fn parse(output: &[u8], source: &str) -> Vec<Diagnostic> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(output) else {
        return Vec::new();
    };
    let Some(messages) = value
        .get(0)
        .and_then(|file| file.get("messages"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    messages
        .iter()
        .filter_map(|message| {
            let line = message.get("line")?.as_u64()?.checked_sub(1)? as usize;
            let source_line = source.split('\n').nth(line)?;
            let start_chars = message
                .get("column")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .saturating_sub(1) as usize;
            let end_chars = message
                .get("endColumn")
                .and_then(|v| v.as_u64())
                .map_or(start_chars + 1, |end| end.saturating_sub(1) as usize);
            let at = |column: usize| {
                source_line
                    .char_indices()
                    .nth(column)
                    .map_or(source_line.len(), |(at, _)| at)
            };
            let start = at(start_chars);
            let end = at(end_chars.max(start_chars + 1));
            Some(Diagnostic {
                line,
                range: start.min(source_line.len().saturating_sub(1))
                    ..end.max(start + 1).min(source_line.len()),
                message: format!(
                    "{}: {}",
                    message
                        .get("ruleId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("ESLint"),
                    message.get("message")?.as_str()?
                ),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_eslint_utf8_columns() {
        let json = br#"[{"messages":[{"line":1,"column":2,"endColumn":3,"ruleId":"no-undef","message":"unknown name"}]}]"#;
        assert_eq!(parse(json, "áx")[0].range, 2..3);
    }
}
