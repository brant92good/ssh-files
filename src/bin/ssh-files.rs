use anyhow::Result;
use clap::Parser;
use ssh_files::{Route, ui};
use std::{path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(
    version,
    about = "Browse and transfer files through your existing OpenSSH route"
)]
struct Args {
    #[arg(long)]
    host: String,
    /// Override HostName while still selecting this SSH alias.
    #[arg(long)]
    hostname: Option<String>,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    user: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    label: Option<String>,
    #[arg(long, default_value = "")]
    machine_id: String,
    #[arg(long, default_value = "")]
    route_id: String,
    #[arg(long, default_value = ".")]
    local: PathBuf,
    #[arg(long, default_value = ".")]
    remote: String,
}
fn main() -> Result<()> {
    let args = Args::parse();
    let options = ui::Options {
        label: args.label.unwrap_or_else(|| args.host.clone()),
        route: Route {
            host: args.host,
            hostname: args.hostname,
            config: args.config,
            user: args.user,
            port: args.port,
        },
        machine_id: args.machine_id,
        route_id: args.route_id,
        local: args.local,
        remote: args.remote,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(ui::run(options));
    // A cancelled filesystem syscall can outlive its future. Bound runtime
    // shutdown rather than claim abort() stops an OS/filesystem operation.
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
}
