use anyhow::Result;
use clap::{Parser, Subcommand};
use rustdesk_account_server::{
    admin::{self, BookCommand, DeviceCommand, SessionCommand, UserCommand},
    config::Config,
    db::{Store, prune},
    http::App,
};
use std::{path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(long, global = true, default_value = "account.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Initialize a new private database and separate encryption key.
    Init,
    /// Serve the client-compatible API. Put HTTPS in front of this listener.
    Serve,
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    Book {
        #[command(subcommand)]
        command: BookCommand,
    },
    Logs {
        #[arg(long)]
        client_reports: bool,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Create a consistent SQLite backup at a NEW file path.
    Backup { destination: PathBuf },
    /// Offline restore to a NEW database path selected in --config; never overwrites.
    Restore { source: PathBuf },
}
fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;
    match cli.command {
        Command::Init => {
            Store::init(&config)?;
            println!("Initialized. Back up the database and master key separately.");
        }
        Command::Restore { source } => {
            Store::restore(&config, &source)?;
            println!("Restored to new path. Verify before changing the service configuration.");
        }
        command => {
            let store = Store::open(config)?;
            match command {
                Command::Serve => {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .max_blocking_threads(4)
                        .build()?;
                    runtime.block_on(serve(store))?;
                }
                Command::User { command } => admin::users(&store, command)?,
                Command::Session { command } => admin::sessions(&store, command)?,
                Command::Device { command } => admin::devices(&store, command)?,
                Command::Book { command } => admin::books(&store, command)?,
                Command::Logs {
                    client_reports,
                    limit,
                } => admin::logs(&store, client_reports, limit)?,
                Command::Backup { destination } => {
                    store.backup(&destination)?;
                    println!("Database backup complete; the master key is NOT included.");
                }
                _ => unreachable!("initialization commands handled above"),
            }
        }
    };
    Ok(())
}
async fn serve(store: Store) -> Result<()> {
    let app = App::new(store.clone())?;
    let listener = tokio::net::TcpListener::bind(store.config.listen).await?;
    eprintln!(
        "rustdesk-account-server {} listening on {}",
        env!("CARGO_PKG_VERSION"),
        listener.local_addr()?
    );
    let maintenance = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            if store
                .run(|c, _, config| {
                    let tx = c.transaction()?;
                    prune(
                        &tx,
                        "audit",
                        config.max_audit_rows,
                        config.log_retention_days,
                    )?;
                    prune(
                        &tx,
                        "client_reports",
                        config.max_client_report_rows,
                        config.log_retention_days,
                    )?;
                    tx.commit()?;
                    Ok(())
                })
                .await
                .is_err()
            {
                eprintln!("Retention maintenance failed");
            }
        }
    });
    axum::serve(
        listener,
        app.router()
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    maintenance.abort();
    Ok(())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {_ = tokio::signal::ctrl_c()=>{}, _ = signal.recv()=>{}};
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
