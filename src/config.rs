use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const CONFIG_DIRECTORY_ENV: &str = "HERDR_PLUGIN_CONFIG_DIR";
pub const STATE_DIRECTORY_ENV: &str = "HERDR_PLUGIN_STATE_DIR";
pub const DEFAULT_MAXIMUM_GAP_SECONDS: u64 = 120;
pub const DEFAULT_SAMPLE_INTERVAL_MILLISECONDS: u64 = 50;
pub const DEFAULT_CORRELATION_WINDOW_MILLISECONDS: u64 = 150;
pub const DEFAULT_FOCUS_SECONDS: i64 = 25 * 60;
pub const DEFAULT_AWAY_GRACE_SECONDS: i64 = 3 * 60;
pub const DEFAULT_WORKSPACE_GRACE_SECONDS: i64 = 3 * 60;
pub const DEFAULT_BREAK_SECONDS: i64 = 5 * 60;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginPaths {
    pub config_directory: PathBuf,
    pub state_directory: PathBuf,
}

impl PluginPaths {
    pub fn discover() -> Result<Self> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set")?;
        Ok(Self {
            config_directory: env::var_os(CONFIG_DIRECTORY_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config/herdr-pomodoro")),
            state_directory: env::var_os(STATE_DIRECTORY_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/state/herdr-pomodoro")),
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_directory.join("config.json")
    }

    pub fn database_file(&self) -> PathBuf {
        self.state_directory.join("activity.sqlite3")
    }

    pub fn watcher_lock_file(&self) -> PathBuf {
        self.state_directory.join("watcher.lock")
    }

    pub fn ensure(&self) -> Result<()> {
        create_private_directory(&self.config_directory)?;
        create_private_directory(&self.state_directory)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct PluginConfig {
    pub maximum_gap_seconds: u64,
    pub sample_interval_milliseconds: u64,
    pub correlation_window_milliseconds: u64,
    pub focus_seconds: i64,
    pub away_grace_seconds: i64,
    pub workspace_grace_seconds: i64,
    pub break_seconds: i64,
    pub grace_expiry_policy: String,
    pub start_break_after_early_end: bool,
    pub remote_sampling_enabled: bool,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            maximum_gap_seconds: DEFAULT_MAXIMUM_GAP_SECONDS,
            sample_interval_milliseconds: DEFAULT_SAMPLE_INTERVAL_MILLISECONDS,
            correlation_window_milliseconds: DEFAULT_CORRELATION_WINDOW_MILLISECONDS,
            focus_seconds: DEFAULT_FOCUS_SECONDS,
            away_grace_seconds: DEFAULT_AWAY_GRACE_SECONDS,
            workspace_grace_seconds: DEFAULT_WORKSPACE_GRACE_SECONDS,
            break_seconds: DEFAULT_BREAK_SECONDS,
            grace_expiry_policy: "end_focus".to_owned(),
            start_break_after_early_end: true,
            remote_sampling_enabled: true,
        }
    }
}

impl PluginConfig {
    pub fn load(paths: &PluginPaths) -> Result<Self> {
        paths.ensure()?;
        let path = paths.config_file();
        if !path.exists() {
            let config = Self::default();
            let rendered = format!("{}\n", serde_json::to_string_pretty(&config)?);
            fs::write(&path, rendered).with_context(|| format!("write {}", path.display()))?;
            set_private_file_permissions(&path)?;
            return Ok(config);
        }
        let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let config: Self =
            serde_json::from_str(&raw).context("plugin config must be valid JSON")?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.maximum_gap_seconds == 0
            || self.sample_interval_milliseconds == 0
            || self.correlation_window_milliseconds == 0
            || self.focus_seconds <= 0
            || self.away_grace_seconds <= 0
            || self.workspace_grace_seconds <= 0
            || self.break_seconds <= 0
        {
            bail!("all configured durations must be positive");
        }
        if self.grace_expiry_policy.is_empty() {
            bail!("grace_expiry_policy must not be empty");
        }
        Ok(())
    }
}

fn create_private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn set_private_file_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_round_trips() {
        let raw = serde_json::to_string(&PluginConfig::default()).unwrap();
        let parsed: PluginConfig = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed, PluginConfig::default());
    }
}
