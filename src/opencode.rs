//! Read-only OpenCode session history and ChatGPT OAuth quota.
use serde_json::Value;
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub project_id: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub files: Vec<String>,
}

fn db(query: &str) -> Result<Value, String> {
    let output = Command::new("opencode")
        .args(["db", query, "--format", "json"])
        .output()
        .map_err(|_| "Instalá OpenCode para consultar sesiones".to_owned())?;
    if !output.status.success() {
        return Err("No se pudo leer la base de sesiones de OpenCode".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "Respuesta inválida de OpenCode".into())
}

pub fn sessions(root: &Path) -> Result<Vec<Session>, String> {
    let directory = root.to_string_lossy().replace('\'', "''");
    sessions_with(|offset| {
        db(&format!(
            "select id,title,project_id, (select json_extract(p.data,'$.snapshot') from part p where p.session_id=session.id and json_extract(p.data,'$.type')='step-start' order by p.time_created,p.id limit 1) as before, (select json_extract(p.data,'$.snapshot') from part p where p.session_id=session.id and json_extract(p.data,'$.type')='step-finish' order by p.time_created desc,p.id desc limit 1) as after from session where directory='{directory}' order by time_updated desc,id desc limit 50 offset {offset}"
        ))
    })
}

fn sessions_with(
    mut page: impl FnMut(usize) -> Result<Value, String>,
) -> Result<Vec<Session>, String> {
    let mut sessions = Vec::new();
    loop {
        let rows = page(sessions.len())?;
        let rows = rows.as_array().ok_or("Lista de sesiones inválida")?;
        for row in rows {
            let (Some(id), Some(title)) = (row["id"].as_str(), row["title"].as_str()) else {
                return Err("Sesión de OpenCode inválida".into());
            };
            sessions.push(Session {
                id: id.into(),
                title: title.into(),
                project_id: row["project_id"].as_str().unwrap_or("").into(),
                before: row["before"].as_str().map(str::to_owned),
                after: row["after"].as_str().map(str::to_owned),
                files: Vec::new(),
            });
        }
        if rows.len() < 50 {
            break;
        }
    }
    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_all_pages_even_when_first_page_is_full() {
        let sessions = sessions_with(|offset| {
            Ok(Value::Array((offset..(offset + 50).min(123))
                .map(|i| serde_json::json!({"id": format!("ses_{i}"), "title": format!("Sesión {i}"), "project_id": "abc"}))
                .collect()))
        }).unwrap();
        assert_eq!(sessions.len(), 123);
        assert_eq!(sessions[122].id, "ses_122");
    }
}

fn snapshot_repo(session: &Session) -> Result<std::path::PathBuf, String> {
    let hash = |s: &str| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit());
    let before = session
        .before
        .as_deref()
        .ok_or("Esta sesión no tiene snapshots de cambios")?;
    let after = session
        .after
        .as_deref()
        .ok_or("Esta sesión no tiene snapshots de cambios")?;
    if !hash(&session.project_id) || !hash(before) || !hash(after) {
        return Err("Snapshots de OpenCode inválidos".into());
    }
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
        })
        .ok_or("No se encontró el directorio de OpenCode")?;
    let dir = home.join("opencode/snapshot").join(&session.project_id);
    for entry in std::fs::read_dir(dir)
        .map_err(|_| "Snapshots de OpenCode no disponibles")?
        .flatten()
    {
        let repo = entry.path();
        if !repo.join("HEAD").is_file() {
            continue;
        }
        let valid = [before, after].iter().all(|hash| {
            Command::new("git")
                .args(["--git-dir"])
                .arg(&repo)
                .args(["cat-file", "-t", hash])
                .output()
                .is_ok_and(|output| output.status.success() && output.stdout == b"tree\n")
        });
        if valid {
            return Ok(repo);
        }
    }
    Err("Snapshots de esta sesión no disponibles".into())
}

fn git_diff(session: &Session, path: Option<&Path>) -> Result<Vec<u8>, String> {
    let repo = snapshot_repo(session)?;
    let mut command = Command::new("git");
    command.arg("--git-dir").arg(repo).args([
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--color=never",
    ]);
    if path.is_none() {
        command.arg("--name-only").arg("-z");
    }
    let before = session.before.as_deref().unwrap();
    let after = session.after.as_deref().unwrap();
    command.args([before, after, "--"]);
    if let Some(path) = path {
        command.arg(path);
    }
    let output = command.output().map_err(|_| "Git no disponible")?;
    if !output.status.success() {
        return Err("No se pudo leer el diff de OpenCode".into());
    }
    Ok(output.stdout)
}

pub fn files(session: &Session) -> Result<Vec<String>, String> {
    let output = git_diff(session, None)?;
    Ok(output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect())
}

/// Read both versions from the session's snapshot trees, never from the current worktree.
pub fn file_versions(session: &Session, file: &str) -> Result<(String, String), String> {
    if !session.files.iter().any(|path| path == file) {
        return Err("Archivo no pertenece a la sesión".into());
    }
    let repo = snapshot_repo(session)?;
    let read = |tree: &str| -> Result<String, String> {
        let spec = format!("{tree}:{file}");
        let exists = Command::new("git")
            .arg("--git-dir")
            .arg(&repo)
            .args(["cat-file", "-e", &spec])
            .stderr(Stdio::null())
            .status()
            .map_err(|_| "Git no disponible".to_owned())?;
        if !exists.success() {
            return Ok(String::new());
        }
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(&repo)
            .args(["show", "--no-textconv", &spec])
            .output()
            .map_err(|_| "Git no disponible".to_owned())?;
        if !output.status.success() {
            return Err("No se pudo leer el snapshot del archivo".into());
        }
        String::from_utf8(output.stdout)
            .map_err(|_| "Archivo binario no disponible como texto".into())
    };
    Ok((
        read(session.before.as_deref().unwrap())?,
        read(session.after.as_deref().unwrap())?,
    ))
}

pub fn quota() -> Result<(u8, u8), String> {
    let path = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
        })
        .ok_or("OpenCode no configurado")?
        .join("opencode/auth.json");
    let auth: Value = serde_json::from_slice(
        &std::fs::read(path).map_err(|_| "Conectá OpenAI en OpenCode".to_owned())?,
    )
    .map_err(|_| "Credenciales de OpenCode inválidas".to_owned())?;
    let entry = &auth["openai"];
    if entry["type"] != "oauth" {
        return Err("Conectá OpenAI por OAuth en OpenCode".into());
    }
    let token = entry["access"]
        .as_str()
        .ok_or("Sesión OAuth no disponible")?;
    let account = entry["accountId"]
        .as_str()
        .ok_or("Cuenta de OpenAI no disponible")?;
    if !account
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("Cuenta inválida".into());
    }
    if token.contains(['\r', '\n', '"']) {
        return Err("Token inválido".into());
    }
    let mut child = Command::new("curl")
        .args([
            "-fsS",
            "--max-time",
            "8",
            "--config",
            "-",
            "https://chatgpt.com/backend-api/wham/usage",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "curl no disponible".to_owned())?;
    let config = format!(
        "header = \"Authorization: Bearer {token}\"\nheader = \"ChatGPT-Account-Id: {account}\"\n"
    );
    child
        .stdin
        .take()
        .unwrap()
        .write_all(config.as_bytes())
        .map_err(|_| "No se pudo consultar OpenAI".to_owned())?;
    let output = child
        .wait_with_output()
        .map_err(|_| "No se pudo consultar OpenAI".to_owned())?;
    if !output.status.success() {
        return Err("Sesión OpenAI vencida o servicio no disponible".into());
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Respuesta de OpenAI inválida".to_owned())?;
    let remaining = |key: &str| {
        value["rate_limit"][key]["used_percent"]
            .as_f64()
            .map(|used| (100. - used).round().clamp(0., 100.) as u8)
    };
    Ok((
        remaining("primary_window").ok_or("Cuota de 5 h no disponible")?,
        remaining("secondary_window").ok_or("Cuota semanal no disponible")?,
    ))
}
