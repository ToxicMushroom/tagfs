#![feature(cell_update)]

use fuser::MountOption;
use log::{error, LevelFilter};
use pretty_env_logger::env_logger::Builder;

use crate::fs::backing::ExternalFS;
use crate::fs::tag::TagFS;

mod file;

mod fs;

fn main() -> std::io::Result<()> {
    setup_logger();

    const PATH: &str = "source";
    let backing = || ExternalFS::new(PATH);

    let mut fs = match TagFS::new_from_save(backing()) {
        Ok(fs) => fs,
        Err(e) => {
            error!("Couldn't recover FS from savefile: {e}, creating empty FS");
            TagFS::new(backing())
        }
    };

    let files = std::fs::read_dir(PATH)?
        .filter_map(|e| {
            e.ok()
                .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
        })
        .map(|e| e.file_name());

    fs.repopulate(files);

    fuser::mount2(
        fs,
        "mount",
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
