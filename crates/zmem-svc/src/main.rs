use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    name = "zmem-svc",
    version,
    about = "Always-on zmem Git-history cache backend"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Add {
        path: PathBuf,
        #[arg(long)]
        timeout_ms: Option<NonZeroU64>,
        #[arg(long)]
        trust_extensions: bool,
        #[arg(long, allow_hyphen_values = true)]
        commit_limit: Option<i64>,
        #[arg(long, allow_hyphen_values = true)]
        node_limit: Option<i64>,
    },
    Query {
        path: PathBuf,
        #[arg(long)]
        timeout_ms: Option<NonZeroU64>,
        #[arg(long)]
        include_invalid: bool,
        #[arg(long = "ref")]
        reference: Option<String>,
        #[arg(long)]
        observed_oid: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        commit_limit: Option<i64>,
        #[arg(long, allow_hyphen_values = true)]
        node_limit: Option<i64>,
    },
    Check {
        path: PathBuf,
        #[arg(long)]
        timeout_ms: Option<NonZeroU64>,
        #[arg(long)]
        deep: bool,
        #[arg(long = "ref")]
        reference: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        commit_limit: Option<i64>,
        #[arg(long, allow_hyphen_values = true)]
        node_limit: Option<i64>,
    },
    Ensure {
        #[arg(long)]
        timeout_ms: Option<NonZeroU64>,
    },
    Status {
        #[arg(long)]
        timeout_ms: Option<NonZeroU64>,
    },
    Stop {
        #[arg(long)]
        timeout_ms: Option<NonZeroU64>,
    },
    JobStatus {
        id: String,
        #[arg(long)]
        timeout_ms: Option<NonZeroU64>,
    },
    JobRetry {
        id: String,
        #[arg(long)]
        timeout_ms: Option<NonZeroU64>,
    },
    Serve,
    VersionJson,
    ValidateJournal,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ServiceState {
    release_version: String,
    protocol_version: u32,
    schema_version: u32,
    pid: u32,
    port: u16,
    token: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ServiceRequest {
    token: String,
    command: String,
    path: Option<PathBuf>,
    #[serde(default)]
    trust_extensions: bool,
    #[serde(default)]
    include_invalid: bool,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    reference: Option<String>,
    #[serde(default)]
    observed_oid: Option<String>,
    #[serde(default)]
    deep: bool,
    #[serde(default)]
    commit_limit: Option<i64>,
    #[serde(default)]
    node_limit: Option<i64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    job_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ServiceResponse {
    ok: bool,
    result: Option<serde_json::Value>,
    error: Option<ServiceFailure>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ServiceFailure {
    code: String,
    message: String,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_oid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stage: Option<String>,
}

impl ServiceFailure {
    fn new(code: &str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
            retryable,
            job_id: None,
            retry_after_ms: None,
            requested_oid: None,
            stage: None,
        }
    }

    fn from_error(error: &anyhow::Error) -> Self {
        if let Some(failure) = error.downcast_ref::<Self>() {
            return failure.clone();
        }
        if let Some(publication) = error.downcast_ref::<zmem_svc::PublicationError>() {
            return match publication {
                zmem_svc::PublicationError::Busy => {
                    Self::new("busy", publication.to_string(), true)
                }
                zmem_svc::PublicationError::Stopped => {
                    Self::new("service", publication.to_string(), true)
                }
            };
        }
        let message = format!("{error:#}");
        if message.starts_with("stale ref:") {
            Self::new("stale_ref", message, false)
        } else if message.contains("service request deadline expired")
            || message.contains("service request cancelled")
        {
            Self::new("timeout", message, true)
        } else {
            Self::new("service", message, false)
        }
    }
}

impl std::fmt::Display for ServiceFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ServiceFailure {}

fn state_path() -> anyhow::Result<PathBuf> {
    Ok(zmem_svc::zmem_home()?.join("service.json"))
}

fn read_state() -> anyhow::Result<ServiceState> {
    Ok(serde_json::from_slice(&std::fs::read(state_path()?)?)?)
}

fn send_request(
    state: &ServiceState,
    request: &ServiceRequest,
    deadline: Instant,
) -> anyhow::Result<serde_json::Value> {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], state.port));
    let mut stream =
        TcpStream::connect_timeout(&address, remaining(deadline)?).map_err(transport_error)?;
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    serde_json::to_writer(&mut stream, request)?;
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    stream.write_all(b"\n").map_err(transport_error)?;
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    stream.flush().map_err(transport_error)?;
    let mut line = Vec::new();
    loop {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        let mut chunk = [0_u8; 8192];
        let count = stream.read(&mut chunk).map_err(transport_error)?;
        anyhow::ensure!(
            count != 0,
            "service closed the connection without a response"
        );
        line.extend_from_slice(&chunk[..count]);
        anyhow::ensure!(
            line.len() <= 256 * 1024 * 1024,
            "service response is too large"
        );
        if line.contains(&b'\n') {
            break;
        }
    }
    let response: ServiceResponse = serde_json::from_slice(&line)?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| ServiceFailure::new("service", "service request failed", false))
            .into());
    }
    Ok(response.result.unwrap_or(serde_json::Value::Null))
}

fn transport_error(error: std::io::Error) -> anyhow::Error {
    if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        ServiceFailure::new("timeout", "service request deadline expired", true).into()
    } else {
        error.into()
    }
}

fn remaining(deadline: Instant) -> anyhow::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            ServiceFailure::new("timeout", "service request deadline expired", true).into()
        })
}

fn deadline(timeout_ms: u64) -> Instant {
    Instant::now() + Duration::from_millis(timeout_ms)
}

fn ping(state: &ServiceState, deadline: Instant) -> bool {
    send_request(
        state,
        &ServiceRequest {
            token: state.token.clone(),
            command: "ping".to_owned(),
            path: None,
            trust_extensions: false,
            include_invalid: false,
            message: None,
            reference: None,
            observed_oid: None,
            deep: false,
            commit_limit: None,
            node_limit: None,
            timeout_ms: Some(1000),
            job_id: None,
        },
        deadline,
    )
    .is_ok()
}

fn healthy_state(deadline: Instant) -> Option<ServiceState> {
    read_state().ok().filter(|state| {
        state.protocol_version == zmem_core::PROTOCOL_VERSION && ping(state, deadline)
    })
}

fn service_status(deadline: Instant) -> serde_json::Value {
    let identity = zmem_svc::ServiceIdentity::current();
    let mut state = healthy_state(deadline);
    if state.is_none()
        && zmem_svc::zmem_home()
            .ok()
            .and_then(|home| zmem_svc::ServiceOwner::is_held(&home).ok())
            == Some(true)
    {
        while state.is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
            state = healthy_state(deadline);
        }
    }
    if let Some(state) = state {
        serde_json::json!({
            "running": true,
            "compatible": state.protocol_version == identity.protocol_version,
            "release_version": state.release_version,
            "protocol_version": state.protocol_version,
            "schema_version": state.schema_version,
            "pid": state.pid,
        })
    } else {
        serde_json::json!({
            "running": false,
            "compatible": true,
            "release_version": identity.release_version,
            "protocol_version": identity.protocol_version,
            "schema_version": identity.schema_version,
            "pid": null,
        })
    }
}

#[cfg(windows)]
fn spawn_service_process(executable: &std::path::Path) -> anyhow::Result<()> {
    let executable = executable.to_string_lossy().replace('\'', "''");
    let script =
        format!("Start-Process -FilePath '{executable}' -ArgumentList 'serve' -WindowStyle Hidden");
    ProcessCommand::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    Ok(())
}

#[cfg(unix)]
fn spawn_service_process(executable: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    let mut command = ProcessCommand::new(executable);
    command
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    Ok(())
}

fn ensure_service(deadline: Instant) -> anyhow::Result<ServiceState> {
    if let Some(state) = healthy_state(deadline) {
        return Ok(state);
    }
    let home = zmem_svc::zmem_home()?;
    let startup = zmem_svc::StartupLock::acquire(
        &home,
        remaining(deadline)?.min(Duration::from_secs(15)),
        Duration::from_secs(10),
    );
    let _startup = match startup {
        Ok(lock) => lock,
        Err(error) if remaining(deadline).is_err() => {
            return Err(ServiceFailure::new("timeout", format!("{error:#}"), true).into());
        }
        Err(error) => return Err(error),
    };
    if let Some(state) = healthy_state(deadline) {
        return Ok(state);
    }
    if zmem_svc::ServiceOwner::is_held(&home)? {
        return Err(
            ServiceFailure::new("busy", "zmem service is running but not answering", true).into(),
        );
    }
    let executable = std::env::current_exe()?;
    spawn_service_process(&executable)?;
    let startup_deadline = deadline.min(Instant::now() + Duration::from_secs(5));
    while Instant::now() < startup_deadline {
        if let Some(state) = healthy_state(deadline) {
            return Ok(state);
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(ServiceFailure::new(
        "timeout",
        "timed out starting the per-user zmem service",
        true,
    )
    .into())
}

struct RequestSpec {
    command: &'static str,
    path: Option<PathBuf>,
    trust_extensions: bool,
    include_invalid: bool,
    message: Option<String>,
    reference: Option<String>,
    observed_oid: Option<String>,
    deep: bool,
    commit_limit: Option<i64>,
    node_limit: Option<i64>,
    timeout_ms: u64,
    job_id: Option<String>,
}

fn request(spec: RequestSpec) -> anyhow::Result<serde_json::Value> {
    let deadline = deadline(spec.timeout_ms);
    let state = ensure_service(deadline)?;
    let server_budget_ms = remaining(deadline)?
        .as_millis()
        .max(1)
        .min(u128::from(u64::MAX)) as u64;
    send_request(
        &state,
        &ServiceRequest {
            token: state.token.clone(),
            command: spec.command.to_owned(),
            path: spec.path,
            trust_extensions: spec.trust_extensions,
            include_invalid: spec.include_invalid,
            message: spec.message,
            reference: spec.reference,
            observed_oid: spec.observed_oid,
            deep: spec.deep,
            commit_limit: spec.commit_limit,
            node_limit: spec.node_limit,
            timeout_ms: Some(server_budget_ms),
            job_id: spec.job_id,
        },
        deadline,
    )
}

fn write_state(path: &std::path::Path, state: &ServiceState) -> anyhow::Result<()> {
    let temporary = path.with_extension("json.tmp");
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    serde_json::to_writer(&mut file, state)?;
    file.flush()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

struct ActiveConnection(Arc<AtomicUsize>);

fn try_admit(counter: &AtomicUsize, limit: usize) -> bool {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < limit).then_some(count + 1)
        })
        .is_ok()
}

#[derive(Clone, Copy)]
struct ExecutionCapacity {
    lookup: usize,
    heavy: usize,
}

struct ConnectionContext {
    accepted_at: Instant,
    capacity: ExecutionCapacity,
}

struct WorkItem {
    stream: TcpStream,
    request: ServiceRequest,
    context: ConnectionContext,
    _connection: ActiveConnection,
    _lane: Option<ActiveConnection>,
    shared_indexed_commits: Option<usize>,
}

#[derive(Clone)]
struct WorkQueues {
    lookup: mpsc::SyncSender<WorkItem>,
    heavy: mpsc::SyncSender<WorkItem>,
    control: mpsc::SyncSender<WorkItem>,
    lookup_admitted: Arc<AtomicUsize>,
    heavy_admitted: Arc<AtomicUsize>,
    lookup_limit: usize,
    heavy_limit: usize,
}

static LOOKUP_ADMITTED_PEAK: AtomicUsize = AtomicUsize::new(0);

fn send_busy(mut stream: TcpStream, message: &str) {
    let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
    let _ = serde_json::to_writer(
        &mut stream,
        &ServiceResponse {
            ok: false,
            result: None,
            error: Some(ServiceFailure::new("busy", message, true)),
        },
    );
    let _ = stream.write_all(b"\n");
}

fn send_failure(mut stream: TcpStream, failure: ServiceFailure) {
    let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
    let _ = serde_json::to_writer(
        &mut stream,
        &ServiceResponse {
            ok: false,
            result: None,
            error: Some(failure),
        },
    );
    let _ = stream.write_all(b"\n");
}

fn shares_real_history(request: &ServiceRequest) -> bool {
    request.command == "add"
        || (request.command == "check"
            && !request.deep
            && request.message.is_some()
            && request.reference.is_none())
}

fn admit_shared_history(
    request: &ServiceRequest,
    jobs: &JobCoordinator,
    deadline: Instant,
) -> anyhow::Result<Option<(JobKey, IndexJob)>> {
    zmem_svc::with_request_deadline_at(deadline, || {
        let mut prepared = request.clone();
        if request.command == "check" {
            let policy = zmem_svc::fast_check_history_policy(
                request
                    .message
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("proposed message is required"))?,
                request.commit_limit,
                request.node_limit,
            )?;
            // Zero history needs no service-owned expansion job. Final preview
            // constructs the empty real-history projection in its own lane.
            if policy.node_limit.maximum() == Some(0) {
                return Ok(None);
            }
            prepared.node_limit = Some(policy.node_limit.as_i64());
        }
        let request = &prepared;
        let path = request
            .path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("repository path is required"))?;
        if request.command == "add" {
            let observed = job_key(request, String::new())?;
            zmem_svc::canonical_write(
                &jobs.database,
                zmem_svc::CanonicalMutation::RegisterRepository {
                    path: observed.path.to_string_lossy().into_owned(),
                    trusted: request.trust_extensions,
                },
            )?;
        }
        let (published, generation) = zmem_svc::query_published_for_indexing(
            path,
            request.reference.as_deref(),
            request.observed_oid.as_deref(),
            request.commit_limit,
            request.node_limit,
        )?;
        if published.is_some() {
            return Ok(None);
        }
        let key = job_key(request, generation)?;
        let job = jobs.admit(key.clone())?;
        Ok(Some((key, job)))
    })
}

fn await_shared_history(
    stream: &TcpStream,
    stopping: &AtomicBool,
    jobs: &JobCoordinator,
    job: &IndexJob,
    deadline: Instant,
) -> anyhow::Result<usize> {
    stream.set_read_timeout(Some(Duration::from_millis(10)))?;
    let mut byte = [0_u8; 1];
    loop {
        if stopping.load(Ordering::Acquire) {
            return Err(anyhow::anyhow!("service request cancelled"));
        }
        remaining(deadline)?;
        match stream.peek(&mut byte) {
            Ok(0) => return Err(anyhow::anyhow!("service request cancelled")),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error.into()),
        }
        match jobs.status(&job.id) {
            Some(status) if status.state == "ready" => {
                return Ok(status.indexed_commits.unwrap_or(0));
            }
            Some(status) if status.state == "failed" => {
                return Err(ServiceFailure::new(
                    "service",
                    status
                        .failure
                        .unwrap_or_else(|| "indexing failed".to_owned()),
                    false,
                )
                .into());
            }
            Some(status) if status.state == "obsolete" => {
                return Err(ServiceFailure::new(
                    "stale_ref",
                    "indexing job became obsolete",
                    false,
                )
                .into());
            }
            Some(_) => {}
            None => {
                return Err(
                    ServiceFailure::new("service", "indexing job disappeared", true).into(),
                );
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}

impl Drop for ActiveConnection {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

fn read_request(stream: &mut TcpStream, accepted_at: Instant) -> anyhow::Result<ServiceRequest> {
    let deadline = accepted_at + Duration::from_secs(1);
    let mut line = Vec::new();
    loop {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        let mut chunk = [0_u8; 8192];
        let count = stream.read(&mut chunk).map_err(transport_error)?;
        anyhow::ensure!(count != 0, "client closed request before newline");
        line.extend_from_slice(&chunk[..count]);
        anyhow::ensure!(line.len() <= 1024 * 1024, "service request is too large");
        if line.contains(&b'\n') {
            break;
        }
    }
    Ok(serde_json::from_slice(&line)?)
}

#[allow(clippy::too_many_arguments)]
fn frame_connection(
    mut stream: TcpStream,
    context: ConnectionContext,
    state: ServiceState,
    stopping: Arc<AtomicBool>,
    heavy: Arc<AtomicUsize>,
    lookups: Arc<AtomicUsize>,
    jobs: Arc<JobCoordinator>,
    queues: WorkQueues,
    connection: ActiveConnection,
) {
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let request = read_request(&mut stream, context.accepted_at);
    let Ok(request) = request else {
        handle_connection(
            stream,
            Some(request),
            context,
            state,
            stopping,
            heavy,
            lookups,
            jobs,
            None,
        );
        return;
    };
    let (sender, lane, busy_message) = if request.token != state.token {
        (None, None, "")
    } else {
        match request.command.as_str() {
            "query" => (
                Some(&queues.lookup),
                Some((&queues.lookup_admitted, queues.lookup_limit)),
                "lookup queue is full",
            ),
            "add" | "check" => (
                Some(&queues.heavy),
                Some((&queues.heavy_admitted, queues.heavy_limit)),
                "service work queue is full",
            ),
            "ping" | "stop" | "job-status" | "job-retry" => {
                (Some(&queues.control), None, "control queue is full")
            }
            _ => (None, None, ""),
        }
    };
    let Some(sender) = sender else {
        handle_connection(
            stream,
            Some(Ok(request)),
            context,
            state,
            stopping,
            heavy,
            lookups,
            jobs,
            None,
        );
        return;
    };
    let lane_guard = if let Some((counter, limit)) = lane {
        if !try_admit(counter, limit) {
            send_busy(stream, busy_message);
            return;
        }
        Some(ActiveConnection(Arc::clone(counter)))
    } else {
        None
    };
    if request.command == "query" && lane_guard.is_some() {
        LOOKUP_ADMITTED_PEAK.fetch_max(
            queues.lookup_admitted.load(Ordering::Acquire),
            Ordering::AcqRel,
        );
    }
    let mut shared_indexed_commits = None;
    if shares_real_history(&request) {
        let deadline =
            context.accepted_at + Duration::from_millis(request.timeout_ms.unwrap_or(120_000));
        let admission_stream = Arc::new(stream);
        let cancelled = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let watcher = watch_disconnect(
            &admission_stream,
            Arc::clone(&cancelled),
            Arc::clone(&finished),
            Arc::clone(&stopping),
        );
        let admission = zmem_svc::with_request_cancellation(cancelled, || {
            admit_shared_history(&request, &jobs, deadline)
        });
        finished.store(true, Ordering::Release);
        if let Some(watcher) = watcher {
            let _ = watcher.join();
        }
        stream = Arc::try_unwrap(admission_stream).expect("admission socket watcher has finished");
        match admission {
            Ok(Some((_key, job))) => {
                match await_shared_history(&stream, &stopping, &jobs, &job, deadline) {
                    Ok(indexed) => shared_indexed_commits = Some(indexed),
                    Err(error) => {
                        send_failure(stream, ServiceFailure::from_error(&error));
                        return;
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                send_failure(stream, ServiceFailure::from_error(&error));
                return;
            }
        }
    }
    let item = WorkItem {
        stream,
        request,
        context,
        _connection: connection,
        _lane: lane_guard,
        shared_indexed_commits,
    };
    if let Err(error) = sender.try_send(item) {
        let item = match error {
            mpsc::TrySendError::Full(item) | mpsc::TrySendError::Disconnected(item) => item,
        };
        send_busy(item.stream, busy_message);
    }
}

fn watch_disconnect(
    stream: &Arc<TcpStream>,
    cancelled: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
) -> Option<thread::JoinHandle<()>> {
    let watcher = Arc::clone(stream);
    watcher
        .set_read_timeout(Some(Duration::from_millis(10)))
        .ok()?;
    Some(thread::spawn(move || {
        let mut byte = [0_u8; 1];
        while !finished.load(Ordering::Acquire) {
            if stopping.load(Ordering::Acquire) {
                cancelled.store(true, Ordering::Release);
                break;
            }
            match watcher.peek(&mut byte) {
                Ok(0) => {
                    if !finished.load(Ordering::Acquire) {
                        cancelled.store(true, Ordering::Release);
                    }
                    break;
                }
                Ok(_) => thread::sleep(Duration::from_millis(10)),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => {
                    if !finished.load(Ordering::Acquire) {
                        cancelled.store(true, Ordering::Release);
                    }
                    break;
                }
            }
        }
    }))
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq)]
struct JobKey {
    path: PathBuf,
    reference: Option<String>,
    observed_oid: String,
    commit_limit: Option<i64>,
    node_limit: Option<i64>,
    generation: String,
    // Observation ownership is not job identity. It must not change the
    // serialized key that protects legacy failed jobs from implicit retries.
    #[serde(skip)]
    route: String,
}

impl PartialEq for JobKey {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.reference == other.reference
            && self.observed_oid == other.observed_oid
            && self.commit_limit == other.commit_limit
            && self.node_limit == other.node_limit
            && self.generation == other.generation
    }
}
impl std::hash::Hash for JobKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(
            &(
                &self.path,
                &self.reference,
                &self.observed_oid,
                self.commit_limit,
                self.node_limit,
                &self.generation,
            ),
            state,
        );
    }
}

#[derive(Clone, Debug)]
struct IndexJob {
    id: String,
    state: String,
    failure: Option<String>,
    stage_started: Instant,
    queue_wait_ms: Option<u64>,
    work_ms: Option<u64>,
    indexed_commits: Option<usize>,
}

fn elapsed_ms(since: Instant) -> u64 {
    since.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[derive(Default)]
struct JobQueue {
    jobs: HashMap<JobKey, IndexJob>,
    by_id: HashMap<String, JobKey>,
    ready: VecDeque<JobKey>,
}

struct JobCoordinator {
    queue: Mutex<JobQueue>,
    wake: Condvar,
    database: PathBuf,
    demand_jobs: Arc<AtomicUsize>,
    completed_foreground: Arc<AtomicUsize>,
}

#[derive(Default)]
struct PrefetchQueue {
    ready: VecDeque<(PathBuf, String)>,
    known: HashSet<(PathBuf, String)>,
}

#[derive(Default)]
struct PrefetchCoordinator {
    queue: Mutex<PrefetchQueue>,
    wake: Condvar,
}

type PrefetchTarget = (usize, Vec<(String, usize)>, String);

impl PrefetchCoordinator {
    fn admit(&self, path: PathBuf, head: String) {
        let mut queue = self.queue.lock().expect("prefetch queue lock poisoned");
        let key = (path, head);
        if queue.known.len() < 32 && queue.known.insert(key.clone()) {
            queue.ready.push_back(key);
            self.wake.notify_one();
        }
    }

    fn finish_turn(&self, key: (PathBuf, String), more: bool) {
        let mut queue = self.queue.lock().expect("prefetch queue lock poisoned");
        if more {
            queue.ready.push_back(key);
        } else {
            queue.known.remove(&key);
        }
    }

    fn worker(
        &self,
        stopping: Arc<AtomicBool>,
        demand_jobs: &AtomicUsize,
        completed_foreground: &AtomicUsize,
    ) {
        let ceiling = zmem_svc::Config::load(&zmem_svc::zmem_home().unwrap().join("config.toml"))
            .unwrap()
            .background_commit_limit as usize;
        let mut sessions: HashMap<(PathBuf, String), (usize, zmem_svc::PrefetchSession)> =
            HashMap::new();
        let mut attempted = HashMap::new();
        let mut capacity_checked = Instant::now();
        let mut capacity_revision = 0;
        while !stopping.load(Ordering::Acquire) {
            let mut targets: std::collections::BTreeMap<(PathBuf, String), PrefetchTarget> =
                std::collections::BTreeMap::new();
            if capacity_checked.elapsed() >= Duration::from_secs(5) {
                let database = zmem_svc::zmem_home().unwrap().join("db").join("entries.db");
                if let Ok(store) = zmem_store::Store::open_readonly(&database)
                    && let Ok(metrics) = store.prefetch_metrics()
                {
                    capacity_revision = metrics.reused_bytes;
                }
                capacity_checked = Instant::now();
            }
            for (route, target) in zmem_svc::demand::targets(ceiling) {
                let revision = format!(
                    "{}:{}:{}:{capacity_revision};",
                    route.route,
                    route.generation,
                    route.revision()
                );
                let entry = targets
                    .entry((PathBuf::from(route.repository), route.oid))
                    .or_default();
                entry.0 = entry.0.max(target);
                entry.1.push((route.route, target));
                entry.2.push_str(&revision);
            }
            attempted.retain(|key, _| targets.contains_key(key));
            for (key, (target, _, revision)) in &targets {
                if attempted.get(key) != Some(&(*target, revision.clone())) {
                    self.admit(key.0.clone(), key.1.clone());
                }
            }
            let next = self
                .queue
                .lock()
                .expect("prefetch queue lock poisoned")
                .ready
                .pop_front();
            let Some(key) = next else {
                thread::sleep(Duration::from_millis(100));
                continue;
            };
            let Some((target, references, revision)) = targets.get(&key) else {
                if let Some((_, mut session)) = sessions.remove(&key) {
                    let _ = session.pause();
                }
                self.finish_turn(key, false);
                continue;
            };
            let turn = zmem_svc::with_request_cancellation(Arc::clone(&stopping), || {
                let session = match sessions.remove(&key) {
                    Some((old_target, session)) if old_target == *target => Some(session),
                    _ => zmem_svc::PrefetchSession::start_target(
                        &key.0,
                        &key.1,
                        *target,
                        references
                            .iter()
                            .map(|(reference, _)| reference.clone())
                            .collect(),
                    )?,
                };
                let Some(mut session) = session else {
                    return Ok((zmem_svc::PrefetchTurn::Done, None));
                };
                session.set_route_targets(references.clone());
                let progress = session.advance(&stopping, demand_jobs, completed_foreground)?;
                Ok::<_, anyhow::Error>((progress, Some(session)))
            });
            match turn {
                Ok((zmem_svc::PrefetchTurn::More, Some(session))) => {
                    sessions.insert(key.clone(), (*target, session));
                    self.finish_turn(key, true);
                }
                other => {
                    if let Err(error) = other {
                        eprintln!("zmem background prefetch failed: {error:#}");
                    }
                    attempted.insert(key.clone(), (*target, revision.clone()));
                    self.finish_turn(key, false);
                }
            }
        }
    }
}

impl JobCoordinator {
    fn new(database: PathBuf) -> anyhow::Result<Self> {
        let mut store = zmem_store::Store::open_writable_existing(&database)?;
        let mut queue = JobQueue::default();
        for persisted in store.recover_index_jobs()? {
            let key: JobKey = serde_json::from_str(&persisted.job_key)?;
            let job = IndexJob {
                id: persisted.id,
                state: persisted.state,
                failure: persisted.failure,
                stage_started: Instant::now(),
                queue_wait_ms: None,
                work_ms: None,
                indexed_commits: None,
            };
            queue.by_id.insert(job.id.clone(), key.clone());
            queue.jobs.insert(key, job);
        }
        Ok(Self {
            queue: Mutex::new(queue),
            wake: Condvar::new(),
            database,
            demand_jobs: Arc::new(AtomicUsize::new(0)),
            completed_foreground: Arc::new(AtomicUsize::new(0)),
        })
    }

    fn persist_state(&self, id: &str, state: &str, failure: Option<&str>) -> anyhow::Result<()> {
        zmem_svc::canonical_write(
            &self.database,
            zmem_svc::CanonicalMutation::SetIndexJobState {
                id: id.to_owned(),
                state: state.to_owned(),
                failure: failure.map(str::to_owned),
            },
        )?;
        Ok(())
    }

    fn refine_identity(&self, old: &JobKey, identity: &str) -> anyhow::Result<JobKey> {
        let store = zmem_store::Store::open_readonly(&self.database)?;
        let trusted = store
            .repository(&old.path.to_string_lossy())?
            .is_some_and(|(_, trusted)| trusted);
        let generation = format!("{identity}:trusted={trusted}");
        if old.generation == generation {
            return Ok(old.clone());
        }
        let mut refined = old.clone();
        refined.generation = generation;
        let mut queue = self.queue.lock().expect("job queue lock poisoned");
        if queue.jobs.contains_key(&refined) {
            return Ok(old.clone());
        }
        let job = queue.jobs.get(old).expect("running job exists").clone();
        zmem_svc::canonical_write(
            &self.database,
            zmem_svc::CanonicalMutation::UpdateIndexJobKey {
                id: job.id.clone(),
                key: serde_json::to_string(&refined)?,
            },
        )?;
        queue.jobs.remove(old);
        queue.by_id.insert(job.id.clone(), refined.clone());
        queue.jobs.insert(refined.clone(), job);
        Ok(refined)
    }

    fn admit(&self, key: JobKey) -> anyhow::Result<IndexJob> {
        let mut queue = self.queue.lock().expect("job queue lock poisoned");
        if let Some(job) = queue.jobs.get(&key) {
            if job.state != "ready" {
                return Ok(job.clone());
            }
            // A ready job can have published an older extension generation,
            // or its trail may have since been evicted. The preceding exact
            // snapshot lookup proved that it cannot satisfy this request.
            let old_id = job.id.clone();
            zmem_svc::canonical_write(
                &self.database,
                zmem_svc::CanonicalMutation::RemoveIndexJob { id: old_id.clone() },
            )?;
            queue.jobs.remove(&key);
            queue.by_id.remove(&old_id);
        }
        if queue.ready.len() >= 32 {
            return Err(ServiceFailure::new("busy", "indexing queue is full", true).into());
        }
        if queue.jobs.len() >= 256 {
            if let Some(old) = queue.jobs.iter().find_map(|(key, job)| {
                matches!(job.state.as_str(), "ready" | "obsolete").then_some(key.clone())
            }) {
                if let Some(job) = queue.jobs.remove(&old) {
                    zmem_svc::canonical_write(
                        &self.database,
                        zmem_svc::CanonicalMutation::RemoveIndexJob { id: job.id.clone() },
                    )?;
                    queue.by_id.remove(&job.id);
                }
            } else {
                return Err(
                    ServiceFailure::new("busy", "indexing job capacity is full", true).into(),
                );
            }
        }
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random)
            .map_err(|error| anyhow::anyhow!("could not generate job ID: {error}"))?;
        let job = IndexJob {
            id: format!(
                "job-{}",
                random
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            ),
            state: "queued".to_owned(),
            failure: None,
            stage_started: Instant::now(),
            queue_wait_ms: None,
            work_ms: None,
            indexed_commits: None,
        };
        zmem_svc::canonical_write(
            &self.database,
            zmem_svc::CanonicalMutation::InsertIndexJob {
                id: job.id.clone(),
                key: serde_json::to_string(&key)?,
            },
        )?;
        queue.by_id.insert(job.id.clone(), key.clone());
        queue.ready.push_back(key.clone());
        queue.jobs.insert(key, job.clone());
        self.demand_jobs.fetch_add(1, Ordering::AcqRel);
        self.wake.notify_one();
        Ok(job)
    }

    fn status(&self, id: &str) -> Option<IndexJob> {
        let queue = self.queue.lock().expect("job queue lock poisoned");
        queue
            .by_id
            .get(id)
            .and_then(|key| queue.jobs.get(key))
            .cloned()
    }

    fn pending_before_identity(&self, key: &JobKey) -> Option<IndexJob> {
        let queue = self.queue.lock().expect("job queue lock poisoned");
        queue.jobs.iter().find_map(|(candidate, job)| {
            (matches!(job.state.as_str(), "queued" | "running")
                && candidate.path == key.path
                && candidate.reference == key.reference
                && candidate.observed_oid == key.observed_oid
                && candidate.commit_limit == key.commit_limit
                && candidate.node_limit == key.node_limit
                && candidate.generation == key.generation)
                .then(|| job.clone())
        })
    }

    fn queue_depth(&self) -> usize {
        self.queue
            .lock()
            .expect("job queue lock poisoned")
            .ready
            .len()
    }

    fn retry(&self, id: &str) -> anyhow::Result<IndexJob> {
        let mut queue = self.queue.lock().expect("job queue lock poisoned");
        let key = queue
            .by_id
            .get(id)
            .cloned()
            .ok_or_else(|| ServiceFailure::new("request", "unknown indexing job", false))?;
        let job = queue.jobs.get(&key).expect("indexed job exists");
        if job.state != "failed" {
            return Err(ServiceFailure::new(
                "request",
                "only failed indexing jobs can be retried",
                false,
            )
            .into());
        }
        if queue.ready.len() >= 32 {
            return Err(ServiceFailure::new("busy", "indexing queue is full", true).into());
        }
        self.persist_state(id, "queued", None)?;
        let job = queue.jobs.get_mut(&key).expect("indexed job exists");
        job.state = "queued".to_owned();
        job.failure = None;
        job.stage_started = Instant::now();
        job.queue_wait_ms = None;
        job.work_ms = None;
        job.indexed_commits = None;
        let result = job.clone();
        queue.ready.push_back(key);
        self.demand_jobs.fetch_add(1, Ordering::AcqRel);
        self.wake.notify_one();
        Ok(result)
    }

    fn worker(&self, stopping: Arc<AtomicBool>, _prefetch: &PrefetchCoordinator) {
        loop {
            let mut key = {
                let mut queue = self.queue.lock().expect("job queue lock poisoned");
                loop {
                    if stopping.load(Ordering::Acquire) {
                        return;
                    }
                    if let Some(key) = queue.ready.pop_front() {
                        let job = queue.jobs.get_mut(&key).expect("queued job exists");
                        job.queue_wait_ms = Some(elapsed_ms(job.stage_started));
                        job.stage_started = Instant::now();
                        job.state = "running".to_owned();
                        break key;
                    }
                    queue = self.wake.wait(queue).expect("job queue lock poisoned");
                }
            };
            let id = self.status_for_key(&key).expect("running job exists").id;
            if let Err(error) = self.persist_state(&id, "running", None) {
                let mut queue = self.queue.lock().expect("job queue lock poisoned");
                if let Some(job) = queue.jobs.get_mut(&key) {
                    job.work_ms = Some(elapsed_ms(job.stage_started));
                    job.stage_started = Instant::now();
                    job.state = "failed".to_owned();
                    job.failure = Some(format!("could not persist indexing state: {error:#}"));
                }
                self.demand_jobs.fetch_sub(1, Ordering::AcqRel);
                self.completed_foreground.fetch_add(1, Ordering::AcqRel);
                continue;
            }
            let result = zmem_svc::with_request_cancellation(Arc::clone(&stopping), || {
                zmem_svc::with_request_deadline(120_000, || {
                    zmem_svc::sync_repository_with_ref_attention(
                        &key.path,
                        None,
                        key.reference.as_deref(),
                        Some(&key.observed_oid),
                        key.commit_limit,
                        key.node_limit,
                    )
                })
            });
            let result = result.and_then(|mut sync| {
                key = self.refine_identity(&key, &sync.summary.trail.extension_identity)?;
                if !key.route.is_empty() {
                    sync.summary.route.clone_from(&key.route);
                }
                Ok(sync)
            });
            if let Ok(sync) = &result {
                zmem_svc::demand::record(&sync.summary);
            }
            let failure = result.as_ref().err().map(|error| format!("{error:#}"));
            if let Err(error) = self.persist_state(
                &id,
                if failure.is_some() { "failed" } else { "ready" },
                failure.as_deref(),
            ) {
                eprintln!("could not persist indexing job {id}: {error:#}");
            }
            let mut queue = self.queue.lock().expect("job queue lock poisoned");
            if let Some(job) = queue.jobs.get_mut(&key) {
                job.work_ms = Some(elapsed_ms(job.stage_started));
                job.stage_started = Instant::now();
                match result {
                    Ok(sync) => {
                        job.state = "ready".to_owned();
                        job.indexed_commits = Some(sync.summary.indexed_commits);
                    }
                    Err(error) => {
                        job.state = "failed".to_owned();
                        job.failure = Some(format!("{error:#}"));
                    }
                }
            }
            self.demand_jobs.fetch_sub(1, Ordering::AcqRel);
            self.completed_foreground.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn status_for_key(&self, key: &JobKey) -> Option<IndexJob> {
        self.queue
            .lock()
            .expect("job queue lock poisoned")
            .jobs
            .get(key)
            .cloned()
    }
}

fn job_key(request: &ServiceRequest, generation: String) -> anyhow::Result<JobKey> {
    let path = request
        .path
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("repository path is required"))?;
    let repo = zmem_core::GitRepo::open(path)?;
    let reference = request.reference.as_deref().unwrap_or("HEAD");
    let observed_oid = request
        .observed_oid
        .as_deref()
        .map(str::to_owned)
        .unwrap_or(repo.resolve(reference)?);
    let resolved = repo.resolve_observed(reference, &observed_oid)?;
    Ok(JobKey {
        path: repo.root().to_path_buf(),
        reference: request.reference.clone(),
        observed_oid,
        commit_limit: request.commit_limit,
        node_limit: request.node_limit,
        generation,
        route: resolved.route,
    })
}

fn deferred_job_response(key: JobKey, job: IndexJob) -> ServiceResponse {
    let mut failure = if job.state == "failed" {
        ServiceFailure::new(
            "service",
            job.failure.unwrap_or_else(|| "indexing failed".to_owned()),
            false,
        )
    } else {
        ServiceFailure::new("not_ready", "requested history is indexing", true)
    };
    failure.job_id = Some(job.id);
    failure.requested_oid = Some(key.observed_oid);
    failure.stage = Some(job.state.to_owned());
    failure.retry_after_ms = (job.state != "failed").then_some(250);
    ServiceResponse {
        ok: false,
        result: None,
        error: Some(failure),
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_connection(
    mut stream: TcpStream,
    prepared_request: Option<anyhow::Result<ServiceRequest>>,
    context: ConnectionContext,
    state: ServiceState,
    stopping: Arc<AtomicBool>,
    heavy: Arc<AtomicUsize>,
    lookups: Arc<AtomicUsize>,
    jobs: Arc<JobCoordinator>,
    shared_indexed_commits: Option<usize>,
) {
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let request =
        prepared_request.unwrap_or_else(|| read_request(&mut stream, context.accepted_at));
    let _lookup = if request
        .as_ref()
        .is_ok_and(|request| request.token == state.token && request.command == "query")
    {
        if !try_admit(&lookups, context.capacity.lookup) {
            let response = ServiceResponse {
                ok: false,
                result: None,
                error: Some(ServiceFailure::new("busy", "lookup capacity is full", true)),
            };
            let _ = serde_json::to_writer(&mut stream, &response);
            let _ = stream.write_all(b"\n");
            return;
        }
        Some(ActiveConnection(lookups))
    } else {
        None
    };
    let _heavy = if request.as_ref().is_ok_and(|request| {
        request.token == state.token && matches!(request.command.as_str(), "add" | "check")
    }) {
        if !try_admit(&heavy, context.capacity.heavy) {
            let response = ServiceResponse {
                ok: false,
                result: None,
                error: Some(ServiceFailure::new(
                    "busy",
                    "service work capacity is full",
                    true,
                )),
            };
            let _ = serde_json::to_writer(&mut stream, &response);
            let _ = stream.write_all(b"\n");
            return;
        }
        Some(ActiveConnection(heavy))
    } else {
        None
    };
    let server_deadline = context.accepted_at
        + Duration::from_millis(
            request
                .as_ref()
                .ok()
                .and_then(|request| request.timeout_ms)
                .unwrap_or(120_000),
        );
    let stream = Arc::new(stream);
    let cancelled = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let monitor = if request.as_ref().is_ok_and(|request| {
        request.token == state.token
            && matches!(request.command.as_str(), "query" | "add" | "check")
    }) {
        watch_disconnect(
            &stream,
            Arc::clone(&cancelled),
            Arc::clone(&finished),
            Arc::clone(&stopping),
        )
    } else {
        None
    };
    let mut completed_demand = None;
    let response = zmem_svc::with_request_cancellation(cancelled, || match request {
        Ok(_) if remaining(server_deadline).is_err() => ServiceResponse {
            ok: false,
            result: None,
            error: Some(ServiceFailure::new(
                "timeout",
                "service request deadline expired",
                true,
            )),
        },
        Ok(request) if request.token != state.token => ServiceResponse {
            ok: false,
            result: None,
            error: Some(ServiceFailure::new(
                "unauthorized",
                "unauthorized local client",
                false,
            )),
        },
        Ok(request) => match request.command.as_str() {
            "ping" => {
                #[cfg(debug_assertions)]
                if let Some(delay) = std::env::var("ZMEM_TEST_PING_DELAY_MS")
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                {
                    thread::sleep(Duration::from_millis(delay.min(2_000)));
                }
                ServiceResponse {
                    ok: true,
                    result: Some(serde_json::json!({"pid": state.pid})),
                    error: None,
                }
            }
            "stop" => {
                stopping.store(true, Ordering::Release);
                ServiceResponse {
                    ok: true,
                    result: Some(serde_json::json!({"stopped": true})),
                    error: None,
                }
            }
            "query" => {
                let pending = if jobs.demand_jobs.load(Ordering::Acquire) > 0 {
                    zmem_svc::with_request_deadline_at(server_deadline, || {
                        let mut key = job_key(&request, String::new())?;
                        let database = zmem_svc::zmem_home()?.join("db").join("entries.db");
                        let store = zmem_store::Store::open_readonly(&database)?;
                        store.set_request_deadline(server_deadline)?;
                        if store
                            .has_published_head(&key.path.to_string_lossy(), &key.observed_oid)?
                        {
                            return Ok(None);
                        }
                        let trusted = store
                            .repository(&key.path.to_string_lossy())?
                            .is_some_and(|(_, trusted)| trusted);
                        key.generation =
                            zmem_svc::pending_repository_generation(&key.path, trusted)?;
                        Ok::<_, anyhow::Error>(
                            jobs.pending_before_identity(&key).map(|job| (key, job)),
                        )
                    })
                } else {
                    Ok(None)
                };
                match pending {
                    Ok(Some((key, job))) => deferred_job_response(key, job),
                    Err(error) => ServiceResponse {
                        ok: false,
                        result: None,
                        error: Some(ServiceFailure::from_error(&error)),
                    },
                    Ok(None) => {
                        let outcome = zmem_svc::with_request_deadline_at(server_deadline, || {
                            request
                                .path
                                .as_deref()
                                .ok_or_else(|| anyhow::anyhow!("repository path is required"))
                                .and_then(|path| {
                                    zmem_svc::query_published_with_ref_attention(
                                        path,
                                        request.reference.as_deref(),
                                        request.observed_oid.as_deref(),
                                        request.commit_limit,
                                        request.node_limit,
                                        request.include_invalid,
                                    )
                                })
                        });
                        match outcome {
                            Ok((Some(sync), _)) => {
                                completed_demand = Some(sync.summary.clone());
                                ServiceResponse {
                                    ok: true,
                                    result: Some(serde_json::json!({
                                        "summary": sync.summary, "entries": sync.entries,
                                        "relationships": sync.relationships, "diagnostics": sync.diagnostics,
                                    })),
                                    error: None,
                                }
                            }
                            Ok((None, generation)) => {
                                match zmem_svc::with_request_deadline_at(server_deadline, || {
                                    let key = job_key(&request, generation)?;
                                    let job = jobs.admit(key.clone())?;
                                    Ok((key, job))
                                }) {
                                    Ok((key, job)) => deferred_job_response(key, job),
                                    Err(error) => ServiceResponse {
                                        ok: false,
                                        result: None,
                                        error: Some(ServiceFailure::from_error(&error)),
                                    },
                                }
                            }
                            Err(error) => ServiceResponse {
                                ok: false,
                                result: None,
                                error: Some(ServiceFailure::from_error(&error)),
                            },
                        }
                    }
                }
            }
            "job-status" => {
                let result = request.job_id.as_deref().and_then(|id| jobs.status(id));
                match result {
                    Some(job) => {
                        let queue_wait_ms = if job.state == "queued" {
                            Some(elapsed_ms(job.stage_started))
                        } else {
                            job.queue_wait_ms
                        };
                        let work_ms = if job.state == "running" {
                            Some(elapsed_ms(job.stage_started))
                        } else {
                            job.work_ms
                        };
                        ServiceResponse {
                            ok: true,
                            result: Some(serde_json::json!({
                                "job_id":job.id,"state":job.state,"error":job.failure,
                                "queue_depth":jobs.queue_depth(),"queue_wait_ms":queue_wait_ms,
                                "work_ms":work_ms,
                                "last_publication":zmem_svc::last_publication_metric(),
                                "lookup_admitted_peak":LOOKUP_ADMITTED_PEAK.load(Ordering::Acquire),
                                "skipped_observations":zmem_svc::demand::skipped(),
                                "advisory_write_failures":zmem_svc::advisory_write_failures(),
                            })),
                            error: None,
                        }
                    }
                    None => ServiceResponse {
                        ok: false,
                        result: None,
                        error: Some(ServiceFailure::new(
                            "request",
                            "unknown indexing job",
                            false,
                        )),
                    },
                }
            }
            "job-retry" => match zmem_svc::with_request_deadline_at(server_deadline, || {
                request
                    .job_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("job ID is required"))
                    .and_then(|id| jobs.retry(id))
            }) {
                Ok(job) => ServiceResponse {
                    ok: true,
                    result: Some(serde_json::json!({"job_id":job.id,"state":job.state})),
                    error: None,
                },
                Err(error) => ServiceResponse {
                    ok: false,
                    result: None,
                    error: Some(ServiceFailure::from_error(&error)),
                },
            },
            "add" => {
                let outcome = zmem_svc::with_request_deadline_at(server_deadline, || {
                    request
                        .path
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("repository path is required"))
                        .and_then(|path| {
                            zmem_svc::sync_repository_with_ref_attention(
                                path,
                                Some(request.trust_extensions),
                                request.reference.as_deref(),
                                request.observed_oid.as_deref(),
                                request.commit_limit,
                                request.node_limit,
                            )
                        })
                });
                match outcome {
                    Ok(mut sync) => {
                        if shared_indexed_commits.is_none() {
                            completed_demand = Some(sync.summary.clone());
                        }
                        if let Some(indexed) = shared_indexed_commits {
                            sync.summary.indexed_commits = indexed;
                        }
                        if !request.include_invalid {
                            sync.entries
                                .retain(|row| row["valid"].as_bool().unwrap_or(false));
                        }
                        let result = serde_json::to_value(sync.summary);
                        match result {
                            Ok(result) => ServiceResponse {
                                ok: true,
                                result: Some(result),
                                error: None,
                            },
                            Err(error) => ServiceResponse {
                                ok: false,
                                result: None,
                                error: Some(ServiceFailure::from_error(&error.into())),
                            },
                        }
                    }
                    Err(error) => ServiceResponse {
                        ok: false,
                        result: None,
                        error: Some(ServiceFailure::from_error(&error)),
                    },
                }
            }
            "check" => {
                let outcome = zmem_svc::with_request_deadline_at(server_deadline, || {
                    request
                        .path
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("repository path is required"))
                        .and_then(|path| {
                            zmem_svc::check_repository_with_attention(
                                path,
                                request.message.as_deref(),
                                request.reference.as_deref(),
                                request.deep,
                                request.commit_limit,
                                request.node_limit,
                            )
                        })
                });
                match outcome {
                    Ok(check) => {
                        if shared_indexed_commits.is_none()
                            && let Some(summary) = &check.demand_summary
                        {
                            completed_demand = Some(summary.clone());
                        }
                        match serde_json::to_value(check) {
                            Ok(result) => ServiceResponse {
                                ok: true,
                                result: Some(result),
                                error: None,
                            },
                            Err(error) => ServiceResponse {
                                ok: false,
                                result: None,
                                error: Some(ServiceFailure::from_error(&error.into())),
                            },
                        }
                    }
                    Err(error) => ServiceResponse {
                        ok: false,
                        result: None,
                        error: Some(ServiceFailure::from_error(&error)),
                    },
                }
            }
            _ => ServiceResponse {
                ok: false,
                result: None,
                error: Some(ServiceFailure::new(
                    "request",
                    "unknown service command",
                    false,
                )),
            },
        },
        Err(error) => ServiceResponse {
            ok: false,
            result: None,
            error: Some(if error.downcast_ref::<ServiceFailure>().is_some() {
                ServiceFailure::from_error(&error)
            } else {
                ServiceFailure::new(
                    "request",
                    format!("invalid service request: {error:#}"),
                    false,
                )
            }),
        },
    });
    finished.store(true, Ordering::Release);
    if let Some(monitor) = monitor {
        let _ = monitor.join();
    }
    let write_budget = remaining(server_deadline).unwrap_or_else(|_| {
        if response
            .error
            .as_ref()
            .is_some_and(|failure| failure.code == "timeout")
        {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(1)
        }
    });
    let _ = stream.set_write_timeout(Some(write_budget));
    let mut writer = stream.as_ref();
    let delivered = serde_json::to_writer(&mut writer, &response).is_ok()
        && writer.write_all(b"\n").is_ok()
        && writer.flush().is_ok();
    if delivered
        && response.ok
        && Instant::now() < server_deadline
        && let Some(summary) = completed_demand
    {
        zmem_svc::demand::record(&summary);
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_execution_workers(
    count: usize,
    receiver: mpsc::Receiver<WorkItem>,
    state: &ServiceState,
    stopping: &Arc<AtomicBool>,
    heavy: &Arc<AtomicUsize>,
    lookups: &Arc<AtomicUsize>,
    jobs: &Arc<JobCoordinator>,
) -> Vec<thread::JoinHandle<()>> {
    let receiver = Arc::new(Mutex::new(receiver));
    (0..count)
        .map(|_| {
            let receiver = Arc::clone(&receiver);
            let state = state.clone();
            let stopping = Arc::clone(stopping);
            let heavy = Arc::clone(heavy);
            let lookups = Arc::clone(lookups);
            let jobs = Arc::clone(jobs);
            thread::spawn(move || {
                loop {
                    let item = receiver.lock().expect("work queue lock poisoned").recv();
                    let Ok(item) = item else { break };
                    let WorkItem {
                        stream,
                        request,
                        context,
                        _connection,
                        _lane,
                        shared_indexed_commits,
                    } = item;
                    handle_connection(
                        stream,
                        Some(Ok(request)),
                        context,
                        state.clone(),
                        Arc::clone(&stopping),
                        Arc::clone(&heavy),
                        Arc::clone(&lookups),
                        Arc::clone(&jobs),
                        shared_indexed_commits,
                    );
                }
            })
        })
        .collect()
}

fn serve() -> anyhow::Result<()> {
    let home = zmem_svc::zmem_home()?;
    let _owner = zmem_svc::ServiceOwner::acquire(&home)?;
    let config = zmem_svc::Config::load(&home.join("config.toml"))?;
    let database = home.join("db").join("entries.db");
    let demand_routes = {
        let mut store = zmem_store::Store::open(&database)?;
        store.set_staging_protection(i64::from(config.protect_recent_days) * 86_400);
        store.reconcile_prefetch_staging()?;
        store.load_route_demand(zmem_svc::demand::wall_time())?
    };
    // Recover interrupted job state before the runtime writer takes sole
    // ownership of mutable canonical data.
    let jobs = Arc::new(JobCoordinator::new(database.clone())?);
    let publication_writer = zmem_svc::PublicationWriter::start(&database)?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let mut token_bytes = [0_u8; 32];
    getrandom::fill(&mut token_bytes)
        .map_err(|error| anyhow::anyhow!("could not generate service token: {error}"))?;
    let token = token_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let identity = zmem_svc::ServiceIdentity::current();
    let state = ServiceState {
        release_version: identity.release_version.to_owned(),
        protocol_version: identity.protocol_version,
        schema_version: identity.schema_version,
        pid: std::process::id(),
        port: listener.local_addr()?.port(),
        token,
    };
    let state_path = state_path()?;
    if let Some(parent) = state_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_state(&state_path, &state)?;

    listener.set_nonblocking(true)?;
    let stopping = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let heavy = Arc::new(AtomicUsize::new(0));
    let lookups = Arc::new(AtomicUsize::new(0));
    let (lookup_sender, lookup_receiver) = mpsc::sync_channel(config.lookup_queue_capacity.get());
    let (heavy_sender, heavy_receiver) = mpsc::sync_channel(32);
    let (control_sender, control_receiver) = mpsc::sync_channel(16);
    let queues = WorkQueues {
        lookup: lookup_sender,
        heavy: heavy_sender,
        control: control_sender,
        lookup_admitted: Arc::new(AtomicUsize::new(0)),
        heavy_admitted: Arc::new(AtomicUsize::new(0)),
        lookup_limit: config.lookup_capacity.get() + config.lookup_queue_capacity.get(),
        heavy_limit: config.heavy_capacity.get(),
    };
    let mut execution_workers = spawn_execution_workers(
        config.lookup_capacity.get(),
        lookup_receiver,
        &state,
        &stopping,
        &heavy,
        &lookups,
        &jobs,
    );
    execution_workers.extend(spawn_execution_workers(
        config
            .heavy_capacity
            .get()
            .min(config.max_concurrency.get()),
        heavy_receiver,
        &state,
        &stopping,
        &heavy,
        &lookups,
        &jobs,
    ));
    execution_workers.extend(spawn_execution_workers(
        2,
        control_receiver,
        &state,
        &stopping,
        &heavy,
        &lookups,
        &jobs,
    ));
    let prefetch = Arc::new(PrefetchCoordinator::default());
    zmem_svc::demand::initialize(demand_routes);
    let worker_jobs = Arc::clone(&jobs);
    let worker_stopping = Arc::clone(&stopping);
    let worker_prefetch = Arc::clone(&prefetch);
    let worker = thread::spawn(move || worker_jobs.worker(worker_stopping, &worker_prefetch));
    let background_prefetch = Arc::clone(&prefetch);
    let background_stopping = Arc::clone(&stopping);
    let background_demand = Arc::clone(&jobs.demand_jobs);
    let background_completed = Arc::clone(&jobs.completed_foreground);
    let background = thread::spawn(move || {
        background_prefetch.worker(
            background_stopping,
            &background_demand,
            &background_completed,
        )
    });
    let mut accept_error = None;
    while !stopping.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let accepted_at = Instant::now();
                if !try_admit(&active, config.connection_capacity.get()) {
                    send_busy(stream, "service connection capacity is full");
                    continue;
                }
                let current = Arc::clone(&active);
                let state = state.clone();
                let stopping = Arc::clone(&stopping);
                let heavy = Arc::clone(&heavy);
                let lookups = Arc::clone(&lookups);
                let jobs = Arc::clone(&jobs);
                let queues = queues.clone();
                let capacity = ExecutionCapacity {
                    lookup: config.lookup_capacity.get(),
                    heavy: config.heavy_capacity.get(),
                };
                thread::spawn(move || {
                    frame_connection(
                        stream,
                        ConnectionContext {
                            accepted_at,
                            capacity,
                        },
                        state,
                        stopping,
                        heavy,
                        lookups,
                        jobs,
                        queues,
                        ActiveConnection(current),
                    );
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => {
                accept_error = Some(error);
                stopping.store(true, Ordering::Release);
            }
        }
    }
    while active.load(Ordering::Acquire) != 0 {
        thread::sleep(Duration::from_millis(5));
    }
    drop(queues);
    for worker in execution_workers {
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("service execution worker panicked"))?;
    }
    jobs.wake.notify_all();
    prefetch.wake.notify_all();
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("indexing worker panicked"))?;
    background
        .join()
        .map_err(|_| anyhow::anyhow!("prefetch worker panicked"))?;
    publication_writer.finish()?;
    if read_state().is_ok_and(|current| current.pid == state.pid) {
        std::fs::remove_file(state_path)?;
    }
    if let Some(error) = accept_error {
        return Err(error.into());
    }
    Ok(())
}

fn run() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Add {
            path,
            timeout_ms,
            trust_extensions,
            commit_limit,
            node_limit,
        } => println!(
            "{}",
            request(RequestSpec {
                command: "add",
                path: Some(path),
                trust_extensions,
                include_invalid: false,
                message: None,
                reference: None,
                observed_oid: None,
                deep: false,
                commit_limit,
                node_limit,
                timeout_ms: timeout_ms.map_or(120_000, NonZeroU64::get),
                job_id: None,
            })?
        ),
        Command::Query {
            path,
            timeout_ms,
            include_invalid,
            reference,
            observed_oid,
            commit_limit,
            node_limit,
        } => println!(
            "{}",
            request(RequestSpec {
                command: "query",
                path: Some(path),
                trust_extensions: false,
                include_invalid,
                message: None,
                reference,
                observed_oid,
                deep: false,
                commit_limit,
                node_limit,
                timeout_ms: timeout_ms.map_or(2_000, NonZeroU64::get),
                job_id: None,
            })?
        ),
        Command::Check {
            path,
            timeout_ms,
            deep,
            reference,
            commit_limit,
            node_limit,
        } => {
            let message = if reference.is_none() {
                let mut message = String::new();
                std::io::stdin().read_to_string(&mut message)?;
                Some(message)
            } else {
                None
            };
            println!(
                "{}",
                request(RequestSpec {
                    command: "check",
                    path: path.into(),
                    trust_extensions: false,
                    include_invalid: false,
                    message,
                    reference,
                    observed_oid: None,
                    deep,
                    commit_limit,
                    node_limit,
                    timeout_ms: timeout_ms.map_or(120_000, NonZeroU64::get),
                    job_id: None,
                })?
            );
        }
        Command::Ensure { timeout_ms } => {
            println!(
                "{}",
                serde_json::to_string(&ensure_service(deadline(
                    timeout_ms.map_or(1000, NonZeroU64::get)
                ))?)?
            );
        }
        Command::Status { timeout_ms } => {
            println!(
                "{}",
                service_status(deadline(timeout_ms.map_or(1000, NonZeroU64::get)))
            );
        }
        Command::JobStatus { id, timeout_ms } => {
            println!(
                "{}",
                request(RequestSpec {
                    command: "job-status",
                    path: None,
                    trust_extensions: false,
                    include_invalid: false,
                    message: None,
                    reference: None,
                    observed_oid: None,
                    deep: false,
                    commit_limit: None,
                    node_limit: None,
                    timeout_ms: timeout_ms.map_or(1000, NonZeroU64::get),
                    job_id: Some(id),
                })?
            );
        }
        Command::JobRetry { id, timeout_ms } => {
            println!(
                "{}",
                request(RequestSpec {
                    command: "job-retry",
                    path: None,
                    trust_extensions: false,
                    include_invalid: false,
                    message: None,
                    reference: None,
                    observed_oid: None,
                    deep: false,
                    commit_limit: None,
                    node_limit: None,
                    timeout_ms: timeout_ms.map_or(1000, NonZeroU64::get),
                    job_id: Some(id),
                })?
            );
        }
        Command::Stop { timeout_ms } => {
            if let Ok(state) = read_state() {
                let stop_deadline = deadline(timeout_ms.map_or(1000, NonZeroU64::get));
                let home = zmem_svc::zmem_home()?;
                let result = send_request(
                    &state,
                    &ServiceRequest {
                        token: state.token.clone(),
                        command: "stop".to_owned(),
                        path: None,
                        trust_extensions: false,
                        include_invalid: false,
                        message: None,
                        reference: None,
                        observed_oid: None,
                        deep: false,
                        commit_limit: None,
                        node_limit: None,
                        timeout_ms: Some(timeout_ms.map_or(1000, NonZeroU64::get)),
                        job_id: None,
                    },
                    stop_deadline,
                )?;
                while zmem_svc::ServiceOwner::is_held(&home)? {
                    remaining(stop_deadline)?;
                    thread::sleep(Duration::from_millis(10));
                }
                println!("{}", result);
            }
        }
        Command::Serve => serve()?,
        Command::VersionJson => println!(
            "{}",
            serde_json::to_string(&zmem_svc::ServiceIdentity::current())?
        ),
        Command::ValidateJournal => {
            let mut input = Vec::new();
            std::io::stdin().read_to_end(&mut input)?;
            let response = zmem_core::validate_action_journal(&input)?;
            println!(
                "{}",
                serde_json::json!({"valid": true, "actions": response.journal.actions.len()})
            );
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        let failure = error
            .downcast_ref::<ServiceFailure>()
            .cloned()
            .unwrap_or_else(|| ServiceFailure::from_error(&error));
        let _ = serde_json::to_writer(std::io::stderr().lock(), &failure);
        eprintln!();
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    #[test]
    fn simultaneous_admission_never_exceeds_capacity() {
        let counter = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(33));
        let mut workers = Vec::new();
        for _ in 0..32 {
            let counter = Arc::clone(&counter);
            let barrier = Arc::clone(&barrier);
            workers.push(thread::spawn(move || {
                barrier.wait();
                try_admit(&counter, 3)
            }));
        }
        barrier.wait();
        let admitted = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(|admitted| *admitted)
            .count();
        assert_eq!(admitted, 3);
        assert_eq!(counter.load(Ordering::Acquire), 3);
    }

    #[test]
    fn full_index_queue_returns_structured_busy() {
        let home = std::env::temp_dir().join(format!(
            "zmem-job-capacity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&home).unwrap();
        zmem_store::Store::open(&home.join("entries.db")).unwrap();
        let coordinator = JobCoordinator::new(home.join("entries.db")).unwrap();
        let key = |index: usize| JobKey {
            path: PathBuf::from("repo"),
            reference: None,
            observed_oid: format!("{index:040x}"),
            commit_limit: None,
            node_limit: None,
            generation: "test".to_owned(),
            route: "refs/heads/main".into(),
        };
        for index in 0..32 {
            assert_eq!(coordinator.admit(key(index)).unwrap().state, "queued");
        }
        assert!(
            coordinator
                .pending_before_identity(&JobKey {
                    generation: "identity-not-yet-resolved".to_owned(),
                    ..key(0)
                })
                .is_none()
        );
        let admitted = coordinator.pending_before_identity(&key(0));
        assert_eq!(admitted.unwrap().id, coordinator.admit(key(0)).unwrap().id);
        let switched = JobKey {
            route: "refs/heads/switched".into(),
            ..key(0)
        };
        assert_eq!(
            coordinator.admit(switched).unwrap().id,
            coordinator.admit(key(0)).unwrap().id
        );
        assert_eq!(
            coordinator
                .queue
                .lock()
                .unwrap()
                .ready
                .front()
                .unwrap()
                .route,
            "refs/heads/main"
        );
        assert!(
            coordinator
                .pending_before_identity(&JobKey {
                    commit_limit: Some(1),
                    ..key(0)
                })
                .is_none()
        );
        assert_eq!(coordinator.queue_depth(), 32);
        let error = coordinator.admit(key(32)).unwrap_err();
        assert_eq!(error.downcast_ref::<ServiceFailure>().unwrap().code, "busy");
        drop(coordinator);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn failed_jobs_survive_capacity_pressure_and_restart() {
        let home = std::env::temp_dir().join(format!(
            "zmem-failed-capacity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&home).unwrap();
        let database = home.join("entries.db");
        let key = |index: usize| JobKey {
            path: PathBuf::from("repo"),
            reference: None,
            observed_oid: format!("{index:040x}"),
            commit_limit: None,
            node_limit: None,
            generation: "test".to_owned(),
            route: "refs/heads/main".into(),
        };
        let mut store = zmem_store::Store::open(&database).unwrap();
        for index in 0..256 {
            let id = format!("failed-{index}");
            store
                .insert_index_job(&id, &serde_json::to_string(&key(index)).unwrap())
                .unwrap();
            store
                .set_index_job_state(&id, "failed", Some("uncertain hook outcome"))
                .unwrap();
        }
        drop(store);
        for _ in 0..2 {
            let coordinator = JobCoordinator::new(database.clone()).unwrap();
            assert_eq!(
                coordinator
                    .admit(key(256))
                    .unwrap_err()
                    .downcast_ref::<ServiceFailure>()
                    .unwrap()
                    .code,
                "busy"
            );
            let failed = coordinator.admit(key(0)).unwrap();
            assert_eq!(failed.state, "failed");
            assert_eq!(failed.failure.as_deref(), Some("uncertain hook outcome"));
            assert_eq!(coordinator.queue_depth(), 0);
        }
        let coordinator = JobCoordinator::new(database).unwrap();
        assert_eq!(coordinator.retry("failed-0").unwrap().state, "queued");
        assert_eq!(coordinator.queue_depth(), 1);
        drop(coordinator);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn validated_identity_refines_a_durable_job_without_changing_its_id() {
        let home = std::env::temp_dir().join(format!(
            "zmem-job-refinement-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&home).unwrap();
        let database = home.join("entries.db");
        zmem_store::Store::open(&database).unwrap();
        let coordinator = JobCoordinator::new(database.clone()).unwrap();
        let pending = JobKey {
            path: PathBuf::from("repo"),
            reference: None,
            observed_oid: "a".repeat(40),
            commit_limit: None,
            node_limit: None,
            generation: "pending:fingerprint:trusted=false".to_owned(),
            route: "refs/heads/main".into(),
        };
        let job = coordinator.admit(pending.clone()).unwrap();
        let refined = coordinator.refine_identity(&pending, "validated").unwrap();
        assert_eq!(refined.generation, "validated:trusted=false");
        assert_eq!(coordinator.admit(refined).unwrap().id, job.id);
        assert_eq!(coordinator.status(&job.id).unwrap().id, job.id);
        drop(coordinator);
        let recovered = zmem_store::Store::open_writable_existing(&database)
            .unwrap()
            .recover_index_jobs()
            .unwrap();
        assert_eq!(recovered.len(), 1);
        assert!(recovered[0].job_key.contains("validated:trusted=false"));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn prefetch_batches_rotate_across_repositories_without_duplicate_admission() {
        let coordinator = PrefetchCoordinator::default();
        let first = (PathBuf::from("repo-a"), "head-a".to_owned());
        let second = (PathBuf::from("repo-b"), "head-b".to_owned());
        coordinator.admit(first.0.clone(), first.1.clone());
        coordinator.admit(second.0.clone(), second.1.clone());
        let next = || coordinator.queue.lock().unwrap().ready.pop_front().unwrap();
        assert_eq!(next(), first);
        coordinator.finish_turn(first.clone(), true);
        coordinator.admit(first.0.clone(), first.1.clone());
        assert_eq!(next(), second);
        coordinator.finish_turn(second.clone(), true);
        assert_eq!(next(), first);
        coordinator.finish_turn(first.clone(), false);
        assert_eq!(next(), second);
        coordinator.finish_turn(second.clone(), false);
        assert!(coordinator.queue.lock().unwrap().known.is_empty());
        for index in 0..40 {
            coordinator.admit(PathBuf::from(format!("repo-{index}")), "head".to_owned());
        }
        let queue = coordinator.queue.lock().unwrap();
        assert_eq!(queue.ready.len(), 32);
        assert_eq!(queue.known.len(), 32);
    }

    #[test]
    fn delayed_frame_returns_typed_timeout() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = ServiceState {
            release_version: "test".to_owned(),
            protocol_version: 5,
            schema_version: 5,
            pid: 1,
            port,
            token: "token".to_owned(),
        };
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(
                stream,
                None,
                ConnectionContext {
                    accepted_at: Instant::now(),
                    capacity: ExecutionCapacity {
                        lookup: 1,
                        heavy: 1,
                    },
                },
                state,
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(JobCoordinator {
                    queue: Mutex::new(JobQueue::default()),
                    wake: Condvar::new(),
                    database: PathBuf::new(),
                    demand_jobs: Arc::new(AtomicUsize::new(0)),
                    completed_foreground: Arc::new(AtomicUsize::new(0)),
                }),
                None,
            );
        });
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        thread::sleep(Duration::from_millis(100));
        client
            .write_all(
                b"{\"token\":\"token\",\"command\":\"ping\",\"path\":null,\"timeout_ms\":25}\n",
            )
            .unwrap();
        let mut line = String::new();
        let received = std::io::BufReader::new(&mut client)
            .read_line(&mut line)
            .unwrap();
        assert!(received > 0);
        let response: ServiceResponse = serde_json::from_str(&line).unwrap();
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "timeout");
        server.join().unwrap();
    }

    #[test]
    fn native_errors_are_single_typed_json_objects() {
        let failure = ServiceFailure::new("not_ready", "indexing", true);
        let payload = serde_json::to_value(failure).unwrap();
        assert_eq!(payload["code"], "not_ready");
        assert_eq!(payload["message"], "indexing");
        assert_eq!(payload["retryable"], true);
        assert!(payload.get("job_id").is_none());
        let stale =
            ServiceFailure::from_error(&anyhow::anyhow!("stale ref: observed old, resolved new"));
        assert_eq!(stale.code, "stale_ref");
    }

    #[test]
    fn native_timeout_options_are_positive() {
        assert!(Cli::try_parse_from(["zmem-svc", "query", ".", "--timeout-ms", "1"]).is_ok());
        assert!(Cli::try_parse_from(["zmem-svc", "query", ".", "--timeout-ms", "0"]).is_err());
        assert!(Cli::try_parse_from(["zmem-svc", "check", ".", "--timeout-ms", "-1"]).is_err());
        assert!(Cli::try_parse_from(["zmem-svc", "status", "--timeout-ms", "1000"]).is_ok());
    }

    #[test]
    fn binary_identity_reports_new_protocol() {
        assert_eq!(zmem_svc::ServiceIdentity::current().protocol_version, 5);
        assert_eq!(zmem_svc::ServiceIdentity::current().schema_version, 6);
    }

    #[test]
    fn slow_service_response_uses_the_original_deadline() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut input = [0_u8; 1024];
            let _ = stream.read(&mut input);
            thread::sleep(Duration::from_millis(100));
            let _ = stream.write_all(b"{\"ok\":true,\"result\":{},\"error\":null}\n");
        });
        let state = ServiceState {
            release_version: "test".to_owned(),
            protocol_version: 5,
            schema_version: 5,
            pid: 1,
            port,
            token: "token".to_owned(),
        };
        let request = ServiceRequest {
            token: state.token.clone(),
            command: "ping".to_owned(),
            path: None,
            trust_extensions: false,
            include_invalid: false,
            message: None,
            reference: None,
            observed_oid: None,
            deep: false,
            commit_limit: None,
            node_limit: None,
            timeout_ms: Some(20),
            job_id: None,
        };
        let started = Instant::now();
        let error = send_request(&state, &request, deadline(20)).unwrap_err();
        assert_eq!(
            error.downcast_ref::<ServiceFailure>().unwrap().code,
            "timeout"
        );
        assert!(started.elapsed() < Duration::from_millis(90));
        server.join().unwrap();
    }
}
