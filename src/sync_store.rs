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

pub fn default_group_name() -> String {
    let raw = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "this-pc".to_string());
    let mut name: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    name.truncate(64);
    if name.is_empty() {
        "this-pc".to_string()
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
    if settings.group_name.trim().is_empty() {
        settings.group_name = default_group_name();
    }
    settings.password = std::fs::read_to_string(password_path().unwrap_or_default())
        .unwrap_or_default()
        .trim()
        .to_string();
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
    let mut ordered: Vec<String> = names.into_iter().filter(|name| name != default_group).collect();
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
        let body = serde_json::to_string(&document).context("serializing group")?;
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
    let mut skipped = 0_usize;
    let mut found = 0_usize;
    for group in groups {
        crate::profile::validate_alias(group)?;
        let Some(body) = remote_store::get_text(&endpoint, &group_key(group)).await? else {
            continue;
        };
        let document: GroupDocument =
            serde_json::from_str(&body).context("unrecognized group document")?;
        if document.schema != 1 && document.schema != 2 {
            anyhow::bail!(crate::tui::locale::unknown_schema(language, document.schema));
        }
        let counts = replace_local_group(group, &settings.group_name, &document)?;
        created += counts.created;
        updated += counts.updated;
        removed += counts.removed;
        skipped += counts.skipped;
        found += 1;
    }
    if found == 0 {
        anyhow::bail!(crate::tui::locale::no_remote_groups(language));
    }
    Ok(crate::tui::locale::downloaded(
        language,
        found,
        created,
        updated,
        removed,
        skipped,
    ))
}

struct ApplyCounts {
    created: usize,
    updated: usize,
    removed: usize,
    skipped: usize,
}

fn replace_local_group(
    group: &str,
    default_group: &str,
    document: &GroupDocument,
) -> Result<ApplyCounts> {
    let mut index = read_index().unwrap_or_default();
    let existing = aliases_in_group(group, default_group, &index)?;
    let mut incoming = BTreeSet::new();
    let mut counts = ApplyCounts {
        created: 0,
        updated: 0,
        removed: 0,
        skipped: 0,
    };
    for snapshot in &document.accounts {
        incoming.insert(snapshot.alias.clone());
        match apply_account(snapshot.clone()) {
            Ok(true) => {
                counts.created += 1;
                index.accounts.insert(snapshot.alias.clone(), group.to_string());
            }
            Ok(false) => {
                counts.updated += 1;
                index.accounts.insert(snapshot.alias.clone(), group.to_string());
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
        if incoming.contains(&alias) {
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
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
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
    if !created
        && let Ok(existing) = crate::provider::load(&profile.alias)
    {
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

fn apply_account(snapshot: AccountSnapshot) -> Result<bool> {
    let created = !crate::profile::profile_auth_path(&snapshot.alias)?.exists();
    crate::profile::save_synced_auth(&snapshot.alias, &snapshot.auth)?;
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
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
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
}
