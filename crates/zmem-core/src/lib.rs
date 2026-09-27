//! Core zmem identities, wire records, and synchronization decisions.

use serde::{Deserialize, Serialize};
pub mod demand;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const PROTOCOL_VERSION: u32 = 5;
pub const SCHEMA_VERSION: u32 = 6;

#[cfg(test)]
mod deadline_tests {
    use super::*;

    #[test]
    fn timed_out_child_is_killed_and_reaped_promptly() {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("ping");
            command.args(["-n", "10", "127.0.0.1"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sleep");
            command.arg("10");
            command
        };
        let started = Instant::now();
        let result = with_git_deadline(Some(started + Duration::from_millis(250)), || {
            run_git_command_input(&mut command, Some(&vec![b'x'; 200_000]))
        });
        assert!(result.unwrap_err().to_string().contains("deadline expired"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn cancelled_child_is_killed_and_reaped_promptly() {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("ping");
            command.args(["-n", "10", "127.0.0.1"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sleep");
            command.arg("10");
            command
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancelled);
        let signaler = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            signal.store(true, Ordering::Release);
        });
        let started = Instant::now();
        let result = with_git_cancellation(Some(cancelled), || {
            run_git_command_input(&mut command, Some(&vec![b'x'; 200_000]))
        });
        signaler.join().unwrap();
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}

thread_local! {
    static GIT_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
    static GIT_CANCELLATION: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

pub fn with_git_cancellation<T>(
    cancelled: Option<Arc<AtomicBool>>,
    action: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<Arc<AtomicBool>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            GIT_CANCELLATION.replace(self.0.take());
        }
    }
    let previous = GIT_CANCELLATION.replace(cancelled);
    let _restore = Restore(previous);
    action()
}

pub fn with_git_deadline<T>(deadline: Option<Instant>, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<Instant>);
    impl Drop for Restore {
        fn drop(&mut self) {
            GIT_DEADLINE.set(self.0);
        }
    }
    let previous = GIT_DEADLINE.replace(deadline);
    let _restore = Restore(previous);
    action()
}

fn run_git_command(command: &mut Command) -> anyhow::Result<Output> {
    run_git_command_input(command, None)
}

fn run_git_command_input(command: &mut Command, input: Option<&[u8]>) -> anyhow::Result<Output> {
    let deadline = GIT_DEADLINE.get();
    if deadline.is_none()
        && input.is_none()
        && !GIT_CANCELLATION.with(|token| token.borrow().is_some())
    {
        return Ok(command.output()?);
    }
    supervise_process(
        command,
        input,
        deadline,
        GIT_CANCELLATION.with(|token| token.borrow().clone()),
    )
}

/// Supervise the process and all three pipes under one budget. Input writes
/// must not block the thread responsible for killing an abandoned process.
pub fn supervise_process(
    command: &mut Command,
    input: Option<&[u8]>,
    deadline: Option<Instant>,
    cancelled: Option<Arc<AtomicBool>>,
) -> anyhow::Result<Output> {
    let is_cancelled = || {
        cancelled
            .as_ref()
            .is_some_and(|token| token.load(Ordering::Acquire))
    };
    anyhow::ensure!(!is_cancelled(), "service request cancelled");
    if let Some(deadline) = deadline {
        anyhow::ensure!(
            Instant::now() < deadline,
            "service request deadline expired"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("git stdout pipe");
    let mut stderr = child.stderr.take().expect("git stderr pipe");
    let out_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let err_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let input_writer = input.map(|input| {
        let mut stdin = child.stdin.take().expect("child stdin pipe");
        let owned = input.to_vec();
        std::thread::spawn(move || stdin.write_all(&owned))
    });
    let mut status = None;
    let completion = loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break Err(anyhow::anyhow!("service request deadline expired"));
        }
        if is_cancelled() {
            break Err(anyhow::anyhow!("service request cancelled"));
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(found) => status = found,
                Err(error) => break Err(error.into()),
            }
        }
        if let Some(status) = status
            && out_reader.is_finished()
            && err_reader.is_finished()
            && input_writer
                .as_ref()
                .is_none_or(std::thread::JoinHandle::is_finished)
        {
            break Ok(status);
        }
        std::thread::sleep(deadline.map_or(Duration::from_millis(5), |deadline| {
            Duration::from_millis(5).min(deadline.saturating_duration_since(Instant::now()))
        }));
    };
    if completion.is_err() {
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            // SAFETY: this child owns the process group created above.
            unsafe {
                kill(-(child.id() as i32), 9);
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        if let Some(writer) = input_writer {
            cancel_pipe_io(writer);
        }
        cancel_pipe_io(out_reader);
        cancel_pipe_io(err_reader);
        return completion.map(|_| unreachable!());
    }
    if let Some(writer) = input_writer {
        writer
            .join()
            .map_err(|_| anyhow::anyhow!("child input writer panicked"))??;
    }
    let stdout = out_reader
        .join()
        .map_err(|_| anyhow::anyhow!("git stdout reader panicked"))??;
    let stderr = err_reader
        .join()
        .map_err(|_| anyhow::anyhow!("git stderr reader panicked"))??;
    Ok(Output {
        status: completion?,
        stdout,
        stderr,
    })
}

fn cancel_pipe_io<T>(worker: std::thread::JoinHandle<T>) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CancelSynchronousIo(thread: *mut std::ffi::c_void) -> i32;
        }
        while !worker.is_finished() {
            // SAFETY: the handle belongs to this live, owned I/O thread. Repeat
            // to cover cancellation racing the thread's first pipe operation.
            unsafe {
                CancelSynchronousIo(worker.as_raw_handle());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let _ = worker.join();
}

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrailIdentity {
    pub repository_id: i64,
    pub head_oid: String,
    pub attention: AttentionPolicy,
    pub extension_identity: String,
    pub protocol_version: u32,
    pub schema_version: u32,
}

impl TrailIdentity {
    pub fn new(
        repository_id: i64,
        head_oid: String,
        attention: AttentionPolicy,
        extension_identity: impl Into<String>,
        protocol_version: u32,
        schema_version: u32,
    ) -> Self {
        Self {
            repository_id,
            head_oid,
            attention,
            extension_identity: extension_identity.into(),
            protocol_version,
            schema_version,
        }
    }

    pub fn key(&self) -> String {
        format!(
            "trail:{}:{}:{}:{}:{}:{}:{}",
            self.repository_id,
            self.head_oid,
            self.attention.commit_limit.as_i64(),
            self.attention.node_limit.as_i64(),
            self.extension_identity,
            self.protocol_version,
            self.schema_version
        )
    }
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
    pub fn from_view_identity(identity: &str) -> Option<Self> {
        let parts = identity.split(':').collect::<Vec<_>>();
        if parts.len() != 8 || parts[0] != "v1" {
            return None;
        }
        let reached = if parts[6].is_empty() {
            Vec::new()
        } else {
            parts[6]
                .split(',')
                .map(|bound| match bound {
                    "commit" => Some(AttentionBound::Commit),
                    "node" => Some(AttentionBound::Node),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?
        };
        Some(Self {
            commit_limit: parts[1].parse().ok()?,
            node_limit: parts[2].parse().ok()?,
            selected_commits: parts[3].parse().ok()?,
            selected_nodes: parts[4].parse().ok()?,
            truncated: parts[5].parse().ok()?,
            reached,
        })
    }

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
    MetadataPatch {
        from_sha: String,
        to_sha: String,
        operations: Vec<MetadataOperation>,
    },
    Diagnose {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataOperation {
    pub key: String,
    pub operator: MetadataOperator,
    pub value: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataOperator {
    Set,
    Add,
    Null,
}

pub fn validate_metadata_operation(operation: &MetadataOperation) -> anyhow::Result<()> {
    anyhow::ensure!(
        matches!(operation.key.as_str(), "affected_areas" | "owner" | "tags"),
        "unsupported metadata key"
    );
    match operation.operator {
        MetadataOperator::Null => {
            anyhow::ensure!(operation.value.is_none(), "null operation carries a value")
        }
        MetadataOperator::Add => {
            anyhow::ensure!(
                operation.key != "owner",
                "metadata add requires a set-valued key"
            );
            anyhow::ensure!(
                operation
                    .value
                    .as_ref()
                    .is_some_and(|value| !value.is_empty()),
                "metadata operation requires a value"
            );
        }
        MetadataOperator::Set => {
            anyhow::ensure!(
                operation
                    .value
                    .as_ref()
                    .is_some_and(|value| !value.is_empty()),
                "metadata operation requires a value"
            );
        }
    }
    if operation.key == "affected_areas" && operation.operator != MetadataOperator::Null {
        validate_area(operation.value.as_deref().unwrap())?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrailCommit {
    pub oid: String,
    pub ancestors: BTreeSet<String>,
}

pub fn resolve_meta_range(
    membership: &[TrailCommit],
    meta_oid: &str,
    from_oid: &str,
    to_oid: &str,
    complete: bool,
) -> anyhow::Result<Vec<String>> {
    anyhow::ensure!(complete, "incomplete META range");
    let by_oid = membership
        .iter()
        .map(|commit| (commit.oid.as_str(), commit))
        .collect::<BTreeMap<_, _>>();
    let from = by_oid
        .get(from_oid)
        .ok_or_else(|| anyhow::anyhow!("missing META from endpoint"))?;
    let to = by_oid
        .get(to_oid)
        .ok_or_else(|| anyhow::anyhow!("missing META to endpoint"))?;
    let meta = by_oid
        .get(meta_oid)
        .ok_or_else(|| anyhow::anyhow!("missing META commit"))?;
    anyhow::ensure!(
        from.oid == to.oid || to.ancestors.contains(&from.oid),
        "META from is not an ancestor of to"
    );
    anyhow::ensure!(
        meta.ancestors.contains(&from.oid) && meta.ancestors.contains(&to.oid),
        "META endpoints must precede META commit"
    );
    Ok(membership
        .iter()
        .filter(|commit| {
            (commit.oid == from.oid || commit.ancestors.contains(&from.oid))
                && (commit.oid == to.oid || to.ancestors.contains(&commit.oid))
        })
        .map(|commit| commit.oid.clone())
        .collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReachableAssignment {
    pub commit_oid: String,
    pub value: String,
    pub ancestors: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectiveMetadataValue {
    Resolved(String),
    Conflict(Vec<String>),
}

pub fn resolve_metadata_assignments(assignments: &[ReachableAssignment]) -> EffectiveMetadataValue {
    let maximal = assignments.iter().filter(|candidate| {
        !assignments.iter().any(|other| {
            other.commit_oid != candidate.commit_oid
                && other.ancestors.contains(&candidate.commit_oid)
        })
    });
    let values = maximal
        .map(|assignment| assignment.value.clone())
        .collect::<BTreeSet<_>>();
    if values.len() == 1 {
        EffectiveMetadataValue::Resolved(values.into_iter().next().unwrap())
    } else {
        EffectiveMetadataValue::Conflict(values.into_iter().collect())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionJournal {
    pub version: u32,
    pub origin: String,
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
            Action::MetadataPatch {
                from_sha,
                to_sha,
                operations,
            } => {
                anyhow::ensure!(
                    !from_sha.is_empty() && !to_sha.is_empty(),
                    "metadata patch endpoints are required"
                );
                anyhow::ensure!(
                    !operations.is_empty(),
                    "metadata patch operations are required"
                );
                for operation in operations {
                    validate_metadata_operation(operation)?;
                }
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitCommit {
    pub sha: String,
    pub commit_time: i64,
    pub message: String,
    pub changes: Vec<ChangedPath>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedPath {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
}

impl ChangedPath {
    pub fn path(path: impl Into<String>) -> Self {
        Self {
            old_path: None,
            new_path: Some(path.into()),
        }
    }

    pub fn delete(path: impl Into<String>) -> Self {
        Self {
            old_path: Some(path.into()),
            new_path: None,
        }
    }

    pub fn rename(old_path: impl Into<String>, new_path: impl Into<String>) -> Self {
        Self {
            old_path: Some(old_path.into()),
            new_path: Some(new_path.into()),
        }
    }

    fn endpoints(&self) -> impl Iterator<Item = &str> {
        self.old_path
            .iter()
            .chain(self.new_path.iter())
            .map(String::as_str)
    }
}

fn validate_area(value: &str) -> anyhow::Result<()> {
    if value == "<root>" {
        return Ok(());
    }
    anyhow::ensure!(
        !value.is_empty()
            && !value.starts_with('/')
            && !value.contains('\\')
            && !value.contains("//")
            && value
                .split('/')
                .all(|part| !matches!(part, "" | "." | "..")),
        "affected area must be a normalized repository-relative path"
    );
    Ok(())
}

pub fn derive_affected_areas(changes: &[ChangedPath]) -> Option<Vec<String>> {
    let mut root = false;
    let mut groups: BTreeMap<String, Vec<Vec<&str>>> = BTreeMap::new();
    for path in changes.iter().flat_map(ChangedPath::endpoints) {
        let parts = path.split('/').collect::<Vec<_>>();
        if parts.len() == 1 {
            root = true;
            continue;
        }
        let parent = &parts[..parts.len() - 1];
        groups
            .entry(parts[0].to_owned())
            .or_default()
            .push(parent.to_vec());
    }
    let mut areas = Vec::new();
    if root {
        areas.push("<root>".to_owned());
    }
    for parents in groups.values() {
        let first = &parents[0];
        let common_len = (0..first.len())
            .take_while(|index| {
                parents
                    .iter()
                    .all(|parent| parent.get(*index) == first.get(*index))
            })
            .count();
        areas.push(first[..common_len.max(1)].join("/"));
    }
    areas.sort_by(|left, right| match (left.as_str(), right.as_str()) {
        ("<root>", "<root>") => std::cmp::Ordering::Equal,
        ("<root>", _) => std::cmp::Ordering::Less,
        (_, "<root>") => std::cmp::Ordering::Greater,
        _ => left.cmp(right),
    });
    areas.dedup();
    (areas.len() <= 3).then_some(areas)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedSelector {
    pub selector: String,
    pub oid: String,
    pub local_branch: bool,
    pub route: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitWalk {
    pub shas: Vec<String>,
    pub parents: std::collections::BTreeMap<String, Vec<String>>,
    pub truncated: bool,
}

impl GitRepo {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let output = run_git_command(Command::new("git").args([
            "-C",
            &path.to_string_lossy(),
            "rev-parse",
            "--show-toplevel",
        ]))?;
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

    pub fn resolve_observed(
        &self,
        reference: &str,
        observed_oid: &str,
    ) -> anyhow::Result<ResolvedSelector> {
        let oid = self.resolve(reference)?;
        anyhow::ensure!(
            oid == observed_oid,
            "stale ref: observed {observed_oid}, resolved {oid}"
        );
        let named = self.git(&["rev-parse", "--symbolic-full-name", reference])?;
        let name = named.trim();
        let route = if name.starts_with("refs/") && !name.contains('\n') {
            name.to_owned()
        } else {
            format!("oid:{oid}")
        };
        let local_branch = route.starts_with("refs/heads/");
        Ok(ResolvedSelector {
            selector: reference.to_owned(),
            oid,
            local_branch,
            route,
        })
    }

    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> anyhow::Result<bool> {
        let status = run_git_command(Command::new("git").args([
            "-C",
            &self.root.to_string_lossy(),
            "merge-base",
            "--is-ancestor",
            ancestor,
            descendant,
        ]))?
        .status;
        Ok(status.success())
    }

    pub fn ancestors(&self, commit: &str) -> anyhow::Result<Vec<String>> {
        Ok(self
            .git(&["rev-list", commit])?
            .lines()
            .skip(1)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
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
        let mut arguments = vec![
            "rev-list".to_owned(),
            "--topo-order".to_owned(),
            "--parents".to_owned(),
        ];
        let sentinel = limit.maximum().map(|maximum| maximum.saturating_add(1));
        if let Some(sentinel) = sentinel {
            arguments.push(format!("--max-count={sentinel}"));
        }
        arguments.push(head.to_owned());
        let references = arguments.iter().map(String::as_str).collect::<Vec<_>>();
        let output = self.git(&references)?;
        let mut parents = std::collections::BTreeMap::new();
        let mut shas = output
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let mut fields = line.split_whitespace();
                let sha = fields
                    .next()
                    .expect("rev-list --parents emits an OID")
                    .to_owned();
                parents.insert(sha.clone(), fields.map(str::to_owned).collect());
                sha
            })
            .collect::<Vec<_>>();
        let truncated = limit.maximum().is_some_and(|maximum| shas.len() > maximum);
        if let Some(maximum) = limit.maximum() {
            shas.truncate(maximum);
        }
        let selected = shas.iter().collect::<std::collections::BTreeSet<_>>();
        parents.retain(|sha, _| selected.contains(sha));
        Ok(GitWalk {
            shas,
            parents,
            truncated,
        })
    }

    pub fn walk_trail(&self, head: &str, limit: AttentionLimit) -> anyhow::Result<GitWalk> {
        let mut walk = self.walk_newest(head, limit)?;
        walk.shas.reverse();
        Ok(walk)
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
            changes: self.changed_paths(sha)?,
        })
    }

    /// Collect commit messages and changed paths with two Git processes per
    /// batch, preserving caller order and the single-commit diff semantics.
    pub fn commits_batch(&self, shas: &[String]) -> anyhow::Result<Vec<GitCommit>> {
        if shas.is_empty() {
            return Ok(Vec::new());
        }
        let mut show_args = vec!["show", "-s", "--format=%H%x00%ct%x00%B%x00"];
        show_args.extend(shas.iter().map(String::as_str));
        let show = self.git(&show_args)?;
        let mut fields = show.split('\0');
        let mut commits = Vec::with_capacity(shas.len());
        for expected in shas {
            let oid = fields
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing batched commit OID"))?
                .trim();
            anyhow::ensure!(oid == expected, "batched commit order changed");
            let commit_time = fields
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing batched commit time"))?
                .parse()?;
            let message = fields
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing batched commit message"))?
                .trim_end()
                .to_owned();
            commits.push(GitCommit {
                sha: oid.to_owned(),
                commit_time,
                message,
                changes: Vec::new(),
            });
        }
        anyhow::ensure!(
            fields.all(|field| field.trim().is_empty()),
            "extra batched commit output"
        );

        let mut input = shas.join("\n").into_bytes();
        input.push(b'\n');
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.root).args([
            "diff-tree",
            "--stdin",
            "--root",
            "--name-status",
            "-r",
            "-M",
            "-z",
        ]);
        let output = run_git_command_input(&mut command, Some(&input))?;
        anyhow::ensure!(
            output.status.success(),
            "batched git diff-tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let changed = String::from_utf8(output.stdout)?;
        let tokens = changed.split('\0').collect::<Vec<_>>();
        let mut position = 0;
        for (index, commit) in commits.iter_mut().enumerate() {
            let next = tokens.get(position).copied().unwrap_or("");
            if next != commit.sha {
                // diff-tree omits merges and empty commits without changed paths.
                anyhow::ensure!(
                    next.is_empty() || shas[index + 1..].iter().any(|sha| sha == next),
                    "batched diff header changed"
                );
                continue;
            }
            position += 1;
            while let Some(status) = tokens.get(position).copied() {
                if status.is_empty() || shas[index + 1..].iter().any(|next| status == next) {
                    break;
                }
                position += 1;
                if status.starts_with('R') || status.starts_with('C') {
                    let old = *tokens
                        .get(position)
                        .ok_or_else(|| anyhow::anyhow!("missing rename source"))?;
                    let new = *tokens
                        .get(position + 1)
                        .ok_or_else(|| anyhow::anyhow!("missing rename destination"))?;
                    commit.changes.push(ChangedPath::rename(old, new));
                    position += 2;
                } else {
                    anyhow::ensure!(
                        matches!(
                            status.chars().next(),
                            Some('A' | 'M' | 'D' | 'T' | 'U' | 'X' | 'B')
                        ),
                        "invalid batched path status"
                    );
                    let path = *tokens
                        .get(position)
                        .ok_or_else(|| anyhow::anyhow!("missing changed path"))?;
                    commit.changes.push(if status.starts_with('D') {
                        ChangedPath::delete(path)
                    } else {
                        ChangedPath::path(path)
                    });
                    position += 1;
                }
            }
        }
        anyhow::ensure!(
            tokens[position..].iter().all(|token| token.is_empty()),
            "extra batched diff output"
        );
        Ok(commits)
    }

    pub fn changed_paths(&self, sha: &str) -> anyhow::Result<Vec<ChangedPath>> {
        let output = self.git(&[
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-status",
            "-r",
            "-M",
            "-z",
            sha,
        ])?;
        let mut fields = output.split('\0').filter(|field| !field.is_empty());
        let mut changes = Vec::new();
        while let Some(status) = fields.next() {
            if status.starts_with('R') || status.starts_with('C') {
                let old_path = fields
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing old rename path"))?;
                let new_path = fields
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing new rename path"))?;
                changes.push(ChangedPath::rename(old_path, new_path));
            } else {
                let path = fields
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing changed path"))?;
                changes.push(if status.starts_with('D') {
                    ChangedPath::delete(path)
                } else {
                    ChangedPath::path(path)
                });
            }
        }
        Ok(changes)
    }

    fn git(&self, args: &[&str]) -> anyhow::Result<String> {
        let output = run_git_command(Command::new("git").arg("-C").arg(&self.root).args(args))?;
        anyhow::ensure!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
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
