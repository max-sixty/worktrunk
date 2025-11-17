//! Dev-only helper binary for TTY isolation in tests.
//!
//! This binary detaches child processes from controlling terminals by calling setsid()
//! before exec'ing the target command. This prevents PTY-related hangs when running
//! nextest in environments like unbuffer or script.
//!
//! Usage: dev-detach \<command\> [args...]
//!
//! The binary becomes a new session leader with no controlling terminal, then replaces
//! itself with the target command via execvp().
//!
//! Note: This binary is Unix-only. On Windows, it builds as a stub that exits with an error.

#[cfg(unix)]
use nix::unistd::{execvp, setsid};
#[cfg(unix)]
use std::ffi::CString;
use std::{env, process};

fn main() {
    #[cfg(unix)]
    {
        // Become a new session leader with no controlling terminal.
        // This is the key to preventing PTY-related hangs - the child process
        // starts life completely detached from any terminal.
        if let Err(e) = setsid() {
            eprintln!("dev-detach: setsid failed: {}", e);
            process::exit(1);
        }

        // Get command and arguments from our argv
        let args: Vec<String> = env::args().skip(1).collect();
        if args.is_empty() {
            eprintln!("usage: dev-detach <command> [args...]");
            process::exit(2);
        }

        // Convert to CStrings for execvp
        let prog = CString::new(args[0].clone()).unwrap();
        let cargs: Vec<CString> = args
            .iter()
            .map(|a| CString::new(a.as_str()).unwrap())
            .collect();

        // Replace this process with the target command.
        // If execvp returns, it failed.
        let _ = execvp(&prog, &cargs);
        eprintln!("dev-detach: execvp failed");
        process::exit(127);
    }

    #[cfg(not(unix))]
    {
        eprintln!("dev-detach is only supported on Unix systems");
        eprintln!("This binary is used for TTY isolation in tests and is not needed on Windows");
        process::exit(1);
    }
}
