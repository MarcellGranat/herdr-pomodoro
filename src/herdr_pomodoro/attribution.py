"""Durable machine, project, workspace, and read-only VCS attribution."""

import hashlib
import platform
import subprocess
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
from typing import Callable

from herdr_pomodoro.discovery import CommandResult


class RevisionKind(StrEnum):
    """Supported revision identity sources."""

    GIT = "git"
    JUJUTSU = "jj"
    NONE = "none"
    TRANSITION = "transition"


@dataclass(frozen=True, slots=True)
class MachineIdentity:
    """Durable machine provenance used to prevent path collisions."""

    machine_id: str
    kind: str
    label: str
    transport: str | None = None


@dataclass(frozen=True, slots=True)
class RevisionMetadata:
    """Read-only revision identity and display details."""

    kind: RevisionKind
    stable_id: str
    commit_id: str | None = None
    branch: str | None = None
    bookmarks: str | None = None
    description: str | None = None


@dataclass(frozen=True, slots=True)
class WorkspaceAttribution:
    """Durable hierarchy for one runtime Herdr workspace."""

    machine: MachineIdentity
    runtime_workspace_id: str
    workspace_path: Path
    repository_root: Path
    repository_identity: str
    project_id: str
    project_name: str
    workspace_instance_id: str
    revision: RevisionMetadata


ProbeRunner = Callable[[tuple[str, ...], Path], CommandResult]


def run_probe(arguments: tuple[str, ...], cwd: Path) -> CommandResult:
    """Run a read-only VCS metadata command."""
    try:
        completed = subprocess.run(  # noqa: S603
            arguments,
            cwd=cwd,
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError as error:
        return CommandResult(127, "", str(error))
    return CommandResult(completed.returncode, completed.stdout, completed.stderr)


def local_machine_identity(label: str | None = None) -> MachineIdentity:
    """Return a stable local machine identity without external writes."""
    resolved_label = label or platform.node() or "local"
    return MachineIdentity(
        machine_id=_stable_id("machine", "local", resolved_label),
        kind="local",
        label=resolved_label,
    )


def remote_machine_identity(label: str, transport: str) -> MachineIdentity:
    """Return a distinct remote identity even for identical filesystem paths."""
    return MachineIdentity(
        machine_id=_stable_id("machine", "remote", label, transport),
        kind="remote",
        label=label,
        transport=transport,
    )


class AttributionResolver:
    """Resolve and cache read-only workspace and revision metadata."""

    def __init__(self, runner: ProbeRunner = run_probe) -> None:
        """Initialize caches for one watcher process."""
        self._runner = runner
        self._repository_cache: dict[tuple[str, Path], tuple[Path, str]] = {}
        self._revision_cache: dict[tuple[str, Path], RevisionMetadata] = {}

    def resolve(
        self,
        *,
        machine: MachineIdentity,
        runtime_workspace_id: str,
        workspace_path: Path,
        refresh_revision: bool = False,
    ) -> WorkspaceAttribution:
        """Resolve durable hierarchy and current revision for a workspace."""
        resolved_path = workspace_path.expanduser().resolve()
        cache_key = (machine.machine_id, resolved_path)
        repository_root, repository_identity = self._repository_cache.get(
            cache_key,
            self._resolve_repository(machine, resolved_path),
        )
        self._repository_cache[cache_key] = (repository_root, repository_identity)
        if refresh_revision or cache_key not in self._revision_cache:
            self._revision_cache[cache_key] = self._resolve_revision(repository_root)
        revision = self._revision_cache[cache_key]
        project_id = _stable_id("project", machine.machine_id, repository_identity)
        workspace_instance_id = _stable_id(
            "workspace",
            machine.machine_id,
            runtime_workspace_id,
            str(resolved_path),
        )
        return WorkspaceAttribution(
            machine=machine,
            runtime_workspace_id=runtime_workspace_id,
            workspace_path=resolved_path,
            repository_root=repository_root,
            repository_identity=repository_identity,
            project_id=project_id,
            project_name=repository_root.name or str(repository_root),
            workspace_instance_id=workspace_instance_id,
            revision=revision,
        )

    def _resolve_repository(self, machine: MachineIdentity, path: Path) -> tuple[Path, str]:
        jj_root = self._runner(("jj", "root"), path)
        if jj_root.returncode == 0 and jj_root.stdout.strip():
            root = Path(jj_root.stdout.strip()).resolve()
            common = self._runner(("git", "rev-parse", "--git-common-dir"), root)
            identity_path = (
                _resolve_command_path(root, common.stdout) if common.returncode == 0 else root
            )
            return root, _stable_id("repository", machine.machine_id, str(identity_path))
        git_root = self._runner(("git", "rev-parse", "--show-toplevel"), path)
        if git_root.returncode == 0 and git_root.stdout.strip():
            root = Path(git_root.stdout.strip()).resolve()
            common = self._runner(("git", "rev-parse", "--git-common-dir"), root)
            identity_path = (
                _resolve_command_path(root, common.stdout) if common.returncode == 0 else root
            )
            return root, _stable_id("repository", machine.machine_id, str(identity_path))
        return path, _stable_id("directory", machine.machine_id, str(path))

    def _resolve_revision(self, root: Path) -> RevisionMetadata:
        jj_root = self._runner(("jj", "root"), root)
        if jj_root.returncode == 0:
            template = (
                'change_id ++ "\\n" ++ commit_id ++ "\\n" ++ '
                'bookmarks.map(|b| b.name()).join(",") ++ "\\n" ++ description.first_line()'
            )
            result = self._runner(("jj", "log", "-r", "@", "--no-graph", "-T", template), root)
            lines = result.stdout.rstrip("\n").split("\n") if result.returncode == 0 else []
            if lines:
                return RevisionMetadata(
                    kind=RevisionKind.JUJUTSU,
                    stable_id=lines[0],
                    commit_id=_line(lines, 1),
                    bookmarks=_line(lines, 2),
                    description=_line(lines, 3),
                )
        commit = self._runner(("git", "rev-parse", "HEAD"), root)
        if commit.returncode == 0 and commit.stdout.strip():
            branch = self._runner(("git", "branch", "--show-current"), root)
            commit_id = commit.stdout.strip()
            return RevisionMetadata(
                kind=RevisionKind.GIT,
                stable_id=commit_id,
                commit_id=commit_id,
                branch=branch.stdout.strip() or None if branch.returncode == 0 else None,
            )
        return RevisionMetadata(RevisionKind.NONE, "unversioned")


def revision_for_interval(
    start: RevisionMetadata,
    end: RevisionMetadata,
) -> RevisionMetadata:
    """Return one revision or an explicit imprecise transition identity."""
    if start.kind == end.kind and start.stable_id == end.stable_id:
        return end
    stable_id = f"{start.kind}:{start.stable_id}->{end.kind}:{end.stable_id}"
    return RevisionMetadata(
        kind=RevisionKind.TRANSITION,
        stable_id=stable_id,
        description="Revision changed during interval; boundary unknown",
    )


def revision_id(project_id: str, revision: RevisionMetadata) -> str:
    """Return the durable database identity for project revision metadata."""
    return _stable_id("revision", project_id, revision.kind, revision.stable_id)


def _stable_id(*parts: object) -> str:
    encoded = "\0".join(str(part) for part in parts).encode()
    return hashlib.sha256(encoded).hexdigest()


def _resolve_command_path(root: Path, output: str) -> Path:
    value = Path(output.strip())
    return value.resolve() if value.is_absolute() else (root / value).resolve()


def _line(lines: list[str], index: int) -> str | None:
    if index >= len(lines) or not lines[index]:
        return None
    return lines[index]
