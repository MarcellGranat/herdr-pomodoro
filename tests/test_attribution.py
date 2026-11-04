"""Tests for durable workspace and read-only revision attribution."""

from pathlib import Path

from herdr_pomodoro.attribution import (
    AttributionResolver,
    RevisionKind,
    RevisionMetadata,
    local_machine_identity,
    remote_machine_identity,
    revision_for_interval,
    run_probe,
)
from herdr_pomodoro.discovery import CommandResult


def test_identical_paths_on_different_machines_do_not_collide(tmp_path: Path) -> None:
    runner = _Runner(tmp_path, vcs="none")
    resolver = AttributionResolver(runner)

    local = resolver.resolve(
        machine=local_machine_identity("mac"), runtime_workspace_id="w1", workspace_path=tmp_path
    )
    remote = resolver.resolve(
        machine=remote_machine_identity("server", "ssh"),
        runtime_workspace_id="w1",
        workspace_path=tmp_path,
    )

    assert local.project_id != remote.project_id
    assert local.workspace_instance_id != remote.workspace_instance_id


def test_git_revision_and_common_repository_identity(tmp_path: Path) -> None:
    resolver = AttributionResolver(_Runner(tmp_path, vcs="git"))

    attribution = resolver.resolve(
        machine=local_machine_identity("mac"), runtime_workspace_id="w1", workspace_path=tmp_path
    )

    assert attribution.repository_root == tmp_path
    assert attribution.revision.kind is RevisionKind.GIT
    assert attribution.revision.commit_id == "abc123"
    assert attribution.revision.branch == "main"


def test_jj_uses_stable_change_id_with_details(tmp_path: Path) -> None:
    resolver = AttributionResolver(_Runner(tmp_path, vcs="jj"))

    attribution = resolver.resolve(
        machine=local_machine_identity("mac"), runtime_workspace_id="w1", workspace_path=tmp_path
    )

    assert attribution.revision.kind is RevisionKind.JUJUTSU
    assert attribution.revision.stable_id == "change-id"
    assert attribution.revision.commit_id == "commit-id"
    assert attribution.revision.bookmarks == "feature"


def test_unversioned_directory_has_explicit_revision(tmp_path: Path) -> None:
    attribution = AttributionResolver(_Runner(tmp_path, vcs="none")).resolve(
        machine=local_machine_identity("mac"), runtime_workspace_id="w1", workspace_path=tmp_path
    )

    assert attribution.revision == RevisionMetadata(RevisionKind.NONE, "unversioned")


def test_revision_transition_is_not_falsely_precise() -> None:
    start = RevisionMetadata(RevisionKind.GIT, "one", commit_id="one")
    end = RevisionMetadata(RevisionKind.GIT, "two", commit_id="two")

    transition = revision_for_interval(start, end)

    assert transition.kind is RevisionKind.TRANSITION
    assert "one" in transition.stable_id
    assert "boundary unknown" in (transition.description or "")


def test_missing_vcs_executable_fails_closed_without_crashing(tmp_path: Path) -> None:
    result = run_probe(("/definitely/missing/jj", "root"), tmp_path)

    assert result.returncode == 127


class _Runner:
    def __init__(self, root: Path, *, vcs: str) -> None:
        self.root = root
        self.vcs = vcs

    def __call__(  # noqa: PLR0911
        self, arguments: tuple[str, ...], _cwd: Path
    ) -> CommandResult:
        if arguments == ("jj", "root"):
            output = f"{self.root}\n" if self.vcs == "jj" else ""
            return CommandResult(0 if output else 1, output, "")
        if arguments[:2] == ("jj", "log"):
            return CommandResult(0, "change-id\ncommit-id\nfeature\nDescription\n", "")
        if arguments == ("git", "rev-parse", "--show-toplevel"):
            output = f"{self.root}\n" if self.vcs == "git" else ""
            return CommandResult(0 if output else 1, output, "")
        if arguments == ("git", "rev-parse", "--git-common-dir"):
            output = ".git\n" if self.vcs in {"git", "jj"} else ""
            return CommandResult(0 if output else 1, output, "")
        if arguments == ("git", "rev-parse", "HEAD"):
            output = "abc123\n" if self.vcs == "git" else ""
            return CommandResult(0 if output else 1, output, "")
        if arguments == ("git", "branch", "--show-current"):
            return CommandResult(0, "main\n", "")
        return CommandResult(1, "", "unsupported")
