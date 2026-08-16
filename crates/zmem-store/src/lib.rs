//! SQLite storage and retention decisions.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use std::path::Path;
use zmem_core::{Action, Anchor, HostResponse, SCHEMA_VERSION};

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
        let connection = Connection::open(path)?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        let existing_version: u32 =
            connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if existing_version != 0 && existing_version != SCHEMA_VERSION {
            connection.execute_batch(
                "DROP TABLE IF EXISTS diagnostics;
                 DROP TABLE IF EXISTS relationships;
                 DROP TABLE IF EXISTS entries;
                 DROP TABLE IF EXISTS commits;
                 DROP TABLE IF EXISTS anchors;
                 DROP TABLE IF EXISTS repositories;",
            )?;
        }
        connection.execute_batch(
            &format!("PRAGMA user_version={SCHEMA_VERSION};
             CREATE TABLE IF NOT EXISTS repositories(id INTEGER PRIMARY KEY,path TEXT NOT NULL UNIQUE,trusted_extensions INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS anchors(repository_id INTEGER PRIMARY KEY REFERENCES repositories(id) ON DELETE CASCADE,head TEXT NOT NULL,schema_version INTEGER NOT NULL,extension_hash TEXT NOT NULL,attention_identity TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS commits(repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,oid TEXT NOT NULL,commit_time INTEGER NOT NULL,message TEXT NOT NULL,PRIMARY KEY(repository_id,oid));
             CREATE TABLE IF NOT EXISTS entries(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,annotation_index INTEGER NOT NULL,entry_type TEXT NOT NULL,content TEXT NOT NULL,score REAL NOT NULL,valid INTEGER NOT NULL,commit_time INTEGER NOT NULL DEFAULT 0,scope TEXT,PRIMARY KEY(repository_id,commit_oid,annotation_index),FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
             CREATE TABLE IF NOT EXISTS relationships(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,source TEXT NOT NULL,target TEXT NOT NULL,score REAL NOT NULL,FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
             CREATE TABLE IF NOT EXISTS diagnostics(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,message TEXT NOT NULL,FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);"),
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
