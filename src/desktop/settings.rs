use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Map, Value};
use sqlite::{Connection, OpenFlags, State};

use crate::{auth, provider};

pub(super) const INHERITED: &str = ".codex-switch-desktop-settings-v1";
pub(super) const GLOBAL_JSON: &str = ".codex-global-state.json";
const GLOBAL_DB: &str = ".codex-global-state.sqlite";
const ATOMS: &str = "electron-persisted-atom-state";
const ONBOARDING: [&str; 2] = [
    "last_completed_onboarding",
    "electron:onboarding-projectless-completed",
];
const LOGIN_ENDPOINTS: [&str; 4] = [
    "chatgpt_base_url",
    "openai_base_url",
    "experimental_realtime_ws_base_url",
    "experimental_realtime_webrtc_call_base_url",
];

pub(super) fn read_config(path: &Path) -> Result<toml::Table> {
    read_if_present(path)?
        .map(|text| toml::from_str(&text).with_context(|| format!("parsing {}", path.display())))
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(super) fn inherit(source: &Path, home: &Path, config: &mut toml::Table) -> Result<()> {
    let mut defaults = read_config(&source.join("config.toml"))?;
    if let Some(name) = defaults.remove("profile")
        && let Some(mut profile) = name
            .as_str()
            .and_then(|name| defaults.get("profiles")?.get(name)?.as_table().cloned())
    {
        merge_missing(&mut profile, defaults);
        defaults = profile;
    }
    for key in LOGIN_ENDPOINTS {
        defaults.remove(key);
    }
    merge_missing(config, defaults);

    for entry in [
        "AGENTS.md",
        "AGENTS.override.md",
        "agents",
        "hooks.json",
        "prompts",
        "rules",
        "skills",
        "vendor_imports/skills",
    ] {
        provider::link_user_entry(&source.join(entry), &home.join(entry))?;
    }
    inherit_onboarding(source, home)
}

fn merge_missing(config: &mut toml::Table, defaults: toml::Table) {
    use toml::map::Entry;

    for (key, value) in defaults {
        let merge_entries = matches!(
            key.as_str(),
            "mcp_servers"
                | "model_providers"
                | "marketplaces"
                | "plugins"
                | "profiles"
                | "projects"
        );
        match config.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(value);
            }
            Entry::Occupied(mut entry) => {
                if let (Some(current), toml::Value::Table(defaults)) =
                    (entry.get_mut().as_table_mut(), value)
                {
                    if merge_entries {
                        // A named server or provider is one unit. Inheriting its
                        // headers independently could mix old keys with a new URL.
                        for (name, item) in defaults {
                            current.entry(name).or_insert(item);
                        }
                    } else {
                        merge_missing(current, defaults);
                    }
                }
            }
        }
    }
}

pub(super) fn apply_account_launch(
    config: &mut toml::Table,
    model: Option<&str>,
    reasoning: provider::ReasoningLaunch,
) {
    // Inherit user settings while keeping the selected account on the OAuth route.
    if config
        .get("model_provider")
        .and_then(toml::Value::as_str)
        .is_some_and(|provider| provider != "openai")
    {
        config.remove("model");
        config.remove("model_reasoning_effort");
    }
    config.insert("cli_auth_credentials_store".into(), "file".into());
    config.insert("model_provider".into(), "openai".into());
    for key in LOGIN_ENDPOINTS {
        config.remove(key);
    }
    config.remove("model_catalog_json");
    if let Some(model) = model {
        config.insert("model".into(), model.into());
        if matches!(reasoning, provider::ReasoningLaunch::Skip) {
            config.remove("model_reasoning_effort");
        }
    }
    if let provider::ReasoningLaunch::Effort(effort) = reasoning {
        config.insert("model_reasoning_effort".into(), effort.into());
    }
}

pub(super) fn read_if_present(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

pub(super) fn read_global_json(home: &Path) -> Result<Map<String, Value>> {
    let path = home.join(GLOBAL_JSON);
    read_if_present(&path)?
        .map(|text| {
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(super) fn settings_db(home: &Path, flags: OpenFlags) -> Result<Option<Connection>> {
    let path = home.join(GLOBAL_DB);
    if !path.is_file() {
        return Ok(None);
    }
    let db = Connection::open_with_flags(&path, flags)
        .with_context(|| format!("opening {}", path.display()))?;
    let supported = db
        .prepare("SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'settings'")?
        .next()?
        == State::Row;
    Ok(supported.then_some(db))
}

fn completed(value: &Value) -> bool {
    value == true || value.as_u64().is_some_and(|stamp| stamp > 0)
}

fn inherit_onboarding(source: &Path, home: &Path) -> Result<()> {
    let mut atoms = read_global_json(source)?
        .remove(ATOMS)
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if let Some(db) = settings_db(source, OpenFlags::new().with_read_only())? {
        let mut select =
            db.prepare("SELECT value FROM settings WHERE scope = 'atom' AND key = ?")?;
        for key in ONBOARDING {
            select.bind((1, key))?;
            if select.next()? == State::Row {
                let value = serde_json::from_str(&select.read::<String, _>(0)?)?;
                atoms.insert(key.into(), value);
            }
            select.reset()?;
        }
    }
    let values: Vec<_> = ONBOARDING
        .into_iter()
        .filter_map(|key| {
            atoms
                .get(key)
                .filter(|value| completed(value))
                .map(|value| (key, value))
        })
        .collect();
    if values.is_empty() {
        return Ok(());
    }

    // New desktops can import these keys from JSON. Desktops already using
    // SQLite read the existing store, so fill the same two entries there too.
    if let Some(db) = settings_db(home, OpenFlags::new().with_read_write())? {
        db.execute("BEGIN IMMEDIATE")?;
        let mut insert = db.prepare(
            "INSERT INTO settings (scope, key, value) VALUES ('atom', ?, ?) \
             ON CONFLICT(scope, key) DO UPDATE SET value = excluded.value \
             WHERE settings.value IN ('false', '0', 'null')",
        )?;
        for (key, value) in &values {
            insert.bind((1, *key))?;
            insert.bind((2, value.to_string().as_str()))?;
            insert.next()?;
            insert.reset()?;
        }
        db.execute("COMMIT")?;
    }

    let mut state = read_global_json(home)?;
    let entry = state
        .entry(ATOMS)
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(target) = entry.as_object_mut() else {
        return Ok(());
    };
    for (key, value) in values {
        if !target.get(key).is_some_and(completed) {
            target.insert(key.into(), value.clone());
        }
    }
    auth::atomic_write_private(&home.join(GLOBAL_JSON), &serde_json::to_vec(&state)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherits_configuration_without_replacing_instance_settings_or_auth() {
        let source = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            source.path().join("config.toml"),
            r#"
                model = 'gpt-5.4'
                profile = 'preferred'
                [profiles.preferred]
                model_reasoning_effort = 'high'
                chatgpt_base_url = 'https://custom.example.invalid'
                [desktop]
                theme = 'light'
                [mcp_servers.local]
                command = 'fixture-mcp'
                [mcp_servers.remote]
                url = 'https://old.example.invalid'
                bearer_token_env_var = 'OLD_SERVER_TOKEN'
            "#,
        )
        .unwrap();
        std::fs::write(source.path().join("auth.json"), "default-account").unwrap();
        std::fs::write(home.path().join("auth.json"), "selected-account").unwrap();
        std::fs::write(source.path().join("AGENTS.md"), "shared instructions").unwrap();
        let mut config = toml::from_str(
            "[desktop]\ntheme = 'dark'\n[mcp_servers.remote]\nurl = 'https://new.example.invalid'",
        )
        .unwrap();

        inherit(source.path(), home.path(), &mut config).unwrap();
        assert!(!config.contains_key("chatgpt_base_url"));
        config.insert(
            "chatgpt_base_url".into(),
            "https://previous.example.invalid".into(),
        );
        apply_account_launch(&mut config, None, provider::ReasoningLaunch::Skip);
        assert!(!config.contains_key("chatgpt_base_url"));
        assert_eq!(config["desktop"]["theme"].as_str(), Some("dark"));
        assert_eq!(config["model"].as_str(), Some("gpt-5.4"));
        assert_eq!(config["model_reasoning_effort"].as_str(), Some("high"));
        assert_eq!(config["model_provider"].as_str(), Some("openai"));
        assert_eq!(
            config["mcp_servers"]["local"]["command"].as_str(),
            Some("fixture-mcp")
        );
        assert!(
            !config["mcp_servers"]["remote"]
                .as_table()
                .unwrap()
                .contains_key("bearer_token_env_var")
        );
        assert_eq!(
            std::fs::read_to_string(home.path().join("auth.json")).unwrap(),
            "selected-account"
        );
        assert_eq!(
            std::fs::read_to_string(home.path().join("AGENTS.md")).unwrap(),
            "shared instructions"
        );
    }

    #[test]
    fn inherits_only_onboarding_and_repairs_existing_sqlite_state() {
        let source = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let fresh = tempfile::tempdir().unwrap();
        std::fs::write(
            source.path().join(GLOBAL_JSON),
            serde_json::json!({ATOMS: {ONBOARDING[0]: 42, "other-account-draft": "private"}})
                .to_string(),
        )
        .unwrap();
        std::fs::write(home.path().join(GLOBAL_JSON), r#"{"existing":"keep"}"#).unwrap();

        let create =
            "CREATE TABLE settings (scope TEXT, key TEXT, value TEXT, PRIMARY KEY (scope, key))";
        for dir in [source.path(), home.path()] {
            Connection::open(dir.join(GLOBAL_DB))
                .unwrap()
                .execute(create)
                .unwrap();
        }
        Connection::open(source.path().join(GLOBAL_DB)).unwrap().execute(
            "INSERT INTO settings VALUES ('atom', 'electron:onboarding-projectless-completed', 'true'),
              ('atom', 'other-account-draft', '\"private\"')",
        ).unwrap();
        Connection::open(home.path().join(GLOBAL_DB)).unwrap().execute(
            "INSERT INTO settings VALUES ('atom', 'electron:onboarding-projectless-completed', 'false'),
              ('atom', 'last_completed_onboarding', '99')",
        ).unwrap();

        for target in [fresh.path(), home.path()] {
            inherit_onboarding(source.path(), target).unwrap();
            let state = read_global_json(target).unwrap();
            assert_eq!(
                state[ATOMS],
                serde_json::json!({ONBOARDING[0]: 42, ONBOARDING[1]: true})
            );
        }
        assert!(!fresh.path().join(GLOBAL_DB).exists());
        assert_eq!(read_global_json(home.path()).unwrap()["existing"], "keep");
        let db = Connection::open(home.path().join(GLOBAL_DB)).unwrap();
        let rows: Vec<_> = db
            .prepare("SELECT value FROM settings ORDER BY key")
            .unwrap()
            .into_iter()
            .map(|row| row.unwrap().read::<&str, _>(0).to_owned())
            .collect();
        assert_eq!(rows, ["true", "99"]);
    }
}
