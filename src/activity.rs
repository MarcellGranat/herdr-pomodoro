use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::attribution::{
    AttributionResolver, MachineIdentity, RevisionKind, RevisionMetadata, WorkspaceAttribution,
    local_machine_identity, revision_for_interval, revision_id,
};
use crate::database::Database;
use crate::model::HumanInputObservation;

const NANOSECONDS_PER_SECOND: i64 = 1_000_000_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IngestResult {
    pub duplicate: bool,
    pub credited_duration_ns: i64,
    pub segment_id: Option<i64>,
}

struct StoredAnchor {
    workspace_id: String,
    machine_id: String,
    project_id: String,
    workspace_instance_id: String,
    revision: RevisionMetadata,
    input_wall_ns: i64,
    observed_monotonic_ns: i64,
}

pub struct ActivityEngine {
    database: Database,
    maximum_gap_ns: i64,
    resolver: AttributionResolver,
    machine: MachineIdentity,
}

impl ActivityEngine {
    pub fn new(database: Database, maximum_gap_seconds: u64) -> Self {
        Self {
            database,
            maximum_gap_ns: i64::try_from(maximum_gap_seconds)
                .unwrap_or(i64::MAX)
                .saturating_mul(NANOSECONDS_PER_SECOND),
            resolver: AttributionResolver::default(),
            machine: local_machine_identity(),
        }
    }

    pub fn ingest(&mut self, observation: &HumanInputObservation) -> Result<IngestResult> {
        let fallback = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("/"));
        let attribution = self.resolver.resolve(
            &self.machine,
            &observation.workspace_id,
            observation.cwd.as_deref().unwrap_or(&fallback),
            true,
        )?;
        self.ingest_with_attribution(observation, &attribution)
    }

    pub fn ingest_with_attribution(
        &self,
        observation: &HumanInputObservation,
        attribution: &WorkspaceAttribution,
    ) -> Result<IngestResult> {
        let mut connection = self.database.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let claimed = transaction.execute(
            "INSERT OR IGNORE INTO processed_observations(observation_id, observed_monotonic_ns, processed_at) VALUES(?1, ?2, ?3)",
            params![observation.observation_id, observation.observed_monotonic_ns, Utc::now().to_rfc3339()],
        )? == 1;
        if !claimed {
            return Ok(IngestResult {
                duplicate: true,
                credited_duration_ns: 0,
                segment_id: None,
            });
        }
        let current_revision_id = upsert_attribution(&transaction, attribution)?;
        let anchor = load_anchor(&transaction)?;
        let mut duration = 0;
        let mut segment_id = None;
        if let Some(anchor) = anchor {
            let gap = observation.observed_monotonic_ns - anchor.observed_monotonic_ns;
            if anchor.workspace_id == observation.workspace_id
                && gap > 0
                && gap <= self.maximum_gap_ns
            {
                let interval_revision =
                    revision_for_interval(&anchor.revision, &attribution.revision);
                let interval_revision_id =
                    upsert_revision(&transaction, &attribution.project_id, &interval_revision)?;
                segment_id = Some(merge_or_insert(
                    &transaction,
                    &anchor,
                    observation,
                    gap,
                    &interval_revision_id,
                )?);
                duration = gap;
            }
        }
        transaction.execute(
            "INSERT INTO activity_anchors(singleton, observation_id, workspace_id, machine_id, project_id, workspace_instance_id, revision_id, input_wall_ns, observed_monotonic_ns) VALUES(1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(singleton) DO UPDATE SET observation_id=excluded.observation_id, workspace_id=excluded.workspace_id, machine_id=excluded.machine_id, project_id=excluded.project_id, workspace_instance_id=excluded.workspace_instance_id, revision_id=excluded.revision_id, input_wall_ns=excluded.input_wall_ns, observed_monotonic_ns=excluded.observed_monotonic_ns",
            params![observation.observation_id, observation.workspace_id, attribution.machine.machine_id, attribution.project_id, attribution.workspace_instance_id, current_revision_id, observation.input_time_ns, observation.observed_monotonic_ns],
        )?;
        transaction.commit()?;
        Ok(IngestResult {
            duplicate: false,
            credited_duration_ns: duration,
            segment_id,
        })
    }
}

fn upsert_attribution(
    transaction: &rusqlite::Transaction<'_>,
    value: &WorkspaceAttribution,
) -> Result<String> {
    transaction.execute("INSERT INTO machines(id, kind, label, transport) VALUES(?1, ?2, ?3, ?4) ON CONFLICT(id) DO UPDATE SET label=excluded.label, transport=excluded.transport", params![value.machine.machine_id, value.machine.kind, value.machine.label, value.machine.transport])?;
    transaction.execute("INSERT INTO projects(id, machine_id, repository_identity, name, root_path) VALUES(?1, ?2, ?3, ?4, ?5) ON CONFLICT(id) DO UPDATE SET name=excluded.name, root_path=excluded.root_path", params![value.project_id, value.machine.machine_id, value.repository_identity, value.project_name, value.repository_root.to_string_lossy()])?;
    transaction.execute("INSERT INTO workspace_instances(id, machine_id, project_id, runtime_workspace_id, path, worktree_path) VALUES(?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(id) DO UPDATE SET runtime_workspace_id=excluded.runtime_workspace_id, path=excluded.path", params![value.workspace_instance_id, value.machine.machine_id, value.project_id, value.runtime_workspace_id, value.workspace_path.to_string_lossy(), value.workspace_path.to_string_lossy()])?;
    upsert_revision(transaction, &value.project_id, &value.revision)
}

fn upsert_revision(
    transaction: &rusqlite::Transaction<'_>,
    project_id: &str,
    value: &RevisionMetadata,
) -> Result<String> {
    let identity = revision_id(project_id, value);
    transaction.execute("INSERT INTO revisions(id, project_id, kind, stable_id, commit_id, branch, bookmarks, description) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(id) DO UPDATE SET commit_id=excluded.commit_id, branch=excluded.branch, bookmarks=excluded.bookmarks, description=excluded.description", params![identity, project_id, value.kind.to_string(), value.stable_id, value.commit_id, value.branch, value.bookmarks, value.description])?;
    Ok(identity)
}

fn load_anchor(transaction: &rusqlite::Transaction<'_>) -> Result<Option<StoredAnchor>> {
    transaction.query_row("SELECT a.workspace_id, a.machine_id, a.project_id, a.workspace_instance_id, a.input_wall_ns, a.observed_monotonic_ns, r.kind, r.stable_id, r.commit_id, r.branch, r.bookmarks, r.description FROM activity_anchors a LEFT JOIN revisions r ON r.id=a.revision_id WHERE a.singleton=1", [], |row| {
        let kind: Option<String> = row.get(6)?;
        let revision = match kind.as_deref() {
            Some("git") => RevisionKind::Git, Some("jj") => RevisionKind::Jj, Some("transition") => RevisionKind::Transition, _ => RevisionKind::None,
        };
        Ok(StoredAnchor { workspace_id: row.get(0)?, machine_id: row.get(1)?, project_id: row.get(2)?, workspace_instance_id: row.get(3)?, input_wall_ns: row.get(4)?, observed_monotonic_ns: row.get(5)?, revision: RevisionMetadata { kind: revision, stable_id: row.get::<_, Option<String>>(7)?.unwrap_or_else(|| "unversioned".to_owned()), commit_id: row.get(8)?, branch: row.get(9)?, bookmarks: row.get(10)?, description: row.get(11)? } })
    }).optional().context("load activity anchor")
}

fn merge_or_insert(
    transaction: &rusqlite::Transaction<'_>,
    anchor: &StoredAnchor,
    observation: &HumanInputObservation,
    duration: i64,
    revision: &str,
) -> Result<i64> {
    let session: Option<String> = transaction
        .query_row(
            "SELECT id FROM focus_sessions WHERE status='active' ORDER BY started_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let previous: Option<i64> = transaction.query_row("SELECT id FROM activity_segments WHERE category='human_active' AND machine_id=?1 AND project_id=?2 AND workspace_instance_id=?3 AND revision_id=?4 AND session_id IS ?5 AND end_monotonic_ns=?6 AND provenance=?7 ORDER BY id DESC LIMIT 1", params![anchor.machine_id, anchor.project_id, anchor.workspace_instance_id, revision, session, anchor.observed_monotonic_ns, observation.provenance], |row| row.get(0)).optional()?;
    if let Some(id) = previous {
        transaction.execute("UPDATE activity_segments SET end_wall_ns=?1, end_monotonic_ns=?2, duration_ns=duration_ns+?3 WHERE id=?4", params![observation.input_time_ns, observation.observed_monotonic_ns, duration, id])?;
        return Ok(id);
    }
    transaction.execute("INSERT INTO activity_segments(category, machine_id, project_id, workspace_instance_id, revision_id, session_id, start_wall_ns, end_wall_ns, start_monotonic_ns, end_monotonic_ns, duration_ns, provenance) VALUES('human_active', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)", params![anchor.machine_id, anchor.project_id, anchor.workspace_instance_id, revision, session, anchor.input_wall_ns, observation.input_time_ns, anchor.observed_monotonic_ns, observation.observed_monotonic_ns, duration, observation.provenance])?;
    Ok(transaction.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribution::{MachineIdentity, RevisionMetadata};
    use crate::model::HumanInputObservation;
    use std::path::Path;

    fn attribution(directory: &Path) -> WorkspaceAttribution {
        WorkspaceAttribution {
            machine: MachineIdentity {
                machine_id: "machine".into(),
                kind: "local".into(),
                label: "Mac".into(),
                transport: None,
            },
            runtime_workspace_id: "A".into(),
            workspace_path: directory.into(),
            repository_root: directory.into(),
            repository_identity: "repo".into(),
            project_id: "project".into(),
            project_name: "project".into(),
            workspace_instance_id: "workspace".into(),
            revision: RevisionMetadata::unversioned(),
        }
    }

    fn observation(id: &str, monotonic: i64) -> HumanInputObservation {
        HumanInputObservation {
            observation_id: id.into(),
            observed_monotonic_ns: monotonic,
            input_time_ns: monotonic,
            correlation_delta_ns: 1,
            client_endpoint_id: "client".into(),
            pane_endpoint_id: "pane".into(),
            workspace_id: "A".into(),
            pane_id: "p1".into(),
            cwd: None,
            provenance: "test".into(),
        }
    }

    #[test]
    fn credits_same_workspace_and_deduplicates() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let engine = ActivityEngine::new(database, 120);
        let value = attribution(directory.path());
        assert_eq!(
            engine
                .ingest_with_attribution(&observation("one", 100), &value)
                .unwrap()
                .credited_duration_ns,
            0
        );
        assert_eq!(
            engine
                .ingest_with_attribution(&observation("two", 200), &value)
                .unwrap()
                .credited_duration_ns,
            100
        );
        assert!(
            engine
                .ingest_with_attribution(&observation("two", 200), &value)
                .unwrap()
                .duplicate
        );
    }
}
