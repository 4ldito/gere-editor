//! On-demand language-server definition lookup. Runs on the background executor.
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Mutex, OnceLock},
};

pub struct Definition {
    pub path: PathBuf,
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy)]
enum Language {
    Rust,
    JavaScript,
    TypeScript,
}

fn language(path: &Path) -> Option<Language> {
    match path.extension()?.to_str()? {
        "rs" => Some(Language::Rust),
        "js" | "mjs" | "cjs" | "jsx" => Some(Language::JavaScript),
        "ts" | "mts" | "cts" | "tsx" => Some(Language::TypeScript),
        _ => None,
    }
}

pub fn supports(path: &Path) -> bool {
    language(path).is_some()
}

pub fn supports_js(path: &Path) -> bool {
    matches!(
        language(path),
        Some(Language::JavaScript | Language::TypeScript)
    )
}

fn uri(path: &Path) -> String {
    let mut result = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            result.push(byte as char);
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?.as_bytes();
    let mut bytes = Vec::new();
    let mut index = 0;
    while index < encoded.len() {
        if encoded[index] == b'%' {
            let hex = std::str::from_utf8(encoded.get(index + 1..index + 3)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            bytes.push(encoded[index]);
            index += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

fn send(writer: &mut impl Write, message: &Value) -> Result<(), String> {
    let body = message.to_string();
    write!(writer, "Content-Length: {}\r\n\r\n{body}", body.len()).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())
}

fn receive(reader: &mut impl BufRead) -> Result<Value, String> {
    let mut length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).map_err(|e| e.to_string())? == 0 {
            return Err("El servidor de lenguaje se cerró".into());
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = Some(value.trim().parse::<usize>().map_err(|e| e.to_string())?);
        }
    }
    let length = length.ok_or("Respuesta sin Content-Length")?;
    if length > 16 * 1024 * 1024 {
        return Err("Respuesta demasiado grande".into());
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}

fn request(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    send(
        writer,
        &json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
    )?;
    loop {
        let response = receive(reader)?;
        if response.get("id") == Some(&json!(id)) {
            if let Some(error) = response.get("error") {
                return Err(format!("{method}: {error}"));
            }
            return Ok(response.get("result").cloned().unwrap_or(Value::Null));
        }
        if let (Some(id), Some(_)) = (response.get("id"), response.get("method")) {
            send(writer, &json!({"jsonrpc":"2.0", "id":id, "result":null}))?;
        }
    }
}

fn parse_location(result: &Value) -> Option<(PathBuf, usize, usize, usize)> {
    let location = if let Some(items) = result.as_array() {
        items.first()?
    } else {
        result
    };
    let path = path_from_uri(
        location
            .get("targetUri")
            .or_else(|| location.get("uri"))?
            .as_str()?,
    )?;
    let range = location
        .get("targetSelectionRange")
        .or_else(|| location.get("range"))?;
    Some((
        path,
        range["start"]["line"].as_u64()? as usize,
        range["start"]["character"].as_u64()? as usize,
        range["end"]["character"].as_u64()? as usize,
    ))
}

struct Session {
    child: Child,
    reader: BufReader<ChildStdout>,
    writer: ChildStdin,
    opened: HashMap<PathBuf, (String, usize)>,
    next_id: u64,
    fresh: bool,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Session {
    fn start(root: &Path, language: Language) -> Result<Self, String> {
        let tsserver = if matches!(language, Language::Rust) {
            None
        } else {
            let local = root.join("node_modules/typescript/lib/tsserver.js");
            if local.is_file() {
                Some(local)
            } else {
                let npm = Command::new("npm").args(["root", "-g"]).output().ok();
                npm.and_then(|output| {
                    output.status.success().then(|| {
                        PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
                            .join("typescript/lib/tsserver.js")
                    })
                })
                .filter(|path| path.is_file())
            }
        };
        let (program, args) = match language {
            Language::Rust => (PathBuf::from("rust-analyzer"), &[][..]),
            Language::JavaScript | Language::TypeScript => {
                let local = root.join("node_modules/.bin/typescript-language-server");
                let program = if local.is_file() {
                    local
                } else {
                    PathBuf::from("typescript-language-server")
                };
                (program, &["--stdio"][..])
            }
        };
        let mut child = Command::new(&program)
            .current_dir(root)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{}: {e}", program.display()))?;
        let writer = child.stdin.take().ok_or("Sin entrada al servidor")?;
        let reader = BufReader::new(child.stdout.take().ok_or("Sin salida del servidor")?);
        let mut session = Self {
            child,
            reader,
            writer,
            opened: HashMap::new(),
            next_id: 1,
            fresh: true,
        };
        session.request("initialize", json!({
            "processId": std::process::id(), "rootUri": uri(root), "capabilities": {},
            "workspaceFolders": [{"uri": uri(root), "name": root.file_name().unwrap_or_default().to_string_lossy()}],
            "initializationOptions": {"tsserver": {"path": tsserver}}
        }))?;
        session.notify("initialized", json!({}))?;
        Ok(session)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        send(
            &mut self.writer,
            &json!({"jsonrpc":"2.0", "method":method, "params":params}),
        )
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        request(&mut self.reader, &mut self.writer, id, method, params)
    }

    fn prepare(&mut self, path: &Path, language: Language, text: &str) -> Result<(), String> {
        let version = if let Some((previous, version)) = self.opened.get(path) {
            if previous == text {
                *version
            } else {
                version + 1
            }
        } else {
            1
        };
        if let Some((previous, _)) = self.opened.get(path) {
            if previous != text {
                self.notify(
                    "textDocument/didChange",
                    json!({
                        "textDocument":{"uri":uri(path),"version":version},
                        "contentChanges":[{"text":text}]
                    }),
                )?;
            }
        } else {
            let language_id = match language {
                Language::Rust => "rust",
                Language::JavaScript => "javascript",
                Language::TypeScript => "typescript",
            };
            self.notify("textDocument/didOpen", json!({
                "textDocument":{"uri":uri(path),"languageId":language_id,"version":version,"text":text}
            }))?;
        }
        self.opened
            .insert(path.to_path_buf(), (text.to_owned(), version));
        Ok(())
    }

    fn lookup(
        &mut self,
        root: &Path,
        path: &Path,
        language: Language,
        text: &str,
        line: usize,
        column: usize,
    ) -> Result<Option<Definition>, String> {
        self.prepare(path, language, text)?;
        let source_line = text
            .split('\n')
            .nth(line)
            .ok_or("Línea fuera del archivo")?;
        let character = source_line
            .chars()
            .take(column)
            .map(char::len_utf16)
            .sum::<usize>();
        let mut result = Value::Null;
        let attempts = if matches!(language, Language::Rust) {
            30
        } else if self.fresh {
            12
        } else {
            1
        };
        for attempt in 0..attempts {
            if matches!(language, Language::JavaScript | Language::TypeScript) {
                let source = self.request(
                    "workspace/executeCommand",
                    json!({"command":"_typescript.goToSourceDefinition", "arguments":[uri(&path), {"line":line,"character":character}]}),
                );
                if let Ok(source) = source {
                    if !source.is_null() && source.as_array().is_none_or(|items| !items.is_empty())
                    {
                        result = source;
                        break;
                    }
                }
            }
            let response = self.request(
                "textDocument/definition",
                json!({
                    "textDocument":{"uri":uri(&path)}, "position":{"line":line,"character":character}
                }),
            );
            result = match response {
                Ok(result) => result,
                Err(error) if error.contains("\"code\":-32801") && attempt + 1 < attempts => {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    continue;
                }
                Err(error) => return Err(error),
            };
            if !result.is_null() && result.as_array().is_none_or(|items| !items.is_empty()) {
                break;
            }
            if attempt + 1 < attempts {
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }
        self.fresh = false;
        Ok(
            parse_location(&result).and_then(|(path, line, start, end)| {
                path.strip_prefix(root).ok().map(|relative| Definition {
                    path: relative.to_path_buf(),
                    line,
                    start,
                    end,
                })
            }),
        )
    }
}

static JS_SESSIONS: OnceLock<Mutex<HashMap<PathBuf, Session>>> = OnceLock::new();

/// Called after the window appears so the first definition request does not pay startup cost.
pub fn warm_project(root: &Path, candidate: Option<&Path>) -> Result<(), String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut sessions = JS_SESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|e| e.to_string())?;
    if !sessions.contains_key(&root) {
        sessions.insert(root.clone(), Session::start(&root, Language::JavaScript)?);
    }
    if let Some(candidate) = candidate {
        let path = root
            .join(candidate)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let session = sessions.get_mut(&root).unwrap();
        if !session.opened.contains_key(&path) {
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            session.prepare(
                &path,
                language(&path).unwrap_or(Language::JavaScript),
                &text,
            )?;
        }
        // Force tsserver to load the project graph now, before the first click.
        let _ = session.request(
            "textDocument/definition",
            json!({
                "textDocument":{"uri":uri(&path)}, "position":{"line":0,"character":0}
            }),
        );
    }
    Ok(())
}

pub fn warm(root: &Path, path: &Path, text: &str) -> Result<(), String> {
    let Some(language @ (Language::JavaScript | Language::TypeScript)) = language(path) else {
        return Ok(());
    };
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = root.join(path).canonicalize().map_err(|e| e.to_string())?;
    let mut sessions = JS_SESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|e| e.to_string())?;
    if !sessions.contains_key(&root) {
        sessions.insert(root.clone(), Session::start(&root, language)?);
    }
    sessions
        .get_mut(&root)
        .unwrap()
        .prepare(&path, language, text)
}

pub fn lookup(
    root: &Path,
    path: &Path,
    text: &str,
    line: usize,
    column: usize,
) -> Result<Option<Definition>, String> {
    let Some(language) = language(path) else {
        return Ok(None);
    };
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = root.join(path).canonicalize().map_err(|e| e.to_string())?;
    if matches!(language, Language::Rust) {
        return Session::start(&root, language)?.lookup(&root, &path, language, text, line, column);
    }
    let mut sessions = JS_SESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|e| e.to_string())?;
    for retry in 0..2 {
        if !sessions.contains_key(&root) {
            sessions.insert(root.clone(), Session::start(&root, language)?);
        }
        let result = sessions
            .get_mut(&root)
            .unwrap()
            .lookup(&root, &path, language, text, line, column);
        if result.is_ok() || retry == 1 {
            return result;
        }
        sessions.remove(&root);
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_and_location_accept_spaces_unicode_and_location_links() {
        let path = Path::new("/tmp/a b/é.rs");
        assert_eq!(path_from_uri(&uri(path)).as_deref(), Some(path));
        let result = json!([{"targetUri":uri(path), "targetSelectionRange": {
            "start":{"line":3,"character":4}, "end":{"line":3,"character":8}}}]);
        assert_eq!(
            parse_location(&result).map(|(_, line, start, end)| (line, start, end)),
            Some((3, 4, 8))
        );
        assert!(parse_location(&Value::Null).is_none());
    }

    #[test]
    #[ignore = "requires rust-analyzer and a Cargo workspace"]
    fn resolves_definition_in_another_file() {
        let root = std::env::temp_dir().join(format!("gere-definition-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"definition_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/other.rs"), "pub fn destination() {}\n").unwrap();
        let text = "mod other;\npub fn caller() { other::destination(); }\n";
        std::fs::write(root.join("src/lib.rs"), text).unwrap();
        let result = lookup(&root, Path::new("src/lib.rs"), text, 1, 27);
        std::fs::remove_dir_all(&root).unwrap();
        let target = result.unwrap().expect("definition");
        assert_eq!(target.path, Path::new("src/other.rs"));
    }

    #[test]
    #[ignore = "requires typescript-language-server and typescript"]
    fn resolves_javascript_import_in_another_file() {
        let root = std::env::temp_dir().join(format!("gere-js-definition-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("jsconfig.json"),
            "{\"compilerOptions\":{\"allowJs\":true}}\n",
        )
        .unwrap();
        std::fs::write(root.join("other.js"), "export function destination() {}\n").unwrap();
        let text = "import { destination } from './other.js';\ndestination();\n";
        std::fs::write(root.join("main.js"), text).unwrap();
        warm_project(&root, Some(Path::new("main.js"))).unwrap();
        warm(&root, Path::new("main.js"), text).unwrap();
        let result = lookup(&root, Path::new("main.js"), text, 1, 2);
        let edited = format!("{text}parseInt('2', 10);\n");
        let start = std::time::Instant::now();
        let builtin = lookup(&root, Path::new("main.js"), &edited, 2, 2);
        let builtin_time = start.elapsed();
        let start = std::time::Instant::now();
        let repeated = lookup(&root, Path::new("main.js"), &edited, 1, 2);
        let repeated_time = start.elapsed();
        let same_file = format!(
            "{edited}async function init(cb) {{ return cb(); }}\n{}module.exports = {{ init }};\n",
            "// gap\n".repeat(150)
        );
        let same_target = lookup(&root, Path::new("main.js"), &same_file, 154, 20);
        std::fs::remove_dir_all(&root).unwrap();
        let target = result.unwrap().expect("JS definition");
        assert_eq!(target.path, Path::new("other.js"));
        assert_eq!(target.line, 0);
        assert!(
            builtin.unwrap().is_none(),
            "builtins outside the project are not opened"
        );
        assert_eq!(
            repeated
                .unwrap()
                .expect("definition after unsaved edit")
                .path,
            Path::new("other.js")
        );
        let same_target = same_target.unwrap().expect("same-file definition");
        assert_eq!(same_target.path, Path::new("main.js"));
        assert_eq!(same_target.line, 3);
        assert!(
            builtin_time < std::time::Duration::from_secs(3),
            "warm builtin lookup: {builtin_time:?}"
        );
        assert!(
            repeated_time < std::time::Duration::from_secs(3),
            "warm project lookup: {repeated_time:?}"
        );
    }
}
