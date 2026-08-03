use anyhow::{ensure, Context, Result};
use clap::{Args, Parser, Subcommand};
use opencode_supervisor::checkpoint;
use opencode_supervisor::sidecar::{self, SidecarConfig};
use opencode_supervisor::supervisor::{self, SupervisorConfig};
use opencode_supervisor::PINNED_OPENCODE_VERSION;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "supervisor",
    version,
    about = "OpenCode sandbox runtime supervisor"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Parser)]
struct DefaultRunCli {
    #[command(flatten)]
    run: RunArgs,
}

#[derive(Subcommand)]
enum Command {
    Run(RunArgs),
    Sidecar(SidecarArgs),
    Checkpoint(CheckpointArgs),
    Restore(RestoreArgs),
}

#[derive(Args)]
struct RunArgs {
    #[arg(long, env = "WORKSPACE_PATH", default_value = "/workspace")]
    workspace: PathBuf,
    #[arg(
        long,
        env = "OPENCODE_BINARY",
        default_value = "/opt/opencode/bin/opencode"
    )]
    opencode: PathBuf,
    #[arg(
        long,
        env = "DIRENV_BINARY",
        default_value = "/opt/opencode/bin/direnv"
    )]
    direnv: PathBuf,
    #[arg(long, env = "OPENCODE_EXPECTED_VERSION", default_value = PINNED_OPENCODE_VERSION)]
    expected_version: String,
    #[arg(long, env = "SUPERVISOR_LISTEN", default_value = "127.0.0.1:4097")]
    listen: String,
    #[arg(long, env = "SUPERVISOR_CONTROL_TOKEN")]
    control_token: Option<String>,
    #[arg(long, env = "SUPERVISOR_CONTROL_TOKEN_FILE")]
    control_token_file: Option<PathBuf>,
    #[arg(long, env = "OPENCODE_HOST", default_value = "0.0.0.0")]
    host: String,
    #[arg(long, env = "OPENCODE_PORT", default_value_t = 4096)]
    port: u16,
    #[arg(long, env = "SUPERVISOR_GRACE_SECONDS", default_value_t = 20)]
    graceful_seconds: u64,
    #[arg(long, env = "SUPERVISOR_HEALTH_SECONDS", default_value_t = 2)]
    health_seconds: u64,
    #[arg(long, env = "SUPERVISOR_STARTUP_SECONDS", default_value_t = 60)]
    startup_seconds: u64,
    #[arg(long, env = "CHECKPOINT_SIDECAR_URL")]
    checkpoint_url: Option<String>,
    #[arg(long, env = "CHECKPOINT_CONTROL_TOKEN")]
    checkpoint_token: Option<String>,
    #[arg(long, env = "CHECKPOINT_CONTROL_TOKEN_FILE")]
    checkpoint_token_file: Option<PathBuf>,
    #[arg(
        long,
        env = "SUPERVISOR_CHECKPOINT_TIMEOUT_SECONDS",
        default_value_t = 120
    )]
    checkpoint_timeout_seconds: u64,
    #[arg(long, env = "OPENCODE_TRUST_TRACKED_ENVRC", default_value_t = false)]
    trust_tracked_envrc: bool,
}

#[derive(Args)]
struct SidecarArgs {
    #[arg(long, env = "WORKSPACE_PATH", default_value = "/workspace")]
    workspace: PathBuf,
    #[arg(long, env = "OPENCODE_WORKSPACE_ID")]
    workspace_id: String,
    #[arg(
        long,
        env = "CHECKPOINT_OUTPUT_DIR",
        default_value = "/tmp/opencode-checkpoints"
    )]
    output_dir: PathBuf,
    #[arg(long, env = "CHECKPOINT_LISTEN", default_value = "127.0.0.1:4098")]
    listen: String,
    #[arg(long, env = "CHECKPOINT_CONTROL_TOKEN")]
    control_token: Option<String>,
    #[arg(long, env = "CHECKPOINT_CONTROL_TOKEN_FILE")]
    control_token_file: Option<PathBuf>,
    #[arg(long, env = "GATEWAY_URL")]
    gateway_url: String,
    #[arg(long, env = "WORKSPACE_RUNTIME_TOKEN")]
    gateway_token: Option<String>,
    #[arg(long, env = "WORKSPACE_RUNTIME_TOKEN_FILE")]
    gateway_token_file: Option<PathBuf>,
}

#[derive(Args)]
struct CheckpointArgs {
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    #[arg(long, env = "OPENCODE_WORKSPACE_ID")]
    workspace_id: String,
    #[arg(long, default_value = "/tmp/opencode-checkpoints")]
    output_dir: PathBuf,
    #[arg(long)]
    gateway_url: Option<String>,
    #[arg(long, env = "WORKSPACE_RUNTIME_TOKEN")]
    gateway_token: Option<String>,
    #[arg(long, env = "WORKSPACE_RUNTIME_TOKEN_FILE")]
    gateway_token_file: Option<PathBuf>,
}

#[derive(Args)]
struct RestoreArgs {
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    #[arg(long)]
    metadata: PathBuf,
    #[arg(long)]
    bundle: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    let command = cli
        .command
        .unwrap_or_else(|| Command::Run(DefaultRunCli::parse_from(["supervisor"]).run));
    match command {
        Command::Run(args) => {
            let checkpoint_token = optional_secret(
                args.checkpoint_token,
                args.checkpoint_token_file,
                "checkpoint control token",
            )?;
            supervisor::run(SupervisorConfig {
                workspace: args.workspace,
                opencode: args.opencode,
                direnv: args.direnv,
                expected_version: args.expected_version,
                listen: args.listen,
                control_token: secret(
                    args.control_token,
                    args.control_token_file,
                    "supervisor control token",
                )?,
                host: args.host,
                port: args.port,
                graceful_timeout: Duration::from_secs(args.graceful_seconds),
                health_interval: Duration::from_secs(args.health_seconds),
                startup_timeout: Duration::from_secs(args.startup_seconds),
                checkpoint_url: args.checkpoint_url,
                checkpoint_token,
                checkpoint_timeout: Duration::from_secs(args.checkpoint_timeout_seconds),
                trust_tracked_envrc: args.trust_tracked_envrc,
            })
            .await
        }
        Command::Sidecar(args) => {
            sidecar::run(SidecarConfig {
                workspace: args.workspace,
                workspace_id: args.workspace_id,
                output_dir: args.output_dir,
                listen: args.listen,
                control_token: secret(
                    args.control_token,
                    args.control_token_file,
                    "checkpoint control token",
                )?,
                gateway_url: args.gateway_url,
                gateway_token: secret(
                    args.gateway_token,
                    args.gateway_token_file,
                    "workspace runtime token",
                )?,
            })
            .await
        }
        Command::Checkpoint(args) => {
            let artifact = checkpoint::capture(&args.repo, &args.workspace_id, &args.output_dir)?;
            if let Some(url) = args.gateway_url {
                let token = secret(
                    args.gateway_token,
                    args.gateway_token_file,
                    "workspace runtime token",
                )?;
                sidecar::upload(&artifact, &url, &token).await?;
            } else {
                ensure!(
                    args.gateway_token.is_none() && args.gateway_token_file.is_none(),
                    "gateway token requires --gateway-url"
                );
            }
            println!("{}", serde_json::to_string(&artifact.metadata)?);
            Ok(())
        }
        Command::Restore(args) => checkpoint::restore(&args.repo, &args.metadata, &args.bundle),
    }
}

fn secret(value: Option<String>, file: Option<PathBuf>, description: &str) -> Result<String> {
    optional_secret(value, file, description)?.with_context(|| format!("{description} is required"))
}

fn optional_secret(
    value: Option<String>,
    file: Option<PathBuf>,
    description: &str,
) -> Result<Option<String>> {
    ensure!(
        value.is_none() || file.is_none(),
        "specify {description} by value or file, not both"
    );
    let value = match (value, file) {
        (Some(value), None) => Some(value),
        (None, Some(path)) => fs::read_to_string(&path)
            .with_context(|| format!("read {description} from {}", path.display()))?
            .trim()
            .to_owned()
            .into(),
        (None, None) => None,
        (Some(_), Some(_)) => unreachable!(),
    };
    ensure!(
        value.as_ref().is_none_or(|value| !value.is_empty()),
        "{description} must not be empty"
    );
    Ok(value)
}
