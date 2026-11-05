use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::Result;
use chrono::Utc;
use rusqlite::{TransactionBehavior, params};

use crate::activity::upsert_attribution;
use crate::attribution::{
    AttributionResolver, MachineIdentity, local_machine_identity, remote_attribution,
};
use crate::database::Database;
use crate::model::RemoteContext;

/// One agent that is currently working, on this machine or a remote one.
#[derive(Clone, Debug)]
pub struct WorkingSample {
    pub key: String,
    pub workspace_id: String,
    pub cwd: PathBuf,
    pub remote: Option<RemoteContext>,
}

/// Records passive agent-working time as `[start, last seen working]` segments.
pub struct AgentTracker {
    database: Database,
    open: HashMap<String, i64>,
    resolver: AttributionResolver,
    machine: MachineIdentity,
}

impl AgentTracker {
    pub fn new(database: Database) -> Self {
        Self {
            database,
            open: HashMap::new(),
            resolver: AttributionResolver::default(),
            machine: local_machine_identity(),
        }
    }

    /// Extends the open segment of every working agent and closes the rest.
    pub fn tick(&mut self, working: &[WorkingSample]) -> Result<()> {
        let now = timestamp_ns();
        let active: HashSet<&str> = working.iter().map(|sample| sample.key.as_str()).collect();
        self.open.retain(|key, _| active.contains(key.as_str()));
        let mut connection = self.database.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for sample in working {
            if let Some(id) = self.open.get(&sample.key) {
                transaction.execute(
                    "UPDATE agent_segments SET end_wall_ns=?1 WHERE id=?2",
                    params![now, id],
                )?;
                continue;
            }
            let attribution = match sample.remote.as_ref() {
                Some(remote) => remote_attribution(remote),
                None => {
                    self.resolver
                        .resolve(&self.machine, &sample.workspace_id, &sample.cwd, true)?
                }
            };
            let revision_id = upsert_attribution(&transaction, &attribution)?;
            transaction.execute(
                "INSERT INTO agent_segments(pane_key, machine_id, project_id, workspace_instance_id, revision_id, start_wall_ns, end_wall_ns) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![
                    sample.key,
                    attribution.machine.machine_id,
                    attribution.project_id,
                    attribution.workspace_instance_id,
                    revision_id,
                    now
                ],
            )?;
            self.open
                .insert(sample.key.clone(), transaction.last_insert_rowid());
        }
        transaction.commit()?;
        Ok(())
    }
}

fn timestamp_ns() -> i64 {
    Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(key: &str) -> WorkingSample {
        WorkingSample {
            key: key.to_owned(),
            workspace_id: "w1".to_owned(),
            cwd: std::env::temp_dir(),
            remote: None,
        }
    }

    #[test]
    fn segments_extend_while_working_and_split_after_idle() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let mut tracker = AgentTracker::new(database.clone());
        tracker.tick(&[sample("a")]).unwrap();
        tracker.tick(&[sample("a")]).unwrap();
        tracker.tick(&[]).unwrap();
        tracker.tick(&[sample("a")]).unwrap();
        let count: i64 = database
            .connect_readonly()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM agent_segments", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
