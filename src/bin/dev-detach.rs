//! Test helper for TTY isolation in shell integration tests.
//!
//! Detaches child processes from controlling terminals by calling setsid()
//! before exec'ing the target command. Prevents PTY-related hangs when running
//! nextest in environments like unbuffer or script.
//!
//! Usage: dev-detach \<command\> [args...]

use std::process;

fn main() {
    #[cfg(unix)]
    {
        use nix::unistd::{execvp, setsid};
        use std::{env, ffi::CString};

        if let Err(e) = setsid() {
            eprintln!("dev-detach: setsid failed: {e}");
            process::exit(1);
        }

        let args: Vec<String> = env::args().skip(1).collect();
        if args.is_empty() {
            eprintln!("usage: dev-detach <command> [args...]");
            process::exit(2);
        }

        let prog = CString::new(args[0].clone()).unwrap_or_else(|_| {
            eprintln!("dev-detach: command contains null byte");
            process::exit(2);
        });
        let cargs: Vec<CString> = args
            .iter()
            .map(|a| {
                CString::new(a.as_str()).unwrap_or_else(|_| {
                    eprintln!("dev-detach: argument contains null byte");
                    process::exit(2);
                })
            })
            .collect();

        // execvp only returns on error (success replaces this process)
        let Err(e) = execvp(prog.as_c_str(), &cargs);
        eprintln!("dev-detach: execvp failed: {e}");
        process::exit(127);
    }

    #[cfg(not(unix))]
    {
        eprintln!("dev-detach is Unix-only (used for TTY isolation in tests)");
        process::exit(1);
    }
}
