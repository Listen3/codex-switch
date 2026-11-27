use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlite::{Connection, OpenFlags, State};

use super::settings;
use crate::auth;

pub(super) const INHERITED: &str = ".codex-switch-local-workspace-v6";
const STATE_DB: &str = "state_5.sqlite";
const LOCAL_PROJECTS: &str = "local-projects";
const PROJECT_ORDER: &str = "project-order";
const PROJECT_IDS: &str = "app-server-project-id-by-legacy-project-id-by-host";
const MIGRATION: &str = "app-server-projects-migration-by-host";
const THREAD_PROJECTS: &str = "thread-project-assignments";
const THREAD_HOSTS: &str = "thread-project-membership-host-ids";
const PROJECTLESS_THREADS: &str = "projectless-thread-ids";
const PINNED_THREADS: &str = "pinned-thread-ids";
const MIGRATED_PINS: &str = "app-server-migrated-pinned-thread-ids-by-host";
// Reserved ID of Codex's built-in Pinned section.
const PINNED_SECTION: &str = "01984de2-8f74-7c91-a3b2-5c5e937cf318";
const RENDERER_OWNER: &str = "electron-renderer-content-owner";
const SCOPED: &str = "electron-owner-value:";
const WORKSPACE_ROOTS: [&str; 2] = ["electron-saved-workspace-roots", "active-workspace-roots"];

fn open_db(path: &Path, flags: OpenFlags) -> Result<Option<Connection>> {
    if !path.is_file() {
        return Ok(None);
    }
    let db = Connection::open_with_flags(path, flags)
        .with_context(|| format!("opening {}", path.display()))?;
    db.execute("PRAGMA busy_timeout=5000")?;
    Ok(Some(db))
}

fn has_table(db: &Connection, name: &str) -> Result<bool> {
    let mut stmt = db.prepare("SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?")?;
    stmt.bind((1, name))?;
    Ok(stmt.next()? == State::Row)
}

fn read_native_projects(source: &Connection) -> Result<Map<String, Value>> {
    let mut found = Map::new();
    if !has_table(source, "projects")? || !has_table(source, "project_roots")? {
        return Ok(found);
    }
    let mut projects = source
        .prepare("SELECT id,name,created_at_ms,updated_at_ms FROM projects ORDER BY position")?;
    let mut roots =
        source.prepare("SELECT path FROM project_roots WHERE project_id=? ORDER BY position")?;
    while projects.next()? == State::Row {
        let id = projects.read::<String, _>(0)?;
        roots.bind((1, id.as_str()))?;
        let mut paths = Vec::new();
        while roots.next()? == State::Row {
            let path = roots.read::<String, _>(0)?;
            if Path::new(&path).is_absolute() {
                paths.push(path);
            }
        }
        roots.reset()?;
        if !paths.is_empty() {
            found.insert(
                id.clone(),
                json!({
                    "id":id,
                    "name":projects.read::<String,_>(1)?,
                    "rootPaths":paths,
                    "createdAt":projects.read::<i64,_>(2)?,
                    "updatedAt":projects.read::<i64,_>(3)?,
                }),
            );
        }
    }
    Ok(found)
}

pub(super) fn inherit_shared_layout(
    source: &Path,
    home: &Path,
    sqlite_home: &Path,
    credentials: &Value,
) -> Result<()> {
    let mut native = Map::new();
    let mut pins = Vec::new();
    let mut custom_section_threads = Vec::new();
    if let Some(db) = open_db(
        &sqlite_home.join(STATE_DB),
        OpenFlags::new().with_read_only(),
    )? {
        native = read_native_projects(&db)?;
        pins = read_section_threads(&db, true)?;
        custom_section_threads = read_section_threads(&db, false)?;
    }
    let mut rollouts = BTreeMap::new();
    for dir in ["sessions", "archived_sessions"] {
        collect_rollouts(source, Path::new(dir), &mut rollouts)?;
    }
    let paths = rollouts
        .iter()
        .filter(|(id, _)| has_complete_history(id, &rollouts))
        .map(|(id, rollout)| (id.clone(), home.join(&rollout.path)))
        .collect();
    copy_project_layout(
        source,
        home,
        renderer_owner(credentials).as_deref(),
        native,
        &paths,
        &pins,
        &custom_section_threads,
    )?;
    Ok(())
}

fn copy_project_layout(
    source: &Path,
    home: &Path,
    owner: Option<&str>,
    mut native: Map<String, Value>,
    threads: &BTreeMap<String, PathBuf>,
    native_pins: &[Value],
    custom_section_threads: &[Value],
) -> Result<Map<String, Value>> {
    let source_db = settings::settings_db(source, OpenFlags::new().with_read_only())?;
    let mut state = match source_db {
        Some(db) => read_layout(&db)?,
        None => settings::read_global_json(source)?,
    };
    let mut projects = state
        .remove(LOCAL_PROJECTS)
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    projects.retain(|_, entry| {
        entry
            .get("rootPaths")
            .and_then(Value::as_array)
            .is_some_and(|roots| !roots.is_empty() && roots.iter().all(local_path))
    });
    for project in projects.values_mut().filter_map(Value::as_object_mut) {
        project.retain(|key, _| {
            matches!(
                key.as_str(),
                "id" | "name" | "rootPaths" | "createdAt" | "updatedAt"
            )
        });
    }

    // The renderer's project list comes from its legacy cache, even when the
    // app server already has native projects. Match those two records using
    // the migration map for this Codex home to avoid re-importing duplicates.
    let mut aliases = Map::new();
    if let Some(ids) = state
        .get(PROJECT_IDS)
        .and_then(|v| v.get(format!("local:{}", source.display())))
        .and_then(Value::as_object)
    {
        for (legacy, id) in ids {
            if projects.contains_key(legacy)
                && id.as_str().is_some_and(|id| native.remove(id).is_some())
            {
                aliases.insert(legacy.clone(), id.clone());
            }
        }
    }
    for (id, project) in native {
        projects.entry(id).or_insert(project);
    }
    let order: Vec<_> = state
        .get(PROJECT_ORDER)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|v| v.as_str().is_some_and(|id| projects.contains_key(id)))
        .cloned()
        .collect();
    let mut assignments = Map::new();
    if let Some(entries) = state.get(THREAD_PROJECTS).and_then(Value::as_object) {
        for (id, entry) in entries {
            if threads.contains_key(id)
                && entry.get("projectKind").and_then(Value::as_str) == Some("local")
                && let Some(project) = entry.get("projectId").and_then(Value::as_str)
                && projects.contains_key(project)
            {
                assignments.insert(
                    id.clone(),
                    json!({"projectId":project,"projectKind":"local"}),
                );
            }
        }
    }

    let source_owner = state.get(RENDERER_OWNER).and_then(Value::as_str);
    let old_host = format!("local:{}", source.display());
    let migrated_pins = state
        .get(&scoped_key(MIGRATED_PINS, source_owner))
        .and_then(|hosts| hosts.get(&old_host))
        .and_then(Value::as_array);
    let mut pins = native_pins.to_vec();
    if let Some(legacy) = state.get(PINNED_THREADS).and_then(Value::as_array) {
        for id in legacy {
            if !pins.contains(id) && !migrated_pins.is_some_and(|items| items.contains(id)) {
                pins.push(id.clone());
            }
        }
    }
    let sessions = home.join("sessions");
    pins.retain(|id| {
        !custom_section_threads.contains(id)
            && id
                .as_str()
                .and_then(|id| threads.get(id))
                .is_some_and(|path| path.starts_with(&sessions))
    });
    let host = format!("local:{}", home.display());
    let merge = |target: &mut Map<String, Value>| {
        if !pins.is_empty() {
            merge_order(target.entry(PINNED_THREADS).or_insert(json!([])), &pins);
        }
        if !projects.is_empty() {
            merge_entries(target.entry(LOCAL_PROJECTS).or_insert(json!({})), &projects);
            merge_order(target.entry(PROJECT_ORDER).or_insert(json!([])), &order);
        }
        if !aliases.is_empty()
            && let Some(hosts) = target
                .entry(PROJECT_IDS)
                .or_insert(json!({}))
                .as_object_mut()
        {
            merge_entries(hosts.entry(&host).or_insert(json!({})), &aliases);
        }
        let mut pending = Vec::new();
        if !assignments.is_empty() {
            let projectless = target
                .get(PROJECTLESS_THREADS)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(entries) = target
                .entry(THREAD_PROJECTS)
                .or_insert(json!({}))
                .as_object_mut()
            {
                for (id, assignment) in &assignments {
                    if !entries.contains_key(id) && !projectless.contains(&json!(id)) {
                        entries.insert(id.clone(), assignment.clone());
                        pending.push(id);
                    }
                }
            }
            if let Some(hosts) = target
                .entry(THREAD_HOSTS)
                .or_insert(json!({}))
                .as_object_mut()
            {
                for id in &pending {
                    hosts.entry(id.as_str()).or_insert(json!("local"));
                }
            }
        }
        for key in WORKSPACE_ROOTS {
            let value = state
                .get(RENDERER_OWNER)
                .and_then(Value::as_str)
                .and_then(|owner| state.get(&format!("{SCOPED}{key}:{owner}")))
                .or_else(|| state.get(key));
            let mut roots: Vec<_> = value
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|root| local_path(root))
                .cloned()
                .collect();
            if roots.is_empty() && key == WORKSPACE_ROOTS[0] {
                // The folder picker also needs usable roots just after the
                // source desktop changes account and has no saved list yet.
                for project in projects.values() {
                    if let Some(paths) = project.get("rootPaths").and_then(Value::as_array) {
                        for path in paths {
                            if !roots.contains(path) {
                                roots.push(path.clone());
                            }
                        }
                    }
                }
            }
            if !roots.is_empty() {
                let key = scoped_key(key, owner);
                merge_order(target.entry(key).or_insert(json!([])), &roots);
            }
        }
        if !pending.is_empty()
            && let Some(hosts) = target
                .entry(scoped_key(MIGRATION, owner))
                .or_insert(json!({}))
                .as_object_mut()
            && let Some(migration) = hosts
                .entry(&host)
                .or_insert(json!({"version":1}))
                .as_object_mut()
        {
            migration.insert("threadAssignmentsMigrated".into(), json!(false));
            let pending: Vec<_> = pending.into_iter().map(|id| json!(id)).collect();
            merge_order(
                migration
                    .entry("pendingThreadAssignmentIds")
                    .or_insert(json!([])),
                &pending,
            );
        }
    };
    let mut target = settings::read_global_json(home)?;
    let before = target.clone();
    merge(&mut target);
    if target != before {
        auth::atomic_write_private(
            &home.join(settings::GLOBAL_JSON),
            &serde_json::to_vec(&target)?,
        )?;
    }
    if let Some(db) = settings::settings_db(home, OpenFlags::new().with_read_write())? {
        db.execute("BEGIN IMMEDIATE")?;
        let mut target = read_layout(&db)?;
        let before = target.clone();
        merge(&mut target);
        let mut write = db.prepare(
            "INSERT INTO settings (scope,key,value) VALUES ('global',?,?)
             ON CONFLICT(scope,key) DO UPDATE SET value=excluded.value",
        )?;
        for (key, value) in &target {
            if before.get(key) != Some(value) {
                write.bind((1, key.as_str()))?;
                write.bind((2, value.to_string().as_str()))?;
                write.next()?;
                write.reset()?;
            }
        }
        db.execute("COMMIT")?;
        return Ok(target);
    }
    Ok(target)
}

fn scoped_key(key: &str, owner: Option<&str>) -> String {
    owner.map_or_else(|| key.to_owned(), |owner| format!("{SCOPED}{key}:{owner}"))
}

fn renderer_owner(credentials: &Value) -> Option<String> {
    let account = crate::jwt::parse_account_info(credentials);
    account
        .user_id
        .zip(account.account_id)
        .map(|(user, account)| hex::encode(Sha256::digest(json!([user, account]).to_string())))
}

fn read_section_threads(db: &Connection, pinned: bool) -> Result<Vec<Value>> {
    let mut pins = Vec::new();
    if has_table(db, "thread_sections")? && has_table(db, "threads")? {
        let operator = if pinned { "=" } else { "<>" };
        let mut query = db.prepare(format!(
            "SELECT id FROM threads WHERE archived=0 AND thread_section_id{operator}? ORDER BY section_position, id"
        ))?;
        query.bind((1, PINNED_SECTION))?;
        while query.next()? == State::Row {
            pins.push(json!(query.read::<String, _>(0)?));
        }
    }
    Ok(pins)
}

fn read_layout(db: &Connection) -> Result<Map<String, Value>> {
    let mut rows = db.prepare("SELECT key,value FROM settings WHERE scope='global'")?;
    let mut state = Map::new();
    while rows.next()? == State::Row {
        let key = rows.read::<String, _>(0)?;
        let base = key
            .strip_prefix(SCOPED)
            .and_then(|key| key.rsplit_once(':'))
            .map_or(key.as_str(), |(key, _)| key);
        if [
            LOCAL_PROJECTS,
            PROJECT_ORDER,
            PROJECT_IDS,
            MIGRATION,
            THREAD_PROJECTS,
            THREAD_HOSTS,
            PROJECTLESS_THREADS,
            RENDERER_OWNER,
            PINNED_THREADS,
            MIGRATED_PINS,
        ]
        .contains(&base)
            || WORKSPACE_ROOTS.contains(&base)
        {
            if let Ok(raw) = rows.read::<String, _>(1)
                && let Ok(value) = serde_json::from_str(&raw)
            {
                state.insert(key, value);
            }
        }
    }
    Ok(state)
}

fn local_path(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|path| Path::new(path).is_absolute())
}

fn merge_entries(target: &mut Value, values: &Map<String, Value>) {
    if let Some(target) = target.as_object_mut() {
        for (key, value) in values {
            target.entry(key).or_insert_with(|| value.clone());
        }
    }
}

fn merge_order(target: &mut Value, values: &[Value]) {
    if let Some(target) = target.as_array_mut() {
        for value in values {
            if !target.contains(value) {
                target.push(value.clone());
            }
        }
    }
}

struct Rollout {
    path: PathBuf,
    base_id: Option<String>,
}

fn collect_rollouts(
    source: &Path,
    dir: &Path,
    candidates: &mut BTreeMap<String, Rollout>,
) -> Result<()> {
    let entries = match fs::read_dir(source.join(dir)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", source.join(dir).display()));
        }
    };
    for entry in entries {
        let entry = entry?;
        let path = dir.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_rollouts(source, &path, candidates)?;
        } else if kind.is_file()
            && let Some(id) = rollout_id(&path)
        {
            let plain = path.extension().is_some_and(|ext| ext == "jsonl");
            if !plain && source.join(path.with_extension("")).is_file() {
                // The plain rollout owns the current metadata when both forms exist.
                continue;
            }
            let line = match rollout_header(&source.join(&path)) {
                Ok(Some(line)) => line,
                Ok(None) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    tracing::warn!(%error, "skipping unreadable local chat metadata");
                    continue;
                }
            };
            let Ok(meta) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let payload = &meta["payload"];
            if meta["type"] != "session_meta"
                || payload["id"] != id
                || !matches!(
                    payload.get("account_scoped"),
                    None | Some(Value::Null | Value::Bool(false))
                )
            {
                continue;
            }
            let id = id.to_owned();
            let candidate = Rollout {
                path,
                base_id: payload
                    .pointer("/history_base/thread_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            };
            if plain {
                candidates.insert(id, candidate);
            } else {
                candidates.entry(id).or_insert(candidate);
            }
        }
    }
    Ok(())
}

fn has_complete_history<'a>(mut id: &'a str, candidates: &'a BTreeMap<String, Rollout>) -> bool {
    for _ in candidates {
        match candidates.get(id) {
            Some(Rollout {
                base_id: Some(base),
                ..
            }) => id = base,
            Some(Rollout { base_id: None, .. }) => return true,
            None => return false,
        }
    }
    false
}

const MAX_ROLLOUT_HEADER_BYTES: usize = 256 * 1024;

fn rollout_header(path: &Path) -> io::Result<Option<String>> {
    let file = File::open(path)?;
    if path.extension().is_some_and(|extension| extension == "zst") {
        read_bounded_first_line(zstd::Decoder::new(file)?)
    } else {
        read_bounded_first_line(file)
    }
}

fn read_bounded_first_line(reader: impl io::Read) -> io::Result<Option<String>> {
    let mut limited = BufReader::new(reader.take((MAX_ROLLOUT_HEADER_BYTES + 1) as u64));
    let mut buf = Vec::new();
    let read = limited.read_until(b'\n', &mut buf)?;
    if read == 0 {
        return Ok(None);
    }
    if buf.len() > MAX_ROLLOUT_HEADER_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "rollout header line exceeds size limit",
        ));
    }
    while matches!(buf.last(), Some(b'\n' | b'\r')) {
        buf.pop();
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn rollout_id(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?.strip_prefix("rollout-")?;
    let stem = name
        .strip_suffix(".zst")
        .unwrap_or(name)
        .strip_suffix(".jsonl")?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    id.bytes()
        .enumerate()
        .all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
        .then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shared_layout_only_inherits_metadata_for_portable_history() {
        let source = tempfile::tempdir().unwrap();
        let source = source.path();
        let local = "00000000-0000-4000-8000-000000000001";
        let private = "00000000-0000-4000-8000-000000000002";
        let incomplete = "00000000-0000-4000-8000-000000000003";
        let followup = "00000000-0000-4000-8000-000000000004";
        let stale = "00000000-0000-4000-8000-000000000005";
        let rollout = |id: &str| format!("sessions/rollout-2026-11-01T01-02-03-{id}.jsonl");
        fs::create_dir(source.join("sessions")).unwrap();
        for (id, scoped, base, compressed) in [
            (local, false, None, true),
            (private, true, None, true),
            (incomplete, false, Some(private), false),
            (followup, false, Some(local), false),
            (stale, true, None, false),
        ] {
            let record = json!({"type":"session_meta","payload":{
                "id":id, "account_scoped": scoped,
                "history_base": base.map(|id| json!({"thread_id":id}))
            }})
            .to_string();
            let path = source.join(rollout(id));
            if compressed {
                let encoded = zstd::stream::encode_all(record.as_bytes(), 1).unwrap();
                fs::write(path.with_extension("jsonl.zst"), encoded).unwrap();
            } else {
                fs::write(path, record).unwrap();
            }
        }
        let stale_header = json!({"type":"session_meta","payload":{"id":stale}}).to_string();
        fs::write(
            source.join(rollout(stale)).with_extension("jsonl.zst"),
            zstd::stream::encode_all(stale_header.as_bytes(), 1).unwrap(),
        )
        .unwrap();
        let damaged = "00000000-0000-4000-8000-000000000006";
        fs::write(
            source.join(rollout(damaged)).with_extension("jsonl.zst"),
            b"not a zstd stream",
        )
        .unwrap();
        let mut candidates = BTreeMap::new();
        collect_rollouts(source, Path::new("sessions"), &mut candidates).unwrap();
        assert_eq!(
            candidates
                .keys()
                .filter(|id| has_complete_history(id, &candidates))
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [local, followup]
        );
        assert!(!candidates.contains_key(private));
        assert!(!candidates.contains_key(stale));
        assert!(!candidates.contains_key(damaged));
    }

    #[test]
    fn native_project_also_has_a_sidebar_entry_in_the_target_account() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

        let source = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let source = source.path();
        let home = home.path();
        let db_name = ".codex-global-state.sqlite";
        let schema =
            "CREATE TABLE settings(scope TEXT, key TEXT, value TEXT, PRIMARY KEY(scope,key))";
        let source_db = Connection::open(source.join(db_name)).unwrap();
        source_db.execute(schema).unwrap();
        let mut seed = source_db
            .prepare("INSERT INTO settings VALUES ('global',?,?)")
            .unwrap();
        let source_host = format!("local:{}", source.display());
        let home_host = format!("local:{}", home.display());
        let saved_root = source.join("saved-root");
        for (key, value) in [
            (
                LOCAL_PROJECTS,
                json!({"legacy":{"id":"legacy","name":"Source name","rootPaths":[source],"chatGptBacking":{"private":true}}}),
            ),
            (
                PROJECT_IDS,
                json!({(source_host.as_str()):{"legacy":"native"},"ssh:remote":{"legacy":"private"}}),
            ),
            (PROJECT_ORDER, json!(["legacy"])),
            (
                THREAD_PROJECTS,
                json!({"local":{"projectId":"legacy","projectKind":"local"},"private":{"projectId":"legacy","projectKind":"local"}}),
            ),
            (RENDERER_OWNER, json!("source-owner")),
            (
                PINNED_THREADS,
                json!(["local", "private", "archived", "removed"]),
            ),
            (
                "electron-owner-value:app-server-migrated-pinned-thread-ids-by-host:source-owner",
                json!({(source_host.as_str()): ["removed"]}),
            ),
            (
                "electron-owner-value:electron-saved-workspace-roots:source-owner",
                json!([saved_root, "ssh:remote/path"]),
            ),
        ] {
            seed.bind((1, key)).unwrap();
            seed.bind((2, value.to_string().as_str())).unwrap();
            seed.next().unwrap();
            seed.reset().unwrap();
        }
        let target_db = Connection::open(home.join(db_name)).unwrap();
        target_db.execute(schema).unwrap();
        let payload = URL_SAFE_NO_PAD.encode(
            json!({"https://api.openai.com/auth":{
                "chatgpt_user_id":"fixture-user", "chatgpt_account_id":"fixture-account"
            }})
            .to_string(),
        );
        let credentials = json!({"tokens":{"id_token":format!("test.{payload}.signature")}});
        let native = Map::from_iter([(
            "native".to_owned(),
            json!({"id":"native","rootPaths":[source]}),
        )]);
        let owner = "920c567e69839fb9fc73119dac1cf71863fbcea775103072d964bd730f2d2dc5";
        assert_eq!(renderer_owner(&credentials).as_deref(), Some(owner));
        let copied_path = home.join("sessions/copied.jsonl.zst");
        let paths = BTreeMap::from([
            ("local".to_owned(), copied_path.clone()),
            ("native-only".to_owned(), home.join("sessions/native.jsonl")),
            (
                "archived".to_owned(),
                home.join("archived_sessions/archive.jsonl"),
            ),
            ("removed".to_owned(), home.join("sessions/removed.jsonl")),
        ]);
        let target = copy_project_layout(
            source,
            home,
            Some(owner),
            native,
            &paths,
            &[json!("native-only"), json!("local")],
            &[],
        )
        .unwrap();
        assert_eq!(target[PINNED_THREADS], json!(["native-only", "local"]));
        assert_eq!(
            target[LOCAL_PROJECTS],
            json!({"legacy":{"id":"legacy","name":"Source name","rootPaths":[source]}})
        );
        assert_eq!(
            target[PROJECT_IDS],
            json!({(home_host.as_str()):{"legacy":"native"}})
        );
        assert_eq!(
            target[THREAD_PROJECTS],
            json!({"local":{"projectId":"legacy","projectKind":"local"}})
        );
        assert_eq!(
            target[&scoped_key(WORKSPACE_ROOTS[0], Some(owner))],
            json!([saved_root])
        );
        assert_eq!(
            target[&scoped_key(MIGRATION, Some(owner))][&home_host]["pendingThreadAssignmentIds"],
            json!(["local"])
        );
        assert_eq!(settings::read_global_json(home).unwrap(), target);

        source_db.execute(
            "UPDATE settings SET value = '\"new-source-owner\"' WHERE key = 'electron-renderer-content-owner'",
        ).unwrap();
        let fresh = tempfile::tempdir().unwrap();
        copy_project_layout(
            source,
            fresh.path(),
            Some(owner),
            Map::new(),
            &BTreeMap::new(),
            &[],
            &[],
        )
        .unwrap();
        let fallback = settings::read_global_json(fresh.path()).unwrap();
        assert_eq!(
            fallback[&scoped_key(WORKSPACE_ROOTS[0], Some(owner))],
            json!([source])
        );
    }

    #[test]
    fn read_layout_tolerates_null_and_malformed_json_and_rollout_header_caps_size() {
        let dir = tempfile::tempdir().unwrap();
        let db = Connection::open(dir.path().join(".codex-global-state.sqlite")).unwrap();
        db.execute("CREATE TABLE settings(scope TEXT, key TEXT, value TEXT)")
            .unwrap();
        db.execute(
            "INSERT INTO settings VALUES \
             ('global', 'pinned-thread-ids', NULL), \
             ('global', 'project-order', '{not-json'), \
             ('global', 'local-projects', '{\"p1\":{\"id\":\"p1\"}}')",
        )
        .unwrap();
        let state = read_layout(&db).unwrap();
        assert!(!state.contains_key(PINNED_THREADS));
        assert!(!state.contains_key(PROJECT_ORDER));
        assert!(state.contains_key(LOCAL_PROJECTS));

        let huge_path = dir.path().join("huge.jsonl");
        fs::write(&huge_path, vec![b'a'; MAX_ROLLOUT_HEADER_BYTES + 16]).unwrap();
        let err = rollout_header(&huge_path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
