use std::env;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use plist::{Dictionary, Value};

use crate::config::{PluginPaths, set_private_file_permissions};

pub const LAUNCH_AGENT_LABEL: &str = "com.chmarax.herdr-pomodoro.watcher";

pub struct WatcherLock {
    file: File,
}

impl WatcherLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        file.try_lock_exclusive()
            .with_context(|| format!("another watcher owns {}", path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for WatcherLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub fn reconcile_macos_service(paths: &PluginPaths) -> Result<PathBuf> {
    paths.ensure()?;
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let agents = home.join("Library/LaunchAgents");
    fs::create_dir_all(&agents)?;
    let plist_path = agents.join(format!("{LAUNCH_AGENT_LABEL}.plist"));
    let executable = env::current_exe().context("resolve current Rust executable")?;
    let mut environment = Dictionary::new();
    environment.insert(
        "HERDR_PLUGIN_CONFIG_DIR".into(),
        Value::String(paths.config_directory.display().to_string()),
    );
    environment.insert(
        "HERDR_PLUGIN_STATE_DIR".into(),
        Value::String(paths.state_directory.display().to_string()),
    );
    environment.insert(
        "HERDR_BIN_PATH".into(),
        Value::String(env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into())),
    );
    environment.insert(
        "PATH".into(),
        Value::String(
            env::var("PATH")
                .unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".into()),
        ),
    );
    if let Ok(socket) = env::var("HERDR_SOCKET_PATH") {
        environment.insert("HERDR_SOCKET_PATH".into(), Value::String(socket));
    }
    let mut payload = Dictionary::new();
    payload.insert("Label".into(), Value::String(LAUNCH_AGENT_LABEL.into()));
    payload.insert(
        "EnvironmentVariables".into(),
        Value::Dictionary(environment),
    );
    payload.insert("KeepAlive".into(), Value::Boolean(true));
    payload.insert("RunAtLoad".into(), Value::Boolean(true));
    payload.insert("ProcessType".into(), Value::String("Background".into()));
    payload.insert(
        "ProgramArguments".into(),
        Value::Array(vec![
            Value::String(executable.display().to_string()),
            Value::String("watch".into()),
        ]),
    );
    payload.insert(
        "StandardErrorPath".into(),
        Value::String(
            paths
                .state_directory
                .join("watcher.stderr.log")
                .display()
                .to_string(),
        ),
    );
    payload.insert(
        "StandardOutPath".into(),
        Value::String(
            paths
                .state_directory
                .join("watcher.stdout.log")
                .display()
                .to_string(),
        ),
    );
    payload.insert("ThrottleInterval".into(), Value::Integer(10.into()));
    let mut rendered = Vec::new();
    Value::Dictionary(payload).to_writer_xml(&mut rendered)?;
    if fs::read(&plist_path).ok().as_deref() != Some(&rendered) {
        fs::write(&plist_path, rendered)?;
        set_private_file_permissions(&plist_path)?;
    }
    let uid = current_uid()?;
    let domain = format!("gui/{uid}");
    let service = format!("{domain}/{LAUNCH_AGENT_LABEL}");
    let _ = launchctl(&["bootout", &service]);
    let result = launchctl(&[
        "bootstrap",
        &domain,
        plist_path.to_str().context("invalid plist path")?,
    ]);
    if !result.0 {
        bail!("could not bootstrap watcher service: {}", result.2.trim());
    }
    let _ = launchctl(&["enable", &service]);
    let _ = launchctl(&["kickstart", "-k", &service]);
    Ok(plist_path)
}

fn launchctl(arguments: &[&str]) -> (bool, String, String) {
    match Command::new("launchctl").args(arguments).output() {
        Ok(output) => (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ),
        Err(error) => (false, String::new(), error.to_string()),
    }
}

fn current_uid() -> Result<u32> {
    let output = Command::new("/usr/bin/id").arg("-u").output()?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().parse()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("watcher.lock");
        let _first = WatcherLock::acquire(&path).unwrap();
        assert!(WatcherLock::acquire(&path).is_err());
    }
}
