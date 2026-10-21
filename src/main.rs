//! veo - a clean, simple video downloader powered by yt-dlp.

// Callback types such as `&dyn Fn(&Path, &[String], &Cancel) -> Result<…>` are
// clearer spelled out at their single use than hidden behind aliases.
#![allow(clippy::type_complexity)]

/// A lazily compiled, cached regular expression.
#[macro_export]
macro_rules! re {
    ($pattern:expr) => {{
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        // Compiled once per call site, even inside loops.
        #[allow(clippy::regex_creation_in_loops)]
        RE.get_or_init(|| regex::Regex::new($pattern).expect("valid regex"))
    }};
}

mod alias;
mod backend;
mod backend_update;
mod cancel;
mod changes;
mod cli;
mod clipboard;
mod compatibility;
mod config;
mod config_diagnostics;
mod config_editor;
mod config_errors;
mod config_reset;
mod config_template;
mod disk_space;
mod doctor;
mod download_cache;
mod downloader;
mod error;
mod execution;
mod experimental;
mod flush;
mod fsutil;
mod help;
mod history;
mod http;
mod inspect_media;
mod interactive;
mod jobs;
mod media_assets;
mod metadata;
mod naming;
mod open_file;
mod options;
mod output;
mod path_links;
mod paths;
mod playlist;
mod process;
mod progress;
mod run_archive;
mod runs;
mod source_discovery;
mod state;
mod stats;
mod suggest;
mod terminal;
mod tool_setup;
mod uninstall;
mod updater;
mod utils;
mod version;

/// Let Windows consoles interpret colors, cursor movement and UTF-8 output.
#[cfg(windows)]
fn enable_virtual_terminal() {
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, SetConsoleOutputCP, ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    // SAFETY: standard handles and out-parameters owned by this function.
    unsafe {
        SetConsoleOutputCP(65001);
        for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = GetStdHandle(which);
            let mut mode = 0;
            if GetConsoleMode(handle, &mut mode) != 0 {
                SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}

fn main() {
    // Ctrl+C, SIGTERM and SIGHUP stop the current run gracefully: child
    // processes are terminated and partial data is kept for --resume.
    let _ = ctrlc::set_handler(|| cancel::Cancel::global().cancel());
    #[cfg(windows)]
    enable_virtual_terminal();
    let args: Vec<String> = std::env::args_os().skip(1).map(|arg| arg.to_string_lossy().into_owned()).collect();
    let code = cli::main(args);
    std::process::exit(code);
}
