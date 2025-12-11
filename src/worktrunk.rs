use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, exit};

fn sibling_wt_path(target_name: &str) -> Option<PathBuf> {
    env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(target_name)))
        .filter(|path| path.is_file())
}

/// A thin wrapper around 'wt' that forwards all arguments to it.
fn main() {
    let target_name = if cfg!(windows) { "wt.exe" } else { "wt" };
    let args: Vec<OsString> = env::args_os().skip(1).collect();

    let wt_path = sibling_wt_path(target_name).unwrap_or_else(|| {
        eprintln!(
            "worktrunk expects '{target_name}' next to it; re-install Worktrunk if it's missing."
        );
        exit(1);
    });

    let status = Command::new(&wt_path)
        .args(&args)
        .status()
        .unwrap_or_else(|err| {
            eprintln!("Failed to launch '{}': {}", wt_path.display(), err);
            exit(1);
        });

    exit(status.code().unwrap_or(1));
}
