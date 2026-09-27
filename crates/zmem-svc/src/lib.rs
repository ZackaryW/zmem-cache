use anyhow::Context;
pub mod demand;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::fs::OpenOptions;
use std::io::Write;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zmem_core::{
    Action, Anchor, AttentionCandidate, AttentionLimit, AttentionPolicy, AttentionUsage, GitCommit,
    GitRepo, HostInspection, HostResponse, SCHEMA_VERSION, TrailIdentity, derive_affected_areas,
    run_ordered, select_attention, validate_action_journal, validate_host_inspection,
    validate_host_inspection_batch,
};
use zmem_store::{
    CommitUpdate, EffectOutcome, InspectionRecord, RetentionPolicy, Store, TrailPublication,
    TrailRecord, select_evictions, select_trail_evictions,
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

/// An OS-backed lifetime lock. The file may remain after a crash; its lock does not.
pub struct ServiceOwner {
    file: std::fs::File,
}

impl ServiceOwner {
    pub fn acquire(home: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(home)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(home.join("service-owner.lock"))?;
        file.try_lock()?;
        Ok(Self { file })
    }

    pub fn is_held(home: &Path) -> anyhow::Result<bool> {
        std::fs::create_dir_all(home)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(home.join("service-owner.lock"))?;
        match file.try_lock() {
            Ok(()) => {
                file.unlock()?;
                Ok(false)
            }
            Err(std::fs::TryLockError::WouldBlock) => Ok(true),
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for ServiceOwner {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
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
    pub lookup_capacity: NonZeroUsize,
    pub lookup_queue_capacity: NonZeroUsize,
    pub heavy_capacity: NonZeroUsize,
    pub connection_capacity: NonZeroUsize,
    pub extension_host_timeout_seconds: NonZeroU64,
    pub max_entries: NonZeroU64,
    pub protect_recent_days: u32,
    pub extension_host: Option<String>,
    pub extension_host_args: Vec<String>,
    pub background_commit_limit: u32,
    pub staging_max_bytes: NonZeroU64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_concurrency: NonZeroUsize::new(8).unwrap(),
            lookup_capacity: NonZeroUsize::new(2).unwrap(),
            lookup_queue_capacity: NonZeroUsize::new(64).unwrap(),
            heavy_capacity: NonZeroUsize::new(32).unwrap(),
            connection_capacity: NonZeroUsize::new(128).unwrap(),
            extension_host_timeout_seconds: NonZeroU64::new(30).unwrap(),
            max_entries: NonZeroU64::new(3_000_000).unwrap(),
            protect_recent_days: 14,
            extension_host: None,
            extension_host_args: Vec::new(),
            background_commit_limit: 10_000,
            staging_max_bytes: NonZeroU64::new(256 * 1024 * 1024).unwrap(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostOperation {
    Identity,
    LookupIdentity,
    Inspection,
    Expansion,
}

struct HostPermit;

thread_local! {
    static REQUEST_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
    static REQUEST_CANCELLATION: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

pub fn with_request_cancellation<T>(cancelled: Arc<AtomicBool>, action: impl FnOnce() -> T) -> T {
    with_cancellation(Some(cancelled), action)
}

fn with_cancellation<T>(cancelled: Option<Arc<AtomicBool>>, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<Arc<AtomicBool>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            REQUEST_CANCELLATION.replace(self.0.take());
        }
    }
    let previous = REQUEST_CANCELLATION.replace(cancelled);
    let _restore = Restore(previous);
    action()
}

fn request_cancellation() -> Option<Arc<AtomicBool>> {
    REQUEST_CANCELLATION.with(|token| token.borrow().clone())
}

fn request_cancelled() -> bool {
    request_cancellation().is_some_and(|token| token.load(Ordering::Acquire))
}

pub fn with_request_deadline<T>(timeout_ms: u64, action: impl FnOnce() -> T) -> T {
    with_absolute_deadline(
        Some(Instant::now() + Duration::from_millis(timeout_ms)),
        action,
    )
}

pub fn with_request_deadline_at<T>(deadline: Instant, action: impl FnOnce() -> T) -> T {
    with_absolute_deadline(Some(deadline), action)
}

fn with_absolute_deadline<T>(deadline: Option<Instant>, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<Instant>);
    impl Drop for Restore {
        fn drop(&mut self) {
            REQUEST_DEADLINE.set(self.0);
        }
    }
    let previous = REQUEST_DEADLINE.replace(deadline);
    let _restore = Restore(previous);
    zmem_core::with_git_cancellation(request_cancellation(), || {
        zmem_core::with_git_deadline(deadline, action)
    })
}

fn request_remaining() -> anyhow::Result<Option<Duration>> {
    anyhow::ensure!(!request_cancelled(), "service request cancelled");
    REQUEST_DEADLINE.with(|deadline| {
        deadline
            .get()
            .map(|deadline| {
                deadline
                    .checked_duration_since(Instant::now())
                    .filter(|remaining| !remaining.is_zero())
                    .ok_or_else(|| anyhow::anyhow!("service request deadline expired"))
            })
            .transpose()
    })
}

fn check_request_deadline() -> anyhow::Result<()> {
    request_remaining().map(|_| ())
}

fn apply_store_deadline(store: &Store) -> anyhow::Result<()> {
    if let Some(deadline) = REQUEST_DEADLINE.with(Cell::get) {
        store.set_request_deadline_and_cancellation(deadline, request_cancellation())?;
    }
    Ok(())
}

fn open_canonical_store(path: &Path) -> anyhow::Result<Store> {
    if path.exists() {
        Store::open_writable_existing(path)
    } else {
        Store::open(path)
    }
}

struct OwnedPublication {
    trail: TrailRecord,
    commits: Vec<GitCommit>,
    parents: BTreeMap<String, Vec<String>>,
    entries: Vec<serde_json::Value>,
    relationships: Vec<serde_json::Value>,
    diagnostics: Vec<serde_json::Value>,
    expansions: BTreeMap<String, HostResponse>,
}

impl OwnedPublication {
    fn from_borrowed(publication: &TrailPublication<'_>) -> Self {
        Self {
            trail: publication.trail.clone(),
            commits: publication.commits.to_vec(),
            parents: publication.parents.clone(),
            entries: publication.entries.to_vec(),
            relationships: publication.relationships.to_vec(),
            diagnostics: publication.diagnostics.to_vec(),
            expansions: publication.expansions.clone(),
        }
    }

    fn borrowed(&self) -> TrailPublication<'_> {
        TrailPublication {
            trail: &self.trail,
            commits: &self.commits,
            parents: &self.parents,
            entries: &self.entries,
            relationships: &self.relationships,
            diagnostics: &self.diagnostics,
            expansions: &self.expansions,
        }
    }
}

enum PublicationCommand {
    Publish {
        publication: Box<OwnedPublication>,
        deadline: Option<Instant>,
        cancelled: Option<Arc<AtomicBool>>,
        response: mpsc::Sender<anyhow::Result<()>>,
    },
    Finalize {
        request: Finalization,
        deadline: Option<Instant>,
        cancelled: Option<Arc<AtomicBool>>,
        response: mpsc::Sender<anyhow::Result<bool>>,
    },
    PrefetchBatch {
        batch: Box<PrefetchWrite>,
        deadline: Instant,
        cancelled: Option<Arc<AtomicBool>>,
        response: mpsc::Sender<anyhow::Result<bool>>,
    },
    PrefetchState {
        repo_id: i64,
        head: String,
        state: &'static str,
        deadline: Instant,
        cancelled: Option<Arc<AtomicBool>>,
        response: mpsc::Sender<anyhow::Result<()>>,
    },
    CanonicalWrite {
        mutation: CanonicalMutation,
        deadline: Instant,
        cancelled: Option<Arc<AtomicBool>>,
        response: mpsc::Sender<anyhow::Result<CanonicalWriteResult>>,
    },
    Stop,
}

pub enum CanonicalMutation {
    RegisterRepository {
        path: String,
        trusted: bool,
    },
    RecordInspections {
        parser_protocol: u32,
        records: Vec<InspectionRecord>,
    },
    InsertIndexJob {
        id: String,
        key: String,
    },
    UpdateIndexJobKey {
        id: String,
        key: String,
    },
    SetIndexJobState {
        id: String,
        state: String,
        failure: Option<String>,
    },
    RemoveIndexJob {
        id: String,
    },
}

pub enum CanonicalWriteResult {
    Done,
    RepositoryId(i64),
}

fn apply_canonical_mutation(
    store: &mut Store,
    mutation: CanonicalMutation,
) -> anyhow::Result<CanonicalWriteResult> {
    match mutation {
        CanonicalMutation::RegisterRepository { path, trusted } => Ok(
            CanonicalWriteResult::RepositoryId(store.register_repository(&path, trusted)?),
        ),
        CanonicalMutation::RecordInspections {
            parser_protocol,
            records,
        } => {
            store.record_inspections(parser_protocol, &records)?;
            Ok(CanonicalWriteResult::Done)
        }
        CanonicalMutation::InsertIndexJob { id, key } => {
            store.insert_index_job(&id, &key)?;
            Ok(CanonicalWriteResult::Done)
        }
        CanonicalMutation::UpdateIndexJobKey { id, key } => {
            store.update_index_job_key(&id, &key)?;
            Ok(CanonicalWriteResult::Done)
        }
        CanonicalMutation::SetIndexJobState { id, state, failure } => {
            store.set_index_job_state(&id, &state, failure.as_deref())?;
            Ok(CanonicalWriteResult::Done)
        }
        CanonicalMutation::RemoveIndexJob { id } => {
            store.remove_index_job(&id)?;
            Ok(CanonicalWriteResult::Done)
        }
    }
}

struct Finalization {
    repo_id: i64,
    alias: Option<String>,
    active_trail_id: String,
    head: String,
    now: i64,
    protect_recent_seconds: i64,
    max_entries: u64,
}

struct PrefetchWrite {
    repo_id: i64,
    head: String,
    ceiling: usize,
    completed_count: usize,
    facts: Vec<GitCommit>,
    parents: BTreeMap<String, Vec<String>>,
    staging_max_bytes: u64,
}

fn apply_prefetch_write(store: &mut Store, batch: &PrefetchWrite) -> anyhow::Result<bool> {
    store.record_prefetch_batch(
        batch.repo_id,
        &batch.head,
        batch.ceiling,
        batch.completed_count,
        zmem_store::RawCommitBatch {
            facts: &batch.facts,
            parents: &batch.parents,
        },
        batch.staging_max_bytes,
    )
}

fn finalize_store(store: &mut Store, request: &Finalization) -> anyhow::Result<bool> {
    if let Some(selector) = &request.alias {
        store.set_ref_alias(
            request.repo_id,
            selector,
            &request.active_trail_id,
            &request.head,
        )?;
    }
    let trail_cohorts = store.trail_cohorts(request.now, request.protect_recent_seconds)?;
    let mut trail_entries = trail_cohorts.iter().map(|row| row.entries).sum::<u64>();
    let sizes = trail_cohorts
        .iter()
        .map(|row| (row.trail_id.as_str(), row.entries))
        .collect::<BTreeMap<_, _>>();
    let mut trail_evictions = Vec::new();
    for trail_id in select_trail_evictions(&trail_cohorts) {
        if trail_entries <= request.max_entries {
            break;
        }
        if trail_id == request.active_trail_id {
            continue;
        }
        trail_entries = trail_entries.saturating_sub(sizes[trail_id.as_str()]);
        trail_evictions.push(trail_id);
    }
    store.evict_trails(&trail_evictions)?;
    let plan = select_evictions(
        &store.cohorts()?,
        request.now,
        RetentionPolicy {
            max_entries: request.max_entries,
            protect_recent_seconds: request.protect_recent_seconds,
        },
    );
    let over_capacity = trail_entries > request.max_entries || plan.over_capacity;
    store.evict(&plan)?;
    Ok(over_capacity)
}

#[derive(Debug)]
pub enum PublicationError {
    Busy,
    Stopped,
}

impl std::fmt::Display for PublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Busy => "publication queue is full",
            Self::Stopped => "publication writer stopped",
        })
    }
}

impl std::error::Error for PublicationError {}

static PUBLICATION_QUEUE: OnceLock<Mutex<Option<mpsc::SyncSender<PublicationCommand>>>> =
    OnceLock::new();
static ADVISORY_QUEUE: Mutex<Option<zmem_store::AdvisoryBatch>> = Mutex::new(None);
static ADVISORY_FAILURES: AtomicUsize = AtomicUsize::new(0);

pub fn advisory_write_failures() -> usize {
    ADVISORY_FAILURES.load(Ordering::Relaxed)
}

pub fn try_advisory_write(build: impl FnOnce() -> zmem_store::AdvisoryBatch) -> bool {
    enqueue_advisory(&ADVISORY_QUEUE, build)
}

fn enqueue_advisory(
    queue: &Mutex<Option<zmem_store::AdvisoryBatch>>,
    build: impl FnOnce() -> zmem_store::AdvisoryBatch,
) -> bool {
    let Ok(mut pending) = queue.try_lock() else {
        return false;
    };
    if pending.is_some() {
        return false;
    }
    // Reserve the only slot before copying the tracker. A stalled writer must
    // not leave both an old queued snapshot and a new rejected snapshot alive.
    *pending = Some(build());
    true
}
static LAST_PUBLICATION: OnceLock<Mutex<Option<PublicationMetric>>> = OnceLock::new();

#[derive(Clone, Debug, Serialize)]
pub struct PublicationMetric {
    pub trail_id: String,
    pub transaction_us: u64,
    pub wal_bytes_before: u64,
    pub wal_bytes_after: u64,
}

pub fn last_publication_metric() -> Option<PublicationMetric> {
    LAST_PUBLICATION
        .get()
        .and_then(|metric| metric.lock().ok()?.clone())
}

fn wal_file_bytes(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

pub struct PublicationWriter {
    worker: Option<thread::JoinHandle<()>>,
}

impl PublicationWriter {
    pub fn start(database: &Path) -> anyhow::Result<Self> {
        let mut store = Store::open_writable_existing(database)?;
        let config = Config::load(
            &database
                .parent()
                .unwrap_or(Path::new("."))
                .parent()
                .unwrap_or(Path::new("."))
                .join("config.toml"),
        )?;
        store.set_staging_protection(i64::from(config.protect_recent_days) * 86_400);
        let mut wal_name = database.as_os_str().to_os_string();
        wal_name.push("-wal");
        let wal_path = PathBuf::from(wal_name);
        *LAST_PUBLICATION
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        let queue = PUBLICATION_QUEUE.get_or_init(|| Mutex::new(None));
        let mut active = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        anyhow::ensure!(active.is_none(), "publication writer is already running");
        let (sender, receiver) = mpsc::sync_channel(8);
        let worker = thread::spawn(move || {
            loop {
                let command = match receiver.recv_timeout(Duration::from_millis(25)) {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Foreground mutations always win. Advisory retries
                        // retain the bounded batch without involving readers.
                        if let Ok(mut pending) = ADVISORY_QUEUE.try_lock()
                            && let Some(batch) = pending.as_ref()
                        {
                            let result = store
                                .set_request_deadline_and_cancellation(
                                    Instant::now() + Duration::from_millis(250),
                                    None,
                                )
                                .and_then(|_| store.persist_advisory(batch));
                            if result.is_ok() {
                                *pending = None;
                            } else {
                                ADVISORY_FAILURES.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        continue;
                    }
                };
                match command {
                    PublicationCommand::Publish {
                        publication,
                        deadline,
                        cancelled,
                        response,
                    } => {
                        let started = Instant::now();
                        let wal_bytes_before = wal_file_bytes(&wal_path);
                        let result = deadline
                            .map(|deadline| {
                                store.set_request_deadline_and_cancellation(deadline, cancelled)
                            })
                            .transpose()
                            .and_then(|_| store.publish_trail(publication.borrowed()));
                        if result.is_ok() {
                            *LAST_PUBLICATION
                                .get()
                                .expect("publication metric initialized")
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                Some(PublicationMetric {
                                    trail_id: publication.trail.id.clone(),
                                    transaction_us: started
                                        .elapsed()
                                        .as_micros()
                                        .min(u128::from(u64::MAX))
                                        as u64,
                                    wal_bytes_before,
                                    wal_bytes_after: wal_file_bytes(&wal_path),
                                });
                        }
                        let _ = response.send(result);
                    }
                    PublicationCommand::Finalize {
                        request,
                        deadline,
                        cancelled,
                        response,
                    } => {
                        let result = deadline
                            .map(|deadline| {
                                store.set_request_deadline_and_cancellation(deadline, cancelled)
                            })
                            .transpose()
                            .and_then(|_| finalize_store(&mut store, &request));
                        let _ = response.send(result);
                    }
                    PublicationCommand::PrefetchBatch {
                        batch,
                        deadline,
                        cancelled,
                        response,
                    } => {
                        let result = store
                            .set_request_deadline_and_cancellation(deadline, cancelled)
                            .and_then(|_| apply_prefetch_write(&mut store, &batch));
                        let _ = response.send(result);
                    }
                    PublicationCommand::PrefetchState {
                        repo_id,
                        head,
                        state,
                        deadline,
                        cancelled,
                        response,
                    } => {
                        let result = store
                            .set_request_deadline_and_cancellation(deadline, cancelled)
                            .and_then(|_| store.set_prefetch_state(repo_id, &head, state));
                        let _ = response.send(result);
                    }
                    PublicationCommand::CanonicalWrite {
                        mutation,
                        deadline,
                        cancelled,
                        response,
                    } => {
                        let result = store
                            .set_request_deadline_and_cancellation(deadline, cancelled)
                            .and_then(|_| apply_canonical_mutation(&mut store, mutation));
                        let _ = response.send(result);
                    }
                    PublicationCommand::Stop => break,
                }
            }
        });
        *active = Some(sender);
        Ok(Self {
            worker: Some(worker),
        })
    }

    pub fn finish(mut self) -> anyhow::Result<()> {
        self.stop()
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        let queue = PUBLICATION_QUEUE
            .get()
            .expect("publication queue initialized");
        let send_result = if let Some(sender) = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            sender
                .send(PublicationCommand::Stop)
                .map_err(anyhow::Error::from)
        } else {
            Ok(())
        };
        let join_result = worker
            .join()
            .map_err(|_| anyhow::anyhow!("publication writer panicked"));
        send_result?;
        join_result
    }
}

impl Drop for PublicationWriter {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn enqueue_canonical_write(
    mutation: CanonicalMutation,
) -> anyhow::Result<Result<CanonicalWriteResult, CanonicalMutation>> {
    check_request_deadline()?;
    let sender = PUBLICATION_QUEUE
        .get()
        .and_then(|queue| queue.lock().ok()?.clone());
    let Some(sender) = sender else {
        return Ok(Err(mutation));
    };
    let deadline = REQUEST_DEADLINE
        .with(Cell::get)
        .unwrap_or_else(|| Instant::now() + Duration::from_secs(120));
    let (response, completed) = mpsc::channel();
    sender
        .try_send(PublicationCommand::CanonicalWrite {
            mutation,
            deadline,
            cancelled: request_cancellation(),
            response,
        })
        .map_err(|error| match error {
            mpsc::TrySendError::Full(_) => PublicationError::Busy,
            mpsc::TrySendError::Disconnected(_) => PublicationError::Stopped,
        })?;
    let result = completed
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|error| {
            anyhow::anyhow!(
                "service request deadline expired while waiting for canonical writer: {error}"
            )
        })??;
    Ok(Ok(result))
}

fn canonical_write_or_direct(
    store: &mut Store,
    mutation: CanonicalMutation,
) -> anyhow::Result<CanonicalWriteResult> {
    match enqueue_canonical_write(mutation)? {
        Ok(result) => Ok(result),
        Err(mutation) => apply_canonical_mutation(store, mutation),
    }
}

/// Submit a service-owned mutation to the sole canonical writer. Direct mode
/// is retained for library and coordinator tests that do not start a daemon.
pub fn canonical_write(
    database: &Path,
    mutation: CanonicalMutation,
) -> anyhow::Result<CanonicalWriteResult> {
    match enqueue_canonical_write(mutation)? {
        Ok(result) => Ok(result),
        Err(mutation) => {
            apply_canonical_mutation(&mut Store::open_writable_existing(database)?, mutation)
        }
    }
}

fn publish_canonical_trail(
    store: &mut Store,
    publication: TrailPublication<'_>,
) -> anyhow::Result<()> {
    let sender = PUBLICATION_QUEUE
        .get()
        .and_then(|queue| queue.lock().ok()?.clone());
    let Some(sender) = sender else {
        return store.publish_trail(publication);
    };
    let (response, completed) = mpsc::channel();
    let command = PublicationCommand::Publish {
        publication: Box::new(OwnedPublication::from_borrowed(&publication)),
        deadline: REQUEST_DEADLINE.with(Cell::get),
        cancelled: request_cancellation(),
        response,
    };
    sender.try_send(command).map_err(|error| match error {
        mpsc::TrySendError::Full(_) => PublicationError::Busy,
        mpsc::TrySendError::Disconnected(_) => PublicationError::Stopped,
    })?;
    if let Some(remaining) = request_remaining()? {
        completed
            .recv_timeout(remaining + Duration::from_secs(1))
            .map_err(|error| anyhow::anyhow!("publication writer did not finish: {error}"))??;
    } else {
        completed.recv()??;
    }
    Ok(())
}

fn finalize_canonical_store(store: &mut Store, request: Finalization) -> anyhow::Result<bool> {
    let sender = PUBLICATION_QUEUE
        .get()
        .and_then(|queue| queue.lock().ok()?.clone());
    let Some(sender) = sender else {
        return finalize_store(store, &request);
    };
    let (response, completed) = mpsc::channel();
    let command = PublicationCommand::Finalize {
        request,
        deadline: REQUEST_DEADLINE.with(Cell::get),
        cancelled: request_cancellation(),
        response,
    };
    sender.try_send(command).map_err(|error| match error {
        mpsc::TrySendError::Full(_) => PublicationError::Busy,
        mpsc::TrySendError::Disconnected(_) => PublicationError::Stopped,
    })?;
    if let Some(remaining) = request_remaining()? {
        completed
            .recv_timeout(remaining + Duration::from_secs(1))
            .map_err(|error| anyhow::anyhow!("publication writer did not finish: {error}"))?
    } else {
        completed.recv()?
    }
}

fn stage_prefetch_batch(store: &mut Store, batch: PrefetchWrite) -> anyhow::Result<bool> {
    let sender = PUBLICATION_QUEUE
        .get()
        .and_then(|queue| queue.lock().ok()?.clone());
    let Some(sender) = sender else {
        return apply_prefetch_write(store, &batch);
    };
    let deadline = REQUEST_DEADLINE
        .with(Cell::get)
        .unwrap_or_else(|| Instant::now() + Duration::from_secs(120));
    let (response, completed) = mpsc::channel();
    sender
        .try_send(PublicationCommand::PrefetchBatch {
            batch: Box::new(batch),
            deadline,
            cancelled: request_cancellation(),
            response,
        })
        .map_err(|error| match error {
            mpsc::TrySendError::Full(_) => PublicationError::Busy,
            mpsc::TrySendError::Disconnected(_) => PublicationError::Stopped,
        })?;
    completed
        .recv_timeout(deadline.saturating_duration_since(Instant::now()) + Duration::from_secs(1))
        .map_err(|error| anyhow::anyhow!("publication writer did not stage facts: {error}"))?
}

fn set_prefetch_state(
    store: &mut Store,
    repo_id: i64,
    head: &str,
    state: &'static str,
) -> anyhow::Result<()> {
    let sender = PUBLICATION_QUEUE
        .get()
        .and_then(|queue| queue.lock().ok()?.clone());
    let Some(sender) = sender else {
        return store.set_prefetch_state(repo_id, head, state);
    };
    let deadline = REQUEST_DEADLINE
        .with(Cell::get)
        .unwrap_or_else(|| Instant::now() + Duration::from_secs(120));
    let (response, completed) = mpsc::channel();
    sender
        .try_send(PublicationCommand::PrefetchState {
            repo_id,
            head: head.to_owned(),
            state,
            deadline,
            cancelled: (state != "paused_shutdown")
                .then(request_cancellation)
                .flatten(),
            response,
        })
        .map_err(|error| match error {
            mpsc::TrySendError::Full(_) => PublicationError::Busy,
            mpsc::TrySendError::Disconnected(_) => PublicationError::Stopped,
        })?;
    completed
        .recv_timeout(deadline.saturating_duration_since(Instant::now()) + Duration::from_secs(1))
        .map_err(|error| anyhow::anyhow!("publication writer did not update prefetch: {error}"))?
}

static HOST_PERMITS: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();

impl HostPermit {
    fn acquire(limit: usize, lookup_identity: bool) -> anyhow::Result<Self> {
        let (count, available) = HOST_PERMITS.get_or_init(|| (Mutex::new(0), Condvar::new()));
        let mut active = count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let admitted_limit = if lookup_identity || limit == 1 {
            limit
        } else {
            limit - 1
        };
        while *active >= admitted_limit {
            let interval = request_remaining()?
                .unwrap_or(Duration::from_millis(10))
                .min(Duration::from_millis(10));
            active = available
                .wait_timeout(active, interval)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        check_request_deadline()?;
        *active += 1;
        Ok(Self)
    }
}

impl Drop for HostPermit {
    fn drop(&mut self) {
        let (count, available) = HOST_PERMITS.get().expect("permit pool initialized");
        let mut active = count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *active -= 1;
        available.notify_all();
    }
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
                Self::Identity | Self::LookupIdentity | Self::Inspection => 2,
                Self::Expansion => 1,
            },
            deadline: Duration::from_secs(config.extension_host_timeout_seconds.get()),
        }
    }
}

fn execute_supervised(
    command: &mut Command,
    input: &[u8],
    budget: Duration,
) -> anyhow::Result<Output> {
    zmem_core::supervise_process(
        command,
        Some(input),
        Some(Instant::now() + budget),
        request_cancellation(),
    )
    .map_err(|error| {
        if error.to_string().contains("deadline expired") {
            error.context("extension host timed out")
        } else {
            error
        }
    })
}

fn execute_host_output_supervised(
    config: &Config,
    operation: HostOperation,
    request: &serde_json::Value,
) -> anyhow::Result<Vec<u8>> {
    let _permit = HostPermit::acquire(
        config.max_concurrency.get(),
        operation == HostOperation::LookupIdentity,
    )?;
    let host = extension_host_command(config);
    let policy = operation.execution_policy(config);
    let input = serde_json::to_vec(request)?;
    let mut last_error = None;
    for _ in 0..policy.attempts {
        let mut command = Command::new(&host.executable);
        command.args(&host.args);
        let attempt_deadline = request_remaining()?
            .map_or(policy.deadline, |remaining| remaining.min(policy.deadline));
        let result =
            execute_supervised(&mut command, &input, attempt_deadline).with_context(|| {
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
            Err(error) => {
                request_remaining()?;
                last_error = Some(error);
            }
        }
    }
    Err(last_error.expect("host execution policy has at least one attempt"))
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let config = if path.exists() {
            toml::from_str(&std::fs::read_to_string(path)?).context("invalid zmem config")?
        } else {
            Self::default()
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.lookup_capacity.get() <= 64,
            "lookup_capacity exceeds 64"
        );
        anyhow::ensure!(
            self.lookup_queue_capacity.get() <= self.connection_capacity.get(),
            "lookup_queue_capacity exceeds connection_capacity"
        );
        anyhow::ensure!(
            self.heavy_capacity.get() <= 256,
            "heavy_capacity exceeds 256"
        );
        anyhow::ensure!(
            self.connection_capacity.get() <= 1024,
            "connection_capacity exceeds 1024"
        );
        anyhow::ensure!(
            self.connection_capacity.get() > self.lookup_capacity.get() + self.heavy_capacity.get(),
            "connection_capacity must exceed lookup_capacity plus heavy_capacity"
        );
        Ok(())
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

type IdentityCache = HashMap<(PathBuf, bool), (Vec<u8>, String)>;
static LOOKUP_IDENTITIES: OnceLock<Mutex<IdentityCache>> = OnceLock::new();

#[derive(Default)]
struct DependencySnapshot(Vec<u8>);

impl DependencySnapshot {
    fn update(&mut self, bytes: impl AsRef<[u8]>) -> anyhow::Result<()> {
        let bytes = bytes.as_ref();
        anyhow::ensure!(
            self.0.len().saturating_add(bytes.len()).saturating_add(8) <= 2 * 1024 * 1024,
            "extension dependency snapshot exceeds cache limit"
        );
        self.0
            .extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        self.0.extend_from_slice(bytes);
        Ok(())
    }
}

fn record_dependency_tree(snapshot: &mut DependencySnapshot, path: &Path) -> anyhow::Result<()> {
    check_request_deadline()?;
    snapshot.update(path.to_string_lossy().as_bytes())?;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            snapshot.update([0])?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        !metadata.file_type().is_symlink(),
        "symlinked extension source"
    );
    if metadata.is_file() {
        anyhow::ensure!(
            metadata.len() <= 2 * 1024 * 1024,
            "extension source is too large"
        );
        snapshot.update([1])?;
        snapshot.update(std::fs::read(path)?)?;
        return Ok(());
    }
    anyhow::ensure!(metadata.is_dir(), "unsupported extension source type");
    snapshot.update([2])?;
    let mut children = std::fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|child| child.to_string_lossy().to_lowercase());
    for child in children {
        if child.file_name().is_some_and(|name| name == "__pycache__")
            || child
                .extension()
                .is_some_and(|extension| extension == "pyc")
        {
            continue;
        }
        record_dependency_tree(snapshot, &child)?;
    }
    Ok(())
}

fn installed_host_package(host: &Path) -> Option<PathBuf> {
    let root = host.parent()?.parent()?;
    #[cfg(windows)]
    {
        let package = root.join("Lib").join("site-packages").join("zmem");
        package.is_dir().then_some(package)
    }
    #[cfg(not(windows))]
    {
        let mut packages = Vec::new();
        for library in ["lib", "lib64"] {
            let Ok(entries) = std::fs::read_dir(root.join(library)) else {
                continue;
            };
            for entry in entries {
                let entry = entry.ok()?;
                let package = entry.path().join("site-packages").join("zmem");
                if package.is_dir() {
                    packages.push(package);
                }
            }
        }
        (packages.len() == 1).then(|| packages.remove(0))
    }
}

fn installed_identity_fingerprint(
    config: &Config,
    home: &Path,
    repo: &GitRepo,
    trusted: bool,
) -> anyhow::Result<Option<Vec<u8>>> {
    if config.extension_host.is_some() || std::env::var_os("ZMEM_EXTENSION_HOST").is_some() {
        return Ok(None);
    }
    if ["PYTHONPATH", "PYTHONHOME", "PYTHONUSERBASE"]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
        || std::env::current_dir().ok().is_some_and(|directory| {
            directory.join("zmem").exists() || directory.join("zmem.py").exists()
        })
    {
        return Ok(None);
    }
    let Some(host) = std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(installed_extension_host)
    else {
        return Ok(None);
    };
    let Some(package) = installed_host_package(&host.executable) else {
        return Ok(None);
    };
    Ok(Some(dependency_snapshot_for_host(
        &host.executable,
        &package,
        home,
        repo.root(),
        trusted,
    )?))
}

fn dependency_snapshot_for_host(
    host_executable: &Path,
    package: &Path,
    home: &Path,
    repo_root: &Path,
    trusted: bool,
) -> anyhow::Result<Vec<u8>> {
    let root = host_executable
        .parent()
        .and_then(Path::parent)
        .expect("installed host layout");
    let mut snapshot = DependencySnapshot::default();
    snapshot.update(b"zmem-installed-identity-v1")?;
    snapshot.update([u8::from(trusted)])?;
    snapshot.update(
        std::fs::canonicalize(host_executable)?
            .to_string_lossy()
            .as_bytes(),
    )?;
    snapshot.update(std::fs::read(host_executable)?)?;
    record_dependency_tree(&mut snapshot, &root.join("pyvenv.cfg"))?;
    record_dependency_tree(&mut snapshot, package)?;
    record_dependency_tree(&mut snapshot, &home.join("config.toml"))?;
    record_dependency_tree(&mut snapshot, &home.join("ext").join("expanders"))?;
    record_dependency_tree(&mut snapshot, &home.join("ext").join("hooks"))?;
    let custom_root = std::env::var_os("ZMEM_CUSTOM_EXT_ROOT").unwrap_or_else(|| ".zmem".into());
    snapshot.update(custom_root.to_string_lossy().as_bytes())?;
    let custom = PathBuf::from(custom_root);
    let custom = if custom.is_absolute() {
        custom
    } else {
        repo_root.join(custom)
    };
    if trusted {
        for mode in ["extend", "overwrite"] {
            for kind in ["expanders", "hooks"] {
                record_dependency_tree(&mut snapshot, &custom.join(mode).join(kind))?;
            }
        }
    } else {
        for mode in ["extend", "overwrite"] {
            snapshot.update([u8::from(custom.join(mode).exists())])?;
        }
    }
    Ok(snapshot.0)
}

fn lookup_identity(
    config: &Config,
    home: &Path,
    repo: &GitRepo,
    trusted: bool,
) -> anyhow::Result<String> {
    let fingerprint = installed_identity_fingerprint(config, home, repo, trusted)
        .ok()
        .flatten();
    cached_identity_for_snapshot(repo.root(), trusted, fingerprint, || {
        Ok(invoke_identity(config, home, repo, trusted, true)?.extension_hash)
    })
}

fn pending_identity_generation(
    config: &Config,
    home: &Path,
    repo: &GitRepo,
    trusted: bool,
) -> anyhow::Result<String> {
    fn fold(hash: &mut u64, bytes: &[u8]) {
        for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    let mut digest = 0xcbf29ce484222325_u64;
    fold(&mut digest, &[u8::from(trusted)]);
    fold(
        &mut digest,
        &std::fs::read(home.join("config.toml")).unwrap_or_default(),
    );
    if let Some(snapshot) = installed_identity_fingerprint(config, home, repo, trusted)
        .ok()
        .flatten()
    {
        fold(&mut digest, b"installed-host-snapshot");
        fold(&mut digest, &snapshot);
    } else {
        // Unknown host layouts cannot prove source compatibility here. Their
        // failed jobs remain failed until an explicit retry, never silently
        // rerunning potentially side-effecting hooks.
        fold(&mut digest, b"uncertain-host");
    }
    Ok(format!("pending:{digest:016x}:trusted={trusted}"))
}

pub fn pending_repository_generation(path: &Path, trusted: bool) -> anyhow::Result<String> {
    let home = zmem_home()?;
    let config = Config::load(&home.join("config.toml"))?;
    let repo = GitRepo::open(path)?;
    pending_identity_generation(&config, &home, &repo, trusted)
}

fn cached_identity_for_snapshot(
    repo_root: &Path,
    trusted: bool,
    fingerprint: Option<Vec<u8>>,
    validate: impl FnOnce() -> anyhow::Result<String>,
) -> anyhow::Result<String> {
    if let Some(ref fingerprint) = fingerprint {
        let cache = LOOKUP_IDENTITIES.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some((cached, identity)) = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(repo_root.to_path_buf(), trusted))
            && cached == fingerprint
        {
            return Ok(identity.clone());
        }
    }
    let identity = validate()?;
    if let Some(fingerprint) = fingerprint {
        let cache = LOOKUP_IDENTITIES.get_or_init(|| Mutex::new(HashMap::new()));
        let mut cache = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cached_bytes = cache
            .values()
            .map(|(snapshot, _)| snapshot.len())
            .sum::<usize>();
        if cache.len() >= 64 || cached_bytes.saturating_add(fingerprint.len()) > 16 * 1024 * 1024 {
            cache.clear();
        }
        cache.insert(
            (repo_root.to_path_buf(), trusted),
            (fingerprint, identity.clone()),
        );
    }
    Ok(identity)
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
    lookup: bool,
) -> anyhow::Result<HostResponse> {
    execute_host(
        config,
        if lookup {
            HostOperation::LookupIdentity
        } else {
            HostOperation::Identity
        },
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
    #[serde(skip)]
    pub demand_summary: Option<SyncSummary>,
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
    let deadline = REQUEST_DEADLINE.get();
    let cancelled = request_cancellation();
    run_ordered(commits, config.max_concurrency.get(), |commit| {
        let response = with_cancellation(cancelled.clone(), || {
            with_absolute_deadline(deadline, || {
                invoke_host(config, home, repo, &commit, trusted, run_hooks, preview)
            })
        });
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
    parents: std::collections::BTreeMap<String, Vec<String>>,
    usage: AttentionUsage,
}

const INSPECTION_BATCH_SIZE: usize = 64;

fn collect_raw_commits(
    store: &Store,
    repo: &GitRepo,
    cached_repo_id: Option<i64>,
    shas: &[String],
) -> anyhow::Result<Vec<GitCommit>> {
    let mut facts = std::collections::HashMap::new();
    let mut missing = Vec::new();
    for sha in shas {
        check_request_deadline()?;
        if let Some(repo_id) = cached_repo_id
            && let Some(fact) = store.raw_commit(repo_id, sha)?
        {
            facts.insert(sha.clone(), fact);
        } else {
            missing.push(sha.clone());
        }
    }
    for batch in missing.chunks(64) {
        check_request_deadline()?;
        for fact in repo.commits_batch(batch)? {
            facts.insert(fact.sha.clone(), fact);
        }
    }
    shas.iter()
        .map(|sha| {
            facts
                .remove(sha)
                .with_context(|| format!("missing raw commit fact {sha}"))
        })
        .collect()
}

fn inspect_commits(
    config: &Config,
    store: &mut Store,
    commits: &[GitCommit],
    canonical: bool,
) -> anyhow::Result<Vec<HostInspection>> {
    let mut results = std::collections::HashMap::new();
    let mut misses = Vec::new();
    for commit in commits {
        check_request_deadline()?;
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
    let deadline = REQUEST_DEADLINE.get();
    let cancelled = request_cancellation();
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
        let response = with_cancellation(cancelled.clone(), || {
            with_absolute_deadline(deadline, || {
                execute_host_output_supervised(config, HostOperation::Inspection, &request)
            })
        })
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
    if canonical {
        canonical_write_or_direct(
            store,
            CanonicalMutation::RecordInspections {
                parser_protocol: zmem_core::PROTOCOL_VERSION,
                records,
            },
        )?;
    } else {
        store.record_inspections(zmem_core::PROTOCOL_VERSION, &records)?;
    }
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
    cached_repo_id: Option<i64>,
    repo: &GitRepo,
    head: &str,
    policy: AttentionPolicy,
    reserved_nodes: usize,
) -> anyhow::Result<SelectedHistory> {
    let walk = repo.walk_newest(head, policy.commit_limit)?;
    let commits = collect_raw_commits(store, repo, cached_repo_id, &walk.shas)?;
    let inspections = inspect_commits(config, store, &commits, cached_repo_id.is_some())?;
    let mut candidates = Vec::with_capacity(commits.len());
    for (commit, inspection) in commits.into_iter().zip(inspections) {
        candidates.push(AttentionCandidate::new(commit, inspection.annotation_count));
    }
    let selection = select_attention(candidates, policy, reserved_nodes, walk.truncated)?;
    Ok(SelectedHistory {
        commits: selection.selected,
        parents: walk.parents,
        usage: selection.usage,
    })
}

fn replay_commits(
    store: &mut Store,
    repo_id: i64,
    context: &CheckContext<'_>,
    shas: &[String],
) -> anyhow::Result<()> {
    let commits = collect_raw_commits(store, context.repo, None, shas)?;
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
            affected_areas: None,
            parents: &[],
            range_complete: true,
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
            affected_areas: None,
            parents: &[],
            range_complete: true,
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
        demand_summary: None,
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

fn history_policy_for_proposal(
    policy: AttentionPolicy,
    proposed_nodes: usize,
) -> anyhow::Result<AttentionPolicy> {
    let node_limit = match policy.node_limit.maximum() {
        Some(maximum) => {
            anyhow::ensure!(
                proposed_nodes <= maximum,
                "proposed message exceeds node attention limit"
            );
            if maximum == proposed_nodes {
                AttentionLimit::zero()
            } else {
                AttentionLimit::parse(i64::try_from(maximum - proposed_nodes)?, "node")?
            }
        }
        None => AttentionLimit::Unlimited,
    };
    Ok(AttentionPolicy {
        commit_limit: policy.commit_limit,
        node_limit,
    })
}

pub fn fast_check_history_policy(
    message: &str,
    commit_limit: Option<i64>,
    node_limit: Option<i64>,
) -> anyhow::Result<AttentionPolicy> {
    let config = Config::load(&zmem_home()?.join("config.toml"))?;
    let policy = resolve_attention_policy(commit_limit, node_limit)?;
    let proposed = GitCommit {
        sha: VIRTUAL_OID.to_owned(),
        commit_time: 0,
        message: message.to_owned(),
        changes: Vec::new(),
    };
    history_policy_for_proposal(policy, inspect_host(&config, &proposed)?.annotation_count)
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
        changes: Vec::new(),
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
        let history_policy = history_policy_for_proposal(policy, proposed_nodes)?;
        let sync = sync_repository_with_policy(path, None, history_policy)?;
        let mut attention = sync.summary.attention.clone();
        attention.node_limit = policy.node_limit.as_i64();
        attention.selected_nodes = attention.selected_nodes.saturating_add(proposed_nodes);
        let mut store = open_canonical_store(&home.join("db").join("entries.db"))?;
        let (repo_id, trusted) = store
            .repository(&canonical)?
            .context("synchronized repository registration is missing")?;
        let parent = repo.head()?;
        let identity = invoke_identity(&config, &home, &repo, trusted, false)?.extension_hash;
        let context = CheckContext {
            config: &config,
            home: &home,
            repo: &repo,
            trusted,
            identity: &identity,
        };
        let mut result = preview_commit(
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
        )?;
        if result.ok {
            result.demand_summary = Some(sync.summary);
        }
        return Ok(result);
    }

    let database = home.join("db").join("entries.db");
    let trusted = if database.exists() {
        open_canonical_store(&database)?
            .repository(&canonical)?
            .map(|(_, trusted)| trusted)
            .unwrap_or(false)
    } else {
        false
    };
    let identity = invoke_identity(&config, &home, &repo, trusted, false)?.extension_hash;
    check_request_deadline()?;
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
        None,
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

#[derive(Clone, Debug, serde::Serialize)]
pub struct SyncSummary {
    pub repository: String,
    #[serde(skip)]
    pub route: String,
    pub head: String,
    pub indexed_commits: usize,
    pub entries: usize,
    pub over_capacity: bool,
    pub max_concurrency: usize,
    pub attention: AttentionUsage,
    pub trail: NativeTrailSummary,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct NativeTrailSummary {
    pub requested_selector: Option<String>,
    pub resolved_oid: String,
    pub trail_id: String,
    pub attention_identity: String,
    pub selected_commits: usize,
    pub selected_nodes: usize,
    pub extension_identity: String,
    pub protocol_version: u32,
    pub schema_version: u32,
}

#[derive(Debug)]
pub struct SyncResult {
    pub summary: SyncSummary,
    pub entries: Vec<serde_json::Value>,
    pub relationships: Vec<serde_json::Value>,
    pub diagnostics: Vec<serde_json::Value>,
}

/// Read an exact published trail without constructing its selected history.
/// A miss leaves all Git history and extension expansion to the indexing lane.
pub fn query_published_with_ref_attention(
    path: &Path,
    selector: Option<&str>,
    observed_oid: Option<&str>,
    commit_limit: Option<i64>,
    node_limit: Option<i64>,
    include_invalid: bool,
) -> anyhow::Result<(Option<SyncResult>, String)> {
    query_published_in_lane(
        path,
        selector,
        observed_oid,
        commit_limit,
        node_limit,
        include_invalid,
        true,
    )
}

pub fn query_published_for_indexing(
    path: &Path,
    selector: Option<&str>,
    observed_oid: Option<&str>,
    commit_limit: Option<i64>,
    node_limit: Option<i64>,
) -> anyhow::Result<(Option<SyncResult>, String)> {
    query_published_in_lane(
        path,
        selector,
        observed_oid,
        commit_limit,
        node_limit,
        false,
        false,
    )
}

fn query_published_in_lane(
    path: &Path,
    selector: Option<&str>,
    observed_oid: Option<&str>,
    commit_limit: Option<i64>,
    node_limit: Option<i64>,
    include_invalid: bool,
    lookup: bool,
) -> anyhow::Result<(Option<SyncResult>, String)> {
    let policy = resolve_attention_policy(commit_limit, node_limit)?;
    let home = zmem_home()?;
    let config = Config::load(&home.join("config.toml"))?;
    let repo = GitRepo::open(path)?;
    let requested = selector.unwrap_or("HEAD");
    let client_oid = observed_oid
        .map(str::to_owned)
        .unwrap_or(repo.resolve(requested)?);
    let resolved = repo.resolve_observed(requested, &client_oid)?;
    let canonical = repo.root().to_string_lossy().into_owned();
    let mut store = Store::open_readonly(&home.join("db").join("entries.db"))?;
    apply_store_deadline(&store)?;
    let repository = store.repository(&canonical)?;
    let trusted = repository.is_some_and(|(_, trusted)| trusted);
    if !store.has_published_head(&canonical, &resolved.oid)? {
        return Ok((
            None,
            pending_identity_generation(&config, &home, &repo, trusted)?,
        ));
    }
    let identity = if lookup {
        lookup_identity(&config, &home, &repo, trusted)?
    } else {
        invoke_identity(&config, &home, &repo, trusted, false)?.extension_hash
    };
    check_request_deadline()?;
    let Some((repo_id, _)) = repository else {
        return Ok((None, format!("{identity}:trusted={trusted}")));
    };
    let generation = format!("{identity}:trusted={trusted}");
    let key = TrailIdentity::new(
        repo_id,
        resolved.oid.clone(),
        policy,
        identity,
        zmem_core::PROTOCOL_VERSION,
        SCHEMA_VERSION,
    );
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let snapshot = store.published_snapshot(
        &key,
        include_invalid,
        config.max_entries.get(),
        now,
        i64::from(config.protect_recent_days) * 86_400,
    );
    check_request_deadline()?;
    let Some(snapshot) = snapshot? else {
        return Ok((None, generation));
    };
    let zmem_store::PublishedSnapshot {
        trail,
        entries,
        relationships,
        diagnostics,
        over_capacity,
    } = snapshot;
    let Some(usage) = AttentionUsage::from_view_identity(&trail.attention_identity) else {
        return Ok((None, generation));
    };
    let summary = SyncSummary {
        repository: canonical,
        route: resolved.route,
        head: resolved.oid.clone(),
        indexed_commits: 0,
        entries: entries.len(),
        over_capacity,
        max_concurrency: config.max_concurrency.get(),
        attention: usage,
        trail: NativeTrailSummary {
            requested_selector: selector.map(str::to_owned),
            resolved_oid: resolved.oid,
            trail_id: trail.id,
            attention_identity: trail.attention_identity,
            selected_commits: trail.selected_commit_count,
            selected_nodes: trail.selected_node_count,
            extension_identity: trail.extension_identity,
            protocol_version: trail.protocol_version,
            schema_version: trail.schema_version,
        },
    };
    Ok((
        Some(SyncResult {
            summary,
            entries,
            relationships,
            diagnostics,
        }),
        generation,
    ))
}

pub fn sync_repository(path: &Path, trust: Option<bool>) -> anyhow::Result<SyncResult> {
    sync_repository_with_attention(path, trust, None, None)
}

/// Collect reusable Git facts for a pinned HEAD without invoking extensions.
/// Each completed batch has an immutable OID checkpoint and a bounded write.
pub fn prefetch_repository(
    path: &Path,
    pinned_head: &str,
    stopping: &AtomicBool,
    demand_jobs: &AtomicUsize,
) -> anyhow::Result<()> {
    let completed_foreground = AtomicUsize::new(0);
    if let Some(mut session) = PrefetchSession::start(path, pinned_head)? {
        while session.advance(stopping, demand_jobs, &completed_foreground)? == PrefetchTurn::More {
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefetchTurn {
    More,
    Done,
}

/// Keep one immutable traversal in memory while the scheduler rotates between
/// repositories. A turn writes no more than 64 raw commits.
pub struct PrefetchSession {
    config: Config,
    ceiling: usize,
    repo: GitRepo,
    store: Store,
    repo_id: i64,
    pinned_head: String,
    references: Vec<(String, usize)>,
    walk: zmem_core::GitWalk,
    position: usize,
}

impl PrefetchSession {
    pub fn start(path: &Path, pinned_head: &str) -> anyhow::Result<Option<Self>> {
        let config = Config::load(&zmem_home()?.join("config.toml"))?;
        Self::start_target(
            path,
            pinned_head,
            config.background_commit_limit as usize,
            vec!["HEAD".into()],
        )
    }

    pub fn start_target(
        path: &Path,
        pinned_head: &str,
        target: usize,
        references: Vec<String>,
    ) -> anyhow::Result<Option<Self>> {
        let home = zmem_home()?;
        let config = Config::load(&home.join("config.toml"))?;
        let ceiling = target.min(config.background_commit_limit as usize);
        if ceiling == 0 {
            return Ok(None);
        }
        let repo = GitRepo::open(path)?;
        let canonical = repo.root().to_string_lossy().into_owned();
        let mut store = open_canonical_store(&home.join("db").join("entries.db"))?;
        store.set_staging_protection(i64::from(config.protect_recent_days) * 86_400);
        let Some((repo_id, _)) = store.repository(&canonical)? else {
            return Ok(None);
        };
        let limit =
            AttentionLimit::Limited(NonZeroUsize::new(ceiling).expect("nonzero prefetch ceiling"));
        let walk = with_request_deadline(120_000, || repo.walk_newest(pinned_head, limit))?;
        let (mut position, checkpoint) = store
            .prefetch_checkpoint(repo_id, pinned_head)?
            .unwrap_or((0, None));
        if position > walk.shas.len()
            || (position > 0
                && checkpoint.as_deref() != walk.shas.get(position - 1).map(String::as_str))
        {
            position = 0;
        }
        // Coverage counters are advisory after reclamation. Verify the prefix
        // against surviving facts before skipping any Git objects.
        for (index, oid) in walk.shas.iter().take(position).enumerate() {
            if store.raw_commit(repo_id, oid)?.is_none() {
                position = index;
                break;
            }
        }
        Ok(Some(Self {
            config,
            ceiling,
            repo,
            store,
            repo_id,
            pinned_head: pinned_head.to_owned(),
            references: references
                .into_iter()
                .map(|reference| (reference, ceiling))
                .collect(),
            walk,
            position,
        }))
    }

    pub fn advance(
        &mut self,
        stopping: &AtomicBool,
        demand_jobs: &AtomicUsize,
        completed_foreground: &AtomicUsize,
    ) -> anyhow::Result<PrefetchTurn> {
        let mut target = self.walk.shas.len();
        if self.position < self.walk.shas.len() {
            let yielding_since = Instant::now();
            let completed_before = completed_foreground.load(Ordering::Acquire);
            while demand_jobs.load(Ordering::Acquire) > 0
                && yielding_since.elapsed() < Duration::from_millis(500)
                && completed_foreground
                    .load(Ordering::Acquire)
                    .wrapping_sub(completed_before)
                    < 8
                && !stopping.load(Ordering::Acquire)
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            if stopping.load(Ordering::Acquire) {
                set_prefetch_state(
                    &mut self.store,
                    self.repo_id,
                    &self.pinned_head,
                    "paused_shutdown",
                )?;
                return Ok(PrefetchTurn::Done);
            }
            let justified = with_request_deadline(120_000, || {
                self.references
                    .iter()
                    .find_map(|(reference, target)| {
                        (reference == &format!("oid:{}", self.pinned_head)
                            || self
                                .repo
                                .resolve(reference)
                                .is_ok_and(|oid| oid == self.pinned_head))
                        .then_some(*target)
                    })
                    .unwrap_or(0)
            });
            target = justified.min(self.ceiling).min(self.walk.shas.len());
            if target == 0 {
                set_prefetch_state(&mut self.store, self.repo_id, &self.pinned_head, "obsolete")?;
                return Ok(PrefetchTurn::Done);
            }
            if self.position >= target {
                self.pause()?;
                return Ok(PrefetchTurn::Done);
            }
            let end = (self.position + 64).min(target);
            let batch = with_request_deadline(120_000, || {
                collect_raw_commits(
                    &self.store,
                    &self.repo,
                    Some(self.repo_id),
                    &self.walk.shas[self.position..end],
                )
            })?;
            let parents = batch
                .iter()
                .filter_map(|fact| {
                    self.walk
                        .parents
                        .get(&fact.sha)
                        .map(|parents| (fact.sha.clone(), parents.clone()))
                })
                .collect();
            if !with_request_deadline(120_000, || {
                stage_prefetch_batch(
                    &mut self.store,
                    PrefetchWrite {
                        repo_id: self.repo_id,
                        head: self.pinned_head.clone(),
                        ceiling: target,
                        completed_count: end,
                        facts: batch,
                        parents,
                        staging_max_bytes: self.config.staging_max_bytes.get(),
                    },
                )
            })? {
                return Ok(PrefetchTurn::Done);
            }
            self.position = end;
        }
        if self.position < target {
            Ok(PrefetchTurn::More)
        } else {
            set_prefetch_state(&mut self.store, self.repo_id, &self.pinned_head, "ready")?;
            Ok(PrefetchTurn::Done)
        }
    }

    pub fn pause(&mut self) -> anyhow::Result<()> {
        set_prefetch_state(
            &mut self.store,
            self.repo_id,
            &self.pinned_head,
            "paused_demand",
        )
    }

    pub fn set_references(&mut self, references: Vec<String>) {
        self.references = references
            .into_iter()
            .map(|reference| (reference, self.ceiling))
            .collect();
    }

    pub fn set_route_targets(&mut self, mut references: Vec<(String, usize)>) {
        references.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        self.references = references;
    }
}

pub fn sync_repository_with_attention(
    path: &Path,
    trust: Option<bool>,
    commit_limit: Option<i64>,
    node_limit: Option<i64>,
) -> anyhow::Result<SyncResult> {
    let policy = resolve_attention_policy(commit_limit, node_limit)?;
    sync_repository_with_selection(path, trust, None, None, policy)
}

pub fn sync_repository_with_ref_attention(
    path: &Path,
    trust: Option<bool>,
    selector: Option<&str>,
    observed_oid: Option<&str>,
    commit_limit: Option<i64>,
    node_limit: Option<i64>,
) -> anyhow::Result<SyncResult> {
    let policy = resolve_attention_policy(commit_limit, node_limit)?;
    sync_repository_with_selection(path, trust, selector, observed_oid, policy)
}

fn sync_repository_with_policy(
    path: &Path,
    trust: Option<bool>,
    policy: AttentionPolicy,
) -> anyhow::Result<SyncResult> {
    sync_repository_with_selection(path, trust, None, None, policy)
}

fn sync_repository_with_selection(
    path: &Path,
    trust: Option<bool>,
    selector: Option<&str>,
    observed_oid: Option<&str>,
    policy: AttentionPolicy,
) -> anyhow::Result<SyncResult> {
    let home = zmem_home()?;
    let config = Config::load(&home.join("config.toml"))?;
    let repo = GitRepo::open(path)?;
    let canonical = repo.root().to_string_lossy().into_owned();
    let requested = selector.unwrap_or("HEAD");
    let client_oid = observed_oid
        .map(str::to_owned)
        .unwrap_or(repo.resolve(requested)?);
    let resolved = repo.resolve_observed(requested, &client_oid)?;
    let head = resolved.oid.clone();
    let mut store = open_canonical_store(&home.join("db").join("entries.db"))?;
    apply_store_deadline(&store)?;
    let (repo_id, trusted) = match store.repository(&canonical)? {
        Some((id, current)) => (id, trust.unwrap_or(current)),
        None => {
            let selected = trust.unwrap_or(false);
            (
                match canonical_write_or_direct(
                    &mut store,
                    CanonicalMutation::RegisterRepository {
                        path: canonical.clone(),
                        trusted: selected,
                    },
                )? {
                    CanonicalWriteResult::RepositoryId(id) => id,
                    CanonicalWriteResult::Done => unreachable!(),
                },
                selected,
            )
        }
    };
    if trust.is_some() {
        canonical_write_or_direct(
            &mut store,
            CanonicalMutation::RegisterRepository {
                path: canonical.clone(),
                trusted,
            },
        )?;
    }

    let identity = invoke_identity(&config, &home, &repo, trusted, false)?.extension_hash;
    let selected = select_history(&config, &mut store, Some(repo_id), &repo, &head, policy, 0)?;
    let lower_boundary = selected.commits.first().map(|commit| commit.sha.as_str());
    let attention_identity = selected.usage.view_identity(lower_boundary);
    let identity_key = TrailIdentity::new(
        repo_id,
        head.clone(),
        policy,
        &identity,
        zmem_core::PROTOCOL_VERSION,
        SCHEMA_VERSION,
    );
    let trail_id = format!("{}:{}", identity_key.key(), attention_identity);
    let existing = store.trail(&trail_id)?;
    let mut indexed = 0;
    let active_trail = if let Some(existing) = existing {
        existing
    } else {
        let mut expansions = std::collections::BTreeMap::new();
        let mut missing = Vec::new();
        for commit in &selected.commits {
            if let Some(response) = store.expansion_fact(repo_id, &commit.sha, &identity)? {
                expansions.insert(commit.sha.clone(), response);
            } else {
                missing.push(commit.clone());
            }
        }
        indexed = missing.len();
        for (commit, response) in
            expand_commits(&config, &home, &repo, missing, trusted, true, false)
        {
            let response = response?;
            anyhow::ensure!(
                response.extension_hash == identity,
                "extension identity changed during indexing"
            );
            expansions.insert(commit.sha, response);
        }
        let areas = selected
            .commits
            .iter()
            .map(|commit| (commit.sha.clone(), derive_affected_areas(&commit.changes)))
            .collect::<std::collections::BTreeMap<_, _>>();
        let final_anchor = Anchor {
            head: head.clone(),
            schema: SCHEMA_VERSION,
            extension_hash: identity.clone(),
            attention_identity: attention_identity.clone(),
        };
        let temporary = TemporaryStore::new()?;
        let mut projection = Store::open(&temporary.database())?;
        apply_store_deadline(&projection)?;
        let projection_repo = projection.register_repository(&canonical, trusted)?;
        let updates = selected
            .commits
            .iter()
            .map(|commit| CommitUpdate {
                oid: &commit.sha,
                commit_time: commit.commit_time,
                message: &commit.message,
                response: expansions
                    .get(&commit.sha)
                    .expect("selected commit has expansion fact"),
                anchor: &final_anchor,
                affected_areas: areas.get(&commit.sha).and_then(Option::as_deref),
                parents: selected
                    .parents
                    .get(&commit.sha)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                range_complete: !selected.usage.truncated,
            })
            .collect::<Vec<_>>();
        projection.replace_projection(projection_repo, &updates, &final_anchor)?;
        let entries = projection.query_entries(projection_repo, true)?;
        let relationships = projection.query_relationships(projection_repo)?;
        let diagnostics = projection.query_diagnostics(projection_repo)?;
        let record = TrailRecord {
            id: trail_id.clone(),
            repository_id: repo_id,
            head_oid: head.clone(),
            attention_identity: attention_identity.clone(),
            extension_identity: identity.clone(),
            protocol_version: zmem_core::PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            legacy: false,
            selected_commit_count: selected.usage.selected_commits,
            selected_node_count: selected.usage.selected_nodes,
            source_time: selected
                .commits
                .iter()
                .map(|commit| commit.commit_time)
                .max()
                .unwrap_or_default(),
        };
        publish_canonical_trail(
            &mut store,
            TrailPublication {
                trail: &record,
                commits: &selected.commits,
                parents: &selected.parents,
                entries: &entries,
                relationships: &relationships,
                diagnostics: &diagnostics,
                expansions: &expansions,
            },
        )?;
        record
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let protect_recent_seconds = i64::from(config.protect_recent_days) * 86_400;
    let over_capacity = finalize_canonical_store(
        &mut store,
        Finalization {
            repo_id,
            alias: (resolved.local_branch && selector.is_some()).then(|| requested.to_owned()),
            active_trail_id: active_trail.id.clone(),
            head: head.clone(),
            now,
            protect_recent_seconds,
            max_entries: config.max_entries.get(),
        },
    )?;
    let entries = store.query_trail_entries(&active_trail.id, true)?;
    let relationships = store.query_trail_relationships(&active_trail.id)?;
    let diagnostics = store.query_trail_diagnostics(&active_trail.id)?;
    let trail = NativeTrailSummary {
        requested_selector: selector.map(str::to_owned),
        resolved_oid: head.clone(),
        trail_id: active_trail.id,
        attention_identity: active_trail.attention_identity,
        selected_commits: active_trail.selected_commit_count,
        selected_nodes: active_trail.selected_node_count,
        extension_identity: active_trail.extension_identity,
        protocol_version: active_trail.protocol_version,
        schema_version: active_trail.schema_version,
    };
    Ok(SyncResult {
        summary: SyncSummary {
            repository: canonical,
            route: resolved.route,
            head,
            indexed_commits: indexed,
            entries: entries.len(),
            over_capacity,
            max_concurrency: config.max_concurrency.get(),
            attention: selected.usage,
            trail,
        },
        entries,
        relationships,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advisory_backpressure_does_not_build_an_extra_snapshot() {
        let queue = Mutex::new(Some(zmem_store::AdvisoryBatch::default()));
        assert!(!enqueue_advisory(&queue, || panic!(
            "full queue copied tracker"
        )));
        *queue.lock().unwrap() = None;
        let locked = queue.lock().unwrap();
        assert!(!enqueue_advisory(&queue, || panic!(
            "locked queue copied tracker"
        )));
        drop(locked);
        assert!(enqueue_advisory(&queue, || zmem_store::AdvisoryBatch {
            now: 42,
            ..Default::default()
        }));
        assert_eq!(queue.lock().unwrap().as_ref().unwrap().now, 42);
    }

    static HOST_TEST_LOCK: Mutex<()> = Mutex::new(());

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
        assert_eq!(config.lookup_capacity.get(), 2);
        assert_eq!(config.lookup_queue_capacity.get(), 64);
        assert_eq!(config.heavy_capacity.get(), 32);
        assert_eq!(config.connection_capacity.get(), 128);
        assert_eq!(config.extension_host_timeout_seconds.get(), 30);
        assert_eq!(config.max_entries.get(), 3_000_000);
        assert_eq!(config.protect_recent_days, 14);
    }

    #[test]
    fn installed_identity_snapshot_invalidates_on_source_trust_host_and_config() {
        let fixture = TestDir::new();
        let home = fixture.path().join("home");
        let repo = fixture.path().join("repo");
        let host_root = fixture.path().join("host");
        let executable = host_root.join("Scripts").join("python.exe");
        let package = host_root.join("Lib").join("site-packages").join("zmem");
        let extension = home.join("ext").join("expanders").join("example.py");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(extension.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(&executable, b"host-v1").unwrap();
        std::fs::write(package.join("host.py"), b"source-v1").unwrap();
        std::fs::write(host_root.join("pyvenv.cfg"), b"version=1").unwrap();
        std::fs::write(&extension, b"extension-v1").unwrap();
        let snapshot =
            || dependency_snapshot_for_host(&executable, &package, &home, &repo, false).unwrap();
        let original = snapshot();
        std::fs::write(&extension, b"extension-v2").unwrap();
        let source_changed = snapshot();
        assert_ne!(original, source_changed);
        std::fs::write(home.join("config.toml"), b"max_concurrency=2").unwrap();
        let config_changed = snapshot();
        assert_ne!(source_changed, config_changed);
        std::fs::write(&executable, b"host-v2").unwrap();
        let host_changed = snapshot();
        assert_ne!(config_changed, host_changed);
        let trusted =
            dependency_snapshot_for_host(&executable, &package, &home, &repo, true).unwrap();
        assert_ne!(host_changed, trusted);
        std::fs::write(package.join("host.py"), b"source-v2").unwrap();
        let package_changed = snapshot();
        assert_ne!(host_changed, package_changed);
    }

    #[test]
    fn compatible_snapshot_reuses_host_identity_until_inputs_change() {
        let fixture = TestDir::new();
        let calls = Cell::new(0);
        let validate = || {
            calls.set(calls.get() + 1);
            Ok(format!("identity-{}", calls.get()))
        };
        let first =
            cached_identity_for_snapshot(fixture.path(), false, Some(vec![1, 2, 3]), validate)
                .unwrap();
        let reused =
            cached_identity_for_snapshot(fixture.path(), false, Some(vec![1, 2, 3]), validate)
                .unwrap();
        assert_eq!(first, reused);
        assert_eq!(calls.get(), 1);
        let changed =
            cached_identity_for_snapshot(fixture.path(), false, Some(vec![1, 2, 4]), validate)
                .unwrap();
        assert_ne!(first, changed);
        assert_eq!(calls.get(), 2);
        let trusted =
            cached_identity_for_snapshot(fixture.path(), true, Some(vec![1, 2, 4]), validate)
                .unwrap();
        assert_ne!(changed, trusted);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn demanded_history_promotes_prefetched_raw_fact_without_git_object_read() {
        let fixture = TestDir::new();
        let repo_path = fixture.path().join("repo");
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&repo_path)
                .status()
                .unwrap()
                .success()
        );
        let repo = GitRepo::open(&repo_path).unwrap();
        let mut store = Store::open(&fixture.path().join("entries.db")).unwrap();
        let repo_id = store
            .register_repository(&repo.root().to_string_lossy(), false)
            .unwrap();
        let sha = "f".repeat(40);
        let fact = GitCommit {
            sha: sha.clone(),
            commit_time: 1,
            message: "prefetched before demand".to_owned(),
            changes: Vec::new(),
        };
        assert!(
            store
                .record_prefetch_batch(
                    repo_id,
                    &sha,
                    1,
                    1,
                    zmem_store::RawCommitBatch {
                        facts: &[fact],
                        parents: &BTreeMap::new(),
                    },
                    4096,
                )
                .unwrap()
        );
        let demanded = collect_raw_commits(&store, &repo, Some(repo_id), &[sha]).unwrap();
        assert_eq!(demanded[0].message, "prefetched before demand");
    }

    #[test]
    fn publication_writer_owns_atomic_trail_publication_and_stops_cleanly() {
        let fixture = TestDir::new();
        let database = fixture.path().join("entries.db");
        let mut store = Store::open(&database).unwrap();
        let repo_id = store.register_repository("repo", false).unwrap();
        let writer = PublicationWriter::start(&database).unwrap();
        let record = TrailRecord {
            id: "test-publication".to_owned(),
            repository_id: repo_id,
            head_oid: "head".to_owned(),
            attention_identity: "view".to_owned(),
            extension_identity: "extension".to_owned(),
            protocol_version: zmem_core::PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            legacy: false,
            selected_commit_count: 0,
            selected_node_count: 0,
            source_time: 0,
        };
        let parents = BTreeMap::new();
        let expansions = BTreeMap::new();
        let bad_record = TrailRecord {
            id: "bad-publication".to_owned(),
            ..record.clone()
        };
        let malformed = [serde_json::json!({})];
        assert!(
            publish_canonical_trail(
                &mut store,
                TrailPublication {
                    trail: &bad_record,
                    commits: &[],
                    parents: &parents,
                    entries: &malformed,
                    relationships: &[],
                    diagnostics: &[],
                    expansions: &expansions,
                },
            )
            .is_err()
        );
        assert!(store.trail(&bad_record.id).unwrap().is_none());
        publish_canonical_trail(
            &mut store,
            TrailPublication {
                trail: &record,
                commits: &[],
                parents: &parents,
                entries: &[],
                relationships: &[],
                diagnostics: &[],
                expansions: &expansions,
            },
        )
        .unwrap();
        assert_eq!(store.trail(&record.id).unwrap().unwrap(), record);
        assert!(store.has_published_head("repo", "head").unwrap());
        assert!(!store.has_published_head("repo", "other-head").unwrap());
        let metric = last_publication_metric().unwrap();
        assert_eq!(metric.trail_id, record.id);
        assert!(
            !finalize_canonical_store(
                &mut store,
                Finalization {
                    repo_id,
                    alias: Some("main".to_owned()),
                    active_trail_id: record.id.clone(),
                    head: record.head_oid.clone(),
                    now: 0,
                    protect_recent_seconds: 0,
                    max_entries: 1,
                }
            )
            .unwrap()
        );
        let fact = GitCommit {
            sha: "prefetched".to_owned(),
            commit_time: 1,
            message: "raw fact".to_owned(),
            changes: Vec::new(),
        };
        assert!(
            stage_prefetch_batch(
                &mut store,
                PrefetchWrite {
                    repo_id,
                    head: "prefetched".to_owned(),
                    ceiling: 1,
                    completed_count: 1,
                    facts: vec![fact],
                    parents: BTreeMap::new(),
                    staging_max_bytes: 4096,
                }
            )
            .unwrap()
        );
        assert!(store.raw_commit(repo_id, "prefetched").unwrap().is_some());
        set_prefetch_state(&mut store, repo_id, "prefetched", "ready").unwrap();
        let other_repo = match canonical_write_or_direct(
            &mut store,
            CanonicalMutation::RegisterRepository {
                path: "other-repo".to_owned(),
                trusted: true,
            },
        )
        .unwrap()
        {
            CanonicalWriteResult::RepositoryId(id) => id,
            CanonicalWriteResult::Done => unreachable!(),
        };
        assert_eq!(
            store.repository("other-repo").unwrap(),
            Some((other_repo, true))
        );
        canonical_write_or_direct(
            &mut store,
            CanonicalMutation::RecordInspections {
                parser_protocol: zmem_core::PROTOCOL_VERSION,
                records: vec![InspectionRecord {
                    oid: "inspected".to_owned(),
                    annotation_count: 2,
                    parser_diagnostics: Vec::new(),
                }],
            },
        )
        .unwrap();
        assert_eq!(
            store
                .inspection("inspected", zmem_core::PROTOCOL_VERSION)
                .unwrap()
                .unwrap()
                .annotation_count,
            2
        );
        canonical_write(
            &database,
            CanonicalMutation::InsertIndexJob {
                id: "test-job".to_owned(),
                key: "{}".to_owned(),
            },
        )
        .unwrap();
        canonical_write(
            &database,
            CanonicalMutation::SetIndexJobState {
                id: "test-job".to_owned(),
                state: "ready".to_owned(),
                failure: None,
            },
        )
        .unwrap();
        canonical_write(
            &database,
            CanonicalMutation::RemoveIndexJob {
                id: "test-job".to_owned(),
            },
        )
        .unwrap();
        writer.finish().unwrap();
        assert!(store.recover_index_jobs().unwrap().is_empty());
        assert!(PUBLICATION_QUEUE.get().unwrap().lock().unwrap().is_none());
        PublicationWriter::start(&database)
            .unwrap()
            .finish()
            .unwrap();
    }

    #[test]
    fn zero_limits_are_rejected() {
        let parsed = toml::from_str::<Config>("max_concurrency=0\nmax_entries=1");
        assert!(parsed.is_err());
        let parsed = toml::from_str::<Config>(
            "max_concurrency=1\nmax_entries=1\nextension_host_timeout_seconds=0",
        );
        assert!(parsed.is_err());
        for name in [
            "lookup_capacity",
            "lookup_queue_capacity",
            "heavy_capacity",
            "connection_capacity",
        ] {
            assert!(toml::from_str::<Config>(&format!("{name}=0")).is_err());
        }
        let mut config = Config {
            connection_capacity: NonZeroUsize::new(34).unwrap(),
            lookup_queue_capacity: NonZeroUsize::new(32).unwrap(),
            ..Config::default()
        };
        assert!(config.validate().is_err());
        config.connection_capacity = NonZeroUsize::new(35).unwrap();
        assert!(config.validate().is_ok());
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
            let mut command = Command::new("ping");
            command.args(["-n", "10", "127.0.0.1"]);
            command
        } else {
            let mut command = Command::new("sleep");
            command.arg("10");
            command
        };
        let started = Instant::now();
        let error = execute_supervised(
            &mut command,
            &vec![b'x'; 200_000],
            Duration::from_millis(50),
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(1));
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

    #[test]
    fn service_owner_lock_survives_a_probe_and_recovers_after_release() {
        let temporary = TestDir::new();
        assert!(!ServiceOwner::is_held(temporary.path()).unwrap());
        let owner = ServiceOwner::acquire(temporary.path()).unwrap();
        assert!(ServiceOwner::is_held(temporary.path()).unwrap());
        assert!(ServiceOwner::acquire(temporary.path()).is_err());
        drop(owner);
        assert!(!ServiceOwner::is_held(temporary.path()).unwrap());
        assert!(ServiceOwner::acquire(temporary.path()).is_ok());
    }

    #[test]
    fn host_permits_bound_concurrent_jobs_globally() {
        let _serial = HOST_TEST_LOCK.lock().unwrap();
        use std::sync::atomic::{AtomicUsize, Ordering};
        for limit in [1, 2, 8] {
            let active = AtomicUsize::new(0);
            let peak = AtomicUsize::new(0);
            std::thread::scope(|scope| {
                for _ in 0..16 {
                    let active = &active;
                    let peak = &peak;
                    scope.spawn(move || {
                        let _permit = HostPermit::acquire(limit, false).unwrap();
                        let now = active.fetch_add(1, Ordering::AcqRel) + 1;
                        peak.fetch_max(now, Ordering::AcqRel);
                        std::thread::sleep(Duration::from_millis(5));
                        active.fetch_sub(1, Ordering::AcqRel);
                    });
                }
            });
            assert!(peak.load(Ordering::Acquire) <= limit);
            assert_eq!(active.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn one_host_permit_remains_available_for_cached_lookup_identity() {
        let _serial = HOST_TEST_LOCK.lock().unwrap();
        let heavy = HostPermit::acquire(2, false).unwrap();
        let denied = with_request_deadline(10, || HostPermit::acquire(2, false));
        assert!(denied.is_err());
        let lookup = with_request_deadline(10, || HostPermit::acquire(2, true)).unwrap();
        drop(lookup);
        drop(heavy);
    }

    #[test]
    fn cancelled_host_wait_releases_before_its_deadline() {
        let _serial = HOST_TEST_LOCK.lock().unwrap();
        let _heavy = HostPermit::acquire(2, false).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancelled);
        let started = Instant::now();
        let waiter = std::thread::spawn(move || {
            with_request_cancellation(cancelled, || {
                with_request_deadline(5_000, || HostPermit::acquire(2, false))
            })
            .is_err()
        });
        std::thread::sleep(Duration::from_millis(50));
        signal.store(true, Ordering::Release);
        assert!(waiter.join().unwrap());
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
