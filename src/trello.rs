//! Per-project Trello preferences and on-demand API access.
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Clone)]
pub struct List {
    pub id: String,
    pub name: String,
    pub cards: Vec<Card>,
}

#[derive(Clone)]
pub struct Card {
    pub id: String,
    pub name: String,
    pub url: String,
}

#[derive(Clone)]
pub struct Board {
    pub url: String,
    pub name: String,
}

pub struct Config {
    pub board_url: String,
    pub list_order: Vec<String>,
}

pub fn config(root: &Path) -> Result<Option<Config>, String> {
    let data = match std::fs::read(root.join(".gere/trello.json")) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "No se pudo leer la configuración de Trello: {error}"
            ))
        }
    };
    let value: Value = serde_json::from_slice(&data)
        .map_err(|_| "El archivo .gere/trello.json no es JSON válido".to_owned())?;
    let url = value["board_url"]
        .as_str()
        .ok_or("Falta board_url en .gere/trello.json")?;
    board_id(url)?;
    Ok(Some(Config {
        board_url: url.to_owned(),
        list_order: value["list_order"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    }))
}

pub fn save_config(root: &Path, config: &Config) -> Result<(), String> {
    board_id(&config.board_url)?;
    let dir = root.join(".gere");
    std::fs::create_dir_all(&dir).map_err(|e| format!("No se pudo crear .gere: {e}"))?;
    let path = dir.join("trello.json");
    // Preserve other project-specific settings, and never overwrite a malformed config.
    let mut value: Value = match std::fs::read(&path) {
        Ok(data) => serde_json::from_slice(&data)
            .map_err(|_| "No se reemplazó .gere/trello.json: JSON inválido")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(format!("No se pudo leer .gere/trello.json: {error}")),
    };
    let object = value
        .as_object_mut()
        .ok_or("No se reemplazó .gere/trello.json: objeto inválido")?;
    object.insert("board_url".into(), Value::String(config.board_url.clone()));
    object.insert("list_order".into(), serde_json::json!(config.list_order));
    let data = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
    // create_new avoids clobbering a user's pre-existing temporary file.
    let temp = dir.join("trello.json.tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| format!("No se pudo guardar Trello: {e}"))?;
    if let Err(error) = file
        .write_all(&data)
        .and_then(|_| file.sync_all())
        .and_then(|_| std::fs::rename(&temp, &path))
    {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("No se pudo guardar Trello: {error}"));
    }
    Ok(())
}

pub fn sorted_lists(mut lists: Vec<List>, order: &[String]) -> Vec<List> {
    lists.sort_by_key(|list| {
        order
            .iter()
            .position(|id| id == &list.id)
            .unwrap_or(usize::MAX)
    });
    lists
}

fn credential_path() -> Result<PathBuf, String> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|dir| dir.join("gere").join("trello-credentials.json"))
        .ok_or("No se encontró HOME ni XDG_CONFIG_HOME para guardar Trello".into())
}

fn validate_credentials(key: &str, token: &str) -> Result<(), String> {
    if ![&key, &token]
        .iter()
        .all(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric()))
    {
        return Err("Las credenciales de Trello deben ser alfanuméricas".into());
    }
    Ok(())
}

fn read_credentials(path: &Path) -> Result<Option<(String, String)>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("No se pudo consultar las credenciales: {error}")),
    };
    if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err("Las credenciales de Trello deben ser un archivo privado (chmod 600)".into());
    }
    let data = fs::read(path).map_err(|e| format!("No se pudieron leer las credenciales: {e}"))?;
    let value: Value = serde_json::from_slice(&data)
        .map_err(|_| "El archivo de credenciales de Trello no es JSON válido".to_owned())?;
    let key = value["key"].as_str().ok_or("Falta la API key de Trello")?;
    let token = value["token"].as_str().ok_or("Falta el token de Trello")?;
    validate_credentials(key, token)?;
    Ok(Some((key.to_owned(), token.to_owned())))
}

pub fn credentials() -> Result<Option<(String, String)>, String> {
    if let Some(saved) = read_credentials(&credential_path()?)? {
        return Ok(Some(saved));
    }
    match (
        std::env::var("TRELLO_API_KEY"),
        std::env::var("TRELLO_TOKEN"),
    ) {
        (Ok(key), Ok(token)) => {
            validate_credentials(&key, &token)?;
            Ok(Some((key, token)))
        }
        _ => Ok(None),
    }
}

fn write_credentials(path: &Path, key: &str, token: &str) -> Result<(), String> {
    validate_credentials(key, token)?;
    let dir = path.parent().ok_or("Ruta de credenciales inválida")?;
    fs::create_dir_all(dir).map_err(|e| format!("No se pudo crear la carpeta de Gere: {e}"))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("No se pudo proteger la carpeta de Gere: {e}"))?;
    // Never overwrite an unexpected file or link, including a pre-existing temp file.
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_file() {
            return Err("La ruta de credenciales no es un archivo normal".into());
        }
    }
    let temp = dir.join("trello-credentials.json.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|e| format!("No se pudo crear el archivo privado de Trello: {e}"))?;
    let data = serde_json::to_vec(&serde_json::json!({ "key": key, "token": token }))
        .map_err(|e| e.to_string())?;
    if let Err(error) = file
        .write_all(&data)
        .and_then(|_| file.sync_all())
        .and_then(|_| fs::rename(&temp, path))
    {
        let _ = fs::remove_file(&temp);
        return Err(format!("No se pudieron guardar las credenciales: {error}"));
    }
    Ok(())
}

pub fn save_credentials(key: &str, token: &str) -> Result<(), String> {
    write_credentials(&credential_path()?, key, token)
}

fn request(method: &str, route: &str, args: &[(&str, &str)]) -> Result<Value, String> {
    let (key, token) = credentials()?.ok_or("Conectá tu cuenta de Trello en Gere")?;
    let mut command = Command::new("curl");
    command.args([
        "--config",
        "-",
        "--silent",
        "--fail",
        "--max-time",
        "12",
        "--max-filesize",
        "2097152",
    ]);
    if method != "GET" {
        command.args(["--request", method]);
    }
    for (field, value) in args {
        command.args(["--data-urlencode", &format!("{field}={value}")]);
    }
    // Credentials are read via stdin, never exposed as command-line arguments.
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "No se pudo ejecutar curl; instalalo para conectar Trello".to_owned())?;
    let query = if route.contains('?') { '&' } else { '?' };
    let config =
        format!("url = \"https://api.trello.com/1/{route}{query}key={key}&token={token}\"\n");
    if child
        .stdin
        .take()
        .unwrap()
        .write_all(config.as_bytes())
        .is_err()
    {
        let _ = child.wait();
        return Err("No se pudo iniciar la consulta a Trello".into());
    }
    let output = child
        .wait_with_output()
        .map_err(|_| "Error de conexión con Trello")?;
    if !output.status.success() {
        return Err(
            "Trello rechazó la operación: comprobá el acceso, el permiso del token y la conexión"
                .into(),
        );
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "Respuesta inválida de Trello".into())
}

pub fn boards() -> Result<Vec<Board>, String> {
    let response = request("GET", "members/me/boards?fields=name,url&filter=open", &[])?;
    response
        .as_array()
        .ok_or("Respuesta inválida de Trello")?
        .iter()
        .map(|board| {
            let url = board["url"].as_str().ok_or("Tablero inválido de Trello")?;
            board_id(url)?;
            Ok(Board {
                url: url.to_owned(),
                name: board["name"]
                    .as_str()
                    .ok_or("Tablero inválido de Trello")?
                    .to_owned(),
            })
        })
        .collect()
}

fn board_id(url: &str) -> Result<&str, String> {
    let path = url
        .trim()
        .strip_prefix("https://trello.com/b/")
        .ok_or("Usá una URL de tablero https://trello.com/b/…")?;
    let id = path.split('/').next().unwrap_or("");
    if id.len() != 8 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("La URL del tablero de Trello no es válida".into());
    }
    Ok(id)
}

fn parse_lists(value: &Value) -> Result<Vec<List>, String> {
    let lists = value.as_array().ok_or("Respuesta inválida de Trello")?;
    lists
        .iter()
        .filter(|list| list["closed"] != true)
        .map(|list| {
            let id = list["id"]
                .as_str()
                .ok_or("Lista inválida de Trello")?
                .to_owned();
            let name = list["name"]
                .as_str()
                .ok_or("Lista inválida de Trello")?
                .to_owned();
            let cards = list["cards"]
                .as_array()
                .ok_or("Tarjetas inválidas de Trello")?
                .iter()
                .filter(|card| card["closed"] != true)
                .map(|card| {
                    let id = card["id"]
                        .as_str()
                        .ok_or("Tarjeta inválida de Trello")?
                        .to_owned();
                    let name = card["name"]
                        .as_str()
                        .ok_or("Tarjeta inválida de Trello")?
                        .to_owned();
                    let url = card["shortUrl"]
                        .as_str()
                        .ok_or("URL inválida de Trello")?
                        .to_owned();
                    if !url.starts_with("https://trello.com/c/") {
                        return Err("URL inválida de Trello".into());
                    }
                    Ok(Card { id, name, url })
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(List { id, name, cards })
        })
        .collect()
}

pub fn load(root: &Path) -> Result<Vec<List>, String> {
    let config = config(root)?.ok_or("Elegí un tablero de Trello para este proyecto")?;
    let board = board_id(&config.board_url)?;
    let response = request("GET", &format!("boards/{board}/lists?filter=open&fields=name,closed&cards=open&card_fields=id,name,shortUrl,closed"), &[])?;
    Ok(sorted_lists(parse_lists(&response)?, &config.list_order))
}

fn safe_id(id: &str) -> Result<&str, String> {
    if id.len() == 24 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(id)
    } else {
        Err("ID de Trello inválido".into())
    }
}

pub enum CardChange {
    Rename { id: String, name: String },
    Move { id: String, list: String },
    Create { list: String, name: String },
}

pub fn change_card(change: CardChange) -> Result<(), String> {
    match change {
        CardChange::Rename { id, name } if !name.trim().is_empty() => {
            request(
                "PUT",
                &format!("cards/{}", safe_id(&id)?),
                &[("name", name.trim())],
            )?;
        }
        CardChange::Move { id, list } => {
            request(
                "PUT",
                &format!("cards/{}", safe_id(&id)?),
                &[("idList", safe_id(&list)?)],
            )?;
        }
        CardChange::Create { list, name } if !name.trim().is_empty() => {
            request(
                "POST",
                "cards",
                &[("idList", safe_id(&list)?), ("name", name.trim())],
            )?;
        }
        _ => return Err("El título de la tarjeta no puede estar vacío".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_board_urls_with_safe_short_ids() {
        assert_eq!(
            board_id("https://trello.com/b/SvO7KniW/editor"),
            Ok("SvO7KniW")
        );
        assert!(board_id("https://example.com/b/SvO7KniW/editor").is_err());
        assert!(board_id("https://trello.com/b/abc\"x123/editor").is_err());
    }

    #[test]
    fn reads_lists_and_cards_and_skips_closed_items() {
        let data: Value = serde_json::from_str(r#"[{"id":"one","name":"Pendiente","cards":[{"id":"abc","name":"Tarea","shortUrl":"https://trello.com/c/abc123","closed":false},{"name":"Vieja","closed":true}],"closed":false},{"name":"Archivada","closed":true,"cards":[]}]"#).unwrap();
        let lists = parse_lists(&data).unwrap();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].name, "Pendiente");
        assert_eq!(lists[0].cards[0].name, "Tarea");
        assert_eq!(lists[0].cards.len(), 1);
    }

    #[test]
    fn preferred_order_keeps_new_lists_in_trello_order() {
        let lists = ["pending", "doing", "done", "new"]
            .map(|id| List {
                id: id.into(),
                name: id.into(),
                cards: Vec::new(),
            })
            .to_vec();
        let sorted = sorted_lists(lists, &["done".into(), "pending".into()]);
        assert_eq!(
            sorted
                .iter()
                .map(|list| list.id.as_str())
                .collect::<Vec<_>>(),
            ["done", "pending", "doing", "new"]
        );
    }

    #[test]
    fn board_selection_and_order_persist_without_overwriting_other_settings() {
        let root = std::env::temp_dir().join(format!(
            "gere-trello-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(config(&root).unwrap().is_none());
        let directory = root.join(".gere");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("trello.json"),
            r#"{"other":"keep","board_url":"https://trello.com/b/SvO7KniW/editor"}"#,
        )
        .unwrap();
        let saved = Config {
            board_url: "https://trello.com/b/A1b2C3d4/otro".into(),
            list_order: vec!["done".into(), "pending".into()],
        };
        save_config(&root, &saved).unwrap();
        let loaded = config(&root).unwrap().unwrap();
        assert_eq!(loaded.board_url, saved.board_url);
        assert_eq!(loaded.list_order, saved.list_order);
        let value: Value =
            serde_json::from_slice(&std::fs::read(directory.join("trello.json")).unwrap()).unwrap();
        assert_eq!(value["other"], "keep");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn credentials_are_private_and_readable_after_restart() {
        let root = std::env::temp_dir().join(format!(
            "gere-trello-secrets-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("gere").join("trello-credentials.json");
        write_credentials(&path, "Key123", "Token456").unwrap();
        assert_eq!(
            read_credentials(&path).unwrap(),
            Some(("Key123".into(), "Token456".into()))
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        // Existing unsafe files must not be read; saving replaces them atomically.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_credentials(&path).is_err());
        write_credentials(&path, "NewKey", "NewToken").unwrap();
        assert_eq!(
            read_credentials(&path).unwrap(),
            Some(("NewKey".into(), "NewToken".into()))
        );
        assert!(!path.with_file_name("trello-credentials.json.tmp").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
