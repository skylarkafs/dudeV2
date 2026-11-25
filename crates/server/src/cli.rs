use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "nextdude-server", version, about = "NextDude headless network monitoring server")]
pub struct Cli {
    #[arg(short, long, default_value = "/etc/nextdude/server.toml", global = true)]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the monitoring daemon and gRPC API server.
    Run,
    /// Validate configuration file, directories, and ICMP socket capability.
    CheckConfig,
    /// Manage administrator accounts from the local CLI.
    Admin(AdminCommand),
    /// Perform an online SQLite database backup.
    Backup(BackupArgs),
    /// Restore SQLite database from a backup file while server is stopped.
    Restore(BackupArgs),
}

#[derive(Debug, Args)]
pub struct AdminCommand {
    #[command(subcommand)]
    pub action: AdminAction,
}

#[derive(Debug, Subcommand)]
pub enum AdminAction {
    /// Create a new administrator user.
    Create {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: String,
        #[arg(long, default_value = "Admin")]
        group: String,
    },
    /// Reset password for an existing administrator user.
    ResetPassword {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: String,
    },
}

#[derive(Debug, Args)]
pub struct BackupArgs {
    #[arg(long)]
    pub path: PathBuf,
}
