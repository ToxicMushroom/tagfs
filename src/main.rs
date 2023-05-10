#![feature(cell_update)]

use clap::Parser;
use fuser::MountOption;
use log::{error, LevelFilter};
use pretty_env_logger::env_logger::Builder;

use cli::Args;

use crate::fs::backing::ExternalFS;
use crate::fs::tag::TagFS;

mod file;

mod fs;

mod cli;

fn main() -> std::io::Result<()> {
    setup_logger();

    let args = Args::parse();

    let source_path = args.source_path.as_str();

    let mut fs = match TagFS::new_from_save(ExternalFS::new(source_path)) {
        Ok(fs) => fs,
        Err(e) => {
            error!("Couldn't recover FS from savefile: {e}, creating empty FS");
            TagFS::new(ExternalFS::new(source_path))
        }
    };

    let files = std::fs::read_dir(source_path)?
        .filter_map(|e| {
            e.ok()
                .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
        })
        .map(|e| e.file_name());

    fs.repopulate(files);

    fuser::mount2(
        fs,
        args.mount_path,
        &[MountOption::AutoUnmount, MountOption::AllowRoot],
    )
}

fn setup_logger() {
    // Create a new `env_logger::Builder`
    let mut builder = Builder::new();

    // Set the minimum log level to `Debug`
    builder.filter_level(LevelFilter::Debug);

    // Configure the log format
    builder.format_timestamp_secs();

    // Initialize the logger
    builder.init();
}
