use clap::Parser;

/// Filesystem for tagging files
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub(crate) struct Args {
    /// Act as a client, and mount FUSE at given path
    #[arg(short, long)]
    pub mount_path: String,

    /// Source files from here, read only
    #[arg(short, long)]
    pub source_path: String,

    /// Don't unmount on process exit
    #[arg(short = 'a', long)]
    pub no_unmount: bool,

    /// Disallow root to access the filesystem
    #[arg(short = 'r', long)]
    pub disallow_root: bool,
}
