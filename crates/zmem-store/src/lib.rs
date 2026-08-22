//! SQLite storage and retention decisions.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use zmem_core::{Action, Anchor, HostResponse, SCHEMA_VERSION};

const LEGACY_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS repositories(id INTEGER PRIMARY KEY,path TEXT NOT NULL UNIQUE,trusted_extensions INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS anchors(repository_id INTEGER PRIMARY KEY REFERENCES repositories(id) ON DELETE CASCADE,head TEXT NOT NULL,schema_version INTEGER NOT NULL,extension_hash TEXT NOT NULL,attention_identity TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS commits(repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,oid TEXT NOT NULL,commit_time INTEGER NOT NULL,message TEXT NOT NULL,PRIMARY KEY(repository_id,oid));
CREATE TABLE IF NOT EXISTS entries(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,annotation_index INTEGER NOT NULL,entry_type TEXT NOT NULL,content TEXT NOT NULL,score REAL NOT NULL,valid INTEGER NOT NULL,commit_time INTEGER NOT NULL DEFAULT 0,scope TEXT,PRIMARY KEY(repository_id,commit_oid,annotation_index),FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS relationships(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,source TEXT NOT NULL,target TEXT NOT NULL,score REAL NOT NULL,FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS diagnostics(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,message TEXT NOT NULL,FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS inspections(commit_oid TEXT NOT NULL,parser_protocol INTEGER NOT NULL,annotation_count INTEGER NOT NULL,parser_diagnostics TEXT NOT NULL,PRIMARY KEY(commit_oid,parser_protocol));";

const TRAIL_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS trails(
    id TEXT PRIMARY KEY,
    repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    head_oid TEXT NOT NULL,
    attention_identity TEXT NOT NULL,
    extension_identity TEXT NOT NULL,
    protocol_version INTEGER NOT NULL,
    schema_version INTEGER NOT NULL,
    legacy INTEGER NOT NULL DEFAULT 0,
    selected_commit_count INTEGER NOT NULL DEFAULT 0,
    selected_node_count INTEGER NOT NULL DEFAULT 0,
    source_time INTEGER NOT NULL DEFAULT 0,
    UNIQUE(repository_id,head_oid,attention_identity,extension_identity,protocol_version,schema_version)
);
CREATE TABLE IF NOT EXISTS trail_membership(
    trail_id TEXT NOT NULL REFERENCES trails(id) ON DELETE CASCADE,
    repository_id INTEGER NOT NULL,
    commit_oid TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY(trail_id,commit_oid),
    FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS commit_metadata(
    repository_id INTEGER NOT NULL,
    commit_oid TEXT NOT NULL,
    affected_areas TEXT,
    owner TEXT,
    tags TEXT NOT NULL DEFAULT '[]',
    reusable_complete INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY(repository_id,commit_oid),
    FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS trail_entry_state(
    trail_id TEXT NOT NULL REFERENCES trails(id) ON DELETE CASCADE,
    repository_id INTEGER NOT NULL,
    commit_oid TEXT NOT NULL,
    annotation_index INTEGER NOT NULL,
    score REAL NOT NULL,
    valid INTEGER NOT NULL,
    PRIMARY KEY(trail_id,commit_oid,annotation_index),
    FOREIGN KEY(repository_id,commit_oid,annotation_index) REFERENCES entries(repository_id,commit_oid,annotation_index) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS trail_metadata(
    trail_id TEXT NOT NULL REFERENCES trails(id) ON DELETE CASCADE,
    repository_id INTEGER NOT NULL,
    commit_oid TEXT NOT NULL,
    affected_areas TEXT,
    owner TEXT,
    tags TEXT,
    conflicts TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY(trail_id,commit_oid),
    FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS ref_aliases(
    repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    selector TEXT NOT NULL,
    trail_id TEXT NOT NULL REFERENCES trails(id) ON DELETE CASCADE,
    resolved_oid TEXT NOT NULL,
    PRIMARY KEY(repository_id,selector)
);";

#[derive(Clone, Debug)]
pub struct Cohort {
    pub repo_id: i64,
    pub oid: String,
    pub commit_time: i64,
    pub entries: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct RetentionPolicy {
    pub max_entries: u64,
    pub protect_recent_seconds: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvictionPlan {
    pub targets: Vec<(i64, String)>,
    pub over_capacity: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrailCohort {
    pub repository_id: i64,
    pub trail_id: String,
    pub source_time: i64,
    pub referenced: bool,
    pub protected: bool,
}

pub fn select_trail_evictions(rows: &[TrailCohort]) -> Vec<String> {
    let mut eligible = rows
        .iter()
        .filter(|row| !row.referenced && !row.protected)
        .collect::<Vec<_>>();
    eligible.sort_by(|left, right| {
        (left.source_time, left.repository_id, left.trail_id.as_str()).cmp(&(
            right.source_time,
            right.repository_id,
            right.trail_id.as_str(),
        ))
    });
    eligible
        .into_iter()
        .map(|row| row.trail_id.clone())
        .collect()
}

pub fn select_evictions(rows: &[Cohort], now: i64, policy: RetentionPolicy) -> EvictionPlan {
    let mut total: u64 = rows.iter().map(|row| row.entries).sum();
    let mut eligible: Vec<&Cohort> = rows
        .iter()
        .filter(|row| {
            policy.protect_recent_seconds == 0
                || row.commit_time <= now - policy.protect_recent_seconds
        })
        .collect();
    eligible.sort_by(|left, right| {
        (left.commit_time, left.repo_id, left.oid.as_str()).cmp(&(
            right.commit_time,
            right.repo_id,
            right.oid.as_str(),
        ))
    });
    let mut targets = Vec::new();
    for row in eligible {
        if total <= policy.max_entries {
            break;
        }
        total = total.saturating_sub(row.entries);
        targets.push((row.repo_id, row.oid.clone()));
    }
    EvictionPlan {
        targets,
        over_capacity: total > policy.max_entries,
    }
}

pub struct Store {
    connection: Connection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrailRecord {
    pub id: String,
    pub head_oid: String,
    pub attention_identity: String,
    pub extension_identity: String,
    pub protocol_version: u32,
    pub schema_version: u32,
    pub legacy: bool,
    pub selected_commit_count: usize,
    pub selected_node_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectionRecord {
    pub oid: String,
    pub annotation_count: usize,
    pub parser_diagnostics: Vec<String>,
}

pub struct CommitUpdate<'a> {
    pub oid: &'a str,
    pub commit_time: i64,
    pub message: &'a str,
    pub response: &'a HostResponse,
    pub anchor: &'a Anchor,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectStatus {
    Applied,
    NoOp,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectOutcome {
    pub kind: String,
    pub target_sha: String,
    pub target_index: u32,
    pub resolved_sha: Option<String>,
    pub target_type: Option<String>,
    pub status: EffectStatus,
    pub before_score: Option<f64>,
    pub before_valid: Option<bool>,
    pub after_score: Option<f64>,
    pub after_valid: Option<bool>,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PreviewResult {
    pub effects: Vec<EffectOutcome>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug)]
struct EntrySnapshot {
    oid: String,
    entry_type: String,
    score: f64,
    valid: bool,
}

fn record_diagnostic(
    tx: &Transaction<'_>,
    repo_id: i64,
    oid: &str,
    message: &str,
) -> anyhow::Result<()> {
    tx.execute(
        "INSERT INTO diagnostics(repository_id,commit_oid,message) VALUES(?1,?2,?3)",
        params![repo_id, oid, message],
    )?;
    Ok(())
}

fn resolve_target(
    tx: &Transaction<'_>,
    repo_id: i64,
    current_oid: &str,
    prefix: &str,
    index: u32,
) -> anyhow::Result<Vec<EntrySnapshot>> {
    let mut statement = tx.prepare(
        "SELECT commit_oid,entry_type,score,valid FROM entries \
         WHERE repository_id=?1 AND commit_oid LIKE (?2 || '%') \
         AND annotation_index=?3 AND commit_oid<>?4 ORDER BY commit_oid LIMIT 2",
    )?;
    Ok(statement
        .query_map(params![repo_id, prefix, index, current_oid], |row| {
            Ok(EntrySnapshot {
                oid: row.get(0)?,
                entry_type: row.get(1)?,
                score: row.get(2)?,
                valid: row.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?)
}

fn rejected_effect(
    kind: &str,
    target_sha: &str,
    target_index: u32,
    diagnostic: &str,
) -> EffectOutcome {
    EffectOutcome {
        kind: kind.to_owned(),
        target_sha: target_sha.to_owned(),
        target_index,
        resolved_sha: None,
        target_type: None,
        status: EffectStatus::Rejected,
        before_score: None,
        before_valid: None,
        after_score: None,
        after_valid: None,
        diagnostic: Some(diagnostic.to_owned()),
    }
}

fn evaluate_update(
    tx: &Transaction<'_>,
    repo_id: i64,
    update: &CommitUpdate<'_>,
    advance_anchor: bool,
) -> anyhow::Result<PreviewResult> {
    let mut result = PreviewResult::default();
    tx.execute(
        "INSERT OR REPLACE INTO commits(repository_id,oid,commit_time,message) VALUES(?1,?2,?3,?4)",
        params![repo_id, update.oid, update.commit_time, update.message],
    )?;
    for action in &update.response.journal.actions {
        match action {
            Action::AddEntry {
                commit_sha,
                annotation_index,
                entry_type,
                content,
                score,
                valid,
                commit_time,
                scope,
            } => {
                tx.execute("INSERT OR REPLACE INTO entries(repository_id,commit_oid,annotation_index,entry_type,content,score,valid,commit_time,scope) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![repo_id, commit_sha, annotation_index, entry_type, content, score, valid, commit_time, scope])?;
            }
            Action::AddRelationship {
                commit_sha,
                source,
                target,
                score,
            } => {
                tx.execute("INSERT INTO relationships(repository_id,commit_oid,source,target,score) VALUES(?1,?2,?3,?4,?5)", params![repo_id, commit_sha, source, target, score])?;
            }
            Action::Diagnose { message } => {
                record_diagnostic(tx, repo_id, update.oid, message)?;
                result.diagnostics.push(message.clone());
            }
            Action::Decay {
                target_sha,
                target_index,
                factor,
            } => {
                let matches = resolve_target(tx, repo_id, update.oid, target_sha, *target_index)?;
                if matches.len() != 1 {
                    let diagnostic = "unresolved or ambiguous effect target";
                    record_diagnostic(tx, repo_id, update.oid, diagnostic)?;
                    result.diagnostics.push(diagnostic.to_owned());
                    result.effects.push(rejected_effect(
                        "decay",
                        target_sha,
                        *target_index,
                        diagnostic,
                    ));
                    continue;
                }
                let before = &matches[0];
                let after_score = if before.valid {
                    before.score * factor
                } else {
                    before.score
                };
                if before.valid {
                    tx.execute("UPDATE entries SET score=?1 WHERE repository_id=?2 AND commit_oid=?3 AND annotation_index=?4", params![after_score, repo_id, before.oid, target_index])?;
                }
                result.effects.push(EffectOutcome {
                    kind: "decay".to_owned(),
                    target_sha: target_sha.clone(),
                    target_index: *target_index,
                    resolved_sha: Some(before.oid.clone()),
                    target_type: Some(before.entry_type.clone()),
                    status: if before.valid && after_score != before.score {
                        EffectStatus::Applied
                    } else {
                        EffectStatus::NoOp
                    },
                    before_score: Some(before.score),
                    before_valid: Some(before.valid),
                    after_score: Some(after_score),
                    after_valid: Some(before.valid),
                    diagnostic: None,
                });
            }
            Action::Cancel {
                target_sha,
                target_index,
            } => {
                let matches = resolve_target(tx, repo_id, update.oid, target_sha, *target_index)?;
                if matches.len() != 1 {
                    let diagnostic = "unresolved or ambiguous effect target";
                    record_diagnostic(tx, repo_id, update.oid, diagnostic)?;
                    result.diagnostics.push(diagnostic.to_owned());
                    result.effects.push(rejected_effect(
                        "cancel",
                        target_sha,
                        *target_index,
                        diagnostic,
                    ));
                    continue;
                }
                let before = &matches[0];
                if before.entry_type != "DECISION" {
                    let diagnostic = "CANCEL target is not a DECISION";
                    record_diagnostic(tx, repo_id, update.oid, diagnostic)?;
                    result.diagnostics.push(diagnostic.to_owned());
                    let mut outcome =
                        rejected_effect("cancel", target_sha, *target_index, diagnostic);
                    outcome.resolved_sha = Some(before.oid.clone());
                    outcome.target_type = Some(before.entry_type.clone());
                    outcome.before_score = Some(before.score);
                    outcome.before_valid = Some(before.valid);
                    outcome.after_score = Some(before.score);
                    outcome.after_valid = Some(before.valid);
                    result.effects.push(outcome);
                    continue;
                }
                tx.execute("UPDATE entries SET score=0.0,valid=0 WHERE repository_id=?1 AND commit_oid=?2 AND annotation_index=?3", params![repo_id, before.oid, target_index])?;
                result.effects.push(EffectOutcome {
                    kind: "cancel".to_owned(),
                    target_sha: target_sha.clone(),
                    target_index: *target_index,
                    resolved_sha: Some(before.oid.clone()),
                    target_type: Some(before.entry_type.clone()),
                    status: if before.valid || before.score != 0.0 {
                        EffectStatus::Applied
                    } else {
                        EffectStatus::NoOp
                    },
                    before_score: Some(before.score),
                    before_valid: Some(before.valid),
                    after_score: Some(0.0),
                    after_valid: Some(false),
                    diagnostic: None,
                });
            }
            Action::MetadataPatch { .. } => {
                anyhow::bail!("metadata patches require trail-aware application")
            }
        }
    }
    for diagnostic in &update.response.hook_diagnostics {
        record_diagnostic(tx, repo_id, update.oid, diagnostic)?;
        result.diagnostics.push(diagnostic.clone());
    }
    if advance_anchor {
        tx.execute("INSERT INTO anchors(repository_id,head,schema_version,extension_hash,attention_identity) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(repository_id) DO UPDATE SET head=excluded.head,schema_version=excluded.schema_version,extension_hash=excluded.extension_hash,attention_identity=excluded.attention_identity",
            params![repo_id, update.anchor.head, update.anchor.schema, update.anchor.extension_hash, update.anchor.attention_identity])?;
    }
    Ok(result)
}

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut connection = Connection::open(path)?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        let mut existing_version: u32 =
            connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if existing_version == 2 {
            let tx = connection.transaction()?;
            tx.execute_batch(
                "CREATE TABLE inspections(
                    commit_oid TEXT NOT NULL,
                    parser_protocol INTEGER NOT NULL,
                    annotation_count INTEGER NOT NULL,
                    parser_diagnostics TEXT NOT NULL,
                    PRIMARY KEY(commit_oid,parser_protocol)
                 );
                 PRAGMA user_version=3;",
            )?;
            tx.commit()?;
            existing_version = 3;
        }
        if existing_version == 3 {
            let tx = connection.transaction()?;
            tx.execute_batch(LEGACY_SCHEMA)?;
            tx.execute_batch(TRAIL_SCHEMA)?;
            tx.execute_batch(
                "INSERT INTO trails(id,repository_id,head_oid,attention_identity,extension_identity,protocol_version,schema_version,legacy,selected_commit_count,selected_node_count,source_time)
                 SELECT 'legacy:' || a.repository_id || ':' || a.head || ':' || a.attention_identity,
                        a.repository_id,a.head,a.attention_identity,a.extension_hash,4,4,1,
                        (SELECT COUNT(*) FROM commits c WHERE c.repository_id=a.repository_id),
                        (SELECT COUNT(*) FROM entries e WHERE e.repository_id=a.repository_id),
                        COALESCE((SELECT MAX(c.commit_time) FROM commits c WHERE c.repository_id=a.repository_id),0)
                 FROM anchors a;
                 INSERT INTO trail_membership(trail_id,repository_id,commit_oid,position)
                 SELECT t.id,c.repository_id,c.oid,c.commit_time
                 FROM trails t JOIN commits c ON c.repository_id=t.repository_id WHERE t.legacy=1;
                 INSERT INTO commit_metadata(repository_id,commit_oid,affected_areas,owner,tags,reusable_complete)
                 SELECT repository_id,oid,NULL,NULL,'[]',0 FROM commits;
                 INSERT INTO trail_entry_state(trail_id,repository_id,commit_oid,annotation_index,score,valid)
                 SELECT t.id,e.repository_id,e.commit_oid,e.annotation_index,e.score,e.valid
                 FROM trails t JOIN entries e ON e.repository_id=t.repository_id WHERE t.legacy=1;
                 PRAGMA user_version=4;",
            )?;
            tx.commit()?;
            existing_version = 4;
        }
        if existing_version != 0 && existing_version != SCHEMA_VERSION {
            anyhow::bail!("unsupported zmem database schema {existing_version}");
        }
        connection.execute_batch(
            &format!("PRAGMA user_version={SCHEMA_VERSION};
             CREATE TABLE IF NOT EXISTS repositories(id INTEGER PRIMARY KEY,path TEXT NOT NULL UNIQUE,trusted_extensions INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS anchors(repository_id INTEGER PRIMARY KEY REFERENCES repositories(id) ON DELETE CASCADE,head TEXT NOT NULL,schema_version INTEGER NOT NULL,extension_hash TEXT NOT NULL,attention_identity TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS commits(repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,oid TEXT NOT NULL,commit_time INTEGER NOT NULL,message TEXT NOT NULL,PRIMARY KEY(repository_id,oid));
             CREATE TABLE IF NOT EXISTS entries(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,annotation_index INTEGER NOT NULL,entry_type TEXT NOT NULL,content TEXT NOT NULL,score REAL NOT NULL,valid INTEGER NOT NULL,commit_time INTEGER NOT NULL DEFAULT 0,scope TEXT,PRIMARY KEY(repository_id,commit_oid,annotation_index),FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
             CREATE TABLE IF NOT EXISTS relationships(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,source TEXT NOT NULL,target TEXT NOT NULL,score REAL NOT NULL,FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
             CREATE TABLE IF NOT EXISTS diagnostics(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,message TEXT NOT NULL,FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
             CREATE TABLE IF NOT EXISTS inspections(commit_oid TEXT NOT NULL,parser_protocol INTEGER NOT NULL,annotation_count INTEGER NOT NULL,parser_diagnostics TEXT NOT NULL,PRIMARY KEY(commit_oid,parser_protocol));
             {TRAIL_SCHEMA}"),
        )?;
        Ok(Self { connection })
    }

    pub fn register_repository(&mut self, path: &str, trusted: bool) -> anyhow::Result<i64> {
        self.connection.execute(
            "INSERT INTO repositories(path,trusted_extensions) VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET trusted_extensions=excluded.trusted_extensions",
            params![path, trusted],
        )?;
        Ok(self.connection.query_row(
            "SELECT id FROM repositories WHERE path=?1",
            [path],
            |row| row.get(0),
        )?)
    }

    pub fn schema_version(&self) -> anyhow::Result<u32> {
        Ok(self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))?)
    }

    pub fn trails(&self, repo_id: i64) -> anyhow::Result<Vec<TrailRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT id,head_oid,attention_identity,extension_identity,protocol_version,schema_version,legacy,selected_commit_count,selected_node_count
             FROM trails WHERE repository_id=?1 ORDER BY id",
        )?;
        Ok(statement
            .query_map([repo_id], |row| {
                Ok(TrailRecord {
                    id: row.get(0)?,
                    head_oid: row.get(1)?,
                    attention_identity: row.get(2)?,
                    extension_identity: row.get(3)?,
                    protocol_version: row.get(4)?,
                    schema_version: row.get(5)?,
                    legacy: row.get(6)?,
                    selected_commit_count: row.get(7)?,
                    selected_node_count: row.get(8)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn query_trail_entries(
        &self,
        trail_id: &str,
        include_invalid: bool,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let valid_clause = if include_invalid {
            ""
        } else {
            " AND COALESCE(s.valid,e.valid)=1"
        };
        let sql = format!(
            "SELECT e.commit_oid,e.annotation_index,e.entry_type,e.content,COALESCE(s.score,e.score),COALESCE(s.valid,e.valid),e.commit_time,e.scope,
                    COALESCE(tm.affected_areas,cm.affected_areas),COALESCE(tm.owner,cm.owner),COALESCE(tm.tags,cm.tags,'[]'),COALESCE(tm.conflicts,'[]')
             FROM trail_membership m
             JOIN entries e ON e.repository_id=m.repository_id AND e.commit_oid=m.commit_oid
             LEFT JOIN trail_entry_state s ON s.trail_id=m.trail_id AND s.commit_oid=e.commit_oid AND s.annotation_index=e.annotation_index
             LEFT JOIN commit_metadata cm ON cm.repository_id=e.repository_id AND cm.commit_oid=e.commit_oid
             LEFT JOIN trail_metadata tm ON tm.trail_id=m.trail_id AND tm.commit_oid=e.commit_oid
             WHERE m.trail_id=?1{valid_clause} ORDER BY m.position,e.annotation_index"
        );
        let mut statement = self.connection.prepare(&sql)?;
        Ok(statement
            .query_map([trail_id], |row| {
                let affected: Option<String> = row.get(8)?;
                let tags: String = row.get(10)?;
                let conflicts: String = row.get(11)?;
                Ok(serde_json::json!({
                    "sha":row.get::<_,String>(0)?,"index":row.get::<_,u32>(1)?,"type":row.get::<_,String>(2)?,
                    "content":row.get::<_,String>(3)?,"score":row.get::<_,f64>(4)?,"valid":row.get::<_,bool>(5)?,
                    "commit_time":row.get::<_,i64>(6)?,"scope":row.get::<_,Option<String>>(7)?,
                    "affected_areas":affected.map(|value| serde_json::from_str::<serde_json::Value>(&value)).transpose().map_err(|error| rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(error)))?,
                    "owner":row.get::<_,Option<String>>(9)?,
                    "tags":serde_json::from_str::<serde_json::Value>(&tags).map_err(|error| rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(error)))?,
                    "metadata_conflicts":serde_json::from_str::<serde_json::Value>(&conflicts).map_err(|error| rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, Box::new(error)))?
                }))
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn repository(&self, path: &str) -> anyhow::Result<Option<(i64, bool)>> {
        Ok(self
            .connection
            .query_row(
                "SELECT id,trusted_extensions FROM repositories WHERE path=?1",
                [path],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    pub fn inspection(
        &self,
        oid: &str,
        parser_protocol: u32,
    ) -> anyhow::Result<Option<InspectionRecord>> {
        let row = self
            .connection
            .query_row(
                "SELECT annotation_count,parser_diagnostics FROM inspections WHERE commit_oid=?1 AND parser_protocol=?2",
                params![oid, parser_protocol],
                |row| Ok((row.get::<_, usize>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        row.map(|(annotation_count, diagnostics)| {
            Ok(InspectionRecord {
                oid: oid.to_owned(),
                annotation_count,
                parser_diagnostics: serde_json::from_str(&diagnostics)?,
            })
        })
        .transpose()
    }

    pub fn record_inspections(
        &mut self,
        parser_protocol: u32,
        records: &[InspectionRecord],
    ) -> anyhow::Result<()> {
        let tx = self.connection.transaction()?;
        for record in records {
            tx.execute(
                "INSERT OR REPLACE INTO inspections(commit_oid,parser_protocol,annotation_count,parser_diagnostics) VALUES(?1,?2,?3,?4)",
                params![
                    record.oid,
                    parser_protocol,
                    record.annotation_count,
                    serde_json::to_string(&record.parser_diagnostics)?
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn anchor(&self, repo_id: i64) -> anyhow::Result<Option<Anchor>> {
        Ok(self
            .connection
            .query_row(
                "SELECT head,schema_version,extension_hash,attention_identity FROM anchors WHERE repository_id=?1",
                [repo_id],
                |row| {
                    Ok(Anchor {
                        head: row.get(0)?,
                        schema: row.get(1)?,
                        extension_hash: row.get(2)?,
                        attention_identity: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn clear_repository(&mut self, repo_id: i64) -> anyhow::Result<()> {
        let tx = self.connection.transaction()?;
        tx.execute("DELETE FROM anchors WHERE repository_id=?1", [repo_id])?;
        tx.execute("DELETE FROM commits WHERE repository_id=?1", [repo_id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn apply_range(
        &mut self,
        repo_id: i64,
        updates: &[CommitUpdate<'_>],
        rebuild: bool,
    ) -> anyhow::Result<()> {
        let tx = self.connection.transaction()?;
        if rebuild {
            tx.execute("DELETE FROM anchors WHERE repository_id=?1", [repo_id])?;
            tx.execute("DELETE FROM commits WHERE repository_id=?1", [repo_id])?;
        }
        for update in updates {
            evaluate_update(&tx, repo_id, update, true)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn replace_projection(
        &mut self,
        repo_id: i64,
        updates: &[CommitUpdate<'_>],
        final_anchor: &Anchor,
    ) -> anyhow::Result<()> {
        let tx = self.connection.transaction()?;
        tx.execute("DELETE FROM anchors WHERE repository_id=?1", [repo_id])?;
        tx.execute("DELETE FROM commits WHERE repository_id=?1", [repo_id])?;
        for update in updates {
            evaluate_update(&tx, repo_id, update, false)?;
        }
        tx.execute(
            "INSERT INTO anchors(repository_id,head,schema_version,extension_hash,attention_identity) VALUES(?1,?2,?3,?4,?5)",
            params![
                repo_id,
                final_anchor.head,
                final_anchor.schema,
                final_anchor.extension_hash,
                final_anchor.attention_identity
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn preview(
        &mut self,
        repo_id: i64,
        update: &CommitUpdate<'_>,
    ) -> anyhow::Result<PreviewResult> {
        let tx = self.connection.transaction()?;
        let result = evaluate_update(&tx, repo_id, update, false)?;
        tx.rollback()?;
        Ok(result)
    }

    pub fn query_entries(
        &self,
        repo_id: i64,
        include_invalid: bool,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let sql = if include_invalid {
            "SELECT commit_oid,annotation_index,entry_type,content,score,valid,commit_time,scope FROM entries WHERE repository_id=?1 ORDER BY rowid"
        } else {
            "SELECT commit_oid,annotation_index,entry_type,content,score,valid,commit_time,scope FROM entries WHERE repository_id=?1 AND valid=1 ORDER BY rowid"
        };
        let mut statement = self.connection.prepare(sql)?;
        Ok(statement.query_map([repo_id], |row| Ok(serde_json::json!({"sha":row.get::<_,String>(0)?,"index":row.get::<_,u32>(1)?,"type":row.get::<_,String>(2)?,"content":row.get::<_,String>(3)?,"score":row.get::<_,f64>(4)?,"valid":row.get::<_,bool>(5)?,"commit_time":row.get::<_,i64>(6)?,"scope":row.get::<_,Option<String>>(7)?})))?.collect::<Result<_, _>>()?)
    }

    pub fn query_relationships(&self, repo_id: i64) -> anyhow::Result<Vec<serde_json::Value>> {
        let mut statement = self.connection.prepare(
            "SELECT source,target,score FROM relationships WHERE repository_id=?1 ORDER BY rowid",
        )?;
        Ok(statement
            .query_map([repo_id], |row| {
                Ok(serde_json::json!({
                    "from": row.get::<_, String>(0)?,
                    "to": row.get::<_, String>(1)?,
                    "score": row.get::<_, f64>(2)?,
                }))
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn query_diagnostics(&self, repo_id: i64) -> anyhow::Result<Vec<serde_json::Value>> {
        let mut statement = self.connection.prepare(
            "SELECT commit_oid,message FROM diagnostics WHERE repository_id=?1 ORDER BY rowid",
        )?;
        Ok(statement
            .query_map([repo_id], |row| {
                Ok(serde_json::json!({
                    "sha": row.get::<_, String>(0)?,
                    "message": row.get::<_, String>(1)?,
                }))
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn cohorts(&self) -> anyhow::Result<Vec<Cohort>> {
        let mut statement = self.connection.prepare("SELECT c.repository_id,c.oid,c.commit_time,COUNT(e.annotation_index) FROM commits c JOIN entries e ON e.repository_id=c.repository_id AND e.commit_oid=c.oid GROUP BY c.repository_id,c.oid,c.commit_time")?;
        Ok(statement
            .query_map([], |row| {
                Ok(Cohort {
                    repo_id: row.get(0)?,
                    oid: row.get(1)?,
                    commit_time: row.get(2)?,
                    entries: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn evict(&mut self, plan: &EvictionPlan) -> anyhow::Result<()> {
        let tx = self.connection.transaction()?;
        for (repo_id, oid) in &plan.targets {
            tx.execute(
                "DELETE FROM commits WHERE repository_id=?1 AND oid=?2",
                params![repo_id, oid],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
