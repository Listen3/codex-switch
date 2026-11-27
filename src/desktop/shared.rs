use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sqlite::{Connection, OpenFlags, State};

use super::settings;

pub(super) const HOME: &str = "codex-home-shared";
const LINKED: [&str; 4] = [
    "sessions",
    "archived_sessions",
    "thread-writer-locks",
    "attachments",
];

pub(super) fn connect(source: &Path, home: &Path) -> Result<PathBuf> {
    let config = settings::read_config(&source.join("config.toml"))?;
    let sqlite_home = if let Some(raw) = config.get("sqlite_home").and_then(toml::Value::as_str) {
        resolve_sqlite_home(source, raw)?
    } else if let Some(raw) = std::env::var_os("CODEX_SQLITE_HOME").filter(|value| !value.is_empty())
    {
        std::path::absolute(PathBuf::from(raw))?
    } else {
        std::path::absolute(source)?
    };
    fs::create_dir_all(&sqlite_home)?;
    fs::create_dir_all(home)?;
    for name in LINKED {
        let target = source.join(name);
        let link = home.join(name);
        fs::create_dir_all(&target)?;
        match link.symlink_metadata() {
            Ok(_) => match link.canonicalize() {
                Ok(existing) => anyhow::ensure!(
                    existing == target.canonicalize()?,
                    "shared desktop directory points to a different Codex home: {}",
                    link.display()
                ),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    // Repair a dangling symlink/junction left behind if the
                    // target directory was recreated or relocated.
                    if remove_link_entry(&link)? {
                        link_directory(&target, &link)
                            .with_context(|| format!("sharing {}", target.display()))?;
                    } else {
                        return Err(error.into());
                    }
                }
                Err(error) => return Err(error.into()),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => link_directory(&target, &link)
                .with_context(|| format!("sharing {}", target.display()))?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(sqlite_home)
}

fn resolve_sqlite_home(source: &Path, raw: &str) -> Result<PathBuf> {
    let expanded = if raw == "~" {
        dirs::home_dir().context("finding home directory for sqlite_home")?
    } else if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        dirs::home_dir()
            .context("finding home directory for sqlite_home")?
            .join(rest)
    } else {
        let path = PathBuf::from(raw);
        if path.is_relative() {
            source.join(path)
        } else {
            path
        }
    };
    Ok(std::path::absolute(expanded)?)
}

#[cfg(windows)]
fn remove_link_entry(link: &Path) -> io::Result<bool> {
    if junction::exists(link).unwrap_or(false) {
        fs::remove_dir(link)?;
        return Ok(true);
    }
    if link
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        if fs::remove_dir(link).is_err() {
            fs::remove_file(link)?;
        }
        return Ok(true);
    }
    Ok(false)
}

#[cfg(unix)]
fn remove_link_entry(link: &Path) -> io::Result<bool> {
    if link
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        fs::remove_file(link)?;
        return Ok(true);
    }
    Ok(false)
}

/// Remove only the directory symlinks/junctions pointing to the shared Codex
/// home before archiving a deleted profile, so deleting the archive later with
/// tools that follow junctions (e.g. PowerShell 5.1) cannot touch shared chats.
pub(super) fn unlink_shared_dirs(home: &Path) -> Result<()> {
    for name in LINKED {
        let link = home.join(name);
        if link.symlink_metadata().is_ok() {
            let _ = remove_link_entry(&link)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn link_directory(source: &Path, link: &Path) -> io::Result<()> {
    junction::create(source, link)
}

#[cfg(unix)]
fn link_directory(source: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, link)
}

/// Codex can record the path through an instance's directory link. Before
/// moving that instance, anchor those paths to the physical shared directory.
pub(super) fn prepare_profile_move(home: &Path) -> Result<()> {
    let config = settings::read_config(&home.join("config.toml"))?;
    let Some(sqlite_home) = config.get("sqlite_home").and_then(toml::Value::as_str) else {
        return Ok(());
    };
    let db_path = Path::new(sqlite_home).join("state_5.sqlite");
    if !db_path.is_file() {
        return Ok(());
    }
    let canonical_home = home.canonicalize()?;
    let roots = ["sessions", "archived_sessions"].map(|name| {
        (
            name,
            home.join(name).canonicalize().map(normalize_db_path),
        )
    });
    let db = Connection::open_with_flags(db_path, OpenFlags::new().with_read_write())?;
    db.execute("PRAGMA busy_timeout=5000")?;
    if !has_table(&db, "threads")? {
        return Ok(());
    }
    let mut rows = db.prepare("SELECT id,rollout_path FROM threads")?;
    let mut changes = Vec::new();
    while rows.next()? == State::Row {
        let current = rows.read::<String, _>(1)?;
        let path = Path::new(&current);
        let Some(relative) = strip_path_prefix(path, home)
            .or_else(|| strip_path_prefix(path, &canonical_home))
        else {
            continue;
        };
        if let Some((target, tail)) = roots.iter().find_map(|(name, root)| {
            strip_path_prefix(&relative, Path::new(name))
                .zip(root.as_ref().ok())
                .map(|(tail, root)| (root, tail))
        }) {
            let canonical_path = target.join(tail);
            changes.push((rows.read::<String, _>(0)?, current, canonical_path));
        }
    }
    drop(rows);
    if changes.is_empty() {
        return Ok(());
    }
    let mut update =
        db.prepare("UPDATE threads SET rollout_path=? WHERE id=? AND rollout_path=?")?;
    db.execute("BEGIN IMMEDIATE")?;
    for (id, current, path) in changes {
        update.bind((
            1,
            path.to_str().context("encoding shared conversation path")?,
        ))?;
        update.bind((2, id.as_str()))?;
        update.bind((3, current.as_str()))?;
        update.next()?;
        update.reset()?;
    }
    db.execute("COMMIT")?;
    Ok(())
}

fn has_table(db: &Connection, name: &str) -> Result<bool> {
    let mut stmt = db.prepare("SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?")?;
    stmt.bind((1, name))?;
    Ok(stmt.next()? == State::Row)
}

#[cfg(windows)]
fn normalize_db_path(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
}

#[cfg(not(windows))]
fn normalize_db_path(path: PathBuf) -> PathBuf {
    path
}

#[cfg(windows)]
fn strip_path_prefix(path: &Path, prefix: &Path) -> Option<PathBuf> {
    let path = normalize_db_path(path.to_path_buf());
    let prefix = normalize_db_path(prefix.to_path_buf());
    let mut path_components = path.components();
    for expected in prefix.components() {
        let actual = path_components.next()?;
        if !actual
            .as_os_str()
            .eq_ignore_ascii_case(expected.as_os_str())
        {
            return None;
        }
    }
    Some(path_components.as_path().to_path_buf())
}

#[cfg(not(windows))]
fn strip_path_prefix(path: &Path, prefix: &Path) -> Option<PathBuf> {
    path.strip_prefix(prefix).ok().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_paths_remain_readable_after_renaming_an_account() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("default");
        let profile = root.path().join("account");
        let home = profile.join(HOME);
        fs::create_dir_all(&source).unwrap();
        let sqlite_home = connect(&source, &home).unwrap();
        fs::write(home.join("sessions/chat.jsonl"), "shared chat").unwrap();
        assert_eq!(
            fs::read_to_string(source.join("sessions/chat.jsonl")).unwrap(),
            "shared chat"
        );
        let mut config = toml::Table::new();
        config.insert("sqlite_home".into(), sqlite_home.to_str().unwrap().into());
        fs::write(home.join("config.toml"), config.to_string()).unwrap();
        let db = Connection::open(sqlite_home.join("state_5.sqlite")).unwrap();
        db.execute("CREATE TABLE threads(id TEXT PRIMARY KEY,rollout_path TEXT NOT NULL)")
            .unwrap();
        let mut row = db.prepare("INSERT INTO threads VALUES ('chat',?)").unwrap();
        let path = home.join("sessions/chat.jsonl");
        row.bind((1, path.to_str().unwrap())).unwrap();
        row.next().unwrap();
        drop(row);
        prepare_profile_move(&home).unwrap();
        let moved = root.path().join("new-account");
        fs::rename(profile, &moved).unwrap();
        fs::remove_dir_all(moved).unwrap();
        let mut row = db.prepare("SELECT rollout_path FROM threads").unwrap();
        assert_eq!(row.next().unwrap(), State::Row);
        let path = row.read::<String, _>(0).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "shared chat");
    }

    #[test]
    fn prepare_profile_move_skips_state_db_without_threads_table() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("default");
        let home = root.path().join("account").join(HOME);
        fs::create_dir_all(&source).unwrap();
        let sqlite_home = connect(&source, &home).unwrap();
        let mut config = toml::Table::new();
        config.insert("sqlite_home".into(), sqlite_home.to_str().unwrap().into());
        fs::write(home.join("config.toml"), config.to_string()).unwrap();
        Connection::open(sqlite_home.join("state_5.sqlite"))
            .unwrap()
            .execute("CREATE TABLE other(id TEXT PRIMARY KEY)")
            .unwrap();
        prepare_profile_move(&home).unwrap();
    }

    #[test]
    fn unlink_shared_dirs_removes_links_without_touching_shared_targets() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("default");
        let home = root.path().join("account").join(HOME);
        fs::create_dir_all(&source).unwrap();
        connect(&source, &home).unwrap();
        fs::write(source.join("sessions/keep.jsonl"), "shared").unwrap();

        unlink_shared_dirs(&home).unwrap();
        for name in LINKED {
            assert!(home.join(name).symlink_metadata().is_err());
        }
        assert_eq!(
            fs::read_to_string(source.join("sessions/keep.jsonl")).unwrap(),
            "shared"
        );
    }

    #[test]
    fn connect_resolves_relative_sqlite_home_against_source_and_repairs_dangling_link() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("default");
        let home = root.path().join("account").join(HOME);
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("config.toml"), "sqlite_home = \"nested-db\"\n").unwrap();

        let sqlite_home = connect(&source, &home).unwrap();
        assert_eq!(sqlite_home, std::path::absolute(source.join("nested-db")).unwrap());
        assert!(sqlite_home.is_dir());

        #[cfg(unix)]
        {
            let broken_target = root.path().join("gone");
            let sessions_link = home.join("sessions");
            fs::remove_file(&sessions_link).unwrap();
            std::os::unix::fs::symlink(&broken_target, &sessions_link).unwrap();
            connect(&source, &home).unwrap();
            assert_eq!(
                sessions_link.canonicalize().unwrap(),
                source.join("sessions").canonicalize().unwrap()
            );
        }
    }
}
