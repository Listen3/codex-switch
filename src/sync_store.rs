//! Local sync settings and one named group of custom API providers.

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

#[derive(Serialize, Deserialize)]
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

pub fn group_key(group_name: &str) -> String {
    format!("cs:v1:group/{group_name}")
}

pub async fn push(settings: &SyncSettings, language: crate::config::Language) -> Result<String> {
    let endpoint = remote_store::parse(&settings.redis_url, &settings.password)?;
    let document = GroupDocument {
        schema: 2,
        group: settings.group_name.clone(),
        accounts: local_accounts()?,
        providers: local_providers()?,
    };
    let body = serde_json::to_string(&document).context("serializing group")?;
    remote_store::set_text(&endpoint, &group_key(&settings.group_name), &body).await?;
    Ok(crate::tui::locale::pushed(
        language,
        &settings.group_name,
        document.accounts.len(),
    ))
}

pub async fn pull(settings: &SyncSettings, language: crate::config::Language) -> Result<String> {
    let endpoint = remote_store::parse(&settings.redis_url, &settings.password)?;
    let Some(body) = remote_store::get_text(&endpoint, &group_key(&settings.group_name)).await?
    else {
        anyhow::bail!(crate::tui::locale::missing_group(
            language,
            &settings.group_name
        ));
    };
    let document: GroupDocument = serde_json::from_str(&body).context("unrecognized group document")?;
    if document.schema != 1 && document.schema != 2 {
        anyhow::bail!(crate::tui::locale::unknown_schema(language, document.schema));
    }
    let mut created = 0_usize;
    let mut updated = 0_usize;
    let mut skipped = 0_usize;
    for snapshot in document.accounts {
        match apply_account(snapshot) {
            Ok(true) => created += 1,
            Ok(false) => updated += 1,
            Err(_) => skipped += 1,
        }
    }
    for snapshot in document.providers {
        match apply_provider(snapshot, language) {
            Ok(true) => created += 1,
            Ok(false) => updated += 1,
            Err(_) => skipped += 1,
        }
    }
    Ok(crate::tui::locale::pulled(
        language,
        &settings.group_name,
        created,
        updated,
        skipped,
    ))
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

fn local_accounts() -> Result<Vec<AccountSnapshot>> {
    let mut snapshots = Vec::new();
    for alias in crate::profile::list_profiles()? {
        let path = crate::profile::profile_auth_path(&alias)?;
        let auth = crate::auth::read_auth(&path)?;
        snapshots.push(AccountSnapshot { alias, auth });
    }
    Ok(snapshots)
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
