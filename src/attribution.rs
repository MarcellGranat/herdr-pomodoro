use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionKind {
    Git,
    Jj,
    None,
    Transition,
}

impl fmt::Display for RevisionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Git => "git",
            Self::Jj => "jj",
            Self::None => "none",
            Self::Transition => "transition",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineIdentity {
    pub machine_id: String,
    pub kind: String,
    pub label: String,
    pub transport: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevisionMetadata {
    pub kind: RevisionKind,
    pub stable_id: String,
    pub commit_id: Option<String>,
    pub branch: Option<String>,
    pub bookmarks: Option<String>,
    pub description: Option<String>,
}

impl RevisionMetadata {
    pub fn unversioned() -> Self {
        Self {
            kind: RevisionKind::None,
            stable_id: "unversioned".to_owned(),
            commit_id: None,
            branch: None,
            bookmarks: None,
            description: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceAttribution {
    pub machine: MachineIdentity,
    pub runtime_workspace_id: String,
    pub workspace_path: PathBuf,
    pub repository_root: PathBuf,
    pub repository_identity: String,
    pub project_id: String,
    pub project_name: String,
    pub workspace_instance_id: String,
    pub revision: RevisionMetadata,
}

#[derive(Default)]
pub struct AttributionResolver {
    repository_cache: HashMap<(String, PathBuf), (PathBuf, String)>,
    revision_cache: HashMap<(String, PathBuf), RevisionMetadata>,
}

impl AttributionResolver {
    pub fn resolve(
        &mut self,
        machine: &MachineIdentity,
        runtime_workspace_id: &str,
        workspace_path: &Path,
        refresh_revision: bool,
    ) -> Result<WorkspaceAttribution> {
        let resolved_path = workspace_path
            .canonicalize()
            .unwrap_or_else(|_| workspace_path.to_path_buf());
        let key = (machine.machine_id.clone(), resolved_path.clone());
        let (repository_root, repository_identity) =
            if let Some(value) = self.repository_cache.get(&key) {
                value.clone()
            } else {
                let value = resolve_repository(machine, &resolved_path);
                self.repository_cache.insert(key.clone(), value.clone());
                value
            };
        if refresh_revision || !self.revision_cache.contains_key(&key) {
            self.revision_cache
                .insert(key.clone(), resolve_revision(&repository_root));
        }
        let revision = self
            .revision_cache
            .get(&key)
            .cloned()
            .unwrap_or_else(RevisionMetadata::unversioned);
        let project_id = stable_id(&["project", &machine.machine_id, &repository_identity]);
        let workspace_instance_id = stable_id(&[
            "workspace",
            &machine.machine_id,
            runtime_workspace_id,
            &resolved_path.to_string_lossy(),
        ]);
        let project_name = repository_root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_else(|| repository_root.to_str().unwrap_or("project"))
            .to_owned();
        Ok(WorkspaceAttribution {
            machine: machine.clone(),
            runtime_workspace_id: runtime_workspace_id.to_owned(),
            workspace_path: resolved_path,
            repository_root,
            repository_identity,
            project_id,
            project_name,
            workspace_instance_id,
            revision,
        })
    }
}

pub fn local_machine_identity() -> MachineIdentity {
    let label = hostname::get()
        .ok()
        .and_then(|value| value.into_string().ok())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "local".to_owned());
    MachineIdentity {
        machine_id: stable_id(&["machine", "local", &label]),
        kind: "local".to_owned(),
        label,
        transport: None,
    }
}

pub fn revision_for_interval(start: &RevisionMetadata, end: &RevisionMetadata) -> RevisionMetadata {
    if start.kind == end.kind && start.stable_id == end.stable_id {
        return end.clone();
    }
    RevisionMetadata {
        kind: RevisionKind::Transition,
        stable_id: format!(
            "{}:{}->{}:{}",
            start.kind, start.stable_id, end.kind, end.stable_id
        ),
        commit_id: None,
        branch: None,
        bookmarks: None,
        description: Some("Revision changed during interval; boundary unknown".to_owned()),
    }
}

pub fn revision_id(project_id: &str, revision: &RevisionMetadata) -> String {
    stable_id(&[
        "revision",
        project_id,
        &revision.kind.to_string(),
        &revision.stable_id,
    ])
}

pub fn stable_id(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(parts.join("\0").as_bytes());
    hex::encode(hasher.finalize())
}

fn resolve_repository(machine: &MachineIdentity, path: &Path) -> (PathBuf, String) {
    if let Some(root) = successful_path(run_probe("jj", &["root"], path)) {
        let identity = common_git_directory(&root).unwrap_or_else(|| root.clone());
        return (
            root,
            stable_id(&[
                "repository",
                &machine.machine_id,
                &identity.to_string_lossy(),
            ]),
        );
    }
    if let Some(root) = successful_path(run_probe("git", &["rev-parse", "--show-toplevel"], path)) {
        let identity = common_git_directory(&root).unwrap_or_else(|| root.clone());
        return (
            root,
            stable_id(&[
                "repository",
                &machine.machine_id,
                &identity.to_string_lossy(),
            ]),
        );
    }
    (
        path.to_path_buf(),
        stable_id(&["directory", &machine.machine_id, &path.to_string_lossy()]),
    )
}

fn common_git_directory(root: &Path) -> Option<PathBuf> {
    let output = run_probe("git", &["rev-parse", "--git-common-dir"], root).ok()?;
    if !output.status.success() {
        return None;
    }
    let value = PathBuf::from(output.stdout.trim());
    Some(
        if value.is_absolute() {
            value
        } else {
            root.join(value)
        }
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf()),
    )
}

fn resolve_revision(root: &Path) -> RevisionMetadata {
    if run_probe("jj", &["root"], root).is_ok_and(|output| output.status.success()) {
        let template = "change_id ++ \"\\n\" ++ commit_id ++ \"\\n\" ++ bookmarks.map(|b| b.name()).join(\",\") ++ \"\\n\" ++ description.first_line()";
        if let Ok(output) = run_probe(
            "jj",
            &["log", "-r", "@", "--no-graph", "-T", template],
            root,
        ) && output.status.success()
        {
            let lines: Vec<&str> = output.stdout.trim_end_matches('\n').split('\n').collect();
            if let Some(stable) = lines.first().filter(|value| !value.is_empty()) {
                return RevisionMetadata {
                    kind: RevisionKind::Jj,
                    stable_id: (*stable).to_owned(),
                    commit_id: optional_line(&lines, 1),
                    branch: None,
                    bookmarks: optional_line(&lines, 2),
                    description: optional_line(&lines, 3),
                };
            }
        }
    }
    if let Ok(output) = run_probe("git", &["rev-parse", "HEAD"], root)
        && output.status.success()
        && !output.stdout.trim().is_empty()
    {
        let commit = output.stdout.trim().to_owned();
        let branch = run_probe("git", &["branch", "--show-current"], root)
            .ok()
            .filter(|value| value.status.success())
            .map(|value| value.stdout.trim().to_owned())
            .filter(|value| !value.is_empty());
        return RevisionMetadata {
            kind: RevisionKind::Git,
            stable_id: commit.clone(),
            commit_id: Some(commit),
            branch,
            bookmarks: None,
            description: None,
        };
    }
    RevisionMetadata::unversioned()
}

struct ProbeOutput {
    status: std::process::ExitStatus,
    stdout: String,
}

fn run_probe(program: &str, arguments: &[&str], cwd: &Path) -> Result<ProbeOutput> {
    let output = Command::new(program)
        .args(arguments)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("run {program}"))?;
    Ok(ProbeOutput {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
}

fn successful_path(result: Result<ProbeOutput>) -> Option<PathBuf> {
    let output = result.ok()?;
    output
        .status
        .success()
        .then(|| PathBuf::from(output.stdout.trim()))
        .filter(|value| !value.as_os_str().is_empty())
}

fn optional_line(lines: &[&str], index: usize) -> Option<String> {
    lines
        .get(index)
        .filter(|value| !value.is_empty())
        .map(|value| (*value).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_match_sha256_contract() {
        assert_eq!(
            stable_id(&["a", "b"]),
            "59b271ae1bbcb1d31d41929817f4b16fb439eb4f31520b5ad1d5ce98920a7138"
        );
    }

    #[test]
    fn differing_revisions_become_transition() {
        let a = RevisionMetadata {
            kind: RevisionKind::Git,
            stable_id: "a".into(),
            commit_id: None,
            branch: None,
            bookmarks: None,
            description: None,
        };
        let b = RevisionMetadata {
            kind: RevisionKind::Git,
            stable_id: "b".into(),
            commit_id: None,
            branch: None,
            bookmarks: None,
            description: None,
        };
        assert_eq!(revision_for_interval(&a, &b).kind, RevisionKind::Transition);
    }
}
