//! Core zmem identities, wire records, and synchronization decisions.

use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

pub const PROTOCOL_VERSION: u32 = 3;
pub const SCHEMA_VERSION: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttentionLimit {
    Zero,
    Limited(NonZeroUsize),
    Unlimited,
}

impl AttentionLimit {
    pub fn parse(value: i64, name: &str) -> anyhow::Result<Self> {
        if value == -1 {
            return Ok(Self::Unlimited);
        }
        anyhow::ensure!(value > 0, "{name} limit must be a positive integer or -1");
        let converted = usize::try_from(value)?;
        Ok(Self::Limited(
            NonZeroUsize::new(converted).expect("positive value is nonzero"),
        ))
    }

    pub fn as_i64(self) -> i64 {
        match self {
            Self::Zero => 0,
            Self::Limited(value) => i64::try_from(value.get()).unwrap_or(i64::MAX),
            Self::Unlimited => -1,
        }
    }

    pub fn maximum(self) -> Option<usize> {
        match self {
            Self::Zero => Some(0),
            Self::Limited(value) => Some(value.get()),
            Self::Unlimited => None,
        }
    }

    pub fn zero() -> Self {
        Self::Zero
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionPolicy {
    pub commit_limit: AttentionLimit,
    pub node_limit: AttentionLimit,
}

impl Default for AttentionPolicy {
    fn default() -> Self {
        Self {
            commit_limit: AttentionLimit::Limited(NonZeroUsize::new(500).unwrap()),
            node_limit: AttentionLimit::Limited(NonZeroUsize::new(400).unwrap()),
        }
    }
}

impl AttentionPolicy {
    pub fn resolve(
        request_commit: Option<i64>,
        request_node: Option<i64>,
        environment_commit: Option<&str>,
        environment_node: Option<&str>,
    ) -> anyhow::Result<Self> {
        fn value(
            requested: Option<i64>,
            environment: Option<&str>,
            fallback: i64,
            name: &str,
        ) -> anyhow::Result<AttentionLimit> {
            let resolved = if let Some(requested) = requested {
                requested
            } else if let Some(environment) = environment {
                environment
                    .parse::<i64>()
                    .map_err(|_| anyhow::anyhow!("invalid {name} limit"))?
            } else {
                fallback
            };
            AttentionLimit::parse(resolved, name)
        }
        Ok(Self {
            commit_limit: value(request_commit, environment_commit, 500, "commit")?,
            node_limit: value(request_node, environment_node, 400, "node")?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionBound {
    Commit,
    Node,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionUsage {
    pub commit_limit: i64,
    pub node_limit: i64,
    pub selected_commits: usize,
    pub selected_nodes: usize,
    pub truncated: bool,
    pub reached: Vec<AttentionBound>,
}

impl AttentionUsage {
    pub fn view_identity(&self, lower_boundary: Option<&str>) -> String {
        let reached = self
            .reached
            .iter()
            .map(|bound| match bound {
                AttentionBound::Commit => "commit",
                AttentionBound::Node => "node",
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "v1:{}:{}:{}:{}:{}:{}:{}",
            self.commit_limit,
            self.node_limit,
            self.selected_commits,
            self.selected_nodes,
            self.truncated,
            reached,
            lower_boundary.unwrap_or("")
        )
    }
}

pub fn attention_identity_allows_incremental(identity: &str, policy: AttentionPolicy) -> bool {
    let parts = identity.split(':').collect::<Vec<_>>();
    parts.len() == 8
        && parts[0] == "v1"
        && parts[1] == policy.commit_limit.as_i64().to_string()
        && parts[2] == policy.node_limit.as_i64().to_string()
        && parts[5] == "false"
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttentionCandidate<T> {
    pub value: T,
    pub annotation_count: usize,
}

impl<T> AttentionCandidate<T> {
    pub fn new(value: T, annotation_count: usize) -> Self {
        Self {
            value,
            annotation_count,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttentionSelection<T> {
    pub selected: Vec<T>,
    pub usage: AttentionUsage,
}

pub fn select_attention<T>(
    candidates: impl IntoIterator<Item = AttentionCandidate<T>>,
    policy: AttentionPolicy,
    reserved_nodes: usize,
    commit_truncated: bool,
) -> anyhow::Result<AttentionSelection<T>> {
    if let Some(maximum) = policy.node_limit.maximum() {
        anyhow::ensure!(
            reserved_nodes <= maximum,
            "proposed message exceeds node attention limit"
        );
    }
    let mut nodes = reserved_nodes;
    let mut selected = Vec::new();
    let mut reached = Vec::new();
    for candidate in candidates {
        if let Some(maximum) = policy.node_limit.maximum()
            && nodes.saturating_add(candidate.annotation_count) > maximum
        {
            reached.push(AttentionBound::Node);
            break;
        }
        nodes += candidate.annotation_count;
        selected.push(candidate.value);
    }
    if commit_truncated {
        reached.insert(0, AttentionBound::Commit);
    }
    selected.reverse();
    Ok(AttentionSelection {
        usage: AttentionUsage {
            commit_limit: policy.commit_limit.as_i64(),
            node_limit: policy.node_limit.as_i64(),
            selected_commits: selected.len(),
            selected_nodes: nodes,
            truncated: !reached.is_empty(),
            reached,
        },
        selected,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub head: String,
    pub schema: u32,
    pub extension_hash: String,
    #[serde(default)]
    pub attention_identity: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncDecision {
    Current,
    Incremental,
    Rebuild,
}

pub fn select_sync(
    anchor: Option<&Anchor>,
    requested_head: &str,
    schema: u32,
    extension_hash: &str,
    attention_identity: &str,
    anchor_is_ancestor: bool,
) -> SyncDecision {
    let Some(anchor) = anchor else {
        return SyncDecision::Rebuild;
    };
    if anchor.schema != schema
        || anchor.extension_hash != extension_hash
        || anchor.attention_identity != attention_identity
    {
        return SyncDecision::Rebuild;
    }
    if anchor.head == requested_head {
        SyncDecision::Current
    } else if anchor_is_ancestor {
        SyncDecision::Incremental
    } else {
        SyncDecision::Rebuild
    }
}

pub fn validate_factor(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    AddEntry {
        commit_sha: String,
        annotation_index: u32,
        #[serde(rename = "type")]
        entry_type: String,
        content: String,
        score: f64,
        valid: bool,
        #[serde(default)]
        commit_time: i64,
        #[serde(default)]
        scope: Option<String>,
    },
    AddRelationship {
        commit_sha: String,
        source: String,
        target: String,
        score: f64,
    },
    Decay {
        target_sha: String,
        target_index: u32,
        factor: f64,
    },
    Cancel {
        target_sha: String,
        target_index: u32,
    },
    Diagnose {
        message: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionJournal {
    pub version: u32,
    pub origin: String,
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostResponse {
    pub protocol_version: u32,
    pub extension_hash: String,
    pub journal: ActionJournal,
    #[serde(default)]
    pub hook_diagnostics: Vec<String>,
    #[serde(default)]
    pub annotation_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInspection {
    pub protocol_version: u32,
    pub annotation_count: usize,
    #[serde(default)]
    pub parser_diagnostics: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdentifiedHostInspection {
    id: String,
    annotation_count: usize,
    #[serde(default)]
    parser_diagnostics: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostInspectionBatch {
    protocol_version: u32,
    inspections: Vec<IdentifiedHostInspection>,
}

pub fn validate_host_inspection_batch(
    data: &[u8],
    expected_ids: &[String],
) -> anyhow::Result<Vec<HostInspection>> {
    let batch: HostInspectionBatch = serde_json::from_slice(data)?;
    anyhow::ensure!(
        batch.protocol_version == PROTOCOL_VERSION,
        "unsupported extension protocol"
    );
    anyhow::ensure!(
        batch.inspections.len() == expected_ids.len(),
        "incomplete extension inspection batch"
    );
    batch
        .inspections
        .into_iter()
        .zip(expected_ids)
        .map(|(inspection, expected)| {
            anyhow::ensure!(
                inspection.id == *expected,
                "extension inspection batch identity or order mismatch"
            );
            Ok(HostInspection {
                protocol_version: PROTOCOL_VERSION,
                annotation_count: inspection.annotation_count,
                parser_diagnostics: inspection.parser_diagnostics,
            })
        })
        .collect()
}

pub fn validate_host_inspection(data: &[u8]) -> anyhow::Result<HostInspection> {
    let inspection: HostInspection = serde_json::from_slice(data)?;
    anyhow::ensure!(
        inspection.protocol_version == PROTOCOL_VERSION,
        "unsupported extension protocol"
    );
    Ok(inspection)
}

pub fn validate_action_journal(data: &[u8]) -> anyhow::Result<HostResponse> {
    let response: HostResponse = serde_json::from_slice(data)?;
    anyhow::ensure!(
        response.protocol_version == PROTOCOL_VERSION,
        "unsupported extension protocol"
    );
    anyhow::ensure!(
        !response.extension_hash.is_empty(),
        "missing extension identity"
    );
    anyhow::ensure!(
        response.journal.version == 1 && response.journal.origin == "zmem-expansion-context",
        "invalid action journal provenance"
    );
    for action in &response.journal.actions {
        match action {
            Action::AddEntry {
                annotation_index,
                score,
                ..
            } => {
                anyhow::ensure!(*annotation_index > 0, "annotation indexes are one-based");
                anyhow::ensure!(validate_factor(*score), "invalid entry score");
            }
            Action::AddRelationship { score, .. } => {
                anyhow::ensure!(validate_factor(*score), "invalid relationship score")
            }
            Action::Decay {
                factor,
                target_index,
                ..
            } => anyhow::ensure!(
                validate_factor(*factor) && *target_index > 0,
                "invalid decay action"
            ),
            Action::Cancel { target_index, .. } => {
                anyhow::ensure!(*target_index > 0, "invalid cancel action")
            }
            Action::Diagnose { message } => {
                anyhow::ensure!(!message.is_empty(), "empty diagnostic action")
            }
        }
    }
    Ok(response)
}

#[derive(Clone, Debug)]
pub struct GitRepo {
    root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct GitCommit {
    pub sha: String,
    pub commit_time: i64,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitWalk {
    pub shas: Vec<String>,
    pub truncated: bool,
}

impl GitRepo {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let output = Command::new("git")
            .args([
                "-C",
                &path.to_string_lossy(),
                "rev-parse",
                "--show-toplevel",
            ])
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "not a Git repository: {}",
            path.display()
        );
        let root = String::from_utf8(output.stdout)?.trim().to_owned();
        Ok(Self {
            root: PathBuf::from(root).canonicalize()?,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn head(&self) -> anyhow::Result<String> {
        self.git(&["rev-parse", "HEAD"])
            .map(|value| value.trim().to_owned())
    }

    pub fn resolve(&self, reference: &str) -> anyhow::Result<String> {
        self.git(&["rev-parse", "--verify", &format!("{reference}^{{commit}}")])
            .map(|value| value.trim().to_owned())
    }

    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> anyhow::Result<bool> {
        let status = Command::new("git")
            .args([
                "-C",
                &self.root.to_string_lossy(),
                "merge-base",
                "--is-ancestor",
                ancestor,
                descendant,
            ])
            .status()?;
        Ok(status.success())
    }

    pub fn walk(&self, after: Option<&str>, head: &str) -> anyhow::Result<Vec<String>> {
        let range = after.map_or_else(|| head.to_owned(), |anchor| format!("{anchor}..{head}"));
        let output = self.git(&["rev-list", "--reverse", "--topo-order", &range])?;
        Ok(output
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    pub fn walk_newest(&self, head: &str, limit: AttentionLimit) -> anyhow::Result<GitWalk> {
        let mut arguments = vec!["rev-list".to_owned(), "--topo-order".to_owned()];
        let sentinel = limit.maximum().map(|maximum| maximum.saturating_add(1));
        if let Some(sentinel) = sentinel {
            arguments.push(format!("--max-count={sentinel}"));
        }
        arguments.push(head.to_owned());
        let references = arguments.iter().map(String::as_str).collect::<Vec<_>>();
        let output = self.git(&references)?;
        let mut shas = output
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let truncated = limit.maximum().is_some_and(|maximum| shas.len() > maximum);
        if let Some(maximum) = limit.maximum() {
            shas.truncate(maximum);
        }
        Ok(GitWalk { shas, truncated })
    }

    pub fn commit(&self, sha: &str) -> anyhow::Result<GitCommit> {
        let output = self.git(&["show", "-s", "--format=%H%x00%ct%x00%B", sha])?;
        let mut parts = output.splitn(3, '\0');
        let full_sha = parts.next().unwrap_or_default().trim().to_owned();
        let commit_time = parts.next().unwrap_or_default().trim().parse()?;
        let message = parts.next().unwrap_or_default().trim_end().to_owned();
        Ok(GitCommit {
            sha: full_sha,
            commit_time,
            message,
        })
    }

    fn git(&self, args: &[&str]) -> anyhow::Result<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(String::from_utf8(output.stdout)?)
    }
}

pub fn run_ordered<T, R, F>(jobs: Vec<T>, limit: usize, worker: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(T) -> R + Send + Sync,
{
    assert!(limit > 0);
    let worker = Arc::new(worker);
    let mut pending = jobs.into_iter().enumerate();
    let mut completed: Vec<(usize, R)> = Vec::new();
    loop {
        let chunk: Vec<_> = pending.by_ref().take(limit).collect();
        if chunk.is_empty() {
            break;
        }
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(chunk.len());
            for (index, job) in chunk {
                let worker = Arc::clone(&worker);
                handles.push(scope.spawn(move || (index, worker(job))));
            }
            for handle in handles {
                completed.push(handle.join().expect("expansion worker panicked"));
            }
        });
    }
    completed.sort_by_key(|(index, _)| *index);
    completed.into_iter().map(|(_, result)| result).collect()
}
