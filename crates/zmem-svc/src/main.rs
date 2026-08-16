use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};
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
        trust_extensions: bool,
        #[arg(long, allow_hyphen_values = true)]
        commit_limit: Option<i64>,
        #[arg(long, allow_hyphen_values = true)]
        node_limit: Option<i64>,
    },
    Query {
        path: PathBuf,
        #[arg(long)]
        include_invalid: bool,
        #[arg(long, allow_hyphen_values = true)]
        commit_limit: Option<i64>,
        #[arg(long, allow_hyphen_values = true)]
        node_limit: Option<i64>,
    },
    Check {
        path: PathBuf,
        #[arg(long)]
        deep: bool,
        #[arg(long = "ref")]
        reference: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        commit_limit: Option<i64>,
        #[arg(long, allow_hyphen_values = true)]
        node_limit: Option<i64>,
    },
    Ensure,
    Status,
    Stop,
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

#[derive(Debug, Deserialize, Serialize)]
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
    deep: bool,
    #[serde(default)]
    commit_limit: Option<i64>,
    #[serde(default)]
    node_limit: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ServiceResponse {
    ok: bool,
    result: Option<serde_json::Value>,
    error: Option<String>,
}

fn state_path() -> anyhow::Result<PathBuf> {
    Ok(zmem_svc::zmem_home()?.join("service.json"))
}

fn read_state() -> anyhow::Result<ServiceState> {
    Ok(serde_json::from_slice(&std::fs::read(state_path()?)?)?)
}

fn send_request(
    state: &ServiceState,
    request: &ServiceRequest,
) -> anyhow::Result<serde_json::Value> {
    let mut stream = TcpStream::connect(("127.0.0.1", state.port))?;
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let response: ServiceResponse = serde_json::from_str(&line)?;
    anyhow::ensure!(
        response.ok,
        "{}",
        response
            .error
            .unwrap_or_else(|| "service request failed".to_owned())
    );
    Ok(response.result.unwrap_or(serde_json::Value::Null))
}

fn ping(state: &ServiceState) -> bool {
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
            deep: false,
            commit_limit: None,
            node_limit: None,
        },
    )
    .is_ok()
}

fn healthy_state() -> Option<ServiceState> {
    read_state()
        .ok()
        .filter(|state| state.protocol_version == zmem_core::PROTOCOL_VERSION && ping(state))
}

fn service_status() -> serde_json::Value {
    let identity = zmem_svc::ServiceIdentity::current();
    if let Some(state) = healthy_state() {
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

fn ensure_service() -> anyhow::Result<ServiceState> {
    if let Some(state) = healthy_state() {
        return Ok(state);
    }
    let home = zmem_svc::zmem_home()?;
    let _startup =
        zmem_svc::StartupLock::acquire(&home, Duration::from_secs(15), Duration::from_secs(10))?;
    if let Some(state) = healthy_state() {
        return Ok(state);
    }
    let executable = std::env::current_exe()?;
    spawn_service_process(&executable)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(state) = healthy_state() {
            return Ok(state);
        }
        thread::sleep(Duration::from_millis(25));
    }
    anyhow::bail!("timed out starting the per-user zmem service")
}

struct RequestSpec {
    command: &'static str,
    path: Option<PathBuf>,
    trust_extensions: bool,
    include_invalid: bool,
    message: Option<String>,
    reference: Option<String>,
    deep: bool,
    commit_limit: Option<i64>,
    node_limit: Option<i64>,
}

fn request(spec: RequestSpec) -> anyhow::Result<serde_json::Value> {
    let state = ensure_service()?;
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
            deep: spec.deep,
            commit_limit: spec.commit_limit,
            node_limit: spec.node_limit,
        },
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

fn serve() -> anyhow::Result<()> {
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

    let mut stopping = false;
    while !stopping {
        let (mut stream, _) = listener.accept()?;
        let request: anyhow::Result<ServiceRequest> = (|| {
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line)?;
            Ok(serde_json::from_str(&line)?)
        })();
        let response = match request {
            Ok(request) if request.token != state.token => ServiceResponse {
                ok: false,
                result: None,
                error: Some("unauthorized local client".to_owned()),
            },
            Ok(request) => match request.command.as_str() {
                "ping" => ServiceResponse {
                    ok: true,
                    result: Some(serde_json::json!({"pid": state.pid})),
                    error: None,
                },
                "stop" => {
                    stopping = true;
                    ServiceResponse {
                        ok: true,
                        result: Some(serde_json::json!({"stopped": true})),
                        error: None,
                    }
                }
                "add" | "query" => {
                    let outcome = request
                        .path
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("repository path is required"))
                        .and_then(|path| {
                            zmem_svc::sync_repository_with_attention(
                                path,
                                (request.command == "add").then_some(request.trust_extensions),
                                request.commit_limit,
                                request.node_limit,
                            )
                        });
                    match outcome {
                        Ok(mut sync) => {
                            if !request.include_invalid {
                                sync.entries
                                    .retain(|row| row["valid"].as_bool().unwrap_or(false));
                            }
                            let result = if request.command == "add" {
                                serde_json::to_value(sync.summary)?
                            } else {
                                serde_json::json!({
                                    "summary": sync.summary,
                                    "entries": sync.entries,
                                    "relationships": sync.relationships,
                                    "diagnostics": sync.diagnostics,
                                })
                            };
                            ServiceResponse {
                                ok: true,
                                result: Some(result),
                                error: None,
                            }
                        }
                        Err(error) => ServiceResponse {
                            ok: false,
                            result: None,
                            error: Some(format!("{error:#}")),
                        },
                    }
                }
                "check" => {
                    let outcome = request
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
                        });
                    match outcome {
                        Ok(check) => ServiceResponse {
                            ok: true,
                            result: Some(serde_json::to_value(check)?),
                            error: None,
                        },
                        Err(error) => ServiceResponse {
                            ok: false,
                            result: None,
                            error: Some(format!("{error:#}")),
                        },
                    }
                }
                _ => ServiceResponse {
                    ok: false,
                    result: None,
                    error: Some("unknown service command".to_owned()),
                },
            },
            Err(error) => ServiceResponse {
                ok: false,
                result: None,
                error: Some(format!("invalid service request: {error:#}")),
            },
        };
        serde_json::to_writer(&mut stream, &response)?;
        stream.write_all(b"\n")?;
        stream.flush()?;
    }
    if read_state().is_ok_and(|current| current.pid == state.pid) {
        std::fs::remove_file(state_path)?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Add {
            path,
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
                deep: false,
                commit_limit,
                node_limit,
            })?
        ),
        Command::Query {
            path,
            include_invalid,
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
                reference: None,
                deep: false,
                commit_limit,
                node_limit,
            })?
        ),
        Command::Check {
            path,
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
                    deep,
                    commit_limit,
                    node_limit,
                })?
            );
        }
        Command::Ensure => println!("{}", serde_json::to_string(&ensure_service()?)?),
        Command::Status => println!("{}", service_status()),
        Command::Stop => {
            if let Ok(state) = read_state() {
                println!(
                    "{}",
                    send_request(
                        &state,
                        &ServiceRequest {
                            token: state.token.clone(),
                            command: "stop".to_owned(),
                            path: None,
                            trust_extensions: false,
                            include_invalid: false,
                            message: None,
                            reference: None,
                            deep: false,
                            commit_limit: None,
                            node_limit: None,
                        },
                    )?
                );
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
