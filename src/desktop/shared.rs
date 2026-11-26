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
    let sqlite_home = config
        .get("sqlite_home")
        .and_then(toml::Value::as_str)
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("CODEX_SQLITE_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| source.to_owned());
    let sqlite_home = std::path::absolute(sqlite_home)?;
    fs::create_dir_all(&sqlite_home)?;
    fs::create_dir_all(home)?;
    for name in LINKED {
        let target = source.join(name);
        let link = home.join(name);
        fs::create_dir_all(&target)?;
        match link.symlink_metadata() {
            Ok(_) => anyhow::ensure!(
                link.canonicalize()? == target.canonicalize()?,
                "shared desktop directory points to a different Codex home: {}",
                link.display()
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => link_directory(&target, &link)
                .with_context(|| format!("sharing {}", target.display()))?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(sqlite_home)
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
    let roots =
        ["sessions", "archived_sessions"].map(|name| (name, home.join(name).canonicalize()));
    let db = Connection::open_with_flags(db_path, OpenFlags::new().with_read_write())?;
    db.execute("PRAGMA busy_timeout=1000")?;
    let mut rows = db.prepare("SELECT id,rollout_path FROM threads")?;
    let mut changes = Vec::new();
    while rows.next()? == State::Row {
        let current = rows.read::<String, _>(1)?;
        let path = Path::new(&current);
        let Ok(relative) = path
            .strip_prefix(home)
            .or_else(|_| path.strip_prefix(&canonical_home))
        else {
            continue;
        };
        if let Some((target, tail)) = roots.iter().find_map(|(name, root)| {
            relative
                .strip_prefix(name)
                .ok()
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
}
