//! SQLite storage and retention decisions.

use anyhow::Context;
mod demand;
pub use demand::{AdvisoryBatch, DemandUse, PrefetchMetrics};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use zmem_core::{
    Action, Anchor, GitCommit, HostResponse, MetadataOperation, MetadataOperator, SCHEMA_VERSION,
    TrailIdentity, derive_affected_areas,
};

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
CREATE INDEX IF NOT EXISTS trail_membership_by_commit ON trail_membership(repository_id,commit_oid);
CREATE TABLE IF NOT EXISTS commit_metadata(
    repository_id INTEGER NOT NULL,
    commit_oid TEXT NOT NULL,
    affected_areas TEXT,
    owner TEXT,
    tags TEXT NOT NULL DEFAULT '[]',
    conflicts TEXT NOT NULL DEFAULT '[]',
    reusable_complete INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY(repository_id,commit_oid),
    FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS commit_ancestry(
    repository_id INTEGER NOT NULL,
    commit_oid TEXT NOT NULL,
    ancestor_oid TEXT NOT NULL,
    PRIMARY KEY(repository_id,commit_oid,ancestor_oid),
    FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS commit_parents(
    repository_id INTEGER NOT NULL,
    commit_oid TEXT NOT NULL,
    parent_oid TEXT NOT NULL,
    PRIMARY KEY(repository_id,commit_oid,parent_oid),
    FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS commit_parents_reverse ON commit_parents(repository_id,parent_oid);
CREATE TABLE IF NOT EXISTS raw_commit_facts(
    repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    commit_oid TEXT NOT NULL,
    fact TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    PRIMARY KEY(repository_id,commit_oid)
);
CREATE TABLE IF NOT EXISTS raw_parent_edges(
    repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    commit_oid TEXT NOT NULL,
    parent_oid TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    PRIMARY KEY(repository_id,commit_oid,parent_oid)
);
CREATE TABLE IF NOT EXISTS prefetch_jobs(
    repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    head_oid TEXT NOT NULL,
    ceiling INTEGER NOT NULL,
    completed_count INTEGER NOT NULL DEFAULT 0,
    checkpoint_oid TEXT,
    state TEXT NOT NULL,
    PRIMARY KEY(repository_id,head_oid)
);
CREATE TABLE IF NOT EXISTS index_jobs(
    id TEXT PRIMARY KEY,
    job_key TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL,
    failure TEXT
);
CREATE TABLE IF NOT EXISTS metadata_assignments(
    repository_id INTEGER NOT NULL,
    target_oid TEXT NOT NULL,
    metadata_key TEXT NOT NULL,
    source_oid TEXT NOT NULL,
    value TEXT,
    PRIMARY KEY(repository_id,target_oid,metadata_key,source_oid),
    FOREIGN KEY(repository_id,target_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE,
    FOREIGN KEY(repository_id,source_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS expansion_facts(
    repository_id INTEGER NOT NULL,
    commit_oid TEXT NOT NULL,
    extension_identity TEXT NOT NULL,
    response TEXT NOT NULL,
    PRIMARY KEY(repository_id,commit_oid,extension_identity),
    FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS trail_entry_state(
    trail_id TEXT NOT NULL REFERENCES trails(id) ON DELETE CASCADE,
    repository_id INTEGER NOT NULL,
    commit_oid TEXT NOT NULL,
    annotation_index INTEGER NOT NULL,
    entry_type TEXT NOT NULL,
    content TEXT NOT NULL,
    score REAL NOT NULL,
    valid INTEGER NOT NULL,
    commit_time INTEGER NOT NULL,
    scope TEXT,
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
    pub entries: u64,
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
    staging_protection: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrailRecord {
    pub id: String,
    pub repository_id: i64,
    pub head_oid: String,
    pub attention_identity: String,
    pub extension_identity: String,
    pub protocol_version: u32,
    pub schema_version: u32,
    pub legacy: bool,
    pub selected_commit_count: usize,
    pub selected_node_count: usize,
    pub source_time: i64,
}

pub struct PublishedSnapshot {
    pub trail: TrailRecord,
    pub entries: Vec<serde_json::Value>,
    pub relationships: Vec<serde_json::Value>,
    pub diagnostics: Vec<serde_json::Value>,
    pub over_capacity: bool,
}

pub struct PersistedIndexJob {
    pub id: String,
    pub job_key: String,
    pub state: String,
    pub failure: Option<String>,
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
    pub affected_areas: Option<&'a [String]>,
    pub parents: &'a [String],
    pub range_complete: bool,
}

pub struct RawCommitBatch<'a> {
    pub facts: &'a [GitCommit],
    pub parents: &'a std::collections::BTreeMap<String, Vec<String>>,
}

pub struct TrailPublication<'a> {
    pub trail: &'a TrailRecord,
    pub commits: &'a [GitCommit],
    pub parents: &'a std::collections::BTreeMap<String, Vec<String>>,
    pub entries: &'a [serde_json::Value],
    pub relationships: &'a [serde_json::Value],
    pub diagnostics: &'a [serde_json::Value],
    pub expansions: &'a std::collections::BTreeMap<String, HostResponse>,
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

fn resolve_commit_prefix(
    tx: &Transaction<'_>,
    repo_id: i64,
    prefix: &str,
) -> anyhow::Result<String> {
    let mut statement = tx.prepare(
        "SELECT oid FROM commits WHERE repository_id=?1 AND oid LIKE (?2 || '%') ORDER BY oid LIMIT 2",
    )?;
    let matches = statement
        .query_map(params![repo_id, prefix], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    anyhow::ensure!(matches.len() == 1, "unresolved or ambiguous META endpoint");
    Ok(matches[0].clone())
}

#[derive(Default)]
struct ReachabilityCache {
    ancestors: HashMap<(String, String), bool>,
    ranges: HashMap<(String, String), Vec<String>>,
}

fn metadata_targets(
    tx: &Transaction<'_>,
    cache: &mut ReachabilityCache,
    repo_id: i64,
    current_oid: &str,
    from_prefix: &str,
    to_prefix: &str,
    complete: bool,
) -> anyhow::Result<Vec<String>> {
    anyhow::ensure!(complete, "incomplete META range");
    let from = resolve_commit_prefix(tx, repo_id, from_prefix)?;
    let to = resolve_commit_prefix(tx, repo_id, to_prefix)?;
    anyhow::ensure!(
        is_ancestor(tx, cache, repo_id, &to, &from)?,
        "META from is not an ancestor of to"
    );
    anyhow::ensure!(
        current_oid != from
            && current_oid != to
            && is_ancestor(tx, cache, repo_id, current_oid, &from)?
            && is_ancestor(tx, cache, repo_id, current_oid, &to)?,
        "META endpoints must precede META commit"
    );
    if let Some(targets) = cache.ranges.get(&(from.clone(), to.clone())) {
        return Ok(targets.clone());
    }
    let mut statement = tx.prepare(
        "WITH RECURSIVE from_desc(oid) AS (
             SELECT ?2 UNION SELECT p.commit_oid FROM commit_parents p JOIN from_desc f ON p.parent_oid=f.oid WHERE p.repository_id=?1
         ), to_anc(oid) AS (
             SELECT ?3 UNION SELECT p.parent_oid FROM commit_parents p JOIN to_anc t ON p.commit_oid=t.oid WHERE p.repository_id=?1
         )
         SELECT c.oid FROM commits c
         WHERE c.repository_id=?1
           AND c.oid IN from_desc
           AND c.oid IN to_anc
         ORDER BY c.rowid",
    )?;
    let targets = statement
        .query_map(params![repo_id, from, to], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    cache.ranges.insert((from, to), targets.clone());
    Ok(targets)
}

fn is_ancestor(
    tx: &Transaction<'_>,
    cache: &mut ReachabilityCache,
    repo_id: i64,
    descendant: &str,
    ancestor: &str,
) -> anyhow::Result<bool> {
    if descendant == ancestor {
        return Ok(true);
    }
    let key = (descendant.to_owned(), ancestor.to_owned());
    if let Some(found) = cache.ancestors.get(&key) {
        return Ok(*found);
    }
    let found = tx.query_row(
        "WITH RECURSIVE reach(oid) AS (
             SELECT ?2 UNION SELECT p.parent_oid FROM commit_parents p JOIN reach r ON p.commit_oid=r.oid WHERE p.repository_id=?1
         )
         SELECT EXISTS(SELECT 1 FROM reach WHERE oid=?3)",
        params![repo_id, descendant, ancestor],
        |row| row.get(0),
    )?;
    cache.ancestors.insert(key, found);
    Ok(found)
}

fn update_conflict_key(
    tx: &Transaction<'_>,
    repo_id: i64,
    target_oid: &str,
    key: &str,
    conflicted: bool,
) -> anyhow::Result<()> {
    let encoded: String = tx.query_row(
        "SELECT conflicts FROM commit_metadata WHERE repository_id=?1 AND commit_oid=?2",
        params![repo_id, target_oid],
        |row| row.get(0),
    )?;
    let mut conflicts = serde_json::from_str::<Vec<String>>(&encoded)?;
    conflicts.retain(|existing| existing != key);
    if conflicted {
        conflicts.push(key.to_owned());
        conflicts.sort();
    }
    tx.execute(
        "UPDATE commit_metadata SET conflicts=?1 WHERE repository_id=?2 AND commit_oid=?3",
        params![serde_json::to_string(&conflicts)?, repo_id, target_oid],
    )?;
    Ok(())
}

fn assign_metadata_value(
    tx: &Transaction<'_>,
    cache: &mut ReachabilityCache,
    repo_id: i64,
    target_oid: &str,
    source_oid: &str,
    operation: &MetadataOperation,
) -> anyhow::Result<()> {
    let mut statement = tx.prepare(
        "SELECT source_oid FROM metadata_assignments WHERE repository_id=?1 AND target_oid=?2 AND metadata_key=?3",
    )?;
    let previous = statement
        .query_map(params![repo_id, target_oid, operation.key], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut descends_from_all = true;
    for source in &previous {
        if !is_ancestor(tx, cache, repo_id, source_oid, source)? {
            descends_from_all = false;
            break;
        }
    }
    if descends_from_all {
        tx.execute(
            "DELETE FROM metadata_assignments WHERE repository_id=?1 AND target_oid=?2 AND metadata_key=?3",
            params![repo_id, target_oid, operation.key],
        )?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO metadata_assignments(repository_id,target_oid,metadata_key,source_oid,value) VALUES(?1,?2,?3,?4,?5)",
        params![repo_id, target_oid, operation.key, source_oid, operation.value],
    )?;
    let mut values_statement = tx.prepare(
        "SELECT DISTINCT value FROM metadata_assignments WHERE repository_id=?1 AND target_oid=?2 AND metadata_key=?3",
    )?;
    let values = values_statement
        .query_map(params![repo_id, target_oid, operation.key], |row| {
            row.get::<_, Option<String>>(0)
        })?
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    let conflicted = values.len() > 1;
    update_conflict_key(tx, repo_id, target_oid, &operation.key, conflicted)?;
    let resolved = (!conflicted)
        .then(|| values.iter().next().cloned().flatten())
        .flatten();
    match operation.key.as_str() {
        "affected_areas" => {
            let encoded = resolved
                .map(|value| serde_json::to_string(&vec![value]))
                .transpose()?;
            tx.execute(
                "UPDATE commit_metadata SET affected_areas=?1 WHERE repository_id=?2 AND commit_oid=?3",
                params![encoded, repo_id, target_oid],
            )?;
        }
        "owner" => {
            tx.execute(
                "UPDATE commit_metadata SET owner=?1 WHERE repository_id=?2 AND commit_oid=?3",
                params![resolved, repo_id, target_oid],
            )?;
        }
        "tags" => {
            let encoded = serde_json::to_string(&resolved.into_iter().collect::<Vec<_>>())?;
            tx.execute(
                "UPDATE commit_metadata SET tags=?1 WHERE repository_id=?2 AND commit_oid=?3",
                params![encoded, repo_id, target_oid],
            )?;
        }
        _ => unreachable!("journal validation restricts metadata keys"),
    }
    Ok(())
}

fn add_metadata_value(
    tx: &Transaction<'_>,
    repo_id: i64,
    target_oid: &str,
    operation: &MetadataOperation,
) -> anyhow::Result<()> {
    let column = match operation.key.as_str() {
        "affected_areas" => "affected_areas",
        "tags" => "tags",
        _ => anyhow::bail!("metadata add requires a set-valued key"),
    };
    let encoded: Option<String> = tx.query_row(
        &format!("SELECT {column} FROM commit_metadata WHERE repository_id=?1 AND commit_oid=?2"),
        params![repo_id, target_oid],
        |row| row.get(0),
    )?;
    if operation.key == "affected_areas" && encoded.is_none() {
        return Ok(());
    }
    let mut values = encoded
        .as_deref()
        .map(serde_json::from_str::<Vec<String>>)
        .transpose()?
        .unwrap_or_default();
    let value = operation
        .value
        .as_ref()
        .context("metadata add value is required")?;
    if !values.contains(value) {
        values.push(value.clone());
        values.sort();
    }
    tx.execute(
        &format!("UPDATE commit_metadata SET {column}=?1 WHERE repository_id=?2 AND commit_oid=?3"),
        params![serde_json::to_string(&values)?, repo_id, target_oid],
    )?;
    Ok(())
}

fn evaluate_update(
    tx: &Transaction<'_>,
    cache: &mut ReachabilityCache,
    repo_id: i64,
    update: &CommitUpdate<'_>,
    advance_anchor: bool,
) -> anyhow::Result<PreviewResult> {
    let mut result = PreviewResult::default();
    tx.execute(
        "INSERT OR REPLACE INTO commits(repository_id,oid,commit_time,message) VALUES(?1,?2,?3,?4)",
        params![repo_id, update.oid, update.commit_time, update.message],
    )?;
    let affected_areas = update
        .affected_areas
        .map(serde_json::to_string)
        .transpose()?;
    tx.execute(
        "INSERT OR IGNORE INTO commit_metadata(repository_id,commit_oid,affected_areas,owner,tags,conflicts,reusable_complete) VALUES(?1,?2,?3,NULL,'[]','[]',1)",
        params![repo_id, update.oid, affected_areas],
    )?;
    for parent in update.parents {
        tx.execute(
            "INSERT OR IGNORE INTO commit_parents(repository_id,commit_oid,parent_oid) VALUES(?1,?2,?3)",
            params![repo_id, update.oid, parent],
        )?;
    }
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
            Action::MetadataPatch {
                from_sha,
                to_sha,
                operations,
            } => {
                let targets = metadata_targets(
                    tx,
                    cache,
                    repo_id,
                    update.oid,
                    from_sha,
                    to_sha,
                    update.range_complete,
                )?;
                for target in targets {
                    for operation in operations {
                        match operation.operator {
                            MetadataOperator::Set | MetadataOperator::Null => {
                                assign_metadata_value(
                                    tx, cache, repo_id, &target, update.oid, operation,
                                )?
                            }
                            MetadataOperator::Add => {
                                add_metadata_value(tx, repo_id, &target, operation)?
                            }
                        }
                    }
                }
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
    /// Limit SQLite lock waits and long-running statements to the caller's
    /// monotonic request budget. The progress hook is connection-local.
    pub fn set_request_deadline(&self, deadline: Instant) -> anyhow::Result<()> {
        self.set_request_deadline_and_cancellation(deadline, None)
    }

    pub fn set_request_deadline_and_cancellation(
        &self,
        deadline: Instant,
        cancelled: Option<Arc<AtomicBool>>,
    ) -> anyhow::Result<()> {
        self.connection.busy_timeout(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(250)),
        )?;
        self.connection.progress_handler(
            1000,
            Some(move || {
                Instant::now() >= deadline
                    || cancelled
                        .as_ref()
                        .is_some_and(|token| token.load(Ordering::Acquire))
            }),
        );
        Ok(())
    }

    pub fn open_readonly(path: &Path) -> anyhow::Result<Self> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(Duration::from_millis(250))?;
        connection.execute_batch("PRAGMA query_only=ON; PRAGMA foreign_keys=ON;")?;
        let version: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        anyhow::ensure!(
            version == SCHEMA_VERSION,
            "unsupported zmem database schema {version}"
        );
        Ok(Self {
            connection,
            staging_protection: 0,
        })
    }

    /// Open an initialized database for daemon-owned writes without rerunning
    /// schema creation or migration on every job-state update.
    pub fn open_writable_existing(path: &Path) -> anyhow::Result<Self> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.busy_timeout(Duration::from_millis(250))?;
        connection.execute_batch("PRAGMA foreign_keys=ON;")?;
        let version: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        anyhow::ensure!(
            version == SCHEMA_VERSION,
            "unsupported zmem database schema {version}"
        );
        Ok(Self {
            connection,
            staging_protection: 0,
        })
    }

    pub fn published_snapshot(
        &mut self,
        identity: &TrailIdentity,
        include_invalid: bool,
        max_entries: u64,
        now: i64,
        protect_recent_seconds: i64,
    ) -> anyhow::Result<Option<PublishedSnapshot>> {
        self.connection.execute_batch("BEGIN")?;
        let result = (|| {
            let prefix = format!("{}:", identity.key());
            let trail = self.connection.query_row(
                "SELECT id,repository_id,head_oid,attention_identity,extension_identity,protocol_version,schema_version,legacy,selected_commit_count,selected_node_count,source_time
                 FROM trails WHERE repository_id=?1 AND head_oid=?2 AND extension_identity=?3 AND protocol_version=?4 AND schema_version=?5 AND legacy=0 AND substr(id,1,length(?6))=?6 ORDER BY id LIMIT 1",
                params![identity.repository_id, identity.head_oid, identity.extension_identity, identity.protocol_version, identity.schema_version, prefix],
                |row| Ok(TrailRecord {
                    id: row.get(0)?, repository_id: row.get(1)?, head_oid: row.get(2)?,
                    attention_identity: row.get(3)?, extension_identity: row.get(4)?,
                    protocol_version: row.get(5)?, schema_version: row.get(6)?, legacy: row.get(7)?,
                    selected_commit_count: row.get(8)?, selected_node_count: row.get(9)?, source_time: row.get(10)?,
                }),
            ).optional()?;
            trail
                .map(|trail| {
                    Ok(PublishedSnapshot {
                        entries: self.query_trail_entries(&trail.id, include_invalid)?,
                        relationships: self.query_trail_relationships(&trail.id)?,
                        diagnostics: self.query_trail_diagnostics(&trail.id)?,
                        over_capacity: self
                            .trail_cohorts(now, protect_recent_seconds)?
                            .iter()
                            .map(|row| row.entries)
                            .sum::<u64>()
                            + self.cohorts()?.iter().map(|row| row.entries).sum::<u64>()
                            > max_entries,
                        trail,
                    })
                })
                .transpose()
        })();
        let finish =
            self.connection
                .execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" });
        finish?;
        result
    }

    /// A cheap guard for the service's pre-identity job shortcut. Any trail
    /// for this HEAD requires full compatibility validation before replying.
    pub fn has_published_head(&self, repository: &str, head: &str) -> anyhow::Result<bool> {
        let present: i64 = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM trails t JOIN repositories r ON r.id=t.repository_id WHERE r.path=?1 AND t.head_oid=?2 AND t.legacy=0)",
            params![repository, head],
            |row| row.get(0),
        )?;
        Ok(present != 0)
    }

    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_millis(250))?;
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
                 ALTER TABLE relationships ADD COLUMN trail_id TEXT;
                 ALTER TABLE diagnostics ADD COLUMN trail_id TEXT;
                 INSERT INTO trail_membership(trail_id,repository_id,commit_oid,position)
                 SELECT t.id,c.repository_id,c.oid,c.commit_time
                 FROM trails t JOIN commits c ON c.repository_id=t.repository_id WHERE t.legacy=1;
                 INSERT INTO commit_metadata(repository_id,commit_oid,affected_areas,owner,tags,reusable_complete)
                 SELECT repository_id,oid,NULL,NULL,'[]',0 FROM commits;
                 INSERT INTO trail_entry_state(trail_id,repository_id,commit_oid,annotation_index,entry_type,content,score,valid,commit_time,scope)
                 SELECT t.id,e.repository_id,e.commit_oid,e.annotation_index,e.entry_type,e.content,e.score,e.valid,e.commit_time,e.scope
                 FROM trails t JOIN entries e ON e.repository_id=t.repository_id WHERE t.legacy=1;
                 UPDATE relationships SET trail_id=(SELECT id FROM trails WHERE trails.repository_id=relationships.repository_id AND legacy=1);
                 UPDATE diagnostics SET trail_id=(SELECT id FROM trails WHERE trails.repository_id=diagnostics.repository_id AND legacy=1);
                 PRAGMA user_version=4;",
            )?;
            tx.commit()?;
            existing_version = 4;
        }
        if existing_version == 4 {
            let tx = connection.transaction()?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS commit_parents(
                    repository_id INTEGER NOT NULL,
                    commit_oid TEXT NOT NULL,
                    parent_oid TEXT NOT NULL,
                    PRIMARY KEY(repository_id,commit_oid,parent_oid),
                    FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS commit_parents_reverse ON commit_parents(repository_id,parent_oid);
                CREATE TABLE IF NOT EXISTS raw_commit_facts(
                    repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                    commit_oid TEXT NOT NULL,
                    fact TEXT NOT NULL,
                    bytes INTEGER NOT NULL,
                    PRIMARY KEY(repository_id,commit_oid)
                );
                CREATE TABLE IF NOT EXISTS raw_parent_edges(
                    repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                    commit_oid TEXT NOT NULL,
                    parent_oid TEXT NOT NULL,
                    bytes INTEGER NOT NULL,
                    PRIMARY KEY(repository_id,commit_oid,parent_oid)
                );
                CREATE TABLE IF NOT EXISTS prefetch_jobs(
                    repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                    head_oid TEXT NOT NULL,
                    ceiling INTEGER NOT NULL,
                    completed_count INTEGER NOT NULL DEFAULT 0,
                    checkpoint_oid TEXT,
                    state TEXT NOT NULL,
                    PRIMARY KEY(repository_id,head_oid)
                );
                CREATE TABLE IF NOT EXISTS index_jobs(
                    id TEXT PRIMARY KEY,
                    job_key TEXT NOT NULL UNIQUE,
                    state TEXT NOT NULL,
                    failure TEXT
                );
                PRAGMA user_version=5;",
            )?;
            tx.commit()?;
            existing_version = 5;
        }
        if existing_version == 5 {
            demand::migrate(&mut connection)?;
            existing_version = 6;
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
             CREATE TABLE IF NOT EXISTS relationships(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,source TEXT NOT NULL,target TEXT NOT NULL,score REAL NOT NULL,trail_id TEXT,FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
             CREATE TABLE IF NOT EXISTS diagnostics(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,message TEXT NOT NULL,trail_id TEXT,FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
             CREATE TABLE IF NOT EXISTS inspections(commit_oid TEXT NOT NULL,parser_protocol INTEGER NOT NULL,annotation_count INTEGER NOT NULL,parser_diagnostics TEXT NOT NULL,PRIMARY KEY(commit_oid,parser_protocol));
             {TRAIL_SCHEMA}"),
        )?;
        connection.execute_batch(demand::SCHEMA)?;
        Ok(Self {
            connection,
            staging_protection: 0,
        })
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

    pub fn recover_index_jobs(&mut self) -> anyhow::Result<Vec<PersistedIndexJob>> {
        self.connection.execute(
            "UPDATE index_jobs SET state='failed',failure='indexing was interrupted; hook execution may be uncertain' WHERE state IN ('queued','running')",
            [],
        )?;
        let mut statement = self
            .connection
            .prepare("SELECT id,job_key,state,failure FROM index_jobs ORDER BY id")?;
        Ok(statement
            .query_map([], |row| {
                Ok(PersistedIndexJob {
                    id: row.get(0)?,
                    job_key: row.get(1)?,
                    state: row.get(2)?,
                    failure: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn insert_index_job(&mut self, id: &str, job_key: &str) -> anyhow::Result<()> {
        self.connection.execute(
            "INSERT INTO index_jobs(id,job_key,state,failure) VALUES(?1,?2,'queued',NULL)",
            params![id, job_key],
        )?;
        Ok(())
    }

    pub fn set_index_job_state(
        &mut self,
        id: &str,
        state: &str,
        failure: Option<&str>,
    ) -> anyhow::Result<()> {
        self.connection.execute(
            "UPDATE index_jobs SET state=?2,failure=?3 WHERE id=?1",
            params![id, state, failure],
        )?;
        Ok(())
    }

    pub fn update_index_job_key(&mut self, id: &str, job_key: &str) -> anyhow::Result<()> {
        self.connection.execute(
            "UPDATE index_jobs SET job_key=?2 WHERE id=?1",
            params![id, job_key],
        )?;
        Ok(())
    }

    pub fn remove_index_job(&mut self, id: &str) -> anyhow::Result<()> {
        self.connection
            .execute("DELETE FROM index_jobs WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn trails(&self, repo_id: i64) -> anyhow::Result<Vec<TrailRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT id,repository_id,head_oid,attention_identity,extension_identity,protocol_version,schema_version,legacy,selected_commit_count,selected_node_count,source_time
             FROM trails WHERE repository_id=?1 ORDER BY id",
        )?;
        Ok(statement
            .query_map([repo_id], |row| {
                Ok(TrailRecord {
                    id: row.get(0)?,
                    repository_id: row.get(1)?,
                    head_oid: row.get(2)?,
                    attention_identity: row.get(3)?,
                    extension_identity: row.get(4)?,
                    protocol_version: row.get(5)?,
                    schema_version: row.get(6)?,
                    legacy: row.get(7)?,
                    selected_commit_count: row.get(8)?,
                    selected_node_count: row.get(9)?,
                    source_time: row.get(10)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn trail(&self, trail_id: &str) -> anyhow::Result<Option<TrailRecord>> {
        Ok(self
            .connection
            .query_row(
                "SELECT id,repository_id,head_oid,attention_identity,extension_identity,protocol_version,schema_version,legacy,selected_commit_count,selected_node_count,source_time FROM trails WHERE id=?1",
                [trail_id],
                |row| {
                    Ok(TrailRecord {
                        id: row.get(0)?,
                        repository_id: row.get(1)?,
                        head_oid: row.get(2)?,
                        attention_identity: row.get(3)?,
                        extension_identity: row.get(4)?,
                        protocol_version: row.get(5)?,
                        schema_version: row.get(6)?,
                        legacy: row.get(7)?,
                        selected_commit_count: row.get(8)?,
                        selected_node_count: row.get(9)?,
                        source_time: row.get(10)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn publish_trail(&mut self, publication: TrailPublication<'_>) -> anyhow::Result<()> {
        let TrailPublication {
            trail,
            commits,
            parents,
            entries,
            relationships,
            diagnostics,
            expansions,
        } = publication;
        let tx = self.connection.transaction()?;
        tx.execute(
            "INSERT INTO trails(id,repository_id,head_oid,attention_identity,extension_identity,protocol_version,schema_version,legacy,selected_commit_count,selected_node_count,source_time) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                trail.id,
                trail.repository_id,
                trail.head_oid,
                trail.attention_identity,
                trail.extension_identity,
                trail.protocol_version,
                trail.schema_version,
                trail.legacy,
                trail.selected_commit_count,
                trail.selected_node_count,
                trail.source_time,
            ],
        )?;
        for (position, commit) in commits.iter().enumerate() {
            let encoded = serde_json::to_string(commit)?;
            tx.execute("INSERT OR IGNORE INTO raw_commit_facts(repository_id,commit_oid,fact,bytes) VALUES(?1,?2,?3,?4)",params![trail.repository_id,commit.sha,encoded,encoded.len()])?;
            for parent in parents.get(&commit.sha).into_iter().flatten() {
                tx.execute(
                    "INSERT OR IGNORE INTO raw_parent_edges VALUES(?1,?2,?3,?4)",
                    params![
                        trail.repository_id,
                        commit.sha,
                        parent,
                        commit.sha.len() + parent.len() + 32
                    ],
                )?;
            }
            tx.execute(
                "INSERT OR IGNORE INTO commits(repository_id,oid,commit_time,message) VALUES(?1,?2,?3,?4)",
                params![trail.repository_id, commit.sha, commit.commit_time, commit.message],
            )?;
            if let Some(response) = expansions.get(&commit.sha) {
                tx.execute(
                    "INSERT OR IGNORE INTO expansion_facts(repository_id,commit_oid,extension_identity,response) VALUES(?1,?2,?3,?4)",
                    params![
                        trail.repository_id,
                        commit.sha,
                        trail.extension_identity,
                        serde_json::to_string(response)?
                    ],
                )?;
            }
            let areas = derive_affected_areas(&commit.changes)
                .map(|areas| serde_json::to_string(&areas))
                .transpose()?;
            tx.execute(
                "INSERT OR IGNORE INTO commit_metadata(repository_id,commit_oid,affected_areas,owner,tags,conflicts,reusable_complete) VALUES(?1,?2,?3,NULL,'[]','[]',1)",
                params![trail.repository_id, commit.sha, areas],
            )?;
            for parent in parents.get(&commit.sha).into_iter().flatten() {
                tx.execute(
                    "INSERT OR IGNORE INTO commit_parents(repository_id,commit_oid,parent_oid) VALUES(?1,?2,?3)",
                    params![trail.repository_id, commit.sha, parent],
                )?;
            }
            tx.execute(
                "INSERT INTO trail_membership(trail_id,repository_id,commit_oid,position) VALUES(?1,?2,?3,?4)",
                params![trail.id, trail.repository_id, commit.sha, position],
            )?;
        }
        for entry in entries {
            let sha = entry["sha"].as_str().context("entry SHA is missing")?;
            let index = entry["index"].as_u64().context("entry index is missing")?;
            tx.execute(
                "INSERT OR IGNORE INTO entries(repository_id,commit_oid,annotation_index,entry_type,content,score,valid,commit_time,scope) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    trail.repository_id,
                    sha,
                    index,
                    entry["type"].as_str().context("entry type is missing")?,
                    entry["content"].as_str().context("entry content is missing")?,
                    entry["score"].as_f64().context("entry score is missing")?,
                    entry["valid"].as_bool().context("entry validity is missing")?,
                    entry["commit_time"].as_i64().context("entry commit time is missing")?,
                    entry["scope"].as_str(),
                ],
            )?;
            tx.execute(
                "INSERT INTO trail_entry_state(trail_id,repository_id,commit_oid,annotation_index,entry_type,content,score,valid,commit_time,scope) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    trail.id,
                    trail.repository_id,
                    sha,
                    index,
                    entry["type"].as_str(),
                    entry["content"].as_str(),
                    entry["score"].as_f64(),
                    entry["valid"].as_bool(),
                    entry["commit_time"].as_i64(),
                    entry["scope"].as_str(),
                ],
            )?;
            let affected = (!entry["affected_areas"].is_null())
                .then(|| serde_json::to_string(&entry["affected_areas"]))
                .transpose()?;
            tx.execute(
                "INSERT OR REPLACE INTO trail_metadata(trail_id,repository_id,commit_oid,affected_areas,owner,tags,conflicts) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    trail.id,
                    trail.repository_id,
                    sha,
                    affected,
                    entry["owner"].as_str(),
                    serde_json::to_string(&entry["tags"])?,
                    serde_json::to_string(&entry["metadata_conflicts"])?,
                ],
            )?;
        }
        for relationship in relationships {
            let source = relationship["from"]
                .as_str()
                .context("relationship source is missing")?;
            let target = relationship["to"]
                .as_str()
                .context("relationship target is missing")?;
            let score = relationship["score"]
                .as_f64()
                .context("relationship score is missing")?;
            let commit_oid = relationship["sha"]
                .as_str()
                .context("relationship commit SHA is missing")?;
            tx.execute(
                "INSERT INTO relationships(repository_id,commit_oid,source,target,score,trail_id) VALUES(?1,?2,?3,?4,?5,?6)",
                params![trail.repository_id, commit_oid, source, target, score, trail.id],
            )?;
        }
        for diagnostic in diagnostics {
            let sha = diagnostic["sha"]
                .as_str()
                .context("diagnostic SHA is missing")?;
            let message = diagnostic["message"]
                .as_str()
                .context("diagnostic message is missing")?;
            tx.execute(
                "INSERT INTO diagnostics(repository_id,commit_oid,message,trail_id) VALUES(?1,?2,?3,?4)",
                params![trail.repository_id, sha, message, trail.id],
            )?;
        }
        demand::record_use(
            &tx,
            &trail.id,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn expansion_fact(
        &self,
        repo_id: i64,
        commit_oid: &str,
        extension_identity: &str,
    ) -> anyhow::Result<Option<HostResponse>> {
        self.connection
            .query_row(
                "SELECT response FROM expansion_facts WHERE repository_id=?1 AND commit_oid=?2 AND extension_identity=?3",
                params![repo_id, commit_oid, extension_identity],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| Ok(serde_json::from_str(&value)?))
            .transpose()
    }

    pub fn raw_commit(&self, repo_id: i64, oid: &str) -> anyhow::Result<Option<GitCommit>> {
        self.connection
            .query_row(
                "SELECT fact FROM raw_commit_facts WHERE repository_id=?1 AND commit_oid=?2",
                params![repo_id, oid],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|encoded| Ok(serde_json::from_str(&encoded)?))
            .transpose()
    }

    pub fn raw_parents(&self, repo_id: i64, oid: &str) -> anyhow::Result<Vec<String>> {
        let mut statement = self.connection.prepare(
            "SELECT parent_oid FROM raw_parent_edges WHERE repository_id=?1 AND commit_oid=?2 ORDER BY parent_oid",
        )?;
        Ok(statement
            .query_map(params![repo_id, oid], |row| row.get(0))?
            .collect::<Result<_, _>>()?)
    }

    pub fn prefetch_checkpoint(
        &self,
        repo_id: i64,
        head: &str,
    ) -> anyhow::Result<Option<(usize, Option<String>)>> {
        self.connection.query_row(
            "SELECT completed_count,checkpoint_oid FROM prefetch_jobs WHERE repository_id=?1 AND head_oid=?2",
            params![repo_id, head],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(Into::into)
    }

    pub fn pending_prefetch_jobs(&self) -> anyhow::Result<Vec<(PathBuf, String)>> {
        let mut statement = self.connection.prepare(
            "SELECT r.path,p.head_oid FROM prefetch_jobs p JOIN repositories r ON r.id=p.repository_id
             WHERE p.state IN ('running','paused_shutdown','paused_capacity') ORDER BY p.rowid LIMIT 32",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok((PathBuf::from(row.get::<_, String>(0)?), row.get(1)?))
            })?
            .collect::<Result<_, _>>()?)
    }

    fn reclaim_unpinned_raw(tx: &Transaction<'_>, protection: i64) -> anyhow::Result<()> {
        demand::reclaim(tx, protection)
    }

    pub fn set_staging_protection(&mut self, seconds: i64) {
        self.staging_protection = seconds;
    }

    fn bound_prefetch_pins(
        tx: &Transaction<'_>,
        preserve: Option<(i64, &str)>,
        protection: i64,
    ) -> anyhow::Result<()> {
        let active: i64 = tx.query_row(
            "SELECT COUNT(*) FROM prefetch_jobs WHERE state IN ('running','paused_shutdown','paused_capacity')",
            [],
            |row| row.get(0),
        )?;
        if active > 32 {
            tx.execute(
                "UPDATE prefetch_jobs SET state='obsolete' WHERE rowid IN (
                    SELECT rowid FROM prefetch_jobs
                    WHERE state IN ('running','paused_shutdown','paused_capacity')
                      AND (?1 IS NULL OR NOT (repository_id=?1 AND head_oid=?2))
                    ORDER BY rowid LIMIT ?3
                 )",
                params![
                    preserve.map(|(repo_id, _)| repo_id),
                    preserve.map(|(_, head)| head),
                    active - 32
                ],
            )?;
            Self::reclaim_unpinned_raw(tx, protection)?;
        }
        Ok(())
    }

    pub fn reconcile_prefetch_staging(&mut self) -> anyhow::Result<()> {
        let tx = self.connection.transaction()?;
        Self::bound_prefetch_pins(&tx, None, self.staging_protection)?;
        Self::reclaim_unpinned_raw(&tx, self.staging_protection)?;
        tx.commit()?;
        Ok(())
    }

    pub fn record_prefetch_batch(
        &mut self,
        repo_id: i64,
        head: &str,
        ceiling: usize,
        completed_count: usize,
        batch: RawCommitBatch<'_>,
        staging_quota_bytes: u64,
    ) -> anyhow::Result<bool> {
        let RawCommitBatch { facts, parents } = batch;
        let mut attempted_reclaims = 0;
        loop {
            let tx = self.connection.transaction()?;
            let used: u64 = tx.query_row(
            "SELECT COALESCE((SELECT SUM(r.bytes) FROM raw_commit_facts r
                 WHERE NOT EXISTS(SELECT 1 FROM trail_membership m WHERE m.repository_id=r.repository_id AND m.commit_oid=r.commit_oid)),0)
               + COALESCE((SELECT SUM(e.bytes) FROM raw_parent_edges e
                 WHERE NOT EXISTS(SELECT 1 FROM trail_membership m WHERE m.repository_id=e.repository_id AND m.commit_oid=e.commit_oid)),0)",
            [],
            |row| row.get(0),
        )?;
            let mut additional = 0_u64;
            let mut encoded = Vec::new();
            let mut new_edges = Vec::new();
            for fact in facts {
                let retained: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM trail_membership WHERE repository_id=?1 AND commit_oid=?2)",
                params![repo_id, fact.sha], |row| row.get(0),
            )?;
                let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM raw_commit_facts WHERE repository_id=?1 AND commit_oid=?2)",
                params![repo_id, fact.sha], |row| row.get(0),
            )?;
                if !exists {
                    let json = serde_json::to_string(fact)?;
                    if !retained {
                        additional = additional.saturating_add(json.len() as u64);
                    }
                    encoded.push((fact.sha.as_str(), json, retained));
                }
                for parent in parents.get(&fact.sha).into_iter().flatten() {
                    let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM raw_parent_edges WHERE repository_id=?1 AND commit_oid=?2 AND parent_oid=?3)",
                    params![repo_id, fact.sha, parent], |row| row.get(0),
                )?;
                    if !exists {
                        let bytes = fact.sha.len() + parent.len() + 32;
                        if !retained {
                            additional = additional.saturating_add(bytes as u64);
                        }
                        new_edges.push((fact.sha.as_str(), parent.as_str(), bytes));
                    }
                }
            }
            if used.saturating_add(additional) > staging_quota_bytes {
                if attempted_reclaims == 0 {
                    Self::reclaim_unpinned_raw(&tx, self.staging_protection)?;
                    tx.commit()?;
                    attempted_reclaims += 1;
                    continue;
                }
                if attempted_reclaims < 257
                    && demand::evict_lru(&tx, repo_id, head, self.staging_protection)?
                {
                    tx.commit()?;
                    attempted_reclaims += 1;
                    continue;
                }
                tx.execute(
                "INSERT INTO prefetch_jobs(repository_id,head_oid,ceiling,completed_count,checkpoint_oid,state) VALUES(?1,?2,?3,?4,?5,'paused_capacity')
                 ON CONFLICT(repository_id,head_oid) DO UPDATE SET state='paused_capacity'",
                params![repo_id, head, ceiling, completed_count.saturating_sub(facts.len()), Option::<&str>::None],
            )?;
                Self::bound_prefetch_pins(&tx, Some((repo_id, head)), self.staging_protection)?;
                tx.commit()?;
                return Ok(false);
            }
            for (oid, json, retained) in encoded {
                let bytes = json.len();
                tx.execute(
                "INSERT OR IGNORE INTO raw_commit_facts(repository_id,commit_oid,fact,bytes) VALUES(?1,?2,?3,?4)",
                params![repo_id, oid, json, bytes],
            )?;
                if !retained {
                    let total_bytes = bytes
                        + new_edges
                            .iter()
                            .filter(|(child, _, _)| *child == oid)
                            .map(|(_, _, bytes)| *bytes)
                            .sum::<usize>();
                    tx.execute("INSERT OR IGNORE INTO speculative_usage(repository_id,commit_oid,bytes) VALUES(?1,?2,?3)",params![repo_id,oid,total_bytes])?;
                    tx.execute("UPDATE prefetch_metrics SET produced_facts=produced_facts+1,produced_bytes=produced_bytes+?1 WHERE id=1",[total_bytes])?;
                }
            }
            for (oid, parent, bytes) in new_edges {
                tx.execute(
                "INSERT OR IGNORE INTO raw_parent_edges(repository_id,commit_oid,parent_oid,bytes) VALUES(?1,?2,?3,?4)",
                params![repo_id, oid, parent, bytes],
            )?;
            }
            tx.execute(
            "INSERT INTO prefetch_jobs(repository_id,head_oid,ceiling,completed_count,checkpoint_oid,state) VALUES(?1,?2,?3,?4,?5,'running')
             ON CONFLICT(repository_id,head_oid) DO UPDATE SET ceiling=excluded.ceiling,completed_count=excluded.completed_count,checkpoint_oid=excluded.checkpoint_oid,state='running'",
            params![repo_id, head, ceiling, completed_count, facts.last().map(|fact| fact.sha.as_str())],
        )?;
            tx.execute("INSERT OR IGNORE INTO speculative_cohorts(repository_id,head_oid,created) VALUES(?1,?2,unixepoch())",params![repo_id,head])?;
            Self::bound_prefetch_pins(&tx, Some((repo_id, head)), self.staging_protection)?;
            tx.commit()?;
            return Ok(true);
        }
    }

    pub fn set_prefetch_state(
        &mut self,
        repo_id: i64,
        head: &str,
        state: &str,
    ) -> anyhow::Result<()> {
        let tx = self.connection.transaction()?;
        tx.execute(
            "UPDATE prefetch_jobs SET state=?3 WHERE repository_id=?1 AND head_oid=?2",
            params![repo_id, head, state],
        )?;
        if matches!(state, "ready" | "obsolete" | "paused_demand") {
            tx.execute(
                "DELETE FROM prefetch_jobs WHERE rowid IN (
                    SELECT p.rowid FROM prefetch_jobs p LEFT JOIN speculative_cohorts c USING(repository_id,head_oid)
                    WHERE p.state IN ('ready','obsolete','paused_demand')
                    ORDER BY c.last_demand IS NOT NULL,c.last_demand,COALESCE(c.created,0),p.repository_id,p.head_oid
                    LIMIT MAX((SELECT COUNT(*) FROM prefetch_jobs WHERE state IN ('ready','obsolete','paused_demand')) - 256, 0)
                 )",
                [],
            )?;
            Self::reclaim_unpinned_raw(&tx, self.staging_protection)?;
        } else if matches!(state, "running" | "paused_shutdown" | "paused_capacity") {
            Self::bound_prefetch_pins(&tx, Some((repo_id, head)), self.staging_protection)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_ref_alias(
        &mut self,
        repo_id: i64,
        selector: &str,
        trail_id: &str,
        resolved_oid: &str,
    ) -> anyhow::Result<()> {
        self.connection.execute(
            "INSERT INTO ref_aliases(repository_id,selector,trail_id,resolved_oid) VALUES(?1,?2,?3,?4)
             ON CONFLICT(repository_id,selector) DO UPDATE SET trail_id=excluded.trail_id,resolved_oid=excluded.resolved_oid",
            params![repo_id, selector, trail_id, resolved_oid],
        )?;
        Ok(())
    }

    pub fn query_trail_entries(
        &self,
        trail_id: &str,
        include_invalid: bool,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let valid_clause = if include_invalid {
            ""
        } else {
            " AND s.valid=1"
        };
        let sql = format!(
            "SELECT s.commit_oid,s.annotation_index,s.entry_type,s.content,s.score,s.valid,s.commit_time,s.scope,
                    CASE WHEN tm.commit_oid IS NOT NULL THEN tm.affected_areas ELSE cm.affected_areas END,
                    CASE WHEN tm.commit_oid IS NOT NULL THEN tm.owner ELSE cm.owner END,
                    CASE WHEN tm.commit_oid IS NOT NULL THEN tm.tags ELSE COALESCE(cm.tags,'[]') END,
                    CASE WHEN tm.commit_oid IS NOT NULL THEN tm.conflicts ELSE COALESCE(cm.conflicts,'[]') END
             FROM trail_entry_state s
             JOIN trail_membership m ON m.trail_id=s.trail_id AND m.commit_oid=s.commit_oid
             LEFT JOIN commit_metadata cm ON cm.repository_id=s.repository_id AND cm.commit_oid=s.commit_oid
             LEFT JOIN trail_metadata tm ON tm.trail_id=s.trail_id AND tm.commit_oid=s.commit_oid
             WHERE s.trail_id=?1{valid_clause} ORDER BY m.position,s.annotation_index"
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

    pub fn query_trail_relationships(
        &self,
        trail_id: &str,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let mut statement = self.connection.prepare(
            "SELECT r.source,r.target,r.score
             FROM relationships r WHERE r.trail_id=?1 ORDER BY r.rowid",
        )?;
        Ok(statement
            .query_map([trail_id], |row| {
                Ok(serde_json::json!({
                    "from":row.get::<_,String>(0)?,
                    "to":row.get::<_,String>(1)?,
                    "score":row.get::<_,f64>(2)?
                }))
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn query_trail_diagnostics(
        &self,
        trail_id: &str,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let mut statement = self.connection.prepare(
            "SELECT d.commit_oid,d.message
             FROM diagnostics d WHERE d.trail_id=?1 ORDER BY d.rowid",
        )?;
        Ok(statement
            .query_map([trail_id], |row| {
                Ok(serde_json::json!({
                    "sha":row.get::<_,String>(0)?,
                    "message":row.get::<_,String>(1)?
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
        let mut cache = ReachabilityCache::default();
        for update in updates {
            evaluate_update(&tx, &mut cache, repo_id, update, true)?;
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
        let mut cache = ReachabilityCache::default();
        for update in updates {
            evaluate_update(&tx, &mut cache, repo_id, update, false)?;
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
        let result = evaluate_update(
            &tx,
            &mut ReachabilityCache::default(),
            repo_id,
            update,
            false,
        )?;
        tx.rollback()?;
        Ok(result)
    }

    pub fn query_entries(
        &self,
        repo_id: i64,
        include_invalid: bool,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let sql = if include_invalid {
            "SELECT e.commit_oid,e.annotation_index,e.entry_type,e.content,e.score,e.valid,e.commit_time,e.scope,m.affected_areas,m.owner,m.tags,m.conflicts FROM entries e LEFT JOIN commit_metadata m ON m.repository_id=e.repository_id AND m.commit_oid=e.commit_oid WHERE e.repository_id=?1 ORDER BY e.rowid"
        } else {
            "SELECT e.commit_oid,e.annotation_index,e.entry_type,e.content,e.score,e.valid,e.commit_time,e.scope,m.affected_areas,m.owner,m.tags,m.conflicts FROM entries e LEFT JOIN commit_metadata m ON m.repository_id=e.repository_id AND m.commit_oid=e.commit_oid WHERE e.repository_id=?1 AND e.valid=1 ORDER BY e.rowid"
        };
        let mut statement = self.connection.prepare(sql)?;
        Ok(statement.query_map([repo_id], |row| {
            let affected: Option<String> = row.get(8)?;
            let tags = row.get::<_, Option<String>>(10)?.unwrap_or_else(|| "[]".to_owned());
            let conflicts = row.get::<_, Option<String>>(11)?.unwrap_or_else(|| "[]".to_owned());
            Ok(serde_json::json!({
                "sha":row.get::<_,String>(0)?,"index":row.get::<_,u32>(1)?,"type":row.get::<_,String>(2)?,
                "content":row.get::<_,String>(3)?,"score":row.get::<_,f64>(4)?,"valid":row.get::<_,bool>(5)?,
                "commit_time":row.get::<_,i64>(6)?,"scope":row.get::<_,Option<String>>(7)?,
                "affected_areas":affected.map(|value| serde_json::from_str::<serde_json::Value>(&value)).transpose().map_err(|error| rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(error)))?,
                "owner":row.get::<_,Option<String>>(9)?,
                "tags":serde_json::from_str::<serde_json::Value>(&tags).map_err(|error| rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(error)))?,
                "metadata_conflicts":serde_json::from_str::<serde_json::Value>(&conflicts).map_err(|error| rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, Box::new(error)))?
            }))
        })?.collect::<Result<_, _>>()?)
    }

    pub fn query_relationships(&self, repo_id: i64) -> anyhow::Result<Vec<serde_json::Value>> {
        let mut statement = self.connection.prepare(
            "SELECT commit_oid,source,target,score FROM relationships WHERE repository_id=?1 ORDER BY rowid",
        )?;
        Ok(statement
            .query_map([repo_id], |row| {
                Ok(serde_json::json!({
                    "sha": row.get::<_, String>(0)?,
                    "from": row.get::<_, String>(1)?,
                    "to": row.get::<_, String>(2)?,
                    "score": row.get::<_, f64>(3)?,
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
        let mut statement = self.connection.prepare("SELECT c.repository_id,c.oid,c.commit_time,COUNT(e.annotation_index) FROM commits c JOIN entries e ON e.repository_id=c.repository_id AND e.commit_oid=c.oid WHERE NOT EXISTS(SELECT 1 FROM trail_membership m WHERE m.repository_id=c.repository_id AND m.commit_oid=c.oid) GROUP BY c.repository_id,c.oid,c.commit_time")?;
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

    pub fn trail_cohorts(
        &self,
        now: i64,
        protect_recent_seconds: i64,
    ) -> anyhow::Result<Vec<TrailCohort>> {
        let mut statement = self.connection.prepare(
            "SELECT t.repository_id,t.id,t.source_time,
                    EXISTS(SELECT 1 FROM ref_aliases a WHERE a.trail_id=t.id),
                    COUNT(s.annotation_index)
             FROM trails t
             LEFT JOIN trail_entry_state s ON s.trail_id=t.id
             GROUP BY t.repository_id,t.id,t.source_time",
        )?;
        Ok(statement
            .query_map([], |row| {
                let source_time = row.get::<_, i64>(2)?;
                Ok(TrailCohort {
                    repository_id: row.get(0)?,
                    trail_id: row.get(1)?,
                    source_time,
                    referenced: row.get(3)?,
                    protected: protect_recent_seconds > 0
                        && source_time > now - protect_recent_seconds,
                    entries: row.get(4)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn evict_trails(&mut self, trail_ids: &[String]) -> anyhow::Result<()> {
        let tx = self.connection.transaction()?;
        for trail_id in trail_ids {
            tx.execute("DELETE FROM trails WHERE id=?1", [trail_id])?;
        }
        tx.commit()?;
        Ok(())
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

#[cfg(test)]
mod deadline_tests {
    use super::*;

    #[test]
    fn expired_request_interrupts_sqlite_work_without_poisoning_connection() {
        let store = Store {
            connection: Connection::open_in_memory().unwrap(),
            staging_protection: 0,
        };
        store
            .set_request_deadline(Instant::now() - Duration::from_millis(1))
            .unwrap();
        let result: rusqlite::Result<i64> = store.connection.query_row(
            "WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n<1000000) SELECT SUM(n) FROM numbers",
            [],
            |row| row.get(0),
        );
        assert!(matches!(
            result,
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::OperationInterrupted
        ));
        store.connection.progress_handler(0, None::<fn() -> bool>);
        assert_eq!(
            store
                .connection
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
