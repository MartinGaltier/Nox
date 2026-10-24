//! Privileged helper entry point (launchd daemon, runs as root).
//!
//! ```text
//! nox-helper [--allowed-client <path>]   serve (launchd)
//! nox-helper --recover                   one-shot journal restore (uninstall)
//! nox-helper --print-launchd-plist --program <path> [--allowed-client <path>]
//! nox-helper --version
//! ```

use nox_helper::backend::RealBackend;
use nox_helper::core::HelperCore;
use nox_helper::journal::JournalStore;
use nox_helper::peer::ConsolePolicy;
use nox_helper::server::{Server, stop_pair};
use nox_helper::{JOURNAL_PATH, SOCKET_PATH, log_line, sys};
use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage: nox-helper [--allowed-client PATH] | --recover | --version | \
         --print-launchd-plist --program PATH [--allowed-client PATH]"
    );
    ExitCode::from(64)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut allowed_client: Option<PathBuf> = None;
    let mut program: Option<PathBuf> = None;
    let mut action = "serve";
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--allowed-client" => match it.next() {
                Some(p) => allowed_client = Some(PathBuf::from(p)),
                None => return usage(),
            },
            "--program" => match it.next() {
                Some(p) => program = Some(PathBuf::from(p)),
                None => return usage(),
            },
            "--recover" => action = "recover",
            "--version" => action = "version",
            "--print-launchd-plist" => action = "plist",
            _ => return usage(),
        }
    }

    match action {
        "version" => {
            println!(
                "nox-helper {} protocol {}",
                nox_helper::VERSION,
                nox_helper::PROTOCOL_VERSION
            );
            ExitCode::SUCCESS
        }
        "plist" => match program {
            Some(p) => {
                print!(
                    "{}",
                    nox_helper::launchd::plist(&p, allowed_client.as_deref())
                );
                ExitCode::SUCCESS
            }
            None => usage(),
        },
        _ if sys::euid() != 0 => {
            eprintln!("nox-helper: must run as root (installed as a LaunchDaemon)");
            ExitCode::from(77)
        }
        "recover" => {
            let mut core = HelperCore::new(RealBackend, JournalStore::new(JOURNAL_PATH));
            let o = core.startup();
            println!("{}", o.as_str());
            if o == nox_helper::protocol::RestoreOutcome::Failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        _ => serve(allowed_client),
    }
}

fn serve(allowed_client: Option<PathBuf>) -> ExitCode {
    log_line(&format!(
        "starting {} (allowed client: {})",
        nox_helper::VERSION,
        allowed_client
            .as_deref()
            .map_or("any console-user process".into(), |p| p
                .display()
                .to_string())
    ));
    let mut core = HelperCore::new(RealBackend, JournalStore::new(JOURNAL_PATH));
    // Boot / relaunch: a journal here means nobody is holding a lease.
    core.startup();

    let (stop_tx, stop_rx) = match stop_pair() {
        Ok(p) => p,
        Err(e) => {
            log_line(&format!("stop pipe: {e}"));
            return ExitCode::FAILURE;
        }
    };
    sys::install_stop_signals(stop_tx.raw_fd());

    let policy = ConsolePolicy::new(allowed_client.as_deref());
    let mut server = match Server::bind(std::path::Path::new(SOCKET_PATH), policy) {
        Ok(s) => s,
        Err(e) => {
            log_line(&format!("bind {SOCKET_PATH}: {e}"));
            return ExitCode::FAILURE;
        }
    };
    let r = server.run(&mut core, &stop_rx);
    core.shutdown();
    drop(server);
    drop(stop_tx);
    match r {
        Ok(()) => {
            log_line("stopped");
            ExitCode::SUCCESS
        }
        Err(e) => {
            log_line(&format!("poll: {e}"));
            ExitCode::FAILURE
        }
    }
}
