use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::json;
use ssh_files::{Route, Transport};
use std::{path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(about = "Unpublished OpenSSH SFTP transport proof; no workspace settings are read")]
struct Options {
    #[arg(long)]
    host: String,
    #[arg(long)]
    hostname: Option<String>,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    user: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    cancel_after_ms: Option<u64>,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    Stat { remote: String },
    Upload { local: PathBuf, remote: String },
    Download { remote: String, local: PathBuf },
}

#[tokio::main]
async fn main() {
    let options = Options::parse();
    let result = run(options).await;
    if let Err(error) = result {
        println!("{}", json!({"ok":false,"error":format!("{error:#}")}));
        std::process::exit(1);
    }
}

async fn run(options: Options) -> Result<()> {
    let route = Route {
        host: options.host,
        hostname: options.hostname,
        config: options.config,
        user: options.user,
        port: options.port,
    };
    // Register Ctrl+C before starting SSH, including during initialization.
    // Dropping a cancelled connect future then runs its owned-process cleanup.
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let mut transport = tokio::select! {
        biased;
        _ = &mut interrupt => anyhow::bail!("Cancelled while connecting; owned SSH process closed"),
        result = Transport::connect(&route) => result?,
    };
    let cancel = async {
        if let Some(ms) = options.cancel_after_ms {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    let result = tokio::select! {
        _ = &mut interrupt => Err(anyhow::anyhow!("Cancelled; owned connection is being closed")),
        _ = cancel => Err(anyhow::anyhow!("Cancelled; owned connection is being closed")),
        result = async {
            match options.action {
                Action::Stat { remote } => { let metadata = transport.metadata(&remote).await?; Ok(json!({"size":metadata.size,"permissions":metadata.permissions})) },
                Action::Upload { local, remote } => { transport.upload(&local, &remote).await?; Ok(json!({})) },
                Action::Download { remote, local } => { transport.download(&remote, &local).await?; Ok(json!({})) },
            }
        } => result,
    };
    let outcome = transport.outcome.clone();
    let cleanup = transport.shutdown().await;
    println!(
        "{}",
        json!({"ok":result.is_ok() && cleanup.is_ok(), "result":result.as_ref().ok(),
        "error":result.as_ref().err().map(|e|format!("{e:#}")), "outcome":outcome,
        "completion_unknown":outcome.phase == "committing", "cleanup_error":cleanup.as_ref().err().map(|e|format!("{e:#}")),
        "ssh_diagnostics":cleanup.as_ref().ok()})
    );
    if result.is_err() || cleanup.is_err() {
        std::process::exit(1);
    }
    Ok(())
}
