use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabState {
    pub path: PathBuf,
    pub untitled: bool,
    pub cursor: usize,
    pub scroll_line: usize,
    pub dirty_text: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub tabs: Vec<TabState>,
    pub active: Option<usize>,
    pub language_overrides: HashMap<PathBuf, String>,
}

static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);
static SAVED: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();

pub fn revision() -> u64 {
    NEXT_REVISION.fetch_add(1, Ordering::Relaxed)
}

fn path(root: &Path) -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    // Stable, short project key; the full root is also checked when reading.
    let hash = root
        .to_string_lossy()
        .bytes()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    Some(
        config
            .join("gere")
            .join("sessions")
            .join(format!("{hash:016x}.json")),
    )
}

pub fn load(root: &Path) -> Session {
    let Some(path) = path(root) else {
        return Session::default();
    };
    load_at(root, &path)
}

fn load_at(root: &Path, path: &Path) -> Session {
    let Ok(text) = fs::read_to_string(path) else {
        return Session::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Session::default();
    };
    if value["root"].as_str() != root.to_str() {
        return Session::default();
    }
    let tabs = value["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tab| {
            Some(TabState {
                path: PathBuf::from(tab["path"].as_str()?),
                untitled: tab["untitled"].as_bool().unwrap_or(false),
                cursor: usize::try_from(tab["cursor"].as_u64()?).ok()?,
                scroll_line: usize::try_from(tab["scroll_line"].as_u64()?).ok()?,
                dirty_text: tab["dirty_text"].as_str().map(str::to_owned),
            })
        })
        .collect();
    Session {
        tabs,
        active: value["active"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok()),
        language_overrides: value["language_overrides"]
            .as_object()
            .into_iter()
            .flat_map(|entries| entries.iter())
            .filter_map(|(path, mode)| Some((PathBuf::from(path), mode.as_str()?.to_owned())))
            .collect(),
    }
}

pub fn save(root: &Path, session: &Session, revision: u64) -> std::io::Result<()> {
    let Some(path) = path(root) else {
        return Ok(());
    };
    save_at(root, &path, session, revision)
}

fn save_at(root: &Path, path: &Path, session: &Session, revision: u64) -> std::io::Result<()> {
    let mut saved = SAVED
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    if saved.get(path).is_some_and(|previous| *previous > revision) {
        return Ok(());
    }
    fs::create_dir_all(path.parent().expect("session parent"))?;
    let tabs = session
        .tabs
        .iter()
        .map(|tab| {
            serde_json::json!({
                "path": tab.path,
                "untitled": tab.untitled,
                "cursor": tab.cursor,
                "scroll_line": tab.scroll_line,
                "dirty_text": tab.dirty_text,
            })
        })
        .collect::<Vec<_>>();
    let data = serde_json::to_vec(&serde_json::json!({
        "root": root, "tabs": tabs, "active": session.active,
        "language_overrides": session.language_overrides,
    }))?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, data)?;
    fs::rename(temporary, &path)?;
    saved.insert(path.to_path_buf(), revision);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untitled_tabs_keep_even_empty_text_across_sessions() {
        let home =
            std::env::temp_dir().join(format!("gere-untitled-session-{}", std::process::id()));
        let file = home.join("session.json");
        let session = Session {
            tabs: vec![TabState {
                path: ".gere-untitled-3".into(),
                untitled: true,
                cursor: 0,
                scroll_line: 0,
                dirty_text: Some(String::new()),
            }],
            active: Some(0),
            language_overrides: HashMap::new(),
        };
        save_at(&home, &file, &session, revision()).unwrap();
        assert_eq!(load_at(&home, &file), session);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn session_round_trip_and_invalid_data() {
        let home = std::env::temp_dir().join(format!("gere-session-{}", std::process::id()));
        let root = home.join("project");
        let file = home.join("session.json");
        let original = Session {
            tabs: vec![TabState {
                path: PathBuf::from("src/main.rs"),
                untitled: false,
                cursor: 8,
                scroll_line: 31,
                dirty_text: Some("unsaved\ntext".into()),
            }],
            active: Some(0),
            language_overrides: HashMap::from([(PathBuf::from("www"), "js".into())]),
        };
        let newest = revision();
        save_at(&root, &file, &original, newest).unwrap();
        save_at(&root, &file, &Session::default(), newest.saturating_sub(1)).unwrap();
        assert_eq!(load_at(&root, &file), original);
        let closed = Session {
            tabs: Vec::new(),
            active: None,
            language_overrides: original.language_overrides.clone(),
        };
        save_at(&root, &file, &closed, revision()).unwrap();
        assert_eq!(load_at(&root, &file), closed);
        assert_eq!(load_at(&home, &file), Session::default());
        fs::write(&file, "broken json").unwrap();
        assert_eq!(load_at(&root, &file), Session::default());
        fs::remove_dir_all(home).unwrap();
    }
}
