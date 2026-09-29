use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Read},
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::SystemTime,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub index: char,
    pub worktree: char,
}

fn run(root: &Path, program: &str, args: &[&str], path: Option<&Path>) -> Result<Output, String> {
    let mut command = Command::new(program);
    command.current_dir(root).args(args);
    if let Some(path) = path {
        command.arg("--").arg(path);
    }
    let output = command.output().map_err(|e| format!("{program}: {e}"))?;
    if output.status.success() || (program == "rg" && output.status.code() == Some(1)) {
        Ok(output)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

pub fn status(root: &Path) -> Result<Vec<Change>, String> {
    let output = run(
        root,
        "git",
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        None,
    )?;
    Ok(parse_status(&output.stdout))
}

fn parse_status(output: &[u8]) -> Vec<Change> {
    let mut records = output.split(|b| *b == 0).filter(|r| !r.is_empty());
    let mut changes = Vec::new();
    while let Some(record) = records.next() {
        if record.len() < 4 || record[2] != b' ' {
            continue;
        }
        let index = record[0] as char;
        let worktree = record[1] as char;
        let path = PathBuf::from(OsString::from_vec(record[3..].to_vec()));
        let original_path = if matches!(index, 'R' | 'C') || matches!(worktree, 'R' | 'C') {
            records
                .next()
                .map(|p| PathBuf::from(OsString::from_vec(p.to_vec())))
        } else {
            None
        };
        changes.push(Change {
            path,
            original_path,
            index,
            worktree,
        });
    }
    changes
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub is_dir: bool,
}

pub fn files(root: &Path) -> Vec<FileEntry> {
    let mut files: Vec<_> = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.file_name() != ".git" && e.depth() < 20)
        .filter_map(Result::ok)
        .filter(|e| e.depth() > 0 && (e.file_type().is_dir() || e.file_type().is_file()))
        .filter_map(|e| {
            e.path().strip_prefix(root).ok().map(|path| FileEntry {
                path: path.to_path_buf(),
                is_dir: e.file_type().is_dir(),
            })
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

pub fn file_index(root: &Path) -> Result<Vec<PathBuf>, String> {
    let output = run(
        root,
        "rg",
        &["--files", "--hidden", "-g", "!.git", "-0"],
        None,
    )?;
    Ok(output
        .stdout
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| PathBuf::from(OsString::from_vec(b.to_vec())))
        .collect())
}

#[derive(Clone)]
pub struct Match {
    pub path: PathBuf,
    pub line: usize,
    pub text: String,
    pub start: usize,
    pub end: usize,
}

pub fn branch(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        let output = Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()?;
        output.status.success().then(|| {
            format!(
                "{} (detached)",
                String::from_utf8_lossy(&output.stdout).trim()
            )
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStatus {
    pub ahead: usize,
    pub behind: usize,
    pub unpublished: bool,
}

pub fn sync_status(root: &Path) -> Option<SyncStatus> {
    let output = run(
        root,
        "git",
        &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
        None,
    );
    if let Ok(output) = output {
        let counts = String::from_utf8_lossy(&output.stdout);
        let mut parts = counts.split_whitespace();
        return Some(SyncStatus {
            ahead: parts.next()?.parse().ok()?,
            behind: parts.next()?.parse().ok()?,
            unpublished: false,
        });
    }
    // Only offer the first push for a local branch with a remote to publish to.
    run(root, "git", &["rev-parse", "--verify", "HEAD"], None).ok()?;
    run(root, "git", &["symbolic-ref", "--quiet", "HEAD"], None).ok()?;
    run(root, "git", &["remote", "get-url", "origin"], None).ok()?;
    Some(SyncStatus {
        ahead: 1,
        behind: 0,
        unpublished: true,
    })
}

pub fn push(root: &Path) -> Result<(), String> {
    if sync_status(root).is_some_and(|status| status.unpublished) {
        run(root, "git", &["push", "-u", "origin", "HEAD"], None).map(|_| ())
    } else {
        run(root, "git", &["push"], None).map(|_| ())
    }
}

pub fn sync(root: &Path) -> Result<(), String> {
    run(root, "git", &["pull", "--no-rebase"], None)?;
    push(root)
}

#[derive(Clone, Copy, Default)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub include_ignored: bool,
}

pub fn search(root: &Path, query: &str, options: SearchOptions) -> Result<Vec<Match>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let mut command = Command::new("rg");
    command
        .current_dir(root)
        .args(["--json", "--hidden", "-g", "!.git", "-m", "100"]);
    if !options.regex {
        command.arg("--fixed-strings");
    }
    if !options.case_sensitive {
        command.arg("--ignore-case");
    }
    if options.whole_word {
        command.arg("--word-regexp");
    }
    if options.include_ignored {
        command.arg("--no-ignore");
    }
    let mut child = command
        .args(["-e", query, "."])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("rg: {e}"))?;
    let mut stderr = child.stderr.take().expect("rg stderr piped");
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stdout = child.stdout.take().expect("rg stdout piped");
    let result = collect_matches(BufReader::new(stdout));
    if result.as_ref().is_ok_and(|(_, limited)| *limited) || result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| format!("rg: {e}"))?;
    let stderr = errors
        .join()
        .map_err(|_| "rg: stderr reader failed".to_string())?
        .map_err(|e| format!("rg: {e}"))?;
    let (matches, limited) = result.map_err(|e| format!("rg: {e}"))?;
    if !limited && !status.success() && status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&stderr).trim().to_owned());
    }
    Ok(matches)
}

fn collect_matches(reader: impl BufRead) -> std::io::Result<(Vec<Match>, bool)> {
    let mut matches = Vec::new();
    for line in reader.split(b'\n') {
        let line = line?;
        let Ok(event) = serde_json::from_slice::<serde_json::Value>(&line) else {
            continue;
        };
        if event["type"] != "match" {
            continue;
        }
        let Some(path) = event["data"]["path"]["text"].as_str() else {
            continue;
        };
        let text = event["data"]["lines"]["text"]
            .as_str()
            .unwrap_or("")
            .trim_end()
            .to_owned();
        let submatches = event["data"]["submatches"].as_array();
        // One result per occurrence, not just per line. rg reports byte offsets in UTF-8.
        let count = submatches.map_or(1, |items| items.len());
        for index in 0..count {
            let start = submatches
                .and_then(|items| items.get(index))
                .and_then(|item| item["start"].as_u64())
                .unwrap_or(0) as usize;
            let end = submatches
                .and_then(|items| items.get(index))
                .and_then(|item| item["end"].as_u64())
                .unwrap_or(start as u64) as usize;
            matches.push(Match {
                path: PathBuf::from(path),
                line: event["data"]["line_number"].as_u64().unwrap_or(0) as usize,
                text: text.clone(),
                start,
                end,
            });
            if matches.len() >= 300 {
                return Ok((matches, true));
            }
        }
    }
    Ok((matches, false))
}

pub fn diff(root: &Path, path: &Path) -> Result<String, String> {
    if status(root)?
        .iter()
        .any(|change| change.path == path && change.index == '?')
    {
        let text = read(root, path)?;
        return Ok(format!(
            "--- /dev/null\n+++ {}\n@@ -0,0 +1 @@\n{}",
            path.display(),
            text.lines()
                .map(|line| format!("+{line}\n"))
                .collect::<String>()
        ));
    }
    let output = run(
        root,
        "git",
        &["diff", "--no-ext-diff", "--no-color", "HEAD"],
        Some(path),
    )?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn head_file(root: &Path, change: &Change) -> Result<String, String> {
    if change.index == '?' || (change.index == 'A' && change.original_path.is_none()) {
        return Ok(String::new());
    }
    let path = change.original_path.as_deref().unwrap_or(&change.path);
    let output = Command::new("git")
        .current_dir(root)
        .arg("show")
        .arg(format!("HEAD:{}", path.to_string_lossy()))
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    if output.stdout.len() > 2_000_000 {
        return Err("Archivo original mayor a 2 MB".into());
    }
    String::from_utf8(output.stdout)
        .map(|text| text.replace("\r\n", "\n").replace('\r', "\n"))
        .map_err(|_| "Archivo original binario o no UTF-8".into())
}

pub fn change_counts(root: &Path, change: &Change) -> (usize, usize) {
    if change.index == '?' {
        return read(root, &change.path)
            .map(|text| (text.lines().count(), 0))
            .unwrap_or((0, 0));
    }
    let Ok(output) = run(
        root,
        "git",
        &["diff", "--numstat", "HEAD"],
        Some(&change.path),
    ) else {
        return (0, 0);
    };
    let mut added = 0;
    let mut removed = 0;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split('\t');
        added += fields.next().unwrap_or("").parse::<usize>().unwrap_or(0);
        removed += fields.next().unwrap_or("").parse::<usize>().unwrap_or(0);
    }
    (added, removed)
}

pub fn stage(root: &Path, path: &Path) -> Result<(), String> {
    run(root, "git", &["add"], Some(path)).map(|_| ())
}

pub fn unstage(root: &Path, change: &Change) -> Result<(), String> {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .args(["restore", "--staged", "--"])
        .arg(&change.path);
    if matches!(change.index, 'R' | 'C') {
        if let Some(original) = &change.original_path {
            command.arg(original);
        }
    }
    let output = command.output().map_err(|e| format!("git: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

pub fn discard(root: &Path, path: &Path) -> Result<(), String> {
    run(root, "git", &["restore", "--worktree"], Some(path)).map(|_| ())
}

pub fn discard_change(root: &Path, change: &Change) -> Result<(), String> {
    if change.index == '?' {
        fs::remove_file(root.join(&change.path)).map_err(|e| e.to_string())
    } else {
        discard(root, &change.path)
    }
}

pub fn commit(root: &Path, message: &str) -> Result<(), String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("Escribí un mensaje de commit".into());
    }
    if !status(root)?
        .iter()
        .any(|change| change.index != ' ' && change.index != '?')
    {
        return Err("No hay cambios staged para commitear".into());
    }
    run(root, "git", &["commit", "-m", message], None).map(|_| ())
}

pub fn stash(root: &Path) -> Result<(), String> {
    if status(root)?.is_empty() {
        return Err("No hay cambios para guardar en stash".into());
    }
    run(root, "git", &["stash", "push", "--include-untracked"], None).map(|_| ())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stash {
    pub reference: String,
    pub message: String,
}

pub fn stashes(root: &Path) -> Result<Vec<Stash>, String> {
    let output = run(
        root,
        "git",
        &["stash", "list", "--format=%gd%x00%s%x00"],
        None,
    )?;
    let mut result = Vec::new();
    for line in output.stdout.split(|byte| *byte == b'\n') {
        let mut parts = line.split(|byte| *byte == 0);
        if let (Some(reference), Some(message)) = (parts.next(), parts.next()) {
            if !reference.is_empty() {
                result.push(Stash {
                    reference: String::from_utf8_lossy(reference).into_owned(),
                    message: String::from_utf8_lossy(message).into_owned(),
                });
            }
        }
    }
    Ok(result)
}

pub fn apply_stash(root: &Path, reference: &str) -> Result<(), String> {
    if !stashes(root)?
        .iter()
        .any(|stash| stash.reference == reference)
    {
        return Err("El stash seleccionado ya no existe".into());
    }
    run(root, "git", &["stash", "apply", "--index", reference], None).map(|_| ())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStamp {
    len: u64,
    modified: SystemTime,
}

pub fn file_stamp(root: &Path, path: &Path) -> Result<FileStamp, String> {
    let metadata = fs::metadata(root.join(path)).map_err(|e| e.to_string())?;
    Ok(FileStamp {
        len: metadata.len(),
        modified: metadata.modified().map_err(|e| e.to_string())?,
    })
}

pub fn read(root: &Path, path: &Path) -> Result<String, String> {
    let bytes = fs::read(root.join(path)).map_err(|e| e.to_string())?;
    if bytes.len() > 2_000_000 {
        return Err("Archivo mayor a 2 MB".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| String::from("Archivo binario o no UTF-8"))?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

pub fn read_with_stamp(root: &Path, path: &Path) -> Result<(String, FileStamp), String> {
    let text = read(root, path)?;
    let stamp = file_stamp(root, path)?;
    Ok((text, stamp))
}

pub fn write(root: &Path, path: &Path, text: &str) -> Result<(), String> {
    fs::write(root.join(path), text).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_search_does_not_start_rg() {
        assert!(
            search(Path::new("/nonexistent"), "", SearchOptions::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn read_normalizes_crlf_and_cr_to_lf() {
        let root = std::env::temp_dir().join(format!(
            "reviewer-read-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file.txt"), b"uno\r\ndos\r\ntres\rfin").unwrap();
        assert_eq!(
            read(&root, Path::new("file.txt")).unwrap(),
            "uno\ndos\ntres\nfin"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_options_control_case_words_regex_and_ignored_files() {
        let root = std::env::temp_dir().join(format!(
            "reviewer-search-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file.txt"), "Cat catalog\ncat\n").unwrap();
        fs::write(root.join("hidden.txt"), "cat\n").unwrap();
        fs::write(root.join(".ignore"), "hidden.txt\n").unwrap();
        let basic = search(&root, "cat", SearchOptions::default()).unwrap();
        assert_eq!(basic.len(), 3);
        assert_eq!((basic[0].start, basic[0].end), (0, 3));
        assert_eq!((basic[1].start, basic[1].end), (4, 7));
        let exact = search(
            &root,
            "cat",
            SearchOptions {
                case_sensitive: true,
                whole_word: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(exact.len(), 1);
        let regex = search(
            &root,
            "c.t",
            SearchOptions {
                regex: true,
                whole_word: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(regex.len(), 2);
        let ignored = search(
            &root,
            "cat",
            SearchOptions {
                include_ignored: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(ignored.len(), 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_both_rename_columns_and_preserves_paths() {
        let changes =
            parse_status(b"R  new name\0old name\0 M modified\0 R other\0previous\0?? new\0");
        assert_eq!(changes.len(), 4);
        assert_eq!(changes[0].path, Path::new("new name"));
        assert_eq!(
            changes[0].original_path.as_deref(),
            Some(Path::new("old name"))
        );
        assert_eq!(changes[1].path, Path::new("modified"));
        assert_eq!(
            changes[2].original_path.as_deref(),
            Some(Path::new("previous"))
        );
        assert_eq!(changes[3].path, Path::new("new"));
    }

    #[test]
    fn untracked_file_diff_shows_added_lines_and_counts() {
        let root = std::env::temp_dir().join(format!(
            "gere-untracked-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        assert!(Command::new("git")
            .current_dir(&root)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success());
        fs::write(root.join("new.txt"), "first\nsecond\n").unwrap();
        let change = status(&root).unwrap().remove(0);
        assert_eq!(change_counts(&root, &change), (2, 0));
        assert_eq!(head_file(&root, &change).unwrap(), "");
        let patch = diff(&root, &change.path).unwrap();
        assert!(patch.contains("+first\n+second\n"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_stops_reading_at_300_matches() {
        let event = br#"{"type":"match","data":{"path":{"text":"file"},"lines":{"text":"hit"},"line_number":1}}"#;
        let mut input = Vec::new();
        for _ in 0..300 {
            input.extend_from_slice(event);
            input.push(b'\n');
        }
        input.extend_from_slice(b"invalid remainder");
        let (matches, limited) = collect_matches(BufReader::new(input.as_slice())).unwrap();
        assert!(limited);
        assert_eq!(matches.len(), 300);
    }

    #[test]
    fn search_preserves_byte_offsets_for_multiple_utf8_occurrences() {
        let event = r#"{"type":"match","data":{"path":{"text":"demo.txt"},"lines":{"text":"áá áá\n"},"line_number":7,"submatches":[{"start":0,"end":4},{"start":5,"end":9}]}}"#;
        let (matches, limited) = collect_matches(BufReader::new(event.as_bytes())).unwrap();
        assert!(!limited);
        assert_eq!(matches.len(), 2);
        assert_eq!(
            (matches[0].line, matches[0].start, matches[0].end),
            (7, 0, 4)
        );
        assert_eq!(
            (matches[1].line, matches[1].start, matches[1].end),
            (7, 5, 9)
        );
    }

    #[test]
    fn unstaging_rename_restores_both_index_paths() {
        let root = std::env::temp_dir().join(format!(
            "reviewer-rename-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        fs::write(root.join("old name"), "content\n").unwrap();
        git(&["add", "--", "old name"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "initial",
        ]);
        git(&["mv", "--", "old name", "new name"]);
        let rename = status(&root).unwrap().into_iter().next().unwrap();
        assert_eq!(rename.index, 'R');
        assert_eq!(rename.path, Path::new("new name"));
        assert_eq!(rename.original_path.as_deref(), Some(Path::new("old name")));
        unstage(&root, &rename).unwrap();
        let changes = status(&root).unwrap();
        assert!(changes
            .iter()
            .any(|c| c.path == Path::new("old name") && c.index == ' ' && c.worktree == 'D'));
        assert!(changes
            .iter()
            .any(|c| c.path == Path::new("new name") && c.index == '?' && c.worktree == '?'));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn commit_keeps_unstaged_work_and_stash_restores_untracked_files() {
        let root = std::env::temp_dir().join(format!(
            "reviewer-git-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        fs::write(root.join("tracked.txt"), "initial\n").unwrap();
        git(&["add", "tracked.txt"]);
        git(&["commit", "-qm", "initial"]);
        fs::write(root.join("tracked.txt"), "staged\n").unwrap();
        git(&["add", "tracked.txt"]);
        fs::write(root.join("tracked.txt"), "unstaged\n").unwrap();
        fs::write(root.join("new.txt"), "new\n").unwrap();
        let tracked = status(&root)
            .unwrap()
            .into_iter()
            .find(|c| c.path == Path::new("tracked.txt"))
            .unwrap();
        assert_eq!(head_file(&root, &tracked).unwrap(), "initial\n");

        commit(&root, "change").unwrap();
        assert_eq!(git(&["show", "HEAD:tracked.txt"]), b"staged\n");
        assert_eq!(fs::read(root.join("tracked.txt")).unwrap(), b"unstaged\n");
        assert!(root.join("new.txt").exists());

        stash(&root).unwrap();
        assert_eq!(fs::read(root.join("tracked.txt")).unwrap(), b"staged\n");
        assert!(!root.join("new.txt").exists());
        let saved = stashes(&root).unwrap();
        assert_eq!(saved.len(), 1);
        apply_stash(&root, &saved[0].reference).unwrap();
        assert_eq!(fs::read(root.join("tracked.txt")).unwrap(), b"unstaged\n");
        let new = status(&root)
            .unwrap()
            .into_iter()
            .find(|c| c.path == Path::new("new.txt"))
            .unwrap();
        discard_change(&root, &new).unwrap();
        assert!(!root.join("new.txt").exists());
        assert_eq!(stashes(&root).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sync_status_tracks_upstream_and_sync_pulls_then_pushes() {
        let root = std::env::temp_dir().join(format!(
            "reviewer-sync-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let local = root.join("local");
        let other = root.join("other");
        let git = |dir: &Path, args: &[&str]| {
            let output = Command::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&root, &["init", "-q", "--bare", "remote.git"]);
        git(&root, &["clone", "-q", "remote.git", "local"]);
        git(&local, &["config", "user.name", "Test"]);
        git(&local, &["config", "user.email", "test@example.com"]);
        assert_eq!(sync_status(&local), None);
        fs::write(local.join("first"), "first").unwrap();
        git(&local, &["add", "first"]);
        git(&local, &["commit", "-qm", "first"]);
        assert_eq!(
            sync_status(&local),
            Some(SyncStatus {
                ahead: 1,
                behind: 0,
                unpublished: true
            })
        );
        push(&local).unwrap();
        assert_eq!(sync_status(&local), Some(SyncStatus::default()));

        git(&root, &["clone", "-q", "remote.git", "other"]);
        git(&other, &["config", "user.name", "Test"]);
        git(&other, &["config", "user.email", "test@example.com"]);
        fs::write(other.join("remote-file"), "remote").unwrap();
        git(&other, &["add", "remote-file"]);
        git(&other, &["commit", "-qm", "remote"]);
        git(&other, &["push"]);
        git(&local, &["fetch", "-q"]);
        assert_eq!(
            sync_status(&local),
            Some(SyncStatus {
                ahead: 0,
                behind: 1,
                unpublished: false,
            })
        );

        fs::write(local.join("local-file"), "local").unwrap();
        git(&local, &["add", "local-file"]);
        git(&local, &["commit", "-qm", "local"]);
        assert_eq!(
            sync_status(&local),
            Some(SyncStatus {
                ahead: 1,
                behind: 1,
                unpublished: false,
            })
        );
        sync(&local).unwrap();
        assert_eq!(sync_status(&local), Some(SyncStatus::default()));
        assert!(local.join("remote-file").exists());
        git(&other, &["pull", "-q"]);
        assert!(other.join("local-file").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
