use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlite::{Connection, OpenFlags, State};

use super::settings;
use crate::auth;

pub(super) const INHERITED: &str = ".codex-switch-local-workspace-v6";
const STATE_DB: &str = "state_5.sqlite";
const STATE_DBS: [&str; 2] = [STATE_DB, "sqlite/state_5.sqlite"];
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
const SESSION_INDEX: &str = "session_index.jsonl";

pub(super) fn inherit(
    source: &Path,
    home: &Path,
    desktop: &Path,
    credentials: &Value,
) -> Result<()> {
    let mut rollouts = BTreeMap::new();
    for dir in ["sessions", "archived_sessions"] {
        collect_rollouts(source, Path::new(dir), &mut rollouts)?;
    }
    let mut ids = HashSet::new();
    let mut paths = BTreeMap::new();
    let mut added_history = false;
    for (id, rollout) in &rollouts {
        if has_complete_history(id, &rollouts) {
            let sibling = if rollout.path.extension().is_some_and(|ext| ext == "zst") {
                rollout.path.with_extension("")
            } else {
                rollout.path.with_extension("jsonl.zst")
            };
            let dest = home.join(&rollout.path);
            let sibling = home.join(sibling);
            let existing = if !dest.is_file() && sibling.is_file() {
                sibling
            } else {
                dest
            };
            if !existing.is_file() {
                match copy_rollout(&source.join(&rollout.path), &home.join(&rollout.path)) {
                    Ok(()) => added_history = true,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                }
            }
            ids.insert(id.clone());
            paths.insert(id.clone(), existing);
        }
    }
    if !ids.is_empty() {
        merge_index(&source.join(SESSION_INDEX), &home.join(SESSION_INDEX), &ids)?;
    }
    let mut native_projects = Map::new();
    let source_db = open_state(source, OpenFlags::new().with_read_only())?;
    if let Some(db) = &source_db
        && STATE_DBS.iter().all(|name| !home.join(name).exists())
        && has_table(db, "project_roots")?
        && db.prepare("SELECT 1 FROM project_roots LIMIT 1")?.next()? == State::Row
        && let Err(error) = initialize_project_store(desktop, home)
    {
        tracing::warn!(%error, "could not initialize the native Codex project store");
    }
    for relative in STATE_DBS {
        if let Some(target) = open_db(&home.join(relative), OpenFlags::new().with_read_write())? {
            if let Some(source) = &source_db {
                copy_native_projects(source, &target, &mut native_projects)?;
            }
            if (added_history
                || (!ids.is_empty()
                    && (1..=5).all(|version| {
                        !home
                            .join(format!(".codex-switch-local-workspace-v{version}"))
                            .is_file()
                    })))
                && has_table(&target, "backfill_state")?
            {
                // The native engine can reconstruct the thread rows from rollouts.
                // Old instances already scanned their previously empty directories.
                target.execute(
                    "UPDATE backfill_state SET status='pending', last_watermark=NULL, updated_at=0",
                )?;
            }
        }
    }
    let pins = match &source_db {
        Some(db) => read_section_threads(db, true)?,
        None => Vec::new(),
    };
    let owner = renderer_owner(credentials);
    let layout = copy_project_layout(
        source,
        home,
        owner.as_deref(),
        native_projects,
        &paths,
        &pins,
    )?;
    if let Some(source) = &source_db {
        for relative in STATE_DBS {
            if let Some(target) = open_db(&home.join(relative), OpenFlags::new().with_read_write())?
            {
                copy_thread_metadata(source, &target, home, &layout, &paths, owner.as_deref())?;
            }
        }
    }
    Ok(())
}

// The bundled engine creates its own schema before reading stdio. Start it
// with EOF in a temporary home so it cannot load either account's credentials.
fn initialize_project_store(desktop: &Path, home: &Path) -> Result<()> {
    let desktop = desktop.canonicalize()?;
    let directory = desktop
        .parent()
        .context("finding the Codex Desktop directory")?;
    let engine = if cfg!(target_os = "macos") {
        directory.join("../Resources/codex")
    } else {
        directory.join(if cfg!(windows) {
            "resources/codex.exe"
        } else {
            "resources/codex"
        })
    };
    let scratch = tempfile::tempdir()?;
    let mut command = Command::new(engine);
    command
        .args(["app-server", "--listen", "stdio://"])
        .env("CODEX_HOME", scratch.path())
        .env_remove("CODEX_SQLITE_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .context("starting the bundled Codex engine")?;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(status) = child.try_wait()? {
            anyhow::ensure!(
                status.success(),
                "Codex project store setup exited with {status}"
            );
            break;
        }
        if Instant::now() >= deadline {
            super::stop_launcher(&mut child);
            anyhow::bail!("Codex project store setup timed out");
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    for name in STATE_DBS {
        let db = scratch.path().join(name);
        if db.is_file() {
            let connection = Connection::open(db)?;
            let snapshot = scratch.path().join("snapshot.sqlite");
            let mut vacuum = connection.prepare("VACUUM INTO ?")?;
            vacuum.bind((
                1,
                snapshot
                    .to_str()
                    .context("encoding the project store path")?,
            ))?;
            vacuum.next()?;
            auth::atomic_write_private(&home.join(name), &fs::read(snapshot)?)?;
            return Ok(());
        }
    }
    anyhow::bail!("the bundled engine did not create a Codex project store")
}

fn open_db(path: &Path, flags: OpenFlags) -> Result<Option<Connection>> {
    if !path.is_file() {
        return Ok(None);
    }
    Connection::open_with_flags(path, flags)
        .with_context(|| format!("opening {}", path.display()))
        .map(Some)
}

fn open_state(home: &Path, flags: OpenFlags) -> Result<Option<Connection>> {
    for name in STATE_DBS {
        if let Some(db) = open_db(&home.join(name), flags)?
            && has_table(&db, "projects")?
        {
            return Ok(Some(db));
        }
    }
    Ok(None)
}

fn has_table(db: &Connection, name: &str) -> Result<bool> {
    let mut stmt = db.prepare("SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?")?;
    stmt.bind((1, name))?;
    Ok(stmt.next()? == State::Row)
}

struct NativeProject {
    id: String,
    name: String,
    position: i64,
    created: i64,
    updated: i64,
    roots: Vec<String>,
}

impl NativeProject {
    fn layout(&self) -> Value {
        json!({
            "id":self.id, "name":self.name, "rootPaths":self.roots,
            "createdAt":self.created, "updatedAt":self.updated
        })
    }
}

fn read_native_projects(source: &Connection) -> Result<Vec<NativeProject>> {
    let mut found = Vec::new();
    if !has_table(source, "projects")? || !has_table(source, "project_roots")? {
        return Ok(found);
    }
    let mut projects = source.prepare(
        "SELECT id,name,position,created_at_ms,updated_at_ms FROM projects ORDER BY position",
    )?;
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
            found.push(NativeProject {
                id,
                name: projects.read(1)?,
                position: projects.read(2)?,
                created: projects.read(3)?,
                updated: projects.read(4)?,
                roots: paths,
            });
        }
    }
    Ok(found)
}

fn copy_native_projects(
    source: &Connection,
    target: &Connection,
    imported: &mut Map<String, Value>,
) -> Result<()> {
    if !has_table(target, "projects")? || !has_table(target, "project_roots")? {
        return Ok(());
    }
    let projects = read_native_projects(source)?;
    let mut insert = target.prepare(
        "INSERT INTO projects (id,name,position,created_at_ms,updated_at_ms)
         VALUES (?,?,?,?,?) ON CONFLICT(id) DO NOTHING",
    )?;
    let mut add_root =
        target.prepare("INSERT INTO project_roots (project_id, position, path) VALUES (?,?,?)")?;
    target.execute("BEGIN IMMEDIATE")?;
    for project in projects {
        imported
            .entry(&project.id)
            .or_insert_with(|| project.layout());
        insert.bind((1, project.id.as_str()))?;
        insert.bind((2, project.name.as_str()))?;
        insert.bind((3, project.position))?;
        insert.bind((4, project.created))?;
        insert.bind((5, project.updated))?;
        insert.next()?;
        insert.reset()?;
        if target.change_count() == 0 {
            continue;
        }
        for (position, path) in project.roots.iter().enumerate() {
            add_root.bind((1, project.id.as_str()))?;
            add_root.bind((2, position as i64))?;
            add_root.bind((3, path.as_str()))?;
            add_root.next()?;
            add_root.reset()?;
        }
    }
    target.execute("COMMIT")?;
    Ok(())
}

pub(super) fn inherit_shared_layout(
    source: &Path,
    home: &Path,
    sqlite_home: &Path,
    credentials: &Value,
) -> Result<()> {
    let mut native = Map::new();
    let mut pins = Vec::new();
    if let Some(db) = open_db(
        &sqlite_home.join(STATE_DB),
        OpenFlags::new().with_read_only(),
    )? {
        for project in read_native_projects(&db)? {
            native.insert(project.id.clone(), project.layout());
        }
        pins = read_section_threads(&db, true)?;
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
    let custom_section_threads = match open_state(home, OpenFlags::new().with_read_only())? {
        Some(db) => read_section_threads(&db, false)?,
        None => Vec::new(),
    };
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
            state.insert(key, serde_json::from_str(&rows.read::<String, _>(1)?)?);
        }
    }
    Ok(state)
}

fn local_path(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|path| Path::new(path).is_absolute())
}

fn copy_thread_metadata(
    source: &Connection,
    target: &Connection,
    home: &Path,
    layout: &Map<String, Value>,
    rollouts: &BTreeMap<String, PathBuf>,
    owner: Option<&str>,
) -> Result<()> {
    let columns = |db: &Connection| -> Result<HashSet<String>> {
        let mut statement = db.prepare("PRAGMA table_info(threads)")?;
        let mut columns = HashSet::new();
        while statement.next()? == State::Row {
            columns.insert(statement.read::<String, _>(1)?);
        }
        Ok(columns)
    };
    let source_columns = columns(source)?;
    let target_columns = columns(target)?;
    if source_columns.is_empty()
        || !target_columns.contains("project_id")
        || !has_table(target, "project_roots")?
    {
        return Ok(());
    }

    let host = format!("local:{}", home.display());
    let aliases = layout.get(PROJECT_IDS).and_then(|ids| ids.get(&host));
    let legacy = layout.get(THREAD_PROJECTS);
    let projectless = layout.get(PROJECTLESS_THREADS).and_then(Value::as_array);
    let migrated = layout
        .get(&scoped_key(MIGRATED_PINS, owner))
        .and_then(|hosts| hosts.get(&host))
        .and_then(Value::as_array);
    let can_pin = target_columns.contains("thread_section_id")
        && target_columns.contains("section_position")
        && has_table(target, "thread_sections")?
        && {
            let mut section = target.prepare("SELECT 1 FROM thread_sections WHERE id=?")?;
            section.bind((1, PINNED_SECTION))?;
            section.next()? == State::Row
        };
    let sessions = home.join("sessions");
    let pending_pins: Vec<_> = layout
        .get(PINNED_THREADS)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|_| can_pin)
        .filter(|id| !migrated.is_some_and(|items| items.contains(id)))
        .filter_map(Value::as_str)
        .filter(|id| {
            rollouts
                .get(*id)
                .is_some_and(|path| path.starts_with(&sessions))
        })
        .collect();
    // Only project members and pins with a verified local rollout need an index row here.
    // The engine backfills the rest. Don't transfer account or cloud ownership.
    let fields: Vec<_> = [
        "created_at",
        "updated_at",
        "source",
        "model_provider",
        "cwd",
        "title",
        "sandbox_policy",
        "approval_mode",
        "has_user_event",
        "archived",
        "archived_at",
        "cli_version",
        "first_user_message",
        "created_at_ms",
        "updated_at_ms",
        "thread_source",
        "recency_at",
        "recency_at_ms",
        "history_mode",
        "name",
        "preview",
    ]
    .into_iter()
    .filter(|field| source_columns.contains(*field) && target_columns.contains(*field))
    .collect();
    if fields.is_empty() {
        return Ok(());
    }
    let field_count = fields.len();
    let selection = std::iter::repeat_n("?", field_count + 3)
        .collect::<Vec<_>>()
        .join(",");
    let fields = fields.join(",");
    let project_column = if source_columns.contains("project_id") {
        "project_id"
    } else {
        "NULL"
    };
    let mut read = source.prepare(format!(
        "SELECT {project_column},{fields} FROM threads WHERE id=?"
    ))?;
    let mut update =
        target.prepare("UPDATE threads SET project_id=? WHERE id=? AND project_id IS NULL")?;
    let mut project_exists =
        target.prepare("SELECT 1 FROM project_roots WHERE project_id=? LIMIT 1")?;
    let mut insert = target.prepare(format!(
        "INSERT INTO threads (id,rollout_path,project_id,{fields}) VALUES ({selection})
         ON CONFLICT(id) DO NOTHING"
    ))?;
    target.execute("BEGIN IMMEDIATE")?;
    for (id, path) in rollouts {
        let keep_projectless = projectless.is_some_and(|ids| ids.contains(&json!(id)));
        let legacy_project = legacy.and_then(|legacy| legacy.get(id));
        let mut project_id = legacy_project
            .filter(|_| !keep_projectless)
            .filter(|entry| entry.get("projectKind").and_then(Value::as_str) == Some("local"))
            .and_then(|entry| entry.get("projectId").and_then(Value::as_str))
            .map(|project| {
                aliases
                    .and_then(|aliases| aliases.get(project))
                    .and_then(Value::as_str)
                    .unwrap_or(project)
                    .to_owned()
            });
        read.bind((1, id.as_str()))?;
        let row = read.next()? == State::Row;
        if row && !keep_projectless && legacy_project.is_none() {
            project_id = read.read::<Option<String>, _>(0)?;
        }
        if let Some(project) = &project_id {
            project_exists.bind((1, project.as_str()))?;
            let exists = project_exists.next()? == State::Row;
            project_exists.reset()?;
            if exists {
                update.bind((1, project.as_str()))?;
                update.bind((2, id.as_str()))?;
                update.next()?;
                update.reset()?;
            } else {
                project_id = None;
            }
        }
        if row && (project_id.is_some() || pending_pins.contains(&id.as_str())) {
            insert.bind((1, id.as_str()))?;
            insert.bind((2, path.to_str().context("encoding copied rollout path")?))?;
            insert.bind((3, project_id.as_deref()))?;
            for offset in 0..field_count {
                insert.bind((offset + 4, read.read::<sqlite::Value, _>(offset + 1)?))?;
            }
            insert.next()?;
            insert.reset()?;
        }
        read.reset()?;
    }
    if !pending_pins.is_empty() {
        let mut pin = target.prepare(
            "UPDATE threads SET thread_section_id=?1,
                section_position=(SELECT COALESCE(MAX(section_position),0)+1024 FROM threads WHERE thread_section_id=?1)
             WHERE id=?2 AND archived=0 AND thread_section_id IS NULL",
        )?;
        for id in pending_pins {
            pin.bind((1, PINNED_SECTION))?;
            pin.bind((2, id))?;
            pin.next()?;
            pin.reset()?;
        }
    }
    target.execute("COMMIT")?;
    Ok(())
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
                Err(error) => return Err(error.into()),
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

fn rollout_header(path: &Path) -> io::Result<Option<String>> {
    let file = File::open(path)?;
    if path.extension().is_some_and(|extension| extension == "zst") {
        BufReader::new(zstd::Decoder::new(file)?)
            .lines()
            .next()
            .transpose()
    } else {
        BufReader::new(file).lines().next().transpose()
    }
}

fn copy_rollout(source: &Path, dest: &Path) -> io::Result<()> {
    let mut source = File::open(source)?;
    let parent = dest.parent().expect("rollout path has a parent");
    fs::create_dir_all(parent)?;
    let mut copy = tempfile::NamedTempFile::new_in(parent)?;
    io::copy(&mut source, &mut copy)?;
    copy.persist_noclobber(dest).map_err(|e| e.error)?;
    Ok(())
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

fn merge_index(source: &Path, dest: &Path, rollouts: &HashSet<String>) -> Result<()> {
    if !source.is_file() {
        return Ok(());
    }
    let mut index = settings::read_if_present(dest)?.unwrap_or_default();
    let mut known: HashSet<String> = index.lines().filter_map(index_id).collect();
    for line in BufReader::new(File::open(source)?).lines() {
        let line = line?;
        if let Some(id) = index_id(&line)
            && rollouts.contains(&id)
            && known.insert(id)
        {
            if !index.is_empty() && !index.ends_with('\n') {
                index.push('\n');
            }
            index.push_str(&line);
            index.push('\n');
        }
    }
    if !index.is_empty() {
        auth::atomic_write_private(dest, index.as_bytes())?;
    }
    Ok(())
}

fn index_id(line: &str) -> Option<String> {
    serde_json::from_str::<Value>(line)
        .ok()?
        .get("id")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn copies_local_workspace_and_only_complete_unscoped_history() {
        let source = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let source = source.path();
        let home = home.path();
        let local = "00000000-0000-4000-8000-000000000001";
        let private = "00000000-0000-4000-8000-000000000002";
        let incomplete = "00000000-0000-4000-8000-000000000003";
        let followup = "00000000-0000-4000-8000-000000000004";
        let stale = "00000000-0000-4000-8000-000000000005";
        let rollout = |id: &str| format!("sessions/rollout-2026-11-01T01-02-03-{id}.jsonl");
        fs::create_dir(source.join("sessions")).unwrap();
        let mut index = String::new();
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
            index.push_str(&format!("{}\n", json!({"id":id,"thread_name":"example"})));
        }
        let stale_header = json!({"type":"session_meta","payload":{"id":stale}}).to_string();
        fs::write(
            source.join(rollout(stale)).with_extension("jsonl.zst"),
            zstd::stream::encode_all(stale_header.as_bytes(), 1).unwrap(),
        )
        .unwrap();
        fs::write(source.join(SESSION_INDEX), index).unwrap();
        fs::write(
            source.join(settings::GLOBAL_JSON),
            json!({
                LOCAL_PROJECTS: {
                    "local": {"id":"local","name":"work","rootPaths":[source]},
                    "remote": {"id":"remote","rootPaths":["ssh:host/path"]}
                },
                PROJECT_ORDER: ["local","remote"],
                "remote-projects":["private"]
            })
            .to_string(),
        )
        .unwrap();

        let db = Connection::open(home.join(STATE_DB)).unwrap();
        db.execute(
            "CREATE TABLE backfill_state(status TEXT,last_watermark TEXT,updated_at INTEGER);
            INSERT INTO backfill_state VALUES ('complete','old',1)",
        )
        .unwrap();
        inherit(source, home, Path::new("unused-desktop"), &Value::Null).unwrap();
        assert!(
            home.join(rollout(local))
                .with_extension("jsonl.zst")
                .is_file()
        );
        assert!(home.join(rollout(followup)).is_file());
        assert!(
            !home
                .join(rollout(private))
                .with_extension("jsonl.zst")
                .exists()
        );
        assert!(!home.join(rollout(incomplete)).exists());
        assert!(
            !home
                .join(rollout(stale))
                .with_extension("jsonl.zst")
                .exists()
        );
        assert_eq!(
            fs::read_to_string(home.join(SESSION_INDEX))
                .unwrap()
                .lines()
                .count(),
            2
        );
        let target = settings::read_global_json(home).unwrap();
        assert_eq!(target[PROJECT_ORDER], json!(["local"]));
        assert!(target.get("remote-projects").is_none());
        let row = db
            .prepare("SELECT status FROM backfill_state")
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(row.read::<&str, _>(0), "pending");
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

        let source_index = Connection::open(":memory:").unwrap();
        let target_index = Connection::open(":memory:").unwrap();
        let thread_schema = "CREATE TABLE threads(id TEXT PRIMARY KEY, rollout_path TEXT,
            project_id TEXT, title TEXT, creator_account_id TEXT, thread_section_id TEXT,
            section_position INTEGER, archived INTEGER NOT NULL DEFAULT 0)";
        source_index.execute(thread_schema).unwrap();
        target_index.execute(thread_schema).unwrap();
        target_index
            .execute(format!(
                "CREATE TABLE project_roots(project_id TEXT);
                    INSERT INTO project_roots VALUES ('native');
                    CREATE TABLE thread_sections(id TEXT);
                    INSERT INTO thread_sections VALUES ('{PINNED_SECTION}')"
            ))
            .unwrap();
        source_index
            .execute("INSERT INTO threads (id,rollout_path,title,creator_account_id)
                VALUES ('local','original-path','Chat','private'),('native-only','native-path','Other chat','private')")
            .unwrap();
        copy_thread_metadata(
            &source_index,
            &target_index,
            home,
            &target,
            &paths,
            Some(owner),
        )
        .unwrap();
        let mut check = target_index
            .prepare("SELECT rollout_path, project_id, title, creator_account_id FROM threads WHERE id='local'")
            .unwrap();
        assert_eq!(check.next().unwrap(), State::Row);
        assert_eq!(
            check.read::<String, _>(0).unwrap(),
            copied_path.to_str().unwrap()
        );
        assert_eq!(check.read::<String, _>(1).unwrap(), "native");
        assert_eq!(check.read::<String, _>(2).unwrap(), "Chat");
        assert_eq!(check.read::<Option<String>, _>(3).unwrap(), None);
        assert_eq!(
            read_section_threads(&target_index, true).unwrap(),
            [json!("native-only"), json!("local")]
        );

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
        )
        .unwrap();
        let fallback = settings::read_global_json(fresh.path()).unwrap();
        assert_eq!(
            fallback[&scoped_key(WORKSPACE_ROOTS[0], Some(owner))],
            json!([source])
        );
    }
}
