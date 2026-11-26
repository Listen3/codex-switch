//! Local sync settings and named groups of saved ChatGPT accounts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::provider::{ProviderModel, ProviderProfile};
use crate::remote_store;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSettings {
    pub group_name: String,
    #[serde(default)]
    pub redis_url: String,
    #[serde(default, skip)]
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderSnapshot {
    alias: String,
    #[serde(default)]
    identity_id: String,
    provider_id: String,
    base_url: String,
    #[serde(default)]
    allow_insecure_http: bool,
    env_key: String,
    #[serde(default)]
    default_model: String,
    models: Vec<ProviderModel>,
    #[serde(default)]
    metadata_fallback: String,
    #[serde(default = "default_wire")]
    wire_api: String,
    #[serde(default)]
    codex_config: Vec<String>,
    api_key: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct AccountSnapshot {
    alias: String,
    auth: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct GroupDocument {
    schema: u32,
    group: String,
    #[serde(default)]
    accounts: Vec<AccountSnapshot>,
    #[serde(default)]
    providers: Vec<ProviderSnapshot>,
}

fn default_wire() -> String {
    "responses".to_string()
}

pub fn sanitize_group_name(raw: &str) -> String {
    let mut name: String = raw
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    while name.contains("--") {
        name = name.replace("--", "-");
    }
    name = name.trim_matches(['-', '.']).to_string();
    name.truncate(64);
    name
}

fn usable_host_name(name: String) -> Option<String> {
    let name = name.trim().to_string();
    if name.is_empty() || name.eq_ignore_ascii_case("localhost") {
        None
    } else {
        Some(name)
    }
}

fn detected_host_name() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .and_then(usable_host_name)
        .or_else(posix_host_name)
        .or_else(|| std::env::var("HOSTNAME").ok().and_then(usable_host_name))
}

#[cfg(unix)]
fn posix_host_name() -> Option<String> {
    let mut buf = [0_u8; 256];
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|byte| *byte == 0).unwrap_or(buf.len());
    usable_host_name(String::from_utf8_lossy(&buf[..end]).into_owned())
}

#[cfg(not(unix))]
fn posix_host_name() -> Option<String> {
    None
}

pub fn default_group_name() -> String {
    if let Some(name) = detected_host_name() {
        let name = sanitize_group_name(&name);
        if !name.is_empty() {
            return name;
        }
    }
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    let name = sanitize_group_name(&user);
    if name.is_empty() {
        "computer".to_string()
    } else {
        name
    }
}

pub fn load() -> SyncSettings {
    let mut settings = read_settings().unwrap_or_else(|_| SyncSettings {
        group_name: default_group_name(),
        redis_url: String::new(),
        password: String::new(),
    });
    settings.password = std::fs::read_to_string(password_path().unwrap_or_default())
        .unwrap_or_default()
        .trim()
        .to_string();
    if settings.group_name.trim().is_empty() || settings.group_name == "this-pc" {
        let detected = default_group_name();
        if settings.group_name != detected {
            let previous = settings.group_name.clone();
            settings.group_name = detected;
            if previous == "this-pc" {
                let _ = rename_group("this-pc", &settings.group_name);
            }
            let _ = save(&settings);
        }
    }
    settings
}

pub fn save(settings: &SyncSettings) -> Result<()> {
    crate::profile::validate_alias(&settings.group_name)?;
    let path = settings_path()?;
    let rendered = toml::to_string_pretty(&StoredSettings {
        group_name: settings.group_name.clone(),
        redis_url: settings.redis_url.clone(),
    })
    .context("serializing sync settings")?;
    crate::auth::atomic_write_private(&path, rendered.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    let secret = password_path()?;
    if settings.password.is_empty() {
        if secret.exists() {
            std::fs::remove_file(&secret).ok();
        }
    } else {
        crate::auth::atomic_write_private(&secret, settings.password.as_bytes())
            .with_context(|| format!("writing {}", secret.display()))?;
    }
    Ok(())
}

pub const GROUP_KEY_PREFIX: &str = "cs:v1:group/";

pub fn group_key(group_name: &str) -> String {
    format!("{GROUP_KEY_PREFIX}{group_name}")
}

pub fn account_group(alias: &str, default_group: &str) -> String {
    let index = read_index().unwrap_or_default();
    group_for(alias, &index, default_group)
}

pub fn local_group_names(default_group: &str) -> Result<Vec<String>> {
    let aliases = crate::profile::list_profiles()?;
    let index = read_index().unwrap_or_default();
    let mut names = BTreeSet::new();
    names.insert(default_group.to_string());
    for alias in aliases {
        names.insert(group_for(&alias, &index, default_group));
    }
    let mut ordered: Vec<String> = names
        .into_iter()
        .filter(|name| name != default_group)
        .collect();
    ordered.insert(0, default_group.to_string());
    Ok(ordered)
}

pub fn remote_groups_to_remove(remote: &[String], keep: &[String]) -> Vec<String> {
    let keep: BTreeSet<&str> = keep.iter().map(String::as_str).collect();
    remote
        .iter()
        .filter(|name| !keep.contains(name.as_str()))
        .cloned()
        .collect()
}

pub fn group_name_from_key(key: &str) -> Option<String> {
    let name = key.strip_prefix(GROUP_KEY_PREFIX)?.trim();
    crate::profile::validate_alias(name).ok()?;
    Some(name.to_string())
}

pub fn rename_group(from: &str, to: &str) -> Result<()> {
    if from == to {
        return Ok(());
    }
    crate::profile::validate_alias(from)?;
    crate::profile::validate_alias(to)?;
    let mut index = read_index().unwrap_or_default();
    for alias in crate::profile::list_profiles()? {
        if group_for(&alias, &index, from) == from {
            index.accounts.insert(alias, to.to_string());
        }
    }
    write_index(&index)
}

pub async fn list_remote_groups(settings: &SyncSettings) -> Result<Vec<String>> {
    let endpoint = remote_store::parse(&settings.redis_url, &settings.password)?;
    let keys = remote_store::list_text(&endpoint, &format!("{GROUP_KEY_PREFIX}*")).await?;
    let mut names = Vec::new();
    for key in keys {
        if let Some(name) = group_name_from_key(&key) {
            names.push(name);
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

pub async fn push_groups(
    settings: &SyncSettings,
    language: crate::config::Language,
    groups: &[String],
    replace_remote: bool,
) -> Result<String> {
    let endpoint = remote_store::parse(&settings.redis_url, &settings.password)?;
    let mut account_count = 0_usize;
    for group in groups {
        crate::profile::validate_alias(group)?;
        let document = document_for(group, &settings.group_name)?;
        account_count += document.accounts.len();
        let plain = serde_json::to_vec(&document).context("serializing group")?;
        let secret = sync_secret(settings, language)?;
        let body = crate::sync_seal::seal(&secret, &plain)?;
        remote_store::set_text(&endpoint, &group_key(group), &body).await?;
    }
    let mut removed = 0_usize;
    if replace_remote {
        let remote = list_remote_groups(settings).await?;
        for name in remote_groups_to_remove(&remote, groups) {
            remote_store::delete_text(&endpoint, &group_key(&name)).await?;
            removed += 1;
        }
    }
    Ok(crate::tui::locale::uploaded(
        language,
        groups.len(),
        account_count,
        removed,
    ))
}

pub async fn pull_groups(
    settings: &SyncSettings,
    language: crate::config::Language,
    groups: &[String],
) -> Result<String> {
    let endpoint = remote_store::parse(&settings.redis_url, &settings.password)?;
    let mut created = 0_usize;
    let mut updated = 0_usize;
    let mut removed = 0_usize;
    let mut renamed = 0_usize;
    let mut skipped = 0_usize;
    let mut found = 0_usize;
    for group in groups {
        crate::profile::validate_alias(group)?;
        let Some(body) = remote_store::get_text(&endpoint, &group_key(group)).await? else {
            continue;
        };
        let document = open_group(&body, settings, language)?;
        if document.schema != 1 && document.schema != 2 {
            anyhow::bail!(crate::tui::locale::unknown_schema(
                language,
                document.schema
            ));
        }
        let counts = replace_local_group(group, &settings.group_name, &document)?;
        created += counts.created;
        updated += counts.updated;
        removed += counts.removed;
        renamed += counts.renamed;
        skipped += counts.skipped;
        found += 1;
    }
    if found == 0 {
        anyhow::bail!(crate::tui::locale::no_remote_groups(language));
    }
    Ok(crate::tui::locale::downloaded(
        language, found, created, updated, removed, renamed, skipped,
    ))
}

fn sync_secret(settings: &SyncSettings, language: crate::config::Language) -> Result<String> {
    remote_store::access_secret(&settings.redis_url, &settings.password)
        .ok_or_else(|| anyhow::anyhow!(crate::tui::locale::sync_password_required(language)))
}

fn open_group(
    body: &str,
    settings: &SyncSettings,
    language: crate::config::Language,
) -> Result<GroupDocument> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("unrecognized group document")?;
    let schema = value
        .get("schema")
        .and_then(|item| item.as_u64())
        .unwrap_or(0);
    if schema == 3 {
        let secret = sync_secret(settings, language)?;
        let plain = crate::sync_seal::open(&secret, body)
            .map_err(|_| anyhow::anyhow!(crate::tui::locale::sync_decrypt_failed(language)))?;
        return serde_json::from_slice(&plain).context("unrecognized group document");
    }
    serde_json::from_value(value).context("unrecognized group document")
}

fn place_synced_account(snapshot: &AccountSnapshot) -> Result<PlacedAccount> {
    let requested = snapshot.alias.clone();
    crate::profile::validate_alias(&requested)?;
    let identity = crate::profile::extract_identity(&snapshot.auth);
    if let Some(existing) = crate::profile::find_profile_by_identity_exact(&identity) {
        crate::profile::save_synced_auth(&existing, &snapshot.auth)?;
        return Ok(PlacedAccount {
            renamed: existing != requested,
            created: false,
            stored: true,
            alias: existing,
        });
    }
    let profile_exists = crate::profile::profile_auth_path(&requested)?.exists();
    let provider_owns_alias = crate::provider::exists(&requested);
    if profile_exists {
        match crate::profile::save_synced_auth(&requested, &snapshot.auth) {
            Ok(()) => {
                return Ok(PlacedAccount {
                    alias: requested,
                    created: false,
                    renamed: false,
                    stored: true,
                });
            }
            Err(_) if proven_different_account(&requested, &snapshot.auth) => {}
            Err(_) => {
                return Ok(PlacedAccount {
                    alias: requested,
                    created: false,
                    renamed: false,
                    stored: false,
                });
            }
        }
    } else if !provider_owns_alias {
        crate::profile::save_synced_auth(&requested, &snapshot.auth)?;
        return Ok(PlacedAccount {
            alias: requested,
            created: true,
            renamed: false,
            stored: true,
        });
    }
    let unique = crate::profile::make_unique_alias(&requested)?;
    crate::profile::save_synced_auth(&unique, &snapshot.auth)?;
    Ok(PlacedAccount {
        alias: unique,
        created: true,
        renamed: true,
        stored: true,
    })
}

fn proven_different_account(alias: &str, incoming: &serde_json::Value) -> bool {
    let Ok(path) = crate::profile::profile_auth_path(alias) else {
        return false;
    };
    let Ok(existing) = crate::auth::read_auth(&path) else {
        return false;
    };
    let existing = crate::profile::extract_identity(&existing);
    let incoming = crate::profile::extract_identity(incoming);
    match (
        existing.account_id.as_deref(),
        incoming.account_id.as_deref(),
    ) {
        (Some(left), Some(right)) => left != right,
        _ => match (existing.email.as_deref(), incoming.email.as_deref()) {
            (Some(left), Some(right)) => !left.eq_ignore_ascii_case(right),
            _ => false,
        },
    }
}

struct ApplyCounts {
    created: usize,
    updated: usize,
    removed: usize,
    renamed: usize,
    skipped: usize,
}

struct PlacedAccount {
    alias: String,
    created: bool,
    renamed: bool,
    stored: bool,
}

fn replace_local_group(
    group: &str,
    default_group: &str,
    document: &GroupDocument,
) -> Result<ApplyCounts> {
    let mut index = read_index().unwrap_or_default();
    let existing = aliases_in_group(group, default_group, &index)?;
    let mut kept = BTreeSet::new();
    let mut displaced = BTreeSet::new();
    let mut counts = ApplyCounts {
        created: 0,
        updated: 0,
        removed: 0,
        renamed: 0,
        skipped: 0,
    };
    for snapshot in &document.accounts {
        match place_synced_account(snapshot) {
            Ok(placed) => {
                if !placed.stored {
                    counts.skipped += 1;
                    kept.insert(placed.alias);
                    continue;
                }
                if placed.created {
                    counts.created += 1;
                } else {
                    counts.updated += 1;
                }
                if placed.renamed {
                    counts.renamed += 1;
                    displaced.insert(snapshot.alias.clone());
                }
                kept.insert(placed.alias.clone());
                index.accounts.insert(placed.alias, group.to_string());
            }
            Err(_) => counts.skipped += 1,
        }
    }
    for snapshot in &document.providers {
        match apply_provider(snapshot.clone(), crate::config::Language::En) {
            Ok(true) => counts.created += 1,
            Ok(false) => counts.updated += 1,
            Err(_) => counts.skipped += 1,
        }
    }
    for alias in existing {
        if kept.contains(&alias) || displaced.contains(&alias) {
            continue;
        }
        if crate::profile::read_current() == alias {
            counts.skipped += 1;
            continue;
        }
        if crate::profile::cmd_delete(&alias).is_err() {
            counts.skipped += 1;
            continue;
        }
        index.accounts.remove(&alias);
        counts.removed += 1;
    }
    write_index(&index)?;
    Ok(counts)
}

fn document_for(group: &str, default_group: &str) -> Result<GroupDocument> {
    Ok(GroupDocument {
        schema: 2,
        group: group.to_string(),
        accounts: accounts_in_group(group, default_group)?,
        providers: if group == default_group {
            local_providers()?
        } else {
            Vec::new()
        },
    })
}

fn aliases_in_group(group: &str, default_group: &str, index: &GroupIndex) -> Result<Vec<String>> {
    let mut aliases = Vec::new();
    for alias in crate::profile::list_profiles()? {
        if group_for(&alias, index, default_group) == group {
            aliases.push(alias);
        }
    }
    Ok(aliases)
}

fn accounts_in_group(group: &str, default_group: &str) -> Result<Vec<AccountSnapshot>> {
    let index = read_index().unwrap_or_default();
    let mut snapshots = Vec::new();
    for alias in aliases_in_group(group, default_group, &index)? {
        let path = crate::profile::profile_auth_path(&alias)?;
        let auth = crate::auth::read_auth(&path)?;
        snapshots.push(AccountSnapshot { alias, auth });
    }
    Ok(snapshots)
}

fn group_for(alias: &str, index: &GroupIndex, default_group: &str) -> String {
    index
        .accounts
        .get(alias)
        .filter(|name| crate::profile::validate_alias(name).is_ok())
        .cloned()
        .unwrap_or_else(|| default_group.to_string())
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct GroupIndex {
    #[serde(default)]
    accounts: BTreeMap<String, String>,
}

fn read_index() -> Result<GroupIndex> {
    let path = index_path()?;
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw).context("parsing sync groups")
}

fn write_index(index: &GroupIndex) -> Result<()> {
    let path = index_path()?;
    let mut pruned = index.clone();
    if let Ok(aliases) = crate::profile::list_profiles() {
        let live: BTreeSet<String> = aliases.into_iter().collect();
        pruned.accounts.retain(|alias, _| live.contains(alias));
    }
    let body = serde_json::to_string_pretty(&pruned).context("serializing sync groups")?;
    crate::auth::atomic_write_private(&path, body.as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

fn index_path() -> Result<PathBuf> {
    Ok(crate::auth::app_home()?.join("sync-groups.json"))
}

fn apply_provider(snapshot: ProviderSnapshot, language: crate::config::Language) -> Result<bool> {
    crate::profile::validate_alias(&snapshot.alias)?;
    if crate::profile::list_profiles()
        .unwrap_or_default()
        .iter()
        .any(|alias| alias == &snapshot.alias)
    {
        bail!(crate::tui::locale::profile_collision(
            language,
            &snapshot.alias
        ));
    }
    let created = !crate::provider::exists(&snapshot.alias);
    let mut profile = ProviderProfile::build(
        snapshot.alias,
        snapshot.base_url,
        snapshot.models,
        snapshot.api_key,
    );
    if !created && let Ok(existing) = crate::provider::load(&profile.alias) {
        profile.identity_id = existing.identity_id;
    } else if !snapshot.identity_id.trim().is_empty() {
        profile.identity_id = snapshot.identity_id;
    }
    profile.provider_id = snapshot.provider_id;
    profile.allow_insecure_http = snapshot.allow_insecure_http;
    profile.env_key = snapshot.env_key;
    profile.default_model = snapshot.default_model;
    profile.metadata_fallback = snapshot.metadata_fallback;
    profile.wire_api = snapshot.wire_api;
    profile.codex_config = snapshot.codex_config;
    crate::provider::save(&profile)?;
    Ok(created)
}

fn local_providers() -> Result<Vec<ProviderSnapshot>> {
    let mut snapshots = Vec::new();
    for alias in crate::provider::list_providers()? {
        let profile = crate::provider::load(&alias)?;
        snapshots.push(ProviderSnapshot {
            alias: profile.alias,
            identity_id: profile.identity_id,
            provider_id: profile.provider_id,
            base_url: profile.base_url,
            allow_insecure_http: profile.allow_insecure_http,
            env_key: profile.env_key,
            default_model: profile.default_model,
            models: profile.models,
            metadata_fallback: profile.metadata_fallback,
            wire_api: profile.wire_api,
            codex_config: profile.codex_config,
            api_key: profile.api_key,
        });
    }
    Ok(snapshots)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSettings {
    group_name: String,
    redis_url: String,
}

fn read_settings() -> Result<SyncSettings> {
    let path = settings_path()?;
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let stored: StoredSettings = toml::from_str(&raw).context("parsing sync settings")?;
    Ok(SyncSettings {
        group_name: stored.group_name,
        redis_url: stored.redis_url,
        password: String::new(),
    })
}

fn settings_path() -> Result<PathBuf> {
    Ok(crate::auth::app_home()?.join("sync.toml"))
}

fn password_path() -> Result<PathBuf> {
    Ok(crate::auth::app_home()?.join("sync.password"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Env {
        _lock: std::sync::MutexGuard<'static, ()>,
        _dir: tempfile::TempDir,
        old_home: Option<std::ffi::OsString>,
        old_codex: Option<std::ffi::OsString>,
        old_app: Option<std::ffi::OsString>,
    }

    impl Env {
        fn new() -> Self {
            let lock = crate::profile::TEST_ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let dir = tempfile::tempdir().unwrap();
            let old_home = std::env::var_os("HOME");
            let old_codex = std::env::var_os("CODEX_HOME");
            let old_app = std::env::var_os("CODEX_SWITCH_HOME");
            let codex = dir.path().join(".codex");
            let app = dir.path().join(".codex-switch");
            std::fs::create_dir_all(&codex).unwrap();
            std::fs::create_dir_all(&app).unwrap();
            unsafe {
                std::env::set_var("HOME", dir.path());
                std::env::set_var("CODEX_HOME", &codex);
                std::env::set_var("CODEX_SWITCH_HOME", &app);
                std::env::remove_var("OPENAI_FEDERATION_RULE_ID");
                std::env::remove_var("OPENAI_IDENTITY_TOKEN_FILE");
            }
            crate::output::set_message_mode(crate::output::MessageMode::Silent);
            Self {
                _lock: lock,
                _dir: dir,
                old_home,
                old_codex,
                old_app,
            }
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            restore("HOME", &self.old_home);
            restore("CODEX_HOME", &self.old_codex);
            restore("CODEX_SWITCH_HOME", &self.old_app);
        }
    }

    fn restore(key: &str, value: &Option<std::ffi::OsString>) {
        match value {
            Some(value) => unsafe { std::env::set_var(key, value) },
            None => unsafe { std::env::remove_var(key) },
        }
    }

    fn save_alias(alias: &str) {
        crate::profile::save_synced_auth(alias, &serde_json::json!({})).unwrap();
    }

    #[test]
    fn group_name_uses_the_computer_name_instead_of_this_pc() {
        assert_ne!(default_group_name(), "this-pc");
        assert!(crate::profile::validate_alias(&default_group_name()).is_ok());
        assert_eq!(
            sanitize_group_name("Listen's MacBook Pro"),
            "Listen-s-MacBook-Pro"
        );
        assert_eq!(sanitize_group_name("..."), "");
    }

    #[test]
    fn placeholder_this_pc_is_replaced_with_the_real_computer_name() {
        let _env = Env::new();
        let path = settings_path().unwrap();
        std::fs::write(&path, "group_name = \"this-pc\"\nredis_url = \"\"\n").unwrap();
        let settings = load();
        assert_ne!(settings.group_name, "this-pc");
        assert_eq!(settings.group_name, default_group_name());
        let stored = std::fs::read_to_string(path).unwrap();
        assert!(!stored.contains("this-pc"));
    }

    #[test]
    fn untagged_accounts_stay_in_the_default_group() {
        let _env = Env::new();
        save_alias("home-a");
        save_alias("office-a");
        let mut index = GroupIndex::default();
        index.accounts.insert("office-a".into(), "office".into());
        write_index(&index).unwrap();

        assert_eq!(local_group_names("home").unwrap(), ["home", "office"]);
        let home = accounts_in_group("home", "home").unwrap();
        let office = accounts_in_group("office", "home").unwrap();
        assert_eq!(home.len(), 1);
        assert_eq!(home[0].alias, "home-a");
        assert_eq!(office.len(), 1);
        assert_eq!(office[0].alias, "office-a");
    }

    #[test]
    fn replacing_a_group_removes_accounts_that_left_it() {
        let _env = Env::new();
        save_alias("keep");
        save_alias("drop");
        save_alias("other");
        let mut index = GroupIndex::default();
        index.accounts.insert("other".into(), "office".into());
        write_index(&index).unwrap();
        std::fs::write(crate::auth::current_file().unwrap(), "keep").unwrap();

        let document = GroupDocument {
            schema: 2,
            group: "home".into(),
            accounts: vec![AccountSnapshot {
                alias: "keep".into(),
                auth: serde_json::json!({}),
            }],
            providers: Vec::new(),
        };
        let counts = replace_local_group("home", "home", &document).unwrap();
        assert_eq!(counts.removed, 1);
        assert!(crate::profile::profile_auth_path("keep").unwrap().exists());
        assert!(!crate::profile::profile_auth_path("drop").unwrap().exists());
        assert!(crate::profile::profile_auth_path("other").unwrap().exists());
        assert_eq!(local_group_names("home").unwrap(), ["home", "office"]);
    }

    #[test]
    fn active_account_is_kept_when_the_snapshot_omits_it() {
        let _env = Env::new();
        save_alias("keep");
        std::fs::write(crate::auth::current_file().unwrap(), "keep").unwrap();
        let document = GroupDocument {
            schema: 2,
            group: "home".into(),
            accounts: Vec::new(),
            providers: Vec::new(),
        };
        let counts = replace_local_group("home", "home", &document).unwrap();
        assert_eq!(counts.removed, 0);
        assert_eq!(counts.skipped, 1);
        assert!(crate::profile::profile_auth_path("keep").unwrap().exists());
    }

    #[test]
    fn full_upload_removes_remote_groups_absent_locally() {
        let remote = vec!["home".into(), "old-laptop".into(), "office".into()];
        let keep = vec!["home".into(), "office".into()];
        assert_eq!(
            remote_groups_to_remove(&remote, &keep),
            vec!["old-laptop".to_string()]
        );
        assert_eq!(
            group_name_from_key("cs:v1:group/office").as_deref(),
            Some("office")
        );
        assert!(group_name_from_key("cs:v1:other/office").is_none());
    }

    fn account(account_id: &str, email: &str) -> serde_json::Value {
        use base64::Engine;
        let payload = format!("{{\"email\":\"{email}\"}}");
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        serde_json::json!({
            "tokens": {
                "account_id": account_id,
                "id_token": format!("e30.{encoded}."),
                "refresh_token": "refresh",
                "access_token": "access"
            }
        })
    }

    #[test]
    fn alias_taken_by_another_account_is_saved_under_a_new_name() {
        let _env = Env::new();
        crate::profile::save_synced_auth("work", &account("acct-a", "a@example.com")).unwrap();
        let document = GroupDocument {
            schema: 2,
            group: "office".into(),
            accounts: vec![AccountSnapshot {
                alias: "work".into(),
                auth: account("acct-b", "b@example.com"),
            }],
            providers: Vec::new(),
        };
        let counts = replace_local_group("office", "home", &document).unwrap();
        assert_eq!(counts.renamed, 1);
        assert_eq!(counts.removed, 0);
        let original = crate::profile::extract_identity(
            &crate::auth::read_auth(&crate::profile::profile_auth_path("work").unwrap()).unwrap(),
        );
        let renamed = crate::profile::extract_identity(
            &crate::auth::read_auth(&crate::profile::profile_auth_path("work_2").unwrap()).unwrap(),
        );
        assert_eq!(original.account_id.as_deref(), Some("acct-a"));
        assert_eq!(renamed.account_id.as_deref(), Some("acct-b"));
    }

    #[test]
    fn sealed_document_opens_only_with_the_sync_password() {
        let plain = serde_json::to_vec(&GroupDocument {
            schema: 2,
            group: "home".into(),
            accounts: Vec::new(),
            providers: Vec::new(),
        })
        .unwrap();
        let sealed = crate::sync_seal::seal("secret", &plain).unwrap();
        assert!(!sealed.contains("refresh_token"));
        let settings = SyncSettings {
            group_name: "home".into(),
            redis_url: "redis://127.0.0.1:6379".into(),
            password: "secret".into(),
        };
        let opened = open_group(&sealed, &settings, crate::config::Language::En).unwrap();
        assert_eq!(opened.group, "home");
        let wrong = SyncSettings {
            password: "other".into(),
            ..settings
        };
        assert!(open_group(&sealed, &wrong, crate::config::Language::En).is_err());
    }
}
