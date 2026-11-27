use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use sysinfo::{ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind};

use crate::{auth, profile, provider};
use provider::ReasoningLaunch;

mod settings;
mod shared;
mod workspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaunchTarget {
    Desktop,
    Cli,
}

impl LaunchTarget {
    pub(crate) fn load(alias: &str, is_provider: bool) -> Self {
        let saved = account_dir(alias, is_provider)
            .ok()
            .and_then(|dir| std::fs::read_to_string(dir.join("launch-target")).ok());
        match saved.as_deref() {
            Some("CLI") => Self::Cli,
            _ => Self::Desktop,
        }
    }

    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Desktop => Self::Cli,
            Self::Cli => Self::Desktop,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Desktop => "Desktop",
            Self::Cli => "CLI",
        }
    }

    pub(crate) fn remember(self, alias: &str, is_provider: bool) {
        let saved = account_dir(alias, is_provider).and_then(|dir| {
            anyhow::ensure!(dir.is_dir(), "profile directory no longer exists");
            auth::atomic_write_private(&dir.join("launch-target"), self.label().as_bytes())
        });
        if let Err(error) = saved {
            tracing::warn!(%alias, %error, "could not save launch preference");
        }
    }
}

fn account_dir(alias: &str, is_provider: bool) -> Result<PathBuf> {
    profile::validate_alias(alias)?;
    if is_provider {
        provider::provider_dir(alias)
    } else {
        Ok(auth::profiles_dir()?.join(alias))
    }
}

pub(crate) enum LaunchOutcome {
    Started,
    Activated,
}

pub(crate) fn launch(
    alias: &str,
    model: Option<&str>,
    reasoning: ReasoningLaunch,
) -> Result<LaunchOutcome> {
    let is_provider = provider::exists(alias);
    let dir = std::path::absolute(account_dir(alias, is_provider)?)?;
    let account_auth = dir.join("auth.json");
    let _transaction = profile::lock_auth_transaction()?;
    let windows = if is_provider {
        dir.join("desktop-home")
    } else {
        dir
    };
    let executable = desktop_executable()?;
    let processes = desktop_processes(&executable)?;
    let running = running_window(&windows, &processes).or_else(|| {
        if is_provider {
            None
        } else {
            running_window_for_same_account(alias, &account_auth, &processes)
        }
    });
    if let Some(data) = running {
        activate_window(&executable, &data)?;
        LaunchTarget::Desktop.remember(alias, is_provider);
        tracing::info!(alias, "activated existing Codex Desktop window");
        return Ok(LaunchOutcome::Activated);
    }

    let data = windows.join("desktop-window-1");
    let home = data.join(if is_provider {
        "codex-home"
    } else {
        shared::HOME
    });
    let mut command = desktop_command(&executable, &data, &home);
    let config_path = home.join("config.toml");
    let mut config = settings::read_config(&config_path)?;
    let inherited = home.join(settings::INHERITED);
    let needs_settings = !inherited.is_file();
    let inherited_workspace = home.join(workspace::INHERITED);
    let needs_workspace = !is_provider && !inherited_workspace.is_file();
    let source = if needs_settings || !is_provider {
        let mut source = std::path::absolute(auth::user_codex_home()?)?;
        if source.starts_with(std::path::absolute(auth::app_home()?)?) {
            source = dirs::home_dir()
                .context("finding the default Codex home")?
                .join(".codex");
        }
        if needs_settings {
            settings::inherit(&source, &home, &mut config)?;
        }
        Some(source)
    } else {
        None
    };
    if is_provider {
        let p = provider::load(alias)?;
        config.remove("model_reasoning_effort");
        // Rebuild the definition from the saved provider, including removals.
        // Headers from an older endpoint must never be sent to a replacement.
        if let Some(providers) = config
            .get_mut("model_providers")
            .and_then(toml::Value::as_table_mut)
        {
            providers.remove(&p.provider_id);
        }
        config.remove("web_search");
        let args = p.codex_config_args_from_saved_catalog_at(model, reasoning, &home)?;
        for [_, assignment] in args.as_chunks::<2>().0 {
            let (key, raw) = assignment
                .split_once('=')
                .context("expected provider key=value")?;
            provider::insert_toml_override(
                &mut config,
                key,
                provider::provider_override_value(raw),
            )?;
        }
        let (key, value) = p.launch_env();
        command.env(key, value);
    } else {
        sync_account_profile_locked(alias, &account_auth);
        auth::ensure_file_credentials_store()?;
        let saved = auth::read_auth(&account_auth)?;
        auth::validate_managed_auth_value(&saved)?;
        let source = source.as_deref().context("finding the common Codex home")?;
        let sqlite_home = shared::connect(source, &home)?;
        config.insert(
            "sqlite_home".into(),
            sqlite_home
                .to_str()
                .context("encoding the common Codex database path")?
                .into(),
        );
        config.insert(
            "log_dir".into(),
            home.join("log")
                .to_str()
                .context("encoding the desktop log directory")?
                .into(),
        );
        if needs_workspace {
            workspace::inherit_shared_layout(source, &home, &sqlite_home, &saved)?;
        }
        let window_auth = home.join("auth.json");
        if auth::read_auth(&window_auth).ok().as_ref() != Some(&saved) {
            auth::write_auth(&window_auth, &saved)?;
        }
        let (_, refresh_token) = auth::extract_tokens(&saved);
        write_token_marker(&home, refresh_token.as_deref())?;
        settings::apply_account_launch(&mut config, model, reasoning);
    }

    // The inherited client configuration can point to a shared runtime database.
    if is_provider {
        config.remove("sqlite_home");
    }
    auth::atomic_write_private(&config_path, toml::to_string(&config)?.as_bytes())?;
    if needs_settings {
        auth::atomic_write_private(&inherited, b"1")?;
    }
    if needs_workspace {
        auth::atomic_write_private(&inherited_workspace, b"1")?;
    }
    let mut child = command.spawn().context("starting Codex Desktop")?;
    if let Err(error) = std::fs::write(data.join("launcher.pid"), child.id().to_string()) {
        stop_launcher(&mut child);
        return Err(error).context("tracking Codex Desktop");
    }
    let account_alias = alias.to_owned();
    // The next codex-switch command can also collect rotations after the TUI exits.
    std::thread::spawn(move || {
        let _ = child.wait();
        if !is_provider {
            sync_account_profile(&account_alias, &account_auth);
        }
    });
    LaunchTarget::Desktop.remember(alias, is_provider);
    tracing::info!(alias, "started Codex Desktop window");
    Ok(LaunchOutcome::Started)
}

fn desktop_command(executable: &Path, data: &Path, home: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .env("CODEX_HOME", home)
        .env("CODEX_ELECTRON_USER_DATA_PATH", data)
        .env_remove("CODEX_CLI_PATH")
        .env_remove("CODEX_SQLITE_HOME")
        .arg(user_data_arg(data))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn activate_window(executable: &Path, data: &Path) -> Result<()> {
    // The desktop app's single-instance lock forwards this invocation to the
    // running window for this particular user-data-dir. It restores and focuses
    // that window without applying another set of account or model settings.
    let shared_home = data.join(shared::HOME);
    let home = if shared_home.join("config.toml").is_file() {
        shared_home
    } else {
        data.join("codex-home")
    };
    let mut relay = desktop_command(executable, data, &home)
        .spawn()
        .context("activating the existing Codex Desktop window")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = relay.try_wait()? {
            anyhow::ensure!(status.success(), "desktop activation exited with {status}");
            return Ok(());
        }
        if Instant::now() >= deadline {
            stop_launcher(&mut relay);
            anyhow::bail!("existing Codex Desktop did not accept the activation request");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn stop_launcher(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

const TOKEN_MARKER: &str = "source-refresh.sha256";

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub(crate) fn write_token_marker(home: &Path, refresh_token: Option<&str>) -> Result<()> {
    let token = refresh_token.filter(|token| !token.is_empty());
    let hash = token.map(token_hash).unwrap_or_default();
    auth::atomic_write_private(&home.join(TOKEN_MARKER), hash.as_bytes())
}

pub(crate) fn sync_profiles() {
    for alias in profile::list_profiles().unwrap_or_default() {
        if let Ok(path) = profile::profile_auth_path(&alias) {
            sync_account_profile(&alias, &path);
        }
    }
}

pub(crate) fn sync_account_profile(alias: &str, path: &Path) {
    if !path
        .parent()
        .is_some_and(|parent| account_homes(parent).any(|home| home.join(TOKEN_MARKER).is_file()))
    {
        return;
    }
    match profile::lock_auth_transaction() {
        Ok(_transaction) => sync_account_profile_locked(alias, path),
        Err(error) => {
            tracing::warn!(%alias, %error, "could not lock refreshed desktop credentials")
        }
    }
}

fn sync_account_profile_locked(alias: &str, path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    for home in account_homes(parent) {
        if let Err(error) = adopt_window_auth(alias, path, &home) {
            tracing::warn!(%alias, %error, "could not save refreshed desktop credentials");
        }
    }
}

fn account_homes(parent: &Path) -> impl Iterator<Item = PathBuf> {
    window_dirs(parent).flat_map(|data| [data.join("codex-home"), data.join(shared::HOME)])
}

fn adopt_window_auth(alias: &str, saved_path: &Path, home: &Path) -> Result<()> {
    let Ok(marker) = std::fs::read_to_string(home.join(TOKEN_MARKER)) else {
        return Ok(());
    };
    if marker.is_empty() {
        return Ok(());
    }
    let Ok(incoming) = auth::read_auth(&home.join("auth.json")) else {
        // Leave the saved profile intact when a window signs out.
        return Ok(());
    };
    let (Some(access), Some(refresh)) = auth::extract_tokens(&incoming) else {
        return Ok(());
    };
    let Some(id) = incoming
        .pointer("/tokens/id_token")
        .and_then(|v| v.as_str())
    else {
        return Ok(());
    };
    if access.is_empty() || refresh.is_empty() || id.is_empty() {
        return Ok(());
    }
    let saved = auth::read_auth(saved_path)?;
    let (_, Some(source)) = auth::extract_tokens(&saved) else {
        return Ok(());
    };
    if marker != token_hash(&source) || !profile::is_newer_desktop_auth(alias, &saved, &incoming) {
        return Ok(());
    }
    if profile::update_profile_tokens_locked(alias, &source, id, &access, &refresh)? {
        write_token_marker(home, Some(&refresh))?;
    }
    Ok(())
}

fn window_dirs(root: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            name.to_str()?
                .strip_prefix("desktop-window-")?
                .parse::<u32>()
                .ok()?;
            Some(entry.path())
        })
}

const USER_DATA_FLAG: &str = "--user-data-dir=";

fn user_data_arg(data: &Path) -> OsString {
    let mut arg = OsString::from(USER_DATA_FLAG);
    arg.push(data);
    arg
}

struct DesktopProcess {
    pid: u32,
    user_data_arg: OsString,
}

fn desktop_data_arg(args: &[OsString], image: &Path) -> Option<OsString> {
    if args
        .iter()
        .any(|arg| arg.as_encoded_bytes().starts_with(b"--type="))
    {
        return None;
    }
    if let Some(arg) = args.iter().find(|arg| {
        arg.as_encoded_bytes()
            .starts_with(USER_DATA_FLAG.as_bytes())
    }) {
        return Some(arg.clone());
    }
    #[cfg(target_os = "linux")]
    if let [command] = args {
        use std::os::unix::ffi::OsStringExt;
        // The Linux desktop sets its process title to image + the original
        // argument. This launcher passes one argument, whose path can contain spaces.
        let arg = command
            .as_encoded_bytes()
            .strip_prefix(image.as_os_str().as_encoded_bytes())?
            .strip_prefix(b" ")?;
        return arg
            .starts_with(USER_DATA_FLAG.as_bytes())
            .then(|| OsString::from_vec(arg.to_vec()));
    }
    let _ = image;
    None
}

fn same_path(left: &OsStr, right: &OsStr) -> bool {
    if cfg!(windows) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn desktop_processes(launcher: &Path) -> Result<Vec<DesktopProcess>> {
    let image = launcher
        .canonicalize()
        .context("resolving Codex Desktop executable for process matching")?;
    // The official Linux package's launcher execs ChatGPT beside the script.
    #[cfg(target_os = "linux")]
    let image = if image.file_name() == Some(OsStr::new("codex-launcher")) {
        image.with_file_name("ChatGPT").canonicalize()?
    } else {
        image
    };
    let mut system = System::new();
    let refresh = ProcessRefreshKind::nothing().with_exe(UpdateKind::Always);
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
    let candidates: Vec<_> = system
        .processes()
        .iter()
        .filter(|(_, process)| {
            process
                .exe()
                .and_then(Path::file_name)
                .zip(image.file_name())
                .is_some_and(|(actual, expected)| same_path(actual, expected))
        })
        .map(|(&pid, _)| pid)
        .collect();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&candidates),
        true,
        refresh.with_cmd(UpdateKind::Always),
    );
    Ok(candidates
        .into_iter()
        .filter_map(|pid| {
            let process = system.process(pid)?;
            let process_image = process.exe()?;
            if matches!(
                process.status(),
                ProcessStatus::Dead | ProcessStatus::Zombie
            ) || !process_image
                .canonicalize()
                .ok()
                .is_some_and(|path| same_path(path.as_os_str(), image.as_os_str()))
            {
                return None;
            }
            Some(DesktopProcess {
                pid: pid.as_u32(),
                user_data_arg: desktop_data_arg(process.cmd(), process_image)?,
            })
        })
        .collect())
}

fn running_window(root: &Path, processes: &[DesktopProcess]) -> Option<PathBuf> {
    window_dirs(root)
        .filter_map(|data| {
            let flag = user_data_arg(&data);
            let mut matches = processes
                .iter()
                .filter(|process| same_path(&process.user_data_arg, &flag))
                .peekable();
            matches.peek()?;
            let saved_pid = std::fs::read_to_string(data.join("launcher.pid"))
                .ok()
                .and_then(|pid| pid.parse::<u32>().ok());
            Some((!matches.any(|process| Some(process.pid) == saved_pid), data))
        })
        .min()
        .map(|(_, data)| data)
}

pub(crate) fn ensure_window_closed(root: &Path, alias: &str) -> Result<()> {
    if window_dirs(root).next().is_some()
        && let Ok(executable) = desktop_executable()
        && running_window(
            &std::path::absolute(root)?,
            &desktop_processes(&executable)?,
        )
        .is_some()
    {
        anyhow::bail!("close the Codex Desktop window for '{alias}' first");
    }
    for data in window_dirs(root) {
        shared::prepare_profile_move(&std::path::absolute(data.join(shared::HOME))?)?;
    }
    Ok(())
}

fn running_window_for_same_account(
    alias: &str,
    auth_path: &Path,
    processes: &[DesktopProcess],
) -> Option<PathBuf> {
    let original = auth::read_auth(auth_path).ok()?;
    let identity = profile::extract_identity(&original);
    let (Some(account_id), Some(email)) = (identity.account_id, identity.email) else {
        return None;
    };
    for other in profile::list_profiles().ok()? {
        if other == alias {
            continue;
        }
        let Ok(path) = profile::profile_auth_path(&other) else {
            continue;
        };
        let Some(data) = path.parent().and_then(|dir| running_window(dir, processes)) else {
            continue;
        };
        if let Ok(value) = auth::read_auth(&path) {
            let identity = profile::extract_identity(&value);
            if identity.account_id.as_ref() == Some(&account_id)
                && identity.email.as_ref() == Some(&email)
            {
                return Some(data);
            }
        }
    }
    None
}

fn desktop_executable() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_SWITCH_DESKTOP_PATH") {
        let path = PathBuf::from(path);
        anyhow::ensure!(
            path.is_file(),
            "Desktop executable missing at {}",
            path.display()
        );
        return Ok(std::path::absolute(path)?);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = r#"[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$p = Get-AppxPackage -Name OpenAI.Codex | Select-Object -First 1
if ($p) {
  $app = (Get-AppxPackageManifest $p).Package.Applications.Application | Where-Object Id -eq 'App' | Select-Object -First 1
  if ($app) { Join-Path $p.InstallLocation $app.Executable }
}"#;
        let output = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .creation_flags(0x08000000)
            .output()
            .context("looking up the installed Codex Desktop package")?;
        let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        if output.status.success() && path.is_file() {
            return Ok(path);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let locations = [
            Some(PathBuf::from("/Applications")),
            dirs::home_dir().map(|p| p.join("Applications")),
        ];
        for name in ["ChatGPT", "Codex"] {
            for apps in locations.iter().flatten() {
                let bundle = apps.join(format!("{name}.app/Contents"));
                if !bundle.join("Frameworks").is_dir() {
                    continue;
                }
                let identifier = Command::new("/usr/libexec/PlistBuddy")
                    .args(["-c", "Print :CFBundleIdentifier"])
                    .arg(bundle.join("Info.plist"))
                    .output()?;
                if identifier.status.success()
                    && String::from_utf8_lossy(&identifier.stdout).trim() == "com.openai.codex"
                {
                    let path = bundle.join("MacOS").join(name);
                    if path.is_file() {
                        return Ok(path);
                    }
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    for path in [
        "/usr/bin/chatgpt",
        "/opt/Codex/codex",
        "/opt/codex-desktop/codex-desktop",
        "/usr/bin/codex-desktop",
    ] {
        if Path::new(path).is_file() {
            return Ok(path.into());
        }
    }
    anyhow::bail!(
        "Codex Desktop was not found. Set CODEX_SWITCH_DESKTOP_PATH to its GUI executable."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_matching_ignores_an_unrelated_saved_pid_and_recovers_by_directory() {
        let temp = tempfile::tempdir().unwrap();
        let window = temp.path().join("desktop-window-1");
        std::fs::create_dir(&window).unwrap();
        std::fs::write(window.join("launcher.pid"), "42").unwrap();

        let mut processes = vec![DesktopProcess {
            pid: 42,
            user_data_arg: user_data_arg(&temp.path().join("desktop-window-10")),
        }];
        assert_eq!(running_window(temp.path(), &processes), None);

        processes.push(DesktopProcess {
            pid: 43,
            user_data_arg: user_data_arg(&window),
        });
        assert_eq!(
            running_window(temp.path(), &processes),
            Some(window.clone())
        );
        std::fs::remove_file(window.join("launcher.pid")).unwrap();
        assert_eq!(running_window(temp.path(), &processes), Some(window));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn desktop_matching_preserves_spaces_in_the_linux_process_title() {
        let image = Path::new("/opt/ChatGPT App/ChatGPT");
        let mut command = image.as_os_str().to_os_string();
        let flag = user_data_arg(Path::new("/home/test/My Desktop/desktop-window-1"));
        command.push(" ");
        command.push(&flag);
        assert_eq!(desktop_data_arg(&[command], image), Some(flag));
    }
}
