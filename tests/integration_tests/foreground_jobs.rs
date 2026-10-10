//! Foreground CLI children use the shell-established native Unix job group.
//! The shared shell fixture also exercises help-pager selection and completion
//! through the actual TTY entrypoint.
#![cfg(all(unix, feature = "shell-integration-tests"))]

use crate::common::{TestRepo, configure_pty_command, open_pty, repo, wt_bin};
use portable_pty::{Child, CommandBuilder, MasterPty};
use rstest::rstest;
use std::io::Write;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

struct Shell {
    master: Box<dyn MasterPty + Send>,
    writer: crate::common::pty::SharedPtyWriter,
    child: Box<dyn Child + Send + Sync>,
    output: String,
    chunks: mpsc::Receiver<Vec<u8>>,
    members: Vec<nix::unistd::Pid>,
}
impl Shell {
    fn start(repo: &TestRepo) -> Self {
        let mut command = CommandBuilder::new("bash");
        command.args(["--noprofile", "--norc", "-i"]);
        let mut shell = Self::spawn(repo, command);
        shell.wait_for("job-test> ");
        shell
    }
    fn spawn(repo: &TestRepo, mut command: CommandBuilder) -> Self {
        configure_pty_command(&mut command);
        command.cwd(repo.root_path());
        command.env("PS1", "job-test> ");
        command.env("TERM", "dumb");
        command.env("WT_JOB_TEST_BINARY", wt_bin());
        command.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
        let pair = open_pty();
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().unwrap();
        let writer = Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
        let chunks =
            crate::common::pty::spawn_pty_reader_answering_queries(reader, Arc::clone(&writer));
        Self {
            master: pair.master,
            writer,
            child,
            output: String::new(),
            chunks,
            members: Vec::new(),
        }
    }
    fn send(&mut self, text: &str) {
        self.output.clear();
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(text.as_bytes()).unwrap();
        writer.flush().unwrap();
    }
    fn wait_for(&mut self, marker: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.output.contains(marker) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let chunk = self.chunks.recv_timeout(remaining).unwrap_or_else(|_| {
                let members: Vec<_> = self
                    .members
                    .iter()
                    .map(|pid| (*pid, nix::unistd::getpgid(Some(*pid))))
                    .collect();
                panic!(
                    "missing {marker:?}; shell={:?}, foreground={:?}, members={members:?}, termios={:?}; terminal output:\n{}",
                    self.child.process_id(),
                    self.master.process_group_leader(),
                    self.master.get_termios(),
                    self.output
                )
            });
            self.output.push_str(&String::from_utf8_lossy(&chunk));
        }
    }
    fn foreground(&self) -> i32 {
        self.master.process_group_leader().unwrap()
    }
    fn remember(&mut self, pid: i32) {
        let pid = nix::unistd::Pid::from_raw(pid);
        assert_eq!(
            nix::unistd::getsid(Some(pid)).unwrap().as_raw(),
            self.child.process_id().unwrap() as i32
        );
        self.members.push(pid);
    }
}
impl Drop for Shell {
    fn drop(&mut self) {
        // Shell never waits/reaps its leader before Drop. Its reserved PID
        // keeps the session identity valid even if the leader already exited.
        let session = self.child.process_id().unwrap() as i32;
        for member in &self.members {
            if nix::unistd::getsid(Some(*member)).is_ok_and(|owner| owner.as_raw() == session)
                && let Ok(group) = nix::unistd::getpgid(Some(*member))
            {
                let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
            }
        }
        if let Some(group) = self.master.process_group_leader() {
            let group = nix::unistd::Pid::from_raw(group);
            if nix::unistd::getsid(Some(group)).is_ok_and(|owner| owner.as_raw() == session) {
                let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
            }
        }
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(session),
            nix::sys::signal::Signal::SIGKILL,
        );
        let _ = self.child.wait();
    }
}

fn native_pipeline(concurrent: bool) -> String {
    let first = if concurrent {
        "{ one = 'exec python3 worker.py', two = 'true' }"
    } else {
        "'exec python3 worker.py'"
    };
    format!("post-start = [{first}, 'printf NEXT > next']")
}

fn native_info(repo: &TestRepo, file: &str) -> Vec<i32> {
    let path = repo.root_path().join(file);
    crate::common::wait_for_file_content(&path);
    std::fs::read_to_string(path)
        .unwrap()
        .split_whitespace()
        .map(|n| n.parse().unwrap())
        .collect()
}

const WAITING_NATIVE_WORKER: &str = r#"import os, time
from pathlib import Path
Path('worker-info').write_text(f'{os.getpid()} {os.getpgrp()} {os.getppid()}')
while not Path('release').exists():
    time.sleep(.01)
"#;

/// An inherited ignored TERM must not become foreground cancellation.
#[rstest]
fn inherited_ignored_term_remains_ignored(repo: TestRepo) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    repo.write_project_config(&native_pipeline(false));
    std::fs::write(
        repo.root_path().join("worker.py"),
        format!(
            "import signal\nsignal.signal(signal.SIGTERM, signal.SIG_DFL)\n{WAITING_NATIVE_WORKER}"
        ),
    )
    .unwrap();
    let mut shell = Shell::start(&repo);
    shell.send(&format!(
        "RUST_LOG={} python3 -c 'import os,signal,sys; signal.signal(signal.SIGTERM,signal.SIG_IGN); os.execv(sys.argv[1],sys.argv[1:])' \"$WT_JOB_TEST_BINARY\" -vv hook post-start --yes --foreground; printf 'RESULT_%s\\n' \"$?\"\n",
        crate::common::FOREGROUND_TRACE_FILTER
    ));
    let info = native_info(&repo, "worker-info");
    shell.remember(info[0]);
    shell.remember(info[2]);
    crate::common::wait_for_foreground_admission(&repo, &[info[0]]);
    kill(Pid::from_raw(info[2]), Signal::SIGTERM).unwrap();
    std::thread::sleep(crate::common::SLEEP_FOR_ABSENCE_CHECK);
    assert_eq!(shell.foreground(), info[1]);
    assert!(nix::unistd::getpgid(Some(Pid::from_raw(info[0]))).is_ok());
    assert!(!repo.root_path().join("next").exists());
    std::fs::write(repo.root_path().join("release"), "").unwrap();
    shell.wait_for("RESULT_0");
    assert!(repo.root_path().join("next").exists());
}

/// An explicit disabled pager ends precedence instead of selecting a lower
/// source or the interactive default. Exercise the actual TTY help entrypoint.
#[rstest]
#[case::git_cat("git", "cat")]
#[case::git_empty("git", "")]
#[case::core_cat("core", "cat")]
#[case::core_empty("core", "")]
#[case::pager_cat("pager", "cat")]
#[case::pager_empty("pager", "")]
fn explicit_disabled_pager_stops_selection(
    repo: TestRepo,
    #[case] source: &str,
    #[case] value: &str,
) {
    let lower = repo.root_path().join("lower-pager.sh");
    std::fs::write(&lower, "printf 'LOWER_PAGER_USED\\n'\ncat > /dev/null\n").unwrap();
    let lower = format!("sh {}", shell_escape::unix::escape(lower.to_string_lossy()));
    if source != "pager" {
        repo.run_git(&[
            "config",
            "core.pager",
            if source == "core" { value } else { &lower },
        ]);
    }
    let assignments = match source {
        "git" => format!(
            "GIT_PAGER={} PAGER={}",
            shell_escape::unix::escape(value.into()),
            shell_escape::unix::escape(lower.into())
        ),
        "core" => format!("PAGER={}", shell_escape::unix::escape(lower.into())),
        "pager" => format!("PAGER={}", shell_escape::unix::escape(value.into())),
        _ => unreachable!(),
    };
    let mut shell = Shell::start(&repo);
    shell.send(&format!(
        "unset GIT_PAGER PAGER; {assignments} \"$WT_JOB_TEST_BINARY\" --help\n"
    ));
    shell.wait_for("job-test> ");
    assert!(shell.output.contains("Usage:"), "{}", shell.output);
    assert!(
        !shell.output.contains("LOWER_PAGER_USED"),
        "{}",
        shell.output
    );
}

/// Global Git pager configuration applies even when help runs outside a repo.
#[rstest]
#[case::cat(Some("cat"))]
#[case::empty(Some(""))]
#[case::command(None)]
fn help_pager_uses_global_config_outside_repository(
    repo: TestRepo,
    #[case] disabled: Option<&str>,
) {
    let outside = repo.home_path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let lower = repo.root_path().join("lower-pager.sh");
    let global = repo.root_path().join("global-pager.sh");
    std::fs::write(&lower, "printf 'LOWER_PAGER_USED\\n'\ncat > /dev/null\n").unwrap();
    std::fs::write(&global, "printf 'GLOBAL_PAGER_USED\\n'\ncat > /dev/null\n").unwrap();
    let quote =
        |path: &std::path::Path| shell_escape::unix::escape(path.to_string_lossy()).into_owned();
    let global_command = format!("sh {}", quote(&global));
    let config = repo.home_path().join("pager.gitconfig");
    repo.run_git(&[
        "config",
        "--file",
        config.to_str().unwrap(),
        "core.pager",
        disabled.unwrap_or(&global_command),
    ]);
    let mut shell = Shell::start(&repo);
    shell.send(&format!(
        "cd {}; unset GIT_PAGER PAGER; GIT_CONFIG_GLOBAL={} PAGER={} \"$WT_JOB_TEST_BINARY\" --help\n",
        quote(&outside), quote(&config), shell_escape::unix::escape(format!("sh {}", quote(&lower)).into()),
    ));
    shell.wait_for("job-test> ");
    assert_eq!(
        shell.output.contains("Usage:"),
        disabled.is_some(),
        "{}",
        shell.output
    );
    assert_eq!(
        shell.output.contains("GLOBAL_PAGER_USED"),
        disabled.is_none(),
        "{}",
        shell.output
    );
    assert!(
        !shell.output.contains("LOWER_PAGER_USED"),
        "{}",
        shell.output
    );
}

/// Paging is optional: unavailable Git or malformed configuration must leave
/// help readable, without selecting an unverified lower-priority command.
#[rstest]
#[case::missing_git(true)]
#[case::invalid_config(false)]
fn help_remains_available_when_pager_lookup_fails(repo: TestRepo, #[case] missing_git: bool) {
    let config = repo.home_path().join("invalid.gitconfig");
    std::fs::write(&config, "[invalid\n").unwrap();
    let lower = repo.root_path().join("lower-pager.sh");
    std::fs::write(&lower, "printf 'LOWER_PAGER_USED\\n'\ncat > /dev/null\n").unwrap();
    let environment = if missing_git {
        "PATH=/worktrunk-test-no-programs".to_string()
    } else {
        format!(
            "GIT_CONFIG_GLOBAL={}",
            shell_escape::unix::escape(config.to_string_lossy())
        )
    };
    let mut shell = Shell::start(&repo);
    shell.send(&format!(
        "unset GIT_PAGER PAGER; {environment} PAGER={} \"$WT_JOB_TEST_BINARY\" --help\n",
        shell_escape::unix::escape(
            format!("sh {}", shell_escape::unix::escape(lower.to_string_lossy())).into()
        ),
    ));
    shell.wait_for("job-test> ");
    assert!(shell.output.contains("Usage:"), "{}", shell.output);
    assert!(
        !shell.output.contains("LOWER_PAGER_USED"),
        "{}",
        shell.output
    );
}

/// From outside the target repository, `-C` selects its local pager over global.
#[rstest]
fn help_pager_honors_c_base_path(repo: TestRepo) {
    let local = repo.root_path().join("local-pager.sh");
    let global = repo.root_path().join("global-pager.sh");
    std::fs::write(&local, "printf 'LOCAL_PAGER_USED\\n'\ncat > /dev/null\n").unwrap();
    std::fs::write(&global, "printf 'GLOBAL_PAGER_USED\\n'\ncat > /dev/null\n").unwrap();
    let quote =
        |path: &std::path::Path| shell_escape::unix::escape(path.to_string_lossy()).into_owned();
    let config = repo.home_path().join("pager.gitconfig");
    repo.run_git(&[
        "config",
        "--file",
        config.to_str().unwrap(),
        "core.pager",
        &format!("sh {}", quote(&global)),
    ]);
    repo.run_git(&["config", "core.pager", &format!("sh {}", quote(&local))]);
    let mut shell = Shell::start(&repo);
    shell.send(&format!(
        "cd {}; unset GIT_PAGER PAGER; GIT_CONFIG_GLOBAL={} \"$WT_JOB_TEST_BINARY\" -C {} --help\n",
        quote(repo.home_path()),
        quote(&config),
        quote(repo.root_path()),
    ));
    shell.wait_for("job-test> ");
    assert!(
        shell.output.contains("LOCAL_PAGER_USED"),
        "{}",
        shell.output
    );
    assert!(
        !shell.output.contains("GLOBAL_PAGER_USED"),
        "{}",
        shell.output
    );
}

/// Lossy decoding could turn an invalid command into executable shell text.
#[rstest]
fn invalid_utf8_pager_is_not_executed(repo: TestRepo) {
    let config = repo.home_path().join("pager.gitconfig");
    std::fs::write(
        &config,
        b"[core]\n\tpager = \"printf INVALID_PAGER_EXECUTED; cat # \xff\"\n",
    )
    .unwrap();
    let mut shell = Shell::start(&repo);
    shell.send(&format!(
        "unset GIT_PAGER PAGER; GIT_CONFIG_GLOBAL={} \"$WT_JOB_TEST_BINARY\" --help\n",
        shell_escape::unix::escape(config.to_string_lossy()),
    ));
    shell.wait_for("job-test> ");
    assert!(shell.output.contains("Usage:"), "{}", shell.output);
    assert!(
        !shell.output.contains("INVALID_PAGER_EXECUTED"),
        "{}",
        shell.output
    );
}

/// Custom commands and help pagers keep the native execution lease until the
/// child finishes. A caught key remains input; targeted TERM cancels wt.
#[rstest]
#[case::custom_caught_success(false, "caught-key", 0, 0)]
#[case::custom_caught_130(false, "caught-key", 130, 130)]
#[case::custom_raw_int(false, "raw-key", 0, 130)]
#[case::custom_caught_term(false, "caught-term", 0, 143)]
#[case::pager_caught_success(true, "caught-key", 0, 0)]
#[case::pager_caught_130(true, "caught-key", 130, 0)]
#[case::pager_raw_int(true, "raw-key", 0, 130)]
#[case::pager_raw_term(true, "raw-term", 0, 143)]
#[case::help_raw_term(true, "help-term", 0, 143)]
#[case::pager_caught_term(true, "caught-term", 0, 143)]
fn custom_commands_and_help_pagers_preserve_native_signals(
    repo: TestRepo,
    #[case] pager: bool,
    #[case] mode: &str,
    #[case] child_exit: i32,
    #[case] expected: i32,
) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::{Pid, getpgid};
    use std::os::unix::fs::PermissionsExt;

    let bin = repo.home_path().join("native-bin");
    std::fs::create_dir(&bin).unwrap();
    let worker = bin.join("wt-nativecancel");
    std::fs::write(
        &worker,
        format!(
            r#"#!{}
import os, signal, sys, time
from pathlib import Path
pager, mode, code = sys.argv[1], sys.argv[2], int(sys.argv[3])
def caught(*_):
    Path('ack').write_text('caught')
    if mode == 'caught-term':
        sys.exit(0)
signal.signal(signal.SIGINT, caught if mode == 'caught-key' else signal.SIG_DFL)
signal.signal(signal.SIGTERM, caught if mode == 'caught-term' else signal.SIG_DFL)
if pager == 'pager':
    sys.stdin.buffer.read()
Path('worker-info').write_text(f'{{os.getpid()}} {{os.getpgrp()}} {{os.getppid()}}')
while not Path('release').exists():
    time.sleep(.01)
sys.exit(code)
"#,
            which::which("python3").unwrap().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
    let quote =
        |path: &std::path::Path| shell_escape::unix::escape(path.to_string_lossy()).into_owned();
    let command = if pager {
        format!(
            "GIT_PAGER={} \"$WT_JOB_TEST_BINARY\" {}",
            shell_escape::unix::escape(
                format!("exec {} pager {mode} {child_exit}", quote(&worker)).into()
            ),
            if mode == "help-term" {
                "--help"
            } else {
                "-vv config show"
            }
        )
    } else {
        format!(
            "PATH={}:$PATH \"$WT_JOB_TEST_BINARY\" -vv nativecancel custom {mode} {child_exit}",
            quote(&bin)
        )
    };
    let mut shell = Shell::start(&repo);
    shell.send(&format!(
        "RUST_LOG={} {command}\n",
        crate::common::FOREGROUND_TRACE_FILTER
    ));
    let info = native_info(&repo, "worker-info");
    shell.remember(info[0]);
    shell.remember(info[2]);
    assert_eq!(info[1], shell.foreground());
    assert_eq!(
        getpgid(Some(Pid::from_raw(info[2]))).unwrap().as_raw(),
        info[1]
    );
    if mode.ends_with("key") {
        crate::common::wait_for_foreground_admission(&repo, &[info[0]]);
        shell.send("\x03");
    } else {
        let target = if matches!(mode, "raw-term" | "help-term") {
            info[0]
        } else {
            info[2]
        };
        kill(Pid::from_raw(target), Signal::SIGTERM).unwrap();
    }
    if mode == "caught-term" {
        // Controller status alone could hide an unforwarded TERM and a live
        // orphan. Establish that the child actually handled the forwarded signal.
        crate::common::wait_for_file_content(&repo.root_path().join("ack"));
    }
    if mode == "caught-key" {
        crate::common::wait_for_file_content(&repo.root_path().join("ack"));
        assert_eq!(
            shell.foreground(),
            info[1],
            "wt must await the caught key's cleanup"
        );
        assert_eq!(
            getpgid(Some(Pid::from_raw(info[2]))).unwrap().as_raw(),
            info[1]
        );
        std::fs::write(repo.root_path().join("release"), "").unwrap();
    }
    shell.wait_for("job-test> ");
    if pager {
        // Verbose trace records retain the physical child status. A separate
        // user-facing failure for cancellation would be misleading.
        assert!(
            !shell.output.contains("✗ killed by signal") && !shell.output.contains("✗ exit code"),
            "{}",
            shell.output
        );
        assert!(
            !shell.output.contains("Usage:"),
            "help must not bypass its pager: {}",
            shell.output
        );
    }
    // Bash aborts a compound command after a child's raw SIGINT. Read its
    // native status from fresh input after the completed foreground job.
    shell.send("printf 'RESULT_%s\\n' \"$?\"\n");
    shell.wait_for(&format!("RESULT_{expected}"));
}

#[rstest]
#[case::single(false, false)]
#[case::concurrent(true, false)]
#[case::single_pipeline(false, true)]
#[case::concurrent_pipeline(true, true)]
fn children_inherit_foreground_group_and_pipeline_terminal(
    repo: TestRepo,
    #[case] concurrent: bool,
    #[case] pipeline: bool,
) {
    repo.write_project_config(&native_pipeline(concurrent));
    std::fs::write(repo.root_path().join("worker.py"), WAITING_NATIVE_WORKER).unwrap();
    std::fs::write(
        repo.root_path().join("sibling.py"),
        r#"import os, subprocess, sys, time
from pathlib import Path
Path('sibling-info').write_text(f'{os.getpid()} {os.getpgrp()}')
while not Path('worker-info').exists():
    time.sleep(.01)
with open('/dev/tty', 'rb') as tty:
    subprocess.run(['stty', '-ixon'], stdin=tty, check=True)
Path('tty-access').write_text('success')
for line in sys.stdin:
    pass
"#,
    )
    .unwrap();
    let mut shell = Shell::start(&repo);
    shell.send(if pipeline {
        "\"$WT_JOB_TEST_BINARY\" hook post-start --yes --foreground | python3 sibling.py; printf 'RESULT_%s\\n' \"$?\"\n"
    } else {
        "\"$WT_JOB_TEST_BINARY\" hook post-start --yes --foreground; printf 'RESULT_%s\\n' \"$?\"\n"
    });
    let info = native_info(&repo, "worker-info");
    shell.remember(info[0]);
    shell.remember(info[2]);
    let group = nix::unistd::Pid::from_raw(info[1]);
    assert_eq!(shell.foreground(), group.as_raw());
    assert_eq!(
        nix::unistd::getpgid(Some(nix::unistd::Pid::from_raw(info[2]))).unwrap(),
        group,
        "wt and its worker must remain in the same native group"
    );
    if pipeline {
        let sibling = native_info(&repo, "sibling-info");
        shell.remember(sibling[0]);
        assert_eq!(sibling[1], group.as_raw());
        crate::common::wait_for_file_content(&repo.root_path().join("tty-access"));
    }
    std::fs::write(repo.root_path().join("release"), "release").unwrap();
    shell.wait_for("RESULT_0");
    assert_eq!(
        std::fs::read_to_string(repo.root_path().join("next")).unwrap(),
        "NEXT"
    );
}

#[rstest]
#[case::single_one(false, 1)]
#[case::single_two(false, 2)]
#[case::concurrent_one(true, 1)]
#[case::concurrent_two(true, 2)]
fn caught_keyboard_interrupt_preserves_success_and_following_step(
    repo: TestRepo,
    #[case] concurrent: bool,
    #[case] keys: usize,
) {
    repo.write_project_config(if concurrent {
        "post-start = [{ one = 'exec python3 worker.py', two = 'exec python3 companion.py' }, 'printf NEXT > next']"
    } else {
        "post-start = ['exec python3 worker.py', 'printf NEXT > next']"
    });
    std::fs::write(
        repo.root_path().join("companion.py"),
        format!(
            r#"import os, signal, time
from pathlib import Path
signal.signal(signal.SIGINT, signal.SIG_IGN)
Path('companion-info').write_text(f'{{os.getpid()}} {{os.getpgrp()}}')
while not Path('count').exists() or Path('count').read_text() != '{keys}':
    time.sleep(.01)
"#
        ),
    )
    .unwrap();
    std::fs::write(
        repo.root_path().join("worker.py"),
        format!(
            r#"import os, signal, time
from pathlib import Path
count = 0
def interrupted(*_):
    global count
    count += 1
    Path('count').write_text(str(count))
signal.signal(signal.SIGINT, interrupted)
Path('worker-info').write_text(f'{{os.getpid()}} {{os.getpgrp()}}')
while count < {keys}:
    time.sleep(.01)
"#
        ),
    )
    .unwrap();
    let mut shell = Shell::start(&repo);
    shell.send(&format!(
        "RUST_LOG={} \"$WT_JOB_TEST_BINARY\" -vv hook post-start --yes --foreground; printf 'RESULT_%s\\n' \"$?\"\n",
        crate::common::FOREGROUND_TRACE_FILTER
    ));
    let info = native_info(&repo, "worker-info");
    shell.remember(info[0]);
    assert_eq!(info[1], shell.foreground());
    if concurrent {
        let companion = native_info(&repo, "companion-info");
        shell.remember(companion[0]);
        assert_eq!(companion[1], info[1]);
        crate::common::wait_for_foreground_admission(&repo, &[companion[0]]);
    }
    crate::common::wait_for_foreground_admission(&repo, &[info[0]]);
    for key in 1..=keys {
        shell.send("\x03");
        let expected = key.to_string();
        let deadline = Instant::now() + Duration::from_secs(20);
        while std::fs::read_to_string(repo.root_path().join("count"))
            .ok()
            .as_deref()
            != Some(expected.as_str())
        {
            assert!(Instant::now() < deadline, "child did not handle key {key}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    shell.wait_for("RESULT_0");
    assert_eq!(
        std::fs::read_to_string(repo.root_path().join("count")).unwrap(),
        keys.to_string()
    );
    assert!(repo.root_path().join("next").exists());
}

#[rstest]
fn startup_interrupt_preserves_already_started_child_cleanup(repo: TestRepo) {
    let mut commands = vec![
        "a000 = 'exec python3 worker.py'".to_string(),
        "a001 = 'while [ ! -f worker-info ]; do :; done; kill -STOP \"$PPID\"; printf ready > paused-admitted; while :; do :; done'".to_string(),
    ];
    commands.extend((2..120).map(|index| {
        format!("a{index:03} = 'printf ready > admitted-{index:03}; while :; do :; done'")
    }));
    repo.write_project_config(&format!(
        "post-start = [{{ {} }}, 'printf NEXT > next']",
        commands.join(", ")
    ));
    std::fs::write(
        repo.root_path().join("worker.py"),
        r#"import os, signal, time
from pathlib import Path
interrupted = False
def on_int(*_):
    global interrupted
    interrupted = True
    Path('ack').write_text('handled')
signal.signal(signal.SIGINT, on_int)
Path('worker-info').write_text(f'{os.getpid()} {os.getpgrp()} {os.getppid()}')
while not interrupted:
    time.sleep(.01)
print('CLEANUP_ENTERED', flush=True)
while not Path('release-cleanup').exists():
    time.sleep(.01)
Path('cleanup').write_text('finished')
print('CLEANUP_FINISHED', flush=True)
"#,
    )
    .unwrap();
    // A waiting caller keeps the job foreground while only wt is stopped;
    // an interactive shell would take the terminal back on its child's stop.
    let mut command = CommandBuilder::new("python3");
    command.args(["-c", r#"import os, signal, subprocess
for number in [signal.SIGINT, signal.SIGTERM]:
    signal.signal(number, lambda *_: None)
owner = subprocess.Popen([os.environ['WT_JOB_TEST_BINARY'], 'hook', 'post-start', '--yes', '--foreground'])
code = owner.wait()
print('RESULT_' + str(128 - code if code < 0 else code), flush=True)
input()
"#]);
    let mut shell = Shell::spawn(&repo, command);
    let info = native_info(&repo, "worker-info");
    shell.remember(info[0]);
    shell.remember(info[2]);
    assert_eq!(info[1], shell.foreground());

    // Starting the second command proves that the first was admitted before
    // only the controller stops. Establish that startup boundary before the key.
    crate::common::wait_for_file_content(&repo.root_path().join("paused-admitted"));
    let owner = info[2].to_string();
    let state = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &owner])
        .output()
        .unwrap();
    assert!(state.status.success());
    assert!(
        String::from_utf8_lossy(&state.stdout)
            .trim()
            .starts_with('T')
    );
    let processes = std::process::Command::new("ps")
        .args(["-Ao", "pid=,ppid="])
        .output()
        .unwrap();
    assert!(processes.status.success());
    let started = String::from_utf8_lossy(&processes.stdout)
        .lines()
        .filter(|line| {
            line.split_whitespace()
                .nth(1)
                .is_some_and(|parent| parent == owner)
        })
        .count();
    assert!(
        (2..120).contains(&started),
        "startup already completed: {started}"
    );

    shell.send("\x03");
    crate::common::wait_for_file_content(&repo.root_path().join("ack"));
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(info[2]),
        nix::sys::signal::Signal::SIGCONT,
    )
    .unwrap();
    // This line can reach the terminal only through wt's output drain. Keep
    // cleanup blocked until that proves startup cancellation is waiting for it.
    shell.wait_for("CLEANUP_ENTERED");
    std::fs::write(repo.root_path().join("release-cleanup"), "release").unwrap();
    shell.wait_for("RESULT_130");
    assert_eq!(
        std::fs::read_to_string(repo.root_path().join("cleanup")).unwrap(),
        "finished"
    );
    assert!(shell.output.contains("CLEANUP_FINISHED"));
    assert!(!repo.root_path().join("next").exists());
}

#[rstest]
fn keyboard_suspend_and_shell_fg_resume_the_native_job(repo: TestRepo) {
    repo.write_project_config(&native_pipeline(true));
    std::fs::write(repo.root_path().join("worker.py"), WAITING_NATIVE_WORKER).unwrap();
    let mut shell = Shell::start(&repo);
    let caller = shell.foreground();
    shell.send("\"$WT_JOB_TEST_BINARY\" hook post-start --yes --foreground\n");
    let info = native_info(&repo, "worker-info");
    shell.remember(info[0]);
    shell.remember(info[2]);
    shell.send("\x1a");
    shell.wait_for("job-test> ");
    assert_eq!(shell.foreground(), caller);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let stopped = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &info[0].to_string()])
            .output()
            .unwrap();
        assert!(stopped.status.success());
        if String::from_utf8_lossy(&stopped.stdout)
            .trim()
            .starts_with('T')
        {
            break;
        }
        assert!(Instant::now() < deadline, "worker did not stop");
        std::thread::sleep(Duration::from_millis(10));
    }
    shell.send("fg; printf 'RESUMED_%s\\n' \"$?\"\n");
    let deadline = Instant::now() + Duration::from_secs(20);
    while shell.foreground() != info[1] {
        assert!(
            Instant::now() < deadline,
            "shell did not foreground the job"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(repo.root_path().join("release"), "release").unwrap();
    shell.wait_for("RESUMED_0");
    assert!(repo.root_path().join("next").exists());
}

#[rstest]
fn cancelled_concurrent_drain_preserves_final_partial_diagnostic(repo: TestRepo) {
    repo.write_project_config(&native_pipeline(true));
    std::fs::write(
        repo.root_path().join("worker.py"),
        r#"import os, signal, subprocess, time
from pathlib import Path
descendant = subprocess.Popen(['sleep', '60'])
def terminated(*_):
    os.write(2, b'FINAL_PARTIAL_DIAGNOSTIC')
    signal.signal(signal.SIGTERM, signal.SIG_DFL)
    os.kill(os.getpid(), signal.SIGTERM)
signal.signal(signal.SIGTERM, terminated)
Path('worker-info').write_text(f'{os.getpid()} {os.getpgrp()} {descendant.pid}')
while True:
    time.sleep(60)
"#,
    )
    .unwrap();
    let mut command = CommandBuilder::new("python3");
    command.args(["-c", r#"import os, signal, subprocess
from pathlib import Path
for number in [signal.SIGINT, signal.SIGTERM]:
    signal.signal(number, lambda *_: None)
owner = subprocess.Popen([os.environ['WT_JOB_TEST_BINARY'], 'hook', 'post-start', '--yes', '--foreground'])
Path('owner').write_text(str(owner.pid))
code = owner.wait()
print('RESULT_' + str(128 - code if code < 0 else code), flush=True)
input()
"#]);
    let mut shell = Shell::spawn(&repo, command);
    let info = native_info(&repo, "worker-info");
    let owner = native_info(&repo, "owner")[0];
    shell.remember(info[0]);
    shell.remember(info[2]);
    shell.remember(owner);
    let session = shell.child.process_id().unwrap() as i32;
    assert_eq!(info[1], session);
    assert_eq!(shell.foreground(), session);
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(owner),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();
    shell.wait_for("RESULT_143");
    assert!(shell.output.contains("FINAL_PARTIAL_DIAGNOSTIC"));
    assert!(!repo.root_path().join("next").exists());
    // The successful waiting caller still pins this foreground group. Dropping
    // Shell kills only that owned session group, including the pipe holder.
}

#[rstest]
fn interrupt_cancels_completed_drain_while_another_prune_hook_is_active(mut repo: TestRepo) {
    std::fs::write(
        repo.root_path().join("prune-worker.py"),
        r#"import os, signal, sys, time
from pathlib import Path
base = Path(__file__).resolve().parent
if sys.argv[1] == 'draining':
    direct = os.getpid()
    if os.fork():
        while not (base / 'draining-info').exists():
            time.sleep(.01)
        os._exit(0)
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    (base / 'draining-info').write_text(f'{os.getpid()} {os.getpgrp()} {direct}')
else:
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    (base / 'active-info').write_text(f'{os.getpid()} {os.getpgrp()}')
while True:
    time.sleep(60)
"#,
    )
    .unwrap();
    let worker = format!(
        "exec python3 {} {{{{ branch }}}}",
        shell_escape::unix::escape(repo.root_path().join("prune-worker.py").to_string_lossy())
    );
    repo.write_project_config(&format!(
        "pre-remove = {{ one = {}, two = 'true' }}",
        serde_json::to_string(&worker).unwrap()
    ));
    repo.commit("Add overlapping prune hooks");
    repo.add_worktree("draining");
    repo.add_worktree("active");

    let mut command = CommandBuilder::new("python3");
    command.args(["-c", r#"import os, signal, subprocess
from pathlib import Path
for number in [signal.SIGINT, signal.SIGTERM]:
    signal.signal(number, lambda *_: None)
owner = subprocess.Popen([os.environ['WT_JOB_TEST_BINARY'], 'step', 'prune', '--yes', '--min-age=0s'], env=dict(os.environ, RAYON_NUM_THREADS='2'))
Path('owner').write_text(str(owner.pid))
code = owner.wait()
print('RESULT_' + str(128 - code if code < 0 else code), flush=True)
input()
"#]);
    let mut shell = Shell::spawn(&repo, command);
    let draining = native_info(&repo, "draining-info");
    let active = native_info(&repo, "active-info");
    let owner = native_info(&repo, "owner")[0];
    shell.remember(draining[0]);
    shell.remember(active[0]);
    shell.remember(owner);
    assert_eq!(draining[1], shell.foreground());
    assert_eq!(active[1], draining[1]);

    // A's direct command has completed, but its ignored-INT descendant still
    // holds the pipes. B keeps the process-wide foreground signal observer live.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match nix::unistd::getpgid(Some(nix::unistd::Pid::from_raw(draining[2]))) {
            Err(nix::errno::Errno::ESRCH) => break,
            Ok(_) => {}
            result => panic!("querying completed command: {result:?}"),
        }
        assert!(Instant::now() < deadline, "direct command did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    shell.send("\x03");
    shell.wait_for("RESULT_130");
    // The waiting caller pins the owned session until Shell's Drop cleans up
    // the descendant. Completion must not depend on releasing its output pipes.
}

/// A best-effort timed capture can consume its own child's TERM failure.
/// It must not turn an independent healthy foreground stream into cancellation.
#[cfg(unix)]
#[test]
fn captured_child_signal_does_not_cancel_foreground_stream() {
    use std::os::unix::process::ExitStatusExt;
    use std::time::{Duration, Instant};
    use worktrunk::shell_exec::Cmd;

    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready");
    let release = dir.path().join("release");
    let body = format!(
        "printf ready > {}; while [ ! -f {} ]; do :; done",
        shell_escape::escape(ready.to_string_lossy()),
        shell_escape::escape(release.to_string_lossy()),
    );

    // Release before thread::scope joins, including assertion unwinding.
    struct ReleaseOnDrop(std::path::PathBuf);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::write(&self.0, "release");
        }
    }

    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            Cmd::new("sh")
                .args(["-c", &body])
                .forward_signals()
                .stream()
        });
        let release_guard = ReleaseOnDrop(release);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ready.exists(), "foreground child did not become ready");

        let captured = Cmd::new("sh")
            .args(["-c", "kill -TERM $$"])
            .timeout(Duration::from_secs(1))
            .run()
            .unwrap();
        assert_eq!(captured.status.signal(), Some(15));
        std::fs::write(&release_guard.0, "release").unwrap();
        worker
            .join()
            .unwrap()
            .expect("a consumed capture failure cancelled a healthy foreground stream");
    });

    Cmd::new("true")
        .forward_signals()
        .stream()
        .expect("interruption leaked into the subsequent foreground operation");
}

/// Deadlines and native interruption cover output EOF, including descendants.
#[test]
fn captured_deadline_and_delayed_interrupt_bound_descendant_output() {
    use worktrunk::git::ErrorExt;
    use worktrunk::shell_exec::Cmd;

    // Capture deadline, fully buffered interruption, and a quiet pre-threshold exit.
    for delay in [None, Some(-1), Some(30_000)] {
        let dir = tempfile::tempdir().unwrap();
        let release = dir.path().join("release");
        let finished = dir.path().join("finished");
        let body = format!(
            "(finish() {{ printf finished > {}; }}; trap 'finish; exit 0' TERM; while [ ! -f {} ]; do sleep .01; done; finish) & printf prior; {}",
            shell_escape::escape(finished.to_string_lossy()),
            shell_escape::escape(release.to_string_lossy()),
            if delay.is_none() {
                "exit 0"
            } else {
                "kill -TERM $$"
            },
        );
        // A safety watchdog releases the pipe holder if the wait regresses.
        let (release_tx, release_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let watchdog = scope.spawn(move || {
                let fallback = release_rx.recv_timeout(Duration::from_secs(10)).is_err();
                std::fs::write(&release, "release").unwrap();
                let deadline = Instant::now() + Duration::from_secs(10);
                while !finished.exists() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                assert!(finished.exists(), "descendant did not finish after release");
                fallback
            });
            if let Some(delay) = delay {
                let error = Cmd::new("sh")
                    .args(["-c", &body])
                    .delayed_stream(delay, None)
                    .unwrap_err();
                assert_eq!(error.interrupt_signal(), Some(15));
                let failure = error
                    .downcast_ref::<worktrunk::shell_exec::StreamCommandError>()
                    .unwrap();
                assert_eq!(failure.output, "prior");
            } else {
                let error = Cmd::new("sh")
                    .args(["-c", &body])
                    .timeout(Duration::from_millis(200))
                    .run()
                    .expect_err("a retained output pipe must respect the capture deadline");
                assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            }
            release_tx.send(()).unwrap();
            assert!(
                !watchdog.join().unwrap(),
                "output wait required watchdog release"
            );
        });
    }
}

fn wrapper_prompt(shell: &mut Shell, marker: &str) {
    shell.wait_for(marker);
    // The setup line itself names the prompt, and a preceding command may
    // leave its prompt queued. Consume the prompt after this actual marker.
    let end = shell.output.rfind(marker).unwrap() + marker.len();
    shell.output.drain(..end);
    shell.wait_for("wrapper-prompt> ");
}

// Record the producer's actual path: BSD mktemp need not use TMPDIR.
fn recording_wrapper_binary(repo: &TestRepo, binary: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let quote = |path: &std::path::Path| {
        shell_escape::escape(path.to_string_lossy().into_owned().into()).into_owned()
    };
    let launcher = repo.root_path().join("wrapper-recording-binary");
    std::fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$WORKTRUNK_DIRECTIVE_CD_FILE\" >> {}\nexec {} \"$@\"\n",
            quote(&repo.root_path().join("wrapper-directive-files")),
            quote(binary)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
    launcher
}

fn assert_wrapper_directives_removed(repo: &TestRepo) {
    let paths = std::fs::read_to_string(repo.root_path().join("wrapper-directive-files")).unwrap();
    assert!(!paths.is_empty(), "the wrapper never ran its binary");
    for path in paths.lines() {
        assert!(
            !std::path::Path::new(path).exists(),
            "directive file leaked: {path}"
        );
    }
}

fn wrapper_shell(repo: &TestRepo, name: &str, binary: &std::path::Path) -> Shell {
    let dialect = if name == "/bin/bash" { "bash" } else { name };
    let temporary = repo.root_path().join("wrapper-temporary");
    std::fs::create_dir(&temporary).unwrap();
    let wrapper = std::process::Command::new(wt_bin())
        .args(["config", "shell", "init", dialect])
        .env("WORKTRUNK_CONFIG_PATH", repo.test_config_path())
        .current_dir(repo.root_path())
        .output()
        .unwrap();
    assert!(wrapper.status.success());
    std::fs::write(repo.root_path().join("wrapper-init"), wrapper.stdout).unwrap();
    let mut command = CommandBuilder::new(crate::common::shell::shell_binary(name));
    match dialect {
        "bash" => command.args(["--noprofile", "--norc", "-i"]),
        "zsh" => command.args(["-f", "-i"]),
        "fish" => command.args(["--no-config", "-i"]),
        _ => unreachable!(),
    }
    let mut shell = Shell::spawn(repo, command);
    let temporary = shell_escape::escape(temporary.to_string_lossy().into_owned().into());
    let binary = recording_wrapper_binary(repo, binary);
    let binary = shell_escape::escape(binary.to_string_lossy().into_owned().into());
    let target = shell_escape::escape(repo.root_path().to_string_lossy().into_owned().into());
    let setup = if name == "fish" {
        format!(
            "function fish_prompt; printf 'wrapper-prompt> '; end; set -gx WORKTRUNK_BIN {binary}; set -gx WT_WRAPPER_TEST_TARGET {target}; set -gx TMPDIR {temporary}; source wrapper-init"
        )
    } else {
        format!(
            "export WORKTRUNK_BIN={binary} WT_WRAPPER_TEST_TARGET={target} TMPDIR={temporary}; source wrapper-init; PS1='wrapper-prompt> '"
        )
    };
    shell.send(&format!("{setup}; printf 'WRAPPER_%s\\n' READY\n"));
    wrapper_prompt(&mut shell, "WRAPPER_READY");
    shell
}

/// Nushell must apply the directive and release its temp files even when a
/// real terminal Ctrl-C unwinds a noninteractive script.
#[rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
fn nushell_interrupt_runs_wrapper_cleanup(
    mut repo: TestRepo,
    #[case] interactive: bool,
    #[case] caught: bool,
) {
    let feature = repo.add_worktree("feature");
    let temporary = repo.root_path().join("wrapper-temporary");
    std::fs::create_dir(&temporary).unwrap();
    let wrapper = repo
        .wt_command()
        .args(["config", "shell", "init", "nu"])
        .output()
        .unwrap();
    assert!(wrapper.status.success());
    std::fs::write(repo.root_path().join("wrapper-init"), wrapper.stdout).unwrap();
    std::fs::write(
        repo.root_path().join("worker.py"),
        format!("import signal, sys\nsignal.signal(signal.SIGINT, {})\nsignal.pthread_sigmask(signal.SIG_UNBLOCK, {{signal.SIGINT}})\n{WAITING_NATIVE_WORKER}", if caught { "lambda *_: sys.exit(0)" } else { "signal.SIG_DFL" }),
    )
    .unwrap();
    let quote = |path: &std::path::Path| serde_json::to_string(&path.to_string_lossy()).unwrap();
    let binary = recording_wrapper_binary(&repo, &wt_bin());
    let script = format!(
        "$env.WORKTRUNK_BIN = {}; $env.TMPDIR = {}; $env.RUST_LOG = '{}'; source wrapper-init; cd {}; try {{ wt -vv switch main --yes --execute python3 -- {} }} finally {{ $env.PWD | save {} }}",
        quote(&binary),
        quote(&temporary),
        crate::common::FOREGROUND_TRACE_FILTER,
        quote(&feature),
        quote(&repo.root_path().join("worker.py")),
        quote(&repo.root_path().join("wrapper-cwd")),
    );
    let mut command = CommandBuilder::new("nu");
    command.arg("--no-config-file");
    if interactive {
        command.arg("-i");
    } else {
        command.args(["-c", &script]);
    }
    let mut shell = Shell::spawn(&repo, command);
    if interactive {
        shell.send(&format!("{script}\n"));
    }
    let info = native_info(&repo, "worker-info");
    shell.remember(info[0]);
    shell.remember(info[2]);
    assert_eq!(info[1], shell.foreground());
    crate::common::wait_for_foreground_admission(&repo, &[info[0]]);
    shell.send("\u{3}");
    crate::common::wait_for_file_content(&repo.root_path().join("wrapper-cwd"));
    assert_eq!(
        std::fs::read_to_string(repo.root_path().join("wrapper-cwd"))
            .unwrap()
            .trim(),
        repo.root_path().to_str().unwrap()
    );
    assert_wrapper_directives_removed(&repo);
    // Nushell's mktemp --tmpdir also puts its captured stdout file here.
    assert_eq!(std::fs::read_dir(temporary).unwrap().count(), 0);
}

#[rstest]
#[case("bash", "default")]
#[case("bash", "custom")]
#[case("bash", "ignored")]
#[case("zsh", "default")]
#[case("zsh", "custom")]
#[case("zsh", "ignored")]
#[case("fish", "default")]
#[case("fish", "custom")]
#[case("fish", "ignored")]
fn interrupted_shell_wrapper_applies_directive_and_cleans_up(
    mut repo: TestRepo,
    #[case] name: &str,
    #[case] disposition: &str,
) {
    let feature = repo.add_worktree("feature");
    std::fs::write(
        repo.root_path().join("wrapper-worker.py"),
        r#"import os, signal, sys, time
from pathlib import Path
if len(sys.argv) > 1:
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    signal.pthread_sigmask(signal.SIG_UNBLOCK, {signal.SIGINT})
    os.kill(os.getpid(), signal.SIGINT)
seen = False
def on_int(*_):
    global seen
    seen = True
signal.signal(signal.SIGINT, on_int)
signal.pthread_sigmask(signal.SIG_UNBLOCK, {signal.SIGINT})
Path('wrapper-info').write_text(f'{os.getpid()} {os.getpgrp()} {os.getppid()}')
while not seen:
    time.sleep(.01)
"#,
    )
    .unwrap();
    let mut shell = wrapper_shell(&repo, name, &wt_bin());
    let quote = |path: &std::path::Path| {
        shell_escape::escape(path.to_string_lossy().into_owned().into()).into_owned()
    };
    let trap = match disposition {
        "custom" => "trap 'printf called > wrapper-trap-called' INT",
        "ignored" => "trap '' INT",
        _ => ":",
    };
    shell.send(&format!(
        "{trap}; trap > wrapper-traps-before; printf 'WRAPPER_%s\\n' READY\n"
    ));
    wrapper_prompt(&mut shell, "WRAPPER_READY");

    shell.send(&format!(
        "cd {}; wt switch main --yes --execute python3 -- {} self\n",
        quote(&feature),
        quote(&repo.root_path().join("wrapper-worker.py"))
    ));
    shell.wait_for("wrapper-prompt> ");
    // A signal can cancel the remainder of the interactive input line. Read
    // its status on the next command, after the shell's cleanup fence ran.
    let status = if name == "fish" { "$status" } else { "$?" };
    shell.send(&format!(
        "printf '%s' {status} > {}; pwd > {}; trap > {}; printf 'WRAPPER_%s\\n' CHECKED\n",
        quote(&repo.root_path().join("wrapper-status")),
        quote(&repo.root_path().join("wrapper-cwd")),
        quote(&repo.root_path().join("wrapper-traps-after"))
    ));
    wrapper_prompt(&mut shell, "WRAPPER_CHECKED");
    assert_eq!(
        std::fs::read_to_string(repo.root_path().join("wrapper-status")).unwrap(),
        "130"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root_path().join("wrapper-cwd"))
            .unwrap()
            .trim(),
        repo.root_path().to_str().unwrap()
    );
    assert_wrapper_directives_removed(&repo);
    assert_eq!(
        std::fs::read(repo.root_path().join("wrapper-traps-before")).unwrap(),
        std::fs::read(repo.root_path().join("wrapper-traps-after")).unwrap()
    );

    let logging = if name == "fish" {
        format!(
            "set -gx RUST_LOG {}",
            crate::common::FOREGROUND_TRACE_FILTER
        )
    } else {
        format!("export RUST_LOG={}", crate::common::FOREGROUND_TRACE_FILTER)
    };
    shell.send(&format!(
        "{logging}; cd {}; wt -vv switch main --yes --execute python3 -- {}; printf '%s' {status} > {}; printf 'WRAPPER_%s\\n' CAUGHT\n",
        quote(&feature),
        quote(&repo.root_path().join("wrapper-worker.py")),
        quote(&repo.root_path().join("wrapper-caught-status"))
    ));
    let info = native_info(&repo, "wrapper-info");
    shell.remember(info[0]);
    shell.remember(info[2]);
    assert_eq!(info[1], shell.foreground());
    crate::common::wait_for_foreground_admission(&repo, &[info[0]]);
    shell.send("\u{3}");
    shell.wait_for("WRAPPER_CAUGHT");
    assert_eq!(
        std::fs::read_to_string(repo.root_path().join("wrapper-caught-status")).unwrap(),
        "0"
    );
    assert_wrapper_directives_removed(&repo);
}

#[rstest]
#[case("bash", false)]
#[case("/bin/bash", false)]
#[case("zsh", false)]
#[case("fish", false)]
#[case("bash", true)]
#[case("/bin/bash", true)]
fn shell_wrapper_preserves_native_compound_interrupts(
    mut repo: TestRepo,
    #[case] name: &str,
    #[case] exported_prompt: bool,
) {
    use std::os::unix::fs::PermissionsExt;
    let feature = repo.add_worktree("feature");
    // Exercise the generated wrapper's documented binary/directive boundary
    // with real raw-SIGINT and ordinary-130 exits. Numeric 130 must not be used
    // to infer that the shell should abort its compound command or loop.
    let executable = repo.root_path().join("native-wrapper-command");
    std::fs::write(
        &executable,
        r#"#!/usr/bin/env python3
import os, signal, sys
from pathlib import Path
Path(os.environ['WORKTRUNK_DIRECTIVE_CD_FILE']).write_text(os.environ['WT_WRAPPER_TEST_TARGET'])
if sys.argv[1] == 'raw':
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    signal.pthread_sigmask(signal.SIG_UNBLOCK, {signal.SIGINT})
    os.kill(os.getpid(), signal.SIGINT)
sys.exit({'zero': 0, 'three': 3}.get(sys.argv[1], 130))
"#,
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut shell = wrapper_shell(&repo, name, &executable);
    let quote = |path: &std::path::Path| {
        shell_escape::escape(path.to_string_lossy().into_owned().into()).into_owned()
    };
    let bash = matches!(name, "bash" | "/bin/bash");
    if bash {
        let export = if exported_prompt { "export " } else { "" };
        shell.send(&format!(
            "{export}PROMPT_COMMAND='status=$?'; printf 'WRAPPER_%s\\n' PROMPT\n"
        ));
        wrapper_prompt(&mut shell, "WRAPPER_PROMPT");
    }
    let feature = quote(&feature);
    let next = quote(&repo.root_path().join("wrapper-next"));
    let status = if name == "fish" { "$status" } else { "$?" };
    let raw_status_path = quote(&repo.root_path().join("wrapper-raw-status"));
    let continuation_cwd = repo.root_path().join("wrapper-continuation-cwd");
    let assert_continuation_cwd = || {
        assert_eq!(
            std::fs::read_to_string(&continuation_cwd).unwrap().trim(),
            repo.root_path().to_str().unwrap()
        );
    };
    // Compare the same function and compound context as the wrapper. Fish
    // 3.x continues after a signal-killed child inside a function, while 4.x
    // aborts the input line; a bare external command is not the same boundary.
    let reference_cd = quote(&repo.root_path().join("wrapper-reference-cd"));
    let reference_command = format!(
        "env WORKTRUNK_DIRECTIVE_CD_FILE={reference_cd} {} raw",
        quote(&executable)
    );
    let reference_function = if name == "fish" {
        format!("function native_reference; {reference_command}; end")
    } else {
        format!("native_reference() {{ {reference_command}; }}")
    };
    shell.send(&format!("{reference_function}\n"));
    shell.wait_for("wrapper-prompt> ");
    let compound = |command: &str, raw_status: &str, next: &str, looping: bool| {
        let body = format!(
            "{command}; printf '%s' {status} > {raw_status}; pwd > {}; printf next >> {next}",
            quote(&continuation_cwd)
        );
        if looping && name == "fish" {
            format!("for iteration in first second; {body}; end")
        } else if looping {
            format!("for iteration in first second; do {body}; done")
        } else {
            body
        }
    };
    let mut native_expectations = Vec::new();
    for looping in [false, true] {
        let reference = repo
            .root_path()
            .join(format!("wrapper-reference-{looping}"));
        let reference_raw_status = repo.root_path().join("wrapper-reference-raw-status");
        let reference_native_status = repo.root_path().join("wrapper-reference-native-status");
        let native = compound(
            "native_reference",
            &quote(&reference_raw_status),
            &quote(&reference),
            looping,
        );
        shell.send(&format!("cd {feature}; {native}\n"));
        shell.wait_for("wrapper-prompt> ");
        let reference_prompt_status = repo.root_path().join("wrapper-reference-prompt-status");
        let record_prompt = if bash {
            format!(
                "printf '%s' \"$status\" > {}; ",
                quote(&reference_prompt_status)
            )
        } else {
            String::new()
        };
        shell.send(&format!(
            "printf '%s' {status} > {}; {record_prompt}printf 'WRAPPER_%s\\n' REFERENCE\n",
            quote(&reference_native_status)
        ));
        wrapper_prompt(&mut shell, "WRAPPER_REFERENCE");
        let native_continues = reference.exists();
        let native_status = std::fs::read_to_string(if native_continues {
            &reference_raw_status
        } else {
            &reference_native_status
        })
        .unwrap();
        let next_path = repo.root_path().join(format!("wrapper-next-{looping}"));
        let interrupted = compound("wt raw", &raw_status_path, &quote(&next_path), looping);
        let reset_prompt_status = if bash { "status=seed; " } else { "" };
        shell.send(&format!(
            "{reset_prompt_status}cd {feature}; {interrupted}\n"
        ));
        shell.wait_for("wrapper-prompt> ");
        let raw_output = shell.output.clone();
        let wrapper_prompt_status = repo.root_path().join("wrapper-prompt-status");
        let record_prompt = if bash {
            format!(
                "printf '%s' \"$status\" > {}; ",
                quote(&wrapper_prompt_status)
            )
        } else {
            String::new()
        };
        shell.send(&format!(
            "printf '%s' {status} > {}; {record_prompt}pwd > {}; printf 'WRAPPER_%s\\n' ABORTED\n",
            quote(&repo.root_path().join("wrapper-native-status")),
            quote(&repo.root_path().join("wrapper-native-cwd"))
        ));
        wrapper_prompt(&mut shell, "WRAPPER_ABORTED");
        if bash {
            assert_eq!(
                std::fs::read_to_string(wrapper_prompt_status).unwrap(),
                std::fs::read_to_string(reference_prompt_status).unwrap(),
                "the prompt callback must run in its caller's variable scope"
            );
        }
        let raw_status = if native_continues {
            "wrapper-raw-status"
        } else {
            "wrapper-native-status"
        };
        assert_eq!(
            std::fs::read_to_string(repo.root_path().join(raw_status)).unwrap_or_else(|error| {
                panic!("missing {raw_status}, shell={name}, looping={looping}, native_continues={native_continues}: {error}; terminal output:\n{raw_output}")
            }),
            native_status,
            "shell={name}, looping={looping}, native_continues={native_continues}; terminal output:\n{raw_output}"
        );
        assert_eq!(
            std::fs::read_to_string(repo.root_path().join("wrapper-native-cwd"))
                .unwrap()
                .trim(),
            repo.root_path().to_str().unwrap()
        );
        assert_eq!(next_path.exists(), native_continues);
        if native_continues {
            assert_continuation_cwd();
        }
        assert_wrapper_directives_removed(&repo);
        native_expectations.push((native_continues, native_status));
    }
    shell.send(&format!(
        "cd {feature}; wt ordinary; printf '%s' {status} > {}; printf next > {next}; printf 'WRAPPER_%s\\n' CONTINUED\n",
        quote(&repo.root_path().join("wrapper-native-status"))
    ));
    wrapper_prompt(&mut shell, "WRAPPER_CONTINUED");
    assert_eq!(
        std::fs::read_to_string(repo.root_path().join("wrapper-native-status")).unwrap(),
        "130"
    );
    assert!(repo.root_path().join("wrapper-next").exists());
    assert_wrapper_directives_removed(&repo);

    let missing = quote(&repo.root_path().join("missing-wrapper-target"));
    let target = if name == "fish" {
        format!("set -gx WT_WRAPPER_TEST_TARGET {missing}")
    } else {
        format!("export WT_WRAPPER_TEST_TARGET={missing}")
    };
    for (mode, expected) in [("zero", "1"), ("three", "3")] {
        shell.send(&format!("{target}; cd {feature}; wt {mode}; printf '%s' {status} > {}; printf 'WRAPPER_%s\\n' CD_FAILED\n", quote(&repo.root_path().join("wrapper-native-status"))));
        wrapper_prompt(&mut shell, "WRAPPER_CD_FAILED");
        assert_eq!(
            std::fs::read_to_string(repo.root_path().join("wrapper-native-status")).unwrap(),
            expected
        );
        assert_wrapper_directives_removed(&repo);
    }
    // A foreground hook uses an exec leaf, so actual wt propagates a real
    // SIGINT rather than a shell intermediary's ordinary exit code 130.
    repo.write_project_config("post-start = 'exec python3 -c \"import os,signal; signal.signal(2,signal.SIG_DFL); signal.pthread_sigmask(signal.SIG_UNBLOCK,{2}); os.kill(os.getpid(),2)\"'");
    let binary = quote(&recording_wrapper_binary(&repo, &wt_bin()));
    let binary = if name == "fish" {
        format!("set -gx WORKTRUNK_BIN {binary}")
    } else {
        format!("export WORKTRUNK_BIN={binary}")
    };
    shell.send(&format!("{binary}; printf 'WRAPPER_%s\\n' ACTUAL\n"));
    wrapper_prompt(&mut shell, "WRAPPER_ACTUAL");
    for (looping, (native_continues, native_status)) in
        [false, true].into_iter().zip(native_expectations)
    {
        let actual_next = repo
            .root_path()
            .join(format!("wrapper-actual-next-{looping}"));
        let command = compound(
            &format!("cd {feature}; wt switch main --yes; wt hook post-start --yes --foreground"),
            &raw_status_path,
            &quote(&actual_next),
            looping,
        );
        shell.send(&format!("{command}\n"));
        shell.wait_for("wrapper-prompt> ");
        let raw_output = shell.output.clone();
        shell.send(&format!(
            "printf '%s' {status} > {}; printf 'WRAPPER_%s\\n' ACTUAL_ABORTED\n",
            quote(&repo.root_path().join("wrapper-native-status"))
        ));
        wrapper_prompt(&mut shell, "WRAPPER_ACTUAL_ABORTED");
        assert_eq!(
            std::fs::read_to_string(repo.root_path().join(if native_continues {
                "wrapper-raw-status"
            } else {
                "wrapper-native-status"
            }))
            .unwrap(),
            native_status,
            "shell={name}, looping={looping}, native_continues={native_continues}; terminal output:\n{raw_output}"
        );
        assert_eq!(actual_next.exists(), native_continues);
        if native_continues {
            assert_continuation_cwd();
        }
        assert_wrapper_directives_removed(&repo);
    }
}

/// A terminal key can exit a noninteractive Bash before RETURN runs. Cleanup
/// must precede the caller's existing EXIT handler and preserve its native status.
#[rstest]
#[case("bash")]
#[case("/bin/bash")]
fn bash_script_interrupt_cleans_before_exit_handler(mut repo: TestRepo, #[case] name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let feature = repo.add_worktree("feature");
    let executable = repo.root_path().join("native-wrapper-command");
    std::fs::write(
        &executable,
        r#"#!/usr/bin/env python3
import os, signal, sys, time
from pathlib import Path
root = Path(os.environ['WT_WRAPPER_TEST_TARGET'])
Path(os.environ['WORKTRUNK_DIRECTIVE_CD_FILE']).write_text(str(root))
signal.signal(signal.SIGINT, signal.SIG_DFL)
signal.pthread_sigmask(signal.SIG_UNBLOCK, {signal.SIGINT})
(root / ('script-info-' + sys.argv[1])).write_text(f'{os.getpid()} {os.getpgrp()} {os.getppid()}')
while True:
    time.sleep(.01)
"#,
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let init = repo
        .wt_command()
        .args(["config", "shell", "init", "bash"])
        .output()
        .unwrap();
    assert!(init.status.success());
    let init_path = repo.root_path().join("wrapper-init");
    std::fs::write(&init_path, init.stdout).unwrap();
    let binary = recording_wrapper_binary(&repo, &executable);
    let quote = |path: &std::path::Path| {
        shell_escape::escape(path.to_string_lossy().into_owned().into()).into_owned()
    };
    let mut expectations = None;
    for mode in ["reference", "wrapper"] {
        let status_path = repo.root_path().join(format!("script-status-{mode}"));
        let cwd_path = repo.root_path().join(format!("script-cwd-{mode}"));
        let global_path = repo.root_path().join(format!("script-global-{mode}"));
        let next_path = repo.root_path().join(format!("script-next-{mode}"));
        let after_path = repo.root_path().join(format!("script-after-{mode}"));
        let command = if mode == "reference" {
            "native_reference"
        } else {
            "wt wrapper"
        };
        let script = format!(
            "export WORKTRUNK_BIN={} WT_WRAPPER_TEST_TARGET={}; source {}; status=seed; native_reference() {{ env WORKTRUNK_DIRECTIVE_CD_FILE={} {} reference; }}; trap 'printf \"%s\\n\" \"$?\" >> {}; printf \"%s\" \"$status\" > {}; pwd > {}' EXIT; cd {}; for iteration in only; do {command}; printf next > {}; done; printf after > {}",
            quote(&binary),
            quote(repo.root_path()),
            quote(&init_path),
            quote(&repo.root_path().join("script-reference-cd")),
            quote(&executable),
            quote(&status_path),
            quote(&global_path),
            quote(&cwd_path),
            quote(&feature),
            quote(&next_path),
            quote(&after_path)
        );
        let mut command = CommandBuilder::new(name);
        command.args(["--noprofile", "--norc", "-c", &script]);
        let mut shell = Shell::spawn(&repo, command);
        let info = native_info(&repo, &format!("script-info-{mode}"));
        shell.remember(info[0]);
        assert_eq!(info[1], shell.foreground());
        shell.send("\u{3}");
        crate::common::wait_for_file_content(&cwd_path);
        let status = std::fs::read_to_string(&status_path).unwrap();
        assert_eq!(std::fs::read_to_string(global_path).unwrap(), "seed");
        assert_eq!(
            status.lines().count(),
            1,
            "EXIT ran more than once: {status}"
        );
        let observed = (status, next_path.exists(), after_path.exists());
        if mode == "reference" {
            expectations = Some(observed);
        } else {
            assert_eq!(&observed, expectations.as_ref().unwrap());
            assert_wrapper_directives_removed(&repo);
            assert_eq!(
                std::fs::read_to_string(cwd_path).unwrap().trim(),
                repo.root_path().to_str().unwrap()
            );
        }
    }
}

/// A subshell can report its parent's inactive EXIT trap. A wrapper must not
/// activate that handler, or replace a subshell's own handler.
#[rstest]
#[case("bash")]
#[case("/bin/bash")]
fn bash_wrapper_preserves_subshell_exit_ownership(repo: TestRepo, #[case] name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let executable = repo.root_path().join("native-wrapper-command");
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s' \"$WORKTRUNK_TEST_WRAPPER_TARGET\" > \"$WORKTRUNK_DIRECTIVE_CD_FILE\"\nexit 3\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let init = repo
        .wt_command()
        .args(["config", "shell", "init", "bash"])
        .output()
        .unwrap();
    assert!(init.status.success());
    let init = String::from_utf8(init.stdout).unwrap();
    let binary = recording_wrapper_binary(&repo, &executable);
    for (body, expected) in [
        (
            "value=$(wt; printf INNER); printf 'VALUE_%s\\n' \"$value\"",
            "VALUE_INNER\nPARENT_0\n",
        ),
        ("wt | cat; printf AFTER", "AFTERPARENT_0\n"),
        ("(wt; printf INNER); printf AFTER", "INNERAFTERPARENT_0\n"),
        (
            "(trap 'printf \"SUB_%s\\n\" \"$?\"' EXIT; wt; exit 17); printf AFTER",
            "SUB_17\nAFTERPARENT_0\n",
        ),
    ] {
        let script = format!("{init}\ntrap 'printf \"PARENT_%s\\n\" \"$?\"' EXIT; {body}");
        let mut command = std::process::Command::new(name);
        repo.configure_wt_cmd(&mut command);
        let output = command
            .args(["--noprofile", "--norc", "-c", &script])
            .current_dir(repo.root_path())
            .env("WORKTRUNK_BIN", &binary)
            .env("WORKTRUNK_TEST_WRAPPER_TARGET", repo.root_path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{name}, {body}: {output:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            expected,
            "{name}, {body}"
        );
        assert_wrapper_directives_removed(&repo);
    }
}

/// Prompt hooks with caller-owned attributes must not prevent ordinary commands,
/// lose those attributes, or export the wrapper's temporary callback to children.
#[rstest]
#[case("bash", "readonly PROMPT_COMMAND=:")]
#[case("/bin/bash", "readonly PROMPT_COMMAND=:")]
#[case("bash", "export PROMPT_COMMAND=:")]
#[case("/bin/bash", "export PROMPT_COMMAND=:")]
#[case("bash", "unset PROMPT_COMMAND; export PROMPT_COMMAND")]
#[case("/bin/bash", "unset PROMPT_COMMAND; export PROMPT_COMMAND")]
#[case("bash", "readonly PROMPT_COMMAND=:; export PROMPT_COMMAND")]
#[case("/bin/bash", "readonly PROMPT_COMMAND=:; export PROMPT_COMMAND")]
#[case("bash", "PROMPT_COMMAND=:; set -a")]
#[case("/bin/bash", "PROMPT_COMMAND=:; set -a")]
#[case("bash", "unset PROMPT_COMMAND; set -a")]
#[case("/bin/bash", "unset PROMPT_COMMAND; set -a")]
#[case("bash", "unset PROMPT_COMMAND; readonly PROMPT_COMMAND")]
#[case("/bin/bash", "unset PROMPT_COMMAND; readonly PROMPT_COMMAND")]
#[case("bash", "unset PROMPT_COMMAND")]
#[case("/bin/bash", "unset PROMPT_COMMAND")]
fn bash_wrapper_preserves_protected_prompt(
    repo: TestRepo,
    #[case] name: &str,
    #[case] setup: &str,
) {
    use std::os::unix::fs::PermissionsExt;
    let executable = repo.root_path().join("native-wrapper-command");
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s' \"$WT_WRAPPER_TEST_TARGET\" > \"$WORKTRUNK_DIRECTIVE_CD_FILE\"\nprintf '%s' \"${PROMPT_COMMAND-}\" > \"$WT_WRAPPER_TEST_TARGET/prompt-child\"\nexit \"$1\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    // A literal x makes flag parsing failures deterministic rather than
    // depending on the allocator's random suffix appearing in the callback.
    let quote = |path: &std::path::Path| {
        shell_escape::escape(path.to_string_lossy().into_owned().into()).into_owned()
    };
    let tools = repo.root_path().join("wrapper-tools");
    std::fs::create_dir(&tools).unwrap();
    let mktemp = tools.join("mktemp");
    std::fs::write(
        &mktemp,
        format!(
            "#!/bin/sh\nexec {} {}\n",
            quote(&which::which("mktemp").unwrap()),
            quote(&repo.root_path().join("directive-x.XXXXXX"))
        ),
    )
    .unwrap();
    std::fs::set_permissions(&mktemp, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut shell = wrapper_shell(&repo, name, &executable);
    shell.send(&format!(
        "PATH={}:$PATH; {setup}; declare -p PROMPT_COMMAND > prompt-before; export -p > prompt-exports-before; printf 'WRAPPER_%s\\n' PROTECTED\n", quote(&tools)
    ));
    wrapper_prompt(&mut shell, "WRAPPER_PROTECTED");
    for status in [0, 130] {
        shell.send(&format!("wt {status}\n"));
        shell.wait_for("wrapper-prompt> ");
        shell.send("printf '%s' $? > prompt-status; declare -p PROMPT_COMMAND > prompt-after; export -p > prompt-exports-after; printf 'WRAPPER_%s\\n' CHECKED\n");
        wrapper_prompt(&mut shell, "WRAPPER_CHECKED");
        assert_eq!(
            std::fs::read_to_string(repo.root_path().join("prompt-status")).unwrap(),
            status.to_string(),
            "{name}, {setup}"
        );
        assert_eq!(
            std::fs::read(repo.root_path().join("prompt-before")).unwrap(),
            std::fs::read(repo.root_path().join("prompt-after")).unwrap(),
            "{name}, {setup}"
        );
        let exported_prompt = |file: &str| {
            std::fs::read_to_string(repo.root_path().join(file))
                .unwrap()
                .lines()
                .find(|line| line.starts_with("declare -x PROMPT_COMMAND"))
                .map(str::to_owned)
        };
        assert_eq!(
            exported_prompt("prompt-exports-before"),
            exported_prompt("prompt-exports-after"),
            "{name}, {setup}"
        );
        assert!(
            !std::fs::read_to_string(repo.root_path().join("prompt-child"))
                .unwrap()
                .contains("_wt_cleanup_"),
            "{name}, {setup}"
        );
        assert_wrapper_directives_removed(&repo);
    }
}

#[rstest]
#[case("bash")]
#[case("/bin/bash")]
fn bash_wrapper_preserves_exported_prompt_in_interactive_child(repo: TestRepo, #[case] name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let executable = repo.root_path().join("native-wrapper-command");
    std::fs::write(&executable, "#!/bin/sh\nexec \"$@\"\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut shell = wrapper_shell(&repo, name, &executable);
    let events = repo.root_path().join("child-prompt-events");
    let events_quoted = shell_escape::escape(events.to_string_lossy().into_owned().into());
    let callback =
        shell_escape::escape(format!("printf 'CHILD_PROMPT\\n' >> {events_quoted}").into());
    shell.send(&format!(
        "export PROMPT_COMMAND={callback}; wt {name} --noprofile --norc -i\n"
    ));
    crate::common::wait_for_file_content(&events);
    assert_eq!(std::fs::read_to_string(&events).unwrap(), "CHILD_PROMPT\n");
    shell.send("exit\n");
    shell.wait_for("wrapper-prompt> ");
    shell.send("printf 'WRAPPER_%s\\n' CHILD_RETURNED\n");
    shell.wait_for("WRAPPER_CHILD_RETURNED");
    assert!(
        !shell.output.contains("command not found"),
        "{}",
        shell.output
    );
    assert!(
        !shell.output.contains("maximum evaluation"),
        "{}",
        shell.output
    );
    wrapper_prompt(&mut shell, "WRAPPER_CHILD_RETURNED");
    assert_wrapper_directives_removed(&repo);
}

#[rstest]
#[case("bash")]
#[case("/bin/bash")]
fn bash_wrapper_preserves_functrace_return_handler(mut repo: TestRepo, #[case] name: &str) {
    let feature = repo.add_worktree("feature");
    let mut shell = wrapper_shell(&repo, name, &wt_bin());
    let quote = |path: &std::path::Path| {
        shell_escape::escape(path.to_string_lossy().into_owned().into()).into_owned()
    };
    shell.send("set -T; trap 'printf \"%s\\n\" \"${FUNCNAME[0]}\" >> wrapper-return-events' RETURN; trap -p RETURN > wrapper-return-before; printf 'WRAPPER_%s\\n' TRACED\n");
    wrapper_prompt(&mut shell, "WRAPPER_TRACED");
    shell.send(&format!(
        "cd {}; wt switch main --yes --execute sh -- -c 'exit 130'\n",
        quote(&feature)
    ));
    shell.wait_for("wrapper-prompt> ");
    shell.send(&format!(
        "printf '%s' $? > {}; trap -p RETURN > {}; printf 'WRAPPER_%s\\n' RETURNED\n",
        quote(&repo.root_path().join("wrapper-return-status")),
        quote(&repo.root_path().join("wrapper-return-after"))
    ));
    wrapper_prompt(&mut shell, "WRAPPER_RETURNED");
    assert_eq!(
        std::fs::read_to_string(repo.root_path().join("wrapper-return-status")).unwrap(),
        "130"
    );
    assert_eq!(
        std::fs::read(repo.root_path().join("wrapper-return-before")).unwrap(),
        std::fs::read(repo.root_path().join("wrapper-return-after")).unwrap()
    );
    // The prior handler receives wt's return as it did before the fence.
    let events = std::fs::read_to_string(repo.root_path().join("wrapper-return-events")).unwrap();
    assert_eq!(events.lines().filter(|name| *name == "wt").count(), 1);
    assert_wrapper_directives_removed(&repo);
}
