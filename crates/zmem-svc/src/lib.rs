use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zmem_core::{
    Action, Anchor, AttentionCandidate, AttentionLimit, AttentionPolicy, AttentionUsage, GitCommit,
    GitRepo, HostInspection, HostResponse, SCHEMA_VERSION, attention_identity_allows_incremental,
    run_ordered, select_attention, validate_action_journal, validate_host_inspection,
    validate_host_inspection_batch,
};
use zmem_store::{
    CommitUpdate, EffectOutcome, InspectionRecord, RetentionPolicy, Store, select_evictions,
};

pub const RELEASE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ServiceIdentity {
    pub release_version: &'static str,
    pub protocol_version: u32,
    pub schema_version: u32,
}

impl ServiceIdentity {
    pub fn current() -> Self {
        Self {
            release_version: RELEASE_VERSION,
            protocol_version: zmem_core::PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostCommand {
    pub executable: PathBuf,
    pub args: Vec<String>,
}

pub fn installed_extension_host(executable: &Path) -> Option<HostCommand> {
    let binary = executable.parent()?;
    if binary.file_name()?.to_str()? != "binary" {
        return None;
    }
    let host = binary.parent()?.join("host");
    let python = if cfg!(windows) {
        host.join("Scripts").join("python.exe")
    } else {
        host.join("bin").join("python")
    };
    python.is_file().then(|| HostCommand {
        executable: python,
        args: vec!["-m".to_owned(), "zmem.host".to_owned()],
    })
}

#[derive(Debug, Deserialize, Serialize)]
struct StartupRecord {
    owner: String,
    created_at: u64,
}

#[derive(Debug)]
pub struct StartupLock {
    path: PathBuf,
    owner: String,
}

impl StartupLock {
    pub fn acquire(
        home: &Path,
        wait_timeout: std::time::Duration,
        stale_after: std::time::Duration,
    ) -> anyhow::Result<Self> {
        std::fs::create_dir_all(home)?;
        let path = home.join("service-start.lock");
        let started = std::time::Instant::now();
        loop {
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            let owner = format!("{}-{now}", std::process::id());
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    serde_json::to_writer(
                        &mut file,
                        &StartupRecord {
                            owner: owner.clone(),
                            created_at: now,
                        },
                    )?;
                    file.flush()?;
                    return Ok(Self { path, owner });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let record = std::fs::read(&path)
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<StartupRecord>(&bytes).ok());
                    let stale = record.map_or_else(
                        || {
                            std::fs::metadata(&path)
                                .and_then(|metadata| metadata.modified())
                                .ok()
                                .and_then(|modified| {
                                    SystemTime::now().duration_since(modified).ok()
                                })
                                .is_some_and(|age| age >= stale_after)
                        },
                        |record| now.saturating_sub(record.created_at) >= stale_after.as_secs(),
                    );
                    if stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() >= wait_timeout {
                        anyhow::bail!("timed out waiting for zmem service startup lock");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for StartupLock {
    fn drop(&mut self) {
        let owned = std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<StartupRecord>(&bytes).ok())
            .is_some_and(|record| record.owner == self.owner);
        if owned {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub max_concurrency: NonZeroUsize,
    pub extension_host_timeout_seconds: NonZeroU64,
    pub max_entries: NonZeroU64,
    pub protect_recent_days: u32,
    pub extension_host: Option<String>,
    pub extension_host_args: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_concurrency: NonZeroUsize::new(8).unwrap(),
            extension_host_timeout_seconds: NonZeroU64::new(30).unwrap(),
            max_entries: NonZeroU64::new(3_000_000).unwrap(),
            protect_recent_days: 14,
            extension_host: None,
            extension_host_args: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostOperation {
    Identity,
    Inspection,
    Expansion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HostExecutionPolicy {
    attempts: usize,
    deadline: Duration,
}

impl HostOperation {
    fn execution_policy(self, config: &Config) -> HostExecutionPolicy {
        HostExecutionPolicy {
            attempts: match self {
                Self::Identity | Self::Inspection => 2,
                Self::Expansion => 1,
            },
            deadline: Duration::from_secs(config.extension_host_timeout_seconds.get()),
        }
    }
}

fn join_output(
    reader: std::thread::JoinHandle<std::io::Result<Vec<u8>>>,
    stream: &str,
) -> anyhow::Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("extension host {stream} drainer panicked"))?
        .with_context(|| format!("could not read extension host {stream}"))
}

fn execute_supervised(
    command: &mut Command,
    input: &[u8],
    deadline: Duration,
) -> anyhow::Result<Output> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .context("extension host stdout unavailable")?;
    let mut stderr = child
        .stderr
        .take()
        .context("extension host stderr unavailable")?;
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes)?;
        Ok(bytes)
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes)?;
        Ok(bytes)
    });

    let write_result = child
        .stdin
        .take()
        .context("extension host stdin unavailable")?
        .write_all(input);
    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        let _ = join_output(stdout_reader, "stdout");
        let _ = join_output(stderr_reader, "stderr");
        return Err(error).context("could not write extension host input");
    }

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let wait_result = child.wait();
                let _ = join_output(stdout_reader, "stdout");
                let _ = join_output(stderr_reader, "stderr");
                wait_result.context("could not reap extension host after wait failure")?;
                return Err(error).context("could not wait for extension host");
            }
        }
        if started.elapsed() >= deadline {
            let _ = child.kill();
            let wait_result = child.wait();
            let _ = join_output(stdout_reader, "stdout");
            let _ = join_output(stderr_reader, "stderr");
            wait_result.context("could not reap timed-out extension host")?;
            anyhow::bail!(
                "extension host timed out after {} seconds",
                deadline.as_secs()
            );
        }
        std::thread::sleep(
            Duration::from_millis(10).min(deadline.saturating_sub(started.elapsed())),
        );
    };
    Ok(Output {
        status,
        stdout: join_output(stdout_reader, "stdout")?,
        stderr: join_output(stderr_reader, "stderr")?,
    })
}

fn execute_host_output_supervised(
    config: &Config,
    operation: HostOperation,
    request: &serde_json::Value,
) -> anyhow::Result<Vec<u8>> {
    let host = extension_host_command(config);
    let policy = operation.execution_policy(config);
    let input = serde_json::to_vec(request)?;
    let mut last_error = None;
    for _ in 0..policy.attempts {
        let mut command = Command::new(&host.executable);
        command.args(&host.args);
        let result = execute_supervised(&mut command, &input, policy.deadline).with_context(|| {
            format!(
                "could not run extension host: {}",
                host.executable.display()
            )
        });
        match result {
            Ok(output) if output.status.success() => return Ok(output.stdout),
            Ok(output) => {
                last_error = Some(anyhow::anyhow!(
                    "extension host failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.expect("host execution policy has at least one attempt"))
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        toml::from_str(&std::fs::read_to_string(path)?).context("invalid zmem config")
    }
}

pub fn zmem_home() -> anyhow::Result<PathBuf> {
    if let Some(value) = std::env::var_os("ZMEM_HOME") {
        return Ok(PathBuf::from(value));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .context("home directory unavailable")?;
    Ok(PathBuf::from(home).join(".zmem"))
}

fn extension_host_command(config: &Config) -> HostCommand {
    if let Some(executable) = std::env::var_os("ZMEM_EXTENSION_HOST") {
        return HostCommand {
            executable: PathBuf::from(executable),
            args: Vec::new(),
        };
    }
    if let Some(executable) = &config.extension_host {
        return HostCommand {
            executable: PathBuf::from(executable),
            args: config.extension_host_args.clone(),
        };
    }
    std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(installed_extension_host)
        .unwrap_or_else(|| HostCommand {
            executable: PathBuf::from("zmem-extension-host"),
            args: Vec::new(),
        })
}

fn execute_host(
    config: &Config,
    operation: HostOperation,
    request: &serde_json::Value,
) -> anyhow::Result<HostResponse> {
    validate_action_journal(&execute_host_output_supervised(config, operation, request)?)
}

fn inspect_host(config: &Config, commit: &GitCommit) -> anyhow::Result<HostInspection> {
    validate_host_inspection(&execute_host_output_supervised(
        config,
        HostOperation::Inspection,
        &serde_json::json!({
            "protocol_version": zmem_core::PROTOCOL_VERSION,
            "operation": "inspect",
            "message": commit.message,
        }),
    )?)
}

fn invoke_identity(
    config: &Config,
    home: &Path,
    repo: &GitRepo,
    trusted: bool,
) -> anyhow::Result<HostResponse> {
    execute_host(
        config,
        HostOperation::Identity,
        &serde_json::json!({
            "protocol_version":zmem_core::PROTOCOL_VERSION,"operation":"identity","repo":repo.root(),"trusted_extensions":trusted,"global_extension_root":home.join("ext")
        }),
    )
}

fn invoke_host(
    config: &Config,
    home: &Path,
    repo: &GitRepo,
    commit: &zmem_core::GitCommit,
    trusted: bool,
    run_hooks: bool,
    preview: bool,
) -> anyhow::Result<HostResponse> {
    execute_host(
        config,
        HostOperation::Expansion,
        &serde_json::json!({
            "protocol_version": zmem_core::PROTOCOL_VERSION, "operation": "expand", "repo": repo.root(), "commit_sha": commit.sha,
            "message": commit.message, "commit_time": commit.commit_time, "trusted_extensions": trusted,
            "global_extension_root": home.join("ext"), "run_hooks": run_hooks, "preview": preview
        }),
    )
}

const VIRTUAL_OID: &str = "0000000000000000000000000000000000000000";

#[derive(Debug, Serialize)]
pub struct CheckResult {
    pub protocol_version: u32,
    pub ok: bool,
    pub mode: &'static str,
    pub repository: String,
    pub parent: String,
    pub target: Option<String>,
    pub extension_hash: String,
    pub annotation_count: usize,
    pub actions: Vec<Action>,
    pub effects: Vec<EffectOutcome>,
    pub diagnostics: Vec<String>,
    pub hooks: &'static str,
    pub attention: AttentionUsage,
}

struct TemporaryStore {
    root: PathBuf,
}

struct CheckContext<'a> {
    config: &'a Config,
    home: &'a Path,
    repo: &'a GitRepo,
    trusted: bool,
    identity: &'a str,
}

struct PreviewRequest {
    mode: &'static str,
    parent: String,
    target: Option<String>,
    attention: AttentionUsage,
}

impl TemporaryStore {
    fn new() -> anyhow::Result<Self> {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root =
            std::env::temp_dir().join(format!("zmem-deep-check-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn database(&self) -> PathBuf {
        self.root.join("entries.db")
    }
}

impl Drop for TemporaryStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn expand_commits(
    config: &Config,
    home: &Path,
    repo: &GitRepo,
    commits: Vec<GitCommit>,
    trusted: bool,
    run_hooks: bool,
    preview: bool,
) -> Vec<(GitCommit, anyhow::Result<HostResponse>)> {
    run_ordered(commits, config.max_concurrency.get(), |commit| {
        let response = invoke_host(config, home, repo, &commit, trusted, run_hooks, preview);
        (commit, response)
    })
}

fn resolve_attention_policy(
    commit_limit: Option<i64>,
    node_limit: Option<i64>,
) -> anyhow::Result<AttentionPolicy> {
    let environment_commit = std::env::var("ZMEM_COMMIT_LIMIT").ok();
    let environment_node = std::env::var("ZMEM_NODE_LIMIT").ok();
    AttentionPolicy::resolve(
        commit_limit,
        node_limit,
        environment_commit.as_deref(),
        environment_node.as_deref(),
    )
}

#[derive(Debug)]
struct SelectedHistory {
    commits: Vec<GitCommit>,
    usage: AttentionUsage,
}

const INSPECTION_BATCH_SIZE: usize = 64;

fn inspect_commits(
    config: &Config,
    store: &mut Store,
    commits: &[GitCommit],
) -> anyhow::Result<Vec<HostInspection>> {
    let mut results = std::collections::HashMap::new();
    let mut misses = Vec::new();
    for commit in commits {
        if let Some(record) = store.inspection(&commit.sha, zmem_core::PROTOCOL_VERSION)? {
            results.insert(
                commit.sha.clone(),
                HostInspection {
                    protocol_version: zmem_core::PROTOCOL_VERSION,
                    annotation_count: record.annotation_count,
                    parser_diagnostics: record.parser_diagnostics,
                },
            );
        } else {
            misses.push(commit.clone());
        }
    }
    let batches = misses
        .chunks(INSPECTION_BATCH_SIZE)
        .map(<[GitCommit]>::to_vec)
        .collect::<Vec<_>>();
    let inspected = run_ordered(batches, config.max_concurrency.get(), |batch| {
        let expected_ids = batch
            .iter()
            .map(|commit| commit.sha.clone())
            .collect::<Vec<_>>();
        let request = serde_json::json!({
            "protocol_version": zmem_core::PROTOCOL_VERSION,
            "operation": "inspect_batch",
            "items": batch.iter().map(|commit| serde_json::json!({"id": commit.sha, "message": commit.message})).collect::<Vec<_>>(),
        });
        let response = execute_host_output_supervised(config, HostOperation::Inspection, &request)
            .and_then(|bytes| validate_host_inspection_batch(&bytes, &expected_ids));
        (expected_ids, response)
    });
    let mut records = Vec::with_capacity(misses.len());
    for (identities, response) in inspected {
        for (oid, inspection) in identities.into_iter().zip(response?) {
            records.push(InspectionRecord {
                oid: oid.clone(),
                annotation_count: inspection.annotation_count,
                parser_diagnostics: inspection.parser_diagnostics.clone(),
            });
            results.insert(oid, inspection);
        }
    }
    store.record_inspections(zmem_core::PROTOCOL_VERSION, &records)?;
    commits
        .iter()
        .map(|commit| {
            results
                .remove(&commit.sha)
                .with_context(|| format!("missing inspection for commit {}", commit.sha))
        })
        .collect()
}

fn select_history(
    config: &Config,
    store: &mut Store,
    repo: &GitRepo,
    head: &str,
    policy: AttentionPolicy,
    reserved_nodes: usize,
) -> anyhow::Result<SelectedHistory> {
    let walk = repo.walk_newest(head, policy.commit_limit)?;
    let commits = walk
        .shas
        .iter()
        .map(|sha| repo.commit(sha))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let inspections = inspect_commits(config, store, &commits)?;
    let mut candidates = Vec::with_capacity(commits.len());
    for (commit, inspection) in commits.into_iter().zip(inspections) {
        candidates.push(AttentionCandidate::new(commit, inspection.annotation_count));
    }
    let selection = select_attention(candidates, policy, reserved_nodes, walk.truncated)?;
    Ok(SelectedHistory {
        commits: selection.selected,
        usage: selection.usage,
    })
}

fn replay_commits(
    store: &mut Store,
    repo_id: i64,
    context: &CheckContext<'_>,
    shas: &[String],
) -> anyhow::Result<()> {
    let commits = shas
        .iter()
        .map(|sha| context.repo.commit(sha))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let expanded = expand_commits(
        context.config,
        context.home,
        context.repo,
        commits,
        context.trusted,
        false,
        true,
    );
    let mut completed = Vec::with_capacity(expanded.len());
    for (commit, response) in expanded {
        let response = response?;
        anyhow::ensure!(
            response.extension_hash == context.identity,
            "extension identity changed during deep checking"
        );
        let anchor = Anchor {
            head: commit.sha.clone(),
            schema: SCHEMA_VERSION,
            extension_hash: response.extension_hash.clone(),
            attention_identity: "legacy".to_owned(),
        };
        completed.push((commit, response, anchor));
    }
    let updates = completed
        .iter()
        .map(|(commit, response, anchor)| CommitUpdate {
            oid: &commit.sha,
            commit_time: commit.commit_time,
            message: &commit.message,
            response,
            anchor,
        })
        .collect::<Vec<_>>();
    store.apply_range(repo_id, &updates, false)
}

fn preview_commit(
    store: &mut Store,
    repo_id: i64,
    context: &CheckContext<'_>,
    commit: &GitCommit,
    request: PreviewRequest,
) -> anyhow::Result<CheckResult> {
    let response = invoke_host(
        context.config,
        context.home,
        context.repo,
        commit,
        context.trusted,
        false,
        true,
    )?;
    anyhow::ensure!(
        response.extension_hash == context.identity,
        "extension identity changed during checking"
    );
    let virtual_anchor = Anchor {
        head: commit.sha.clone(),
        schema: SCHEMA_VERSION,
        extension_hash: response.extension_hash.clone(),
        attention_identity: "legacy".to_owned(),
    };
    let preview = store.preview(
        repo_id,
        &CommitUpdate {
            oid: &commit.sha,
            commit_time: commit.commit_time,
            message: &commit.message,
            response: &response,
            anchor: &virtual_anchor,
        },
    )?;
    let mut diagnostics = preview.diagnostics;
    if request.attention.truncated
        && preview.effects.iter().any(|effect| {
            effect.status == zmem_store::EffectStatus::Rejected
                && effect.diagnostic.as_deref() == Some("unresolved or ambiguous effect target")
        })
    {
        diagnostics.push(
            "attention threshold reached; effect target may be outside selected history".to_owned(),
        );
    }
    Ok(CheckResult {
        protocol_version: zmem_core::PROTOCOL_VERSION,
        ok: diagnostics.is_empty(),
        mode: request.mode,
        repository: context.repo.root().to_string_lossy().into_owned(),
        parent: request.parent,
        target: request.target,
        extension_hash: response.extension_hash,
        annotation_count: response.annotation_count,
        actions: response.journal.actions,
        effects: preview.effects,
        diagnostics,
        hooks: "skipped",
        attention: request.attention,
    })
}

pub fn check_repository(
    path: &Path,
    message: Option<&str>,
    reference: Option<&str>,
    deep: bool,
) -> anyhow::Result<CheckResult> {
    check_repository_with_attention(path, message, reference, deep, None, None)
}

pub fn check_repository_with_attention(
    path: &Path,
    message: Option<&str>,
    reference: Option<&str>,
    deep: bool,
    commit_limit: Option<i64>,
    node_limit: Option<i64>,
) -> anyhow::Result<CheckResult> {
    anyhow::ensure!(
        message.is_some() ^ reference.is_some(),
        "exactly one proposed message or commit reference is required"
    );
    anyhow::ensure!(
        deep || reference.is_none(),
        "existing commits require --deep"
    );
    let home = zmem_home()?;
    let config = Config::load(&home.join("config.toml"))?;
    let repo = GitRepo::open(path)?;
    let canonical = repo.root().to_string_lossy().into_owned();
    let policy = resolve_attention_policy(commit_limit, node_limit)?;

    let proposed_commit = message.map(|message| GitCommit {
        sha: VIRTUAL_OID.to_owned(),
        commit_time: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default(),
        message: message.to_owned(),
    });
    let proposed_nodes = proposed_commit
        .as_ref()
        .map(|commit| inspect_host(&config, commit).map(|inspection| inspection.annotation_count))
        .transpose()?
        .unwrap_or_default();
    if let Some(maximum) = policy.node_limit.maximum() {
        anyhow::ensure!(
            proposed_nodes <= maximum,
            "proposed message exceeds node attention limit"
        );
    }

    if !deep {
        let history_node_limit = match policy.node_limit.maximum() {
            Some(maximum) if maximum == proposed_nodes => AttentionLimit::zero(),
            Some(maximum) => AttentionLimit::parse(
                i64::try_from(maximum.saturating_sub(proposed_nodes))?,
                "node",
            )?,
            None => AttentionLimit::Unlimited,
        };
        let history_policy = AttentionPolicy {
            commit_limit: policy.commit_limit,
            node_limit: history_node_limit,
        };
        let sync = sync_repository_with_policy(path, None, history_policy)?;
        let mut attention = sync.summary.attention;
        attention.node_limit = policy.node_limit.as_i64();
        attention.selected_nodes = attention.selected_nodes.saturating_add(proposed_nodes);
        let mut store = Store::open(&home.join("db").join("entries.db"))?;
        let (repo_id, trusted) = store
            .repository(&canonical)?
            .context("synchronized repository registration is missing")?;
        let parent = repo.head()?;
        let identity = invoke_identity(&config, &home, &repo, trusted)?.extension_hash;
        let context = CheckContext {
            config: &config,
            home: &home,
            repo: &repo,
            trusted,
            identity: &identity,
        };
        return preview_commit(
            &mut store,
            repo_id,
            &context,
            proposed_commit
                .as_ref()
                .context("proposed message is required")?,
            PreviewRequest {
                mode: "fast",
                parent,
                target: None,
                attention,
            },
        );
    }

    let database = home.join("db").join("entries.db");
    let trusted = if database.exists() {
        Store::open(&database)?
            .repository(&canonical)?
            .map(|(_, trusted)| trusted)
            .unwrap_or(false)
    } else {
        false
    };
    let identity = invoke_identity(&config, &home, &repo, trusted)?.extension_hash;
    let context = CheckContext {
        config: &config,
        home: &home,
        repo: &repo,
        trusted,
        identity: &identity,
    };
    let temporary = TemporaryStore::new()?;
    let mut store = Store::open(&temporary.database())?;
    let repo_id = store.register_repository(&canonical, trusted)?;
    let (history_head, target_commit, target, reserved_nodes) = if let Some(reference) = reference {
        let resolved = repo.resolve(reference)?;
        let commit = repo.commit(&resolved)?;
        (resolved.clone(), commit, Some(resolved), 0)
    } else {
        let head = repo.head()?;
        (
            head,
            proposed_commit.context("proposed message is required")?,
            None,
            proposed_nodes,
        )
    };
    let selected = select_history(
        &config,
        &mut store,
        &repo,
        &history_head,
        policy,
        reserved_nodes,
    )?;
    let mut history = selected
        .commits
        .iter()
        .map(|commit| commit.sha.clone())
        .collect::<Vec<_>>();
    if target.is_some() {
        anyhow::ensure!(
            history.iter().any(|sha| sha == &target_commit.sha),
            "target commit exceeds attention limits"
        );
        history.retain(|sha| sha != &target_commit.sha);
    }
    replay_commits(&mut store, repo_id, &context, &history)?;
    let parent = history.last().cloned().unwrap_or_default();
    preview_commit(
        &mut store,
        repo_id,
        &context,
        &target_commit,
        PreviewRequest {
            mode: "deep",
            parent,
            target,
            attention: selected.usage,
        },
    )
}

#[derive(Debug, serde::Serialize)]
pub struct SyncSummary {
    pub repository: String,
    pub head: String,
    pub indexed_commits: usize,
    pub entries: usize,
    pub over_capacity: bool,
    pub max_concurrency: usize,
    pub attention: AttentionUsage,
}

#[derive(Debug)]
pub struct SyncResult {
    pub summary: SyncSummary,
    pub entries: Vec<serde_json::Value>,
    pub relationships: Vec<serde_json::Value>,
    pub diagnostics: Vec<serde_json::Value>,
}

pub fn sync_repository(path: &Path, trust: Option<bool>) -> anyhow::Result<SyncResult> {
    sync_repository_with_attention(path, trust, None, None)
}

pub fn sync_repository_with_attention(
    path: &Path,
    trust: Option<bool>,
    commit_limit: Option<i64>,
    node_limit: Option<i64>,
) -> anyhow::Result<SyncResult> {
    let policy = resolve_attention_policy(commit_limit, node_limit)?;
    sync_repository_with_policy(path, trust, policy)
}

fn sync_repository_with_policy(
    path: &Path,
    trust: Option<bool>,
    policy: AttentionPolicy,
) -> anyhow::Result<SyncResult> {
    let home = zmem_home()?;
    let config = Config::load(&home.join("config.toml"))?;
    let repo = GitRepo::open(path)?;
    let canonical = repo.root().to_string_lossy().into_owned();
    let head = repo.head()?;
    let mut store = Store::open(&home.join("db").join("entries.db"))?;
    let (repo_id, trusted) = match store.repository(&canonical)? {
        Some((id, current)) => (id, trust.unwrap_or(current)),
        None => {
            let selected = trust.unwrap_or(false);
            (store.register_repository(&canonical, selected)?, selected)
        }
    };
    if trust.is_some() {
        store.register_repository(&canonical, trusted)?;
    }

    let identity = invoke_identity(&config, &home, &repo, trusted)?.extension_hash;
    let selected = select_history(&config, &mut store, &repo, &head, policy, 0)?;
    let lower_boundary = selected.commits.first().map(|commit| commit.sha.as_str());
    let final_anchor = Anchor {
        head: head.clone(),
        schema: SCHEMA_VERSION,
        extension_hash: identity.clone(),
        attention_identity: selected.usage.view_identity(lower_boundary),
    };
    let anchor = store.anchor(repo_id)?;
    let current = anchor.as_ref().is_some_and(|anchor| {
        anchor.head == head
            && anchor.schema == SCHEMA_VERSION
            && anchor.extension_hash == identity
            && anchor.attention_identity == final_anchor.attention_identity
    });
    let incremental = if current {
        false
    } else if let Some(anchor) = &anchor {
        anchor.schema == SCHEMA_VERSION
            && anchor.extension_hash == identity
            && !selected.usage.truncated
            && attention_identity_allows_incremental(&anchor.attention_identity, policy)
            && repo.is_ancestor(&anchor.head, &head)?
            && selected
                .commits
                .iter()
                .any(|commit| commit.sha == anchor.head)
    } else {
        false
    };
    let commits = if current {
        Vec::new()
    } else if incremental {
        let previous = &anchor.as_ref().expect("incremental anchor exists").head;
        selected
            .commits
            .iter()
            .skip_while(|commit| commit.sha != *previous)
            .skip(1)
            .cloned()
            .collect()
    } else {
        selected.commits.clone()
    };
    let expanded = expand_commits(&config, &home, &repo, commits, trusted, true, false);
    let mut completed = Vec::with_capacity(expanded.len());
    for (commit, response) in expanded {
        let response = response?;
        anyhow::ensure!(
            response.extension_hash == identity,
            "extension identity changed during indexing"
        );
        completed.push((commit, response));
    }
    let updates = completed
        .iter()
        .map(|(commit, response)| CommitUpdate {
            oid: &commit.sha,
            commit_time: commit.commit_time,
            message: &commit.message,
            response,
            anchor: &final_anchor,
        })
        .collect::<Vec<_>>();
    if !current {
        if incremental {
            store.apply_range(repo_id, &updates, false)?;
        } else {
            store.replace_projection(repo_id, &updates, &final_anchor)?;
        }
    }
    let indexed = updates.len();
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let plan = select_evictions(
        &store.cohorts()?,
        now,
        RetentionPolicy {
            max_entries: config.max_entries.get(),
            protect_recent_seconds: i64::from(config.protect_recent_days) * 86_400,
        },
    );
    let over_capacity = plan.over_capacity;
    store.evict(&plan)?;
    let entries = store.query_entries(repo_id, true)?;
    let relationships = store.query_relationships(repo_id)?;
    let diagnostics = store.query_diagnostics(repo_id)?;
    Ok(SyncResult {
        summary: SyncSummary {
            repository: canonical,
            head,
            indexed_commits: indexed,
            entries: entries.len(),
            over_capacity,
            max_concurrency: config.max_concurrency.get(),
            attention: selected.usage,
        },
        entries,
        relationships,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(std::path::PathBuf);

    impl TestDir {
        fn new() -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("zmem-svc-test-{}-{unique}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn defaults_match_product_contract() {
        let config = Config::default();
        assert_eq!(config.max_concurrency.get(), 8);
        assert_eq!(config.extension_host_timeout_seconds.get(), 30);
        assert_eq!(config.max_entries.get(), 3_000_000);
        assert_eq!(config.protect_recent_days, 14);
    }

    #[test]
    fn zero_limits_are_rejected() {
        let parsed = toml::from_str::<Config>("max_concurrency=0\nmax_entries=1");
        assert!(parsed.is_err());
        let parsed = toml::from_str::<Config>(
            "max_concurrency=1\nmax_entries=1\nextension_host_timeout_seconds=0",
        );
        assert!(parsed.is_err());
    }

    #[test]
    fn host_operation_policy_limits_attempts_and_deadline() {
        let config = Config::default();
        assert_eq!(
            HostOperation::Identity.execution_policy(&config).attempts,
            2
        );
        assert_eq!(
            HostOperation::Inspection.execution_policy(&config).attempts,
            2
        );
        assert_eq!(
            HostOperation::Expansion.execution_policy(&config).attempts,
            1
        );
        assert_eq!(
            HostOperation::Inspection.execution_policy(&config).deadline,
            std::time::Duration::from_secs(30)
        );
    }

    #[test]
    fn supervised_host_timeout_kills_and_reaps_child() {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("powershell");
            command.args([
                "-NoProfile",
                "-Command",
                "[Console]::Out.Write('started'); Start-Sleep -Seconds 5",
            ]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "printf started; sleep 5"]);
            command
        };
        let started = Instant::now();
        let error =
            execute_supervised(&mut command, b"request", Duration::from_millis(50)).unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn installed_layout_resolves_sibling_python_host() {
        let temporary = TestDir::new();
        let executable = temporary
            .path()
            .join("runtime")
            .join("binary")
            .join(if cfg!(windows) {
                "zmem-svc.exe"
            } else {
                "zmem-svc"
            });
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        let host = temporary.path().join("runtime").join("host");
        let python = if cfg!(windows) {
            host.join("Scripts").join("python.exe")
        } else {
            host.join("bin").join("python")
        };
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::fs::write(&python, b"python").unwrap();

        let command = installed_extension_host(&executable).unwrap();
        assert_eq!(command.executable, python);
        assert_eq!(command.args, ["-m", "zmem.host"]);
    }

    #[test]
    fn startup_lock_is_exclusive_and_recovers_stale_record() {
        let temporary = TestDir::new();
        let first = StartupLock::acquire(
            temporary.path(),
            std::time::Duration::from_millis(20),
            std::time::Duration::from_secs(60),
        )
        .unwrap();
        assert!(
            StartupLock::acquire(
                temporary.path(),
                std::time::Duration::from_millis(20),
                std::time::Duration::from_secs(60),
            )
            .is_err()
        );
        drop(first);

        std::fs::write(temporary.path().join("service-start.lock"), b"{").unwrap();
        assert!(
            StartupLock::acquire(
                temporary.path(),
                std::time::Duration::from_millis(20),
                std::time::Duration::from_secs(60),
            )
            .is_err()
        );

        std::fs::write(
            temporary.path().join("service-start.lock"),
            r#"{"owner":"dead","created_at":0}"#,
        )
        .unwrap();
        let recovered = StartupLock::acquire(
            temporary.path(),
            std::time::Duration::from_millis(20),
            std::time::Duration::from_millis(1),
        )
        .unwrap();
        drop(recovered);
        assert!(!temporary.path().join("service-start.lock").exists());
    }
}
