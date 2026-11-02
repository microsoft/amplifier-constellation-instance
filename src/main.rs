use agent_instance::{auth, config::InstanceConfig, host, state::AppState};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "One independent ACP agent, with its own setup and observability"
)]
struct Cli {
    #[arg(long, global = true, default_value = "./agent-home")]
    home: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Init {
        #[arg(long)]
        name: String,
        #[arg(long)]
        owner: Option<String>,
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        #[arg(long, default_value_t = 0)]
        port: u16,
    },
    Serve {
        /// Optional private log directory for startup managers without console capture.
        #[arg(long)]
        log_directory: Option<PathBuf>,
    },
    Status {
        #[arg(long)]
        json: bool,
    },
    Endpoints,
    Backend {
        #[command(subcommand)]
        command: BackendCommand,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
    Acp {
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        token: Option<String>,
    },
    /// Inspect and authorize this instance's durable background work.
    Background {
        #[command(subcommand)]
        command: BackgroundCommand,
    },
    Api {
        #[arg(default_value = "GET")]
        method: String,
        path: String,
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Connect the native engine's own ChatGPT account (separate from any Codex account).
    NativeAccount {
        #[command(subcommand)]
        command: NativeAccountCommand,
    },
    Schema,
}
#[derive(Subcommand)]
enum NativeAccountCommand {
    Status,
    LoginStatus,
    SignIn {
        #[arg(long, default_value_t = 0)]
        callback_port: u16,
    },
    Cancel {
        #[arg(long)]
        job_id: Option<String>,
    },
    Models,
    SignOut {
        #[arg(long, required = true)]
        confirm: bool,
    },
}
#[derive(Subcommand)]
enum BackgroundCommand {
    Status,
    /// Apply revisioned settings, including explicit local-backend approval.
    Configure {
        #[arg(long)]
        file: PathBuf,
    },
    Runs {
        work_id: String,
    },
    /// Queue, cancel, reconcile or retry using the versioned execution command schema.
    Command {
        work_id: String,
        #[arg(long)]
        file: PathBuf,
    },
}
#[derive(Subcommand)]
enum ConfigCommand {
    Get,
    Validate {
        #[arg(long)]
        file: Option<PathBuf>,
    },
    Set {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        expected_revision: u64,
    },
}
#[derive(Subcommand)]
enum BackendCommand {
    /// Install this version's default ACP engine and its private provider state.
    Install {
        #[arg(long, default_value = "native", value_parser = ["native", "codex"])]
        engine: String,
        /// Explicit local Constellation executable; otherwise use the pinned release.
        #[arg(long)]
        runtime_binary: Option<PathBuf>,
        #[arg(long)]
        no_reuse_account: bool,
    },
    Status,
}
#[derive(Subcommand)]
enum TokenCommand {
    Create {
        #[arg(long)]
        name: String,
        #[arg(long, value_delimiter = ',')]
        scopes: Vec<String>,
        #[arg(long, default_value_t = 2592000)]
        expires_in: u64,
        #[arg(long)]
        local: bool,
    },
}
fn read_json(path: PathBuf) -> Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
fn print(value: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    match cli.command {
        Command::Init {
            name,
            owner,
            bind,
            port,
        } => print(host::init(&cli.home, name, owner, bind, port)?),
        Command::Serve { log_directory } => {
            let _logs = log_directory
                .as_deref()
                .map(agent_instance::platform::redirect_logs)
                .transpose()?;
            host::serve(cli.home).await
        }
        Command::Endpoints => print(read_json(cli.home.join("endpoints.json"))?),
        Command::Backend { command } => {
            let home = dunce::canonicalize(&cli.home)?;
            let state = AppState::open(home.clone())?;
            anyhow::ensure!(
                state.instance_config().owner == host::current_user()?,
                "Run as this installation's OS owner"
            );
            if let BackendCommand::Install {
                engine,
                runtime_binary,
                no_reuse_account,
            } = command
            {
                let backend = if engine == "native" {
                    agent_instance::provision::install_native(&home, runtime_binary.as_deref())
                        .await?
                } else {
                    anyhow::ensure!(
                        runtime_binary.is_none(),
                        "--runtime-binary is for Constellation only"
                    );
                    agent_instance::provision::install_codex(&home, !no_reuse_account).await?
                };
                // Use the live revisioned API when serving, and the same replacement
                // logic under the service lock before its first start.
                let lock = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(home.join("service.lock"))?;
                if fs2::FileExt::try_lock_exclusive(&lock).is_ok() {
                    let current = AppState::open(home.clone())?;
                    let mut doc = current.config.read().unwrap().clone();
                    doc.config.backend = Some(backend);
                    agent_instance::server::replace_config(
                        &current,
                        doc.revision,
                        doc.config,
                        "owner-cli",
                    )
                    .map_err(|e| anyhow::anyhow!(e.1))?;
                    current.event(
                        "backend.installed",
                        json!({"actor":"owner-cli","runtime":if engine == "native" {"Constellation"} else {"Codex"}}),
                    )?;
                } else {
                    let mut document = host::api(&home, "GET", "config", None).await?;
                    document["config"]["backend"] = serde_json::to_value(backend)?;
                    host::api(&home, "PUT", "config", Some(json!({"expected_revision":document["revision"],"config":document["config"]}))).await?;
                }
            }
            let state = AppState::open(home.clone())?;
            print(agent_instance::provision::status(
                &home,
                &state.instance_config(),
            ))
        }
        Command::Background { command } => match command {
            BackgroundCommand::Status => {
                print(host::api(&cli.home, "GET", "scheduler", None).await?)
            }
            BackgroundCommand::Configure { file } => {
                let body: agent_instance::scheduler::Settings =
                    serde_json::from_value(read_json(file)?)?;
                print(
                    host::api(
                        &cli.home,
                        "PUT",
                        "scheduler",
                        Some(serde_json::to_value(body)?),
                    )
                    .await?,
                )
            }
            BackgroundCommand::Runs { work_id } => {
                uuid::Uuid::parse_str(&work_id)?;
                print(
                    host::api(&cli.home, "GET", &format!("work/{work_id}/execution"), None).await?,
                )
            }
            BackgroundCommand::Command { work_id, file } => {
                uuid::Uuid::parse_str(&work_id)?;
                let body: agent_instance::scheduler::Command =
                    serde_json::from_value(read_json(file)?)?;
                print(
                    host::api(
                        &cli.home,
                        "POST",
                        &format!("work/{work_id}/execution"),
                        Some(serde_json::to_value(body)?),
                    )
                    .await?,
                )
            }
        },
        Command::Schema => print(json!(schemars::schema_for!(InstanceConfig))),
        Command::Status { .. } => print(host::api(&cli.home, "GET", "status", None).await?),
        Command::Config { command } => match command {
            ConfigCommand::Get => print(read_json(cli.home.join("config.json"))?),
            ConfigCommand::Validate { file } => {
                let value = read_json(file.unwrap_or_else(|| cli.home.join("config.json")))?;
                let config: InstanceConfig =
                    serde_json::from_value(value.get("config").cloned().unwrap_or(value))?;
                config.validate()?;
                print(json!({"valid":true}))
            }
            ConfigCommand::Set {
                file,
                expected_revision,
            } => {
                let value = read_json(file)?;
                let config: InstanceConfig =
                    serde_json::from_value(value.get("config").cloned().unwrap_or(value))?;
                config.validate()?;
                print(
                    host::api(
                        &cli.home,
                        "PUT",
                        "config",
                        Some(json!({"expected_revision":expected_revision,"config":config})),
                    )
                    .await?,
                )
            }
        },
        Command::Token {
            command:
                TokenCommand::Create {
                    name,
                    scopes,
                    expires_in,
                    local,
                },
        } => {
            if local {
                // Filesystem ownership is the bootstrap/recovery authority. Do not expose this path over HTTP.
                let state = AppState::open(cli.home)?;
                anyhow::ensure!(
                    state.instance_config().owner == host::current_user()?,
                    "Run as the installation's OS owner"
                );
                let token = auth::issue_token(&state, &name, &scopes, expires_in)
                    .map_err(|e| anyhow::anyhow!(e.1))?;
                state.event(
                    "token.created_locally",
                    json!({"id":token["id"],"scopes":scopes}),
                )?;
                print(token)
            } else {
                print(
                    host::api(
                        &cli.home,
                        "POST",
                        "tokens",
                        Some(json!({"name":name,"scopes":scopes,"expires_in_seconds":expires_in})),
                    )
                    .await?,
                )
            }
        }
        Command::Acp { url, token } => agent_instance::acp::attach(cli.home, url, token).await,
        Command::NativeAccount { command } => {
            let (method, path, body) = match command {
                NativeAccountCommand::Status => ("GET", "native-account", None),
                NativeAccountCommand::LoginStatus => ("GET", "native-account/login", None),
                NativeAccountCommand::Models => ("POST", "native-account/models", Some(json!({}))),
                NativeAccountCommand::SignOut { confirm } => {
                    ("DELETE", "native-account", Some(json!({"confirm":confirm})))
                }
                NativeAccountCommand::Cancel { job_id } => {
                    let id = match job_id {
                        Some(id) => id,
                        None => {
                            host::api(&cli.home, "GET", "native-account/login", None).await?["id"]
                                .as_str()
                                .context("There is no sign-in to cancel")?
                                .to_owned()
                        }
                    };
                    ("DELETE", "native-account/login", Some(json!({"job_id":id})))
                }
                NativeAccountCommand::SignIn { callback_port } => {
                    let config = host::api(&cli.home, "GET", "native-config", None).await?;
                    (
                        "POST",
                        "native-account/login",
                        Some(
                            json!({"expected_backend_sha256":config["backend_sha256"],"callback_port":callback_port}),
                        ),
                    )
                }
            };
            let mut result = host::api(&cli.home, method, path, body).await?;
            if method == "POST" && path == "native-account/login" {
                for _ in 0..50 {
                    if result["state"] != "starting" {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    result = host::api(&cli.home, "GET", "native-account/login", None).await?;
                }
            }
            print(result)
        }
        Command::Api { method, path, file } => print(
            host::api(&cli.home, &method, &path, file.map(read_json).transpose()?)
                .await
                .context("Management request failed")?,
        ),
    }
}
