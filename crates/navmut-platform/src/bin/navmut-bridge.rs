use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use navmut_platform::{capabilities, BridgeServer, WindowsBridgeBackend};

// Windows ends the process soon after console close, logoff, or shutdown.
// The handler waits for connection-file cleanup within this limit.
#[cfg(windows)]
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(4);

static SHUTDOWN: Shutdown = Shutdown::new();

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Stage {
    Running,
    Stopping,
    Stopped,
}

// Console control events arrive on a separate thread from main-thread cleanup.
struct Shutdown {
    stage: Mutex<Stage>,
    changed: Condvar,
}

impl Shutdown {
    const fn new() -> Self {
        Self {
            stage: Mutex::new(Stage::Running),
            changed: Condvar::new(),
        }
    }

    fn set(&self, next: Stage) {
        let mut stage = self.stage.lock().unwrap_or_else(|error| error.into_inner());
        if next > *stage {
            *stage = next;
            self.changed.notify_all();
        }
    }

    fn wait_until(&self, wanted: Stage, timeout: Option<Duration>) -> bool {
        let stage = self.stage.lock().unwrap_or_else(|error| error.into_inner());
        let before = |stage: &mut Stage| *stage < wanted;
        match timeout {
            Some(timeout) => {
                let (stage, _) = self
                    .changed
                    .wait_timeout_while(stage, timeout, before)
                    .unwrap_or_else(|error| error.into_inner());
                *stage >= wanted
            }
            None => {
                let _stage = self
                    .changed
                    .wait_while(stage, before)
                    .unwrap_or_else(|error| error.into_inner());
                true
            }
        }
    }

    #[cfg(any(windows, test))]
    fn request_stop(&self, timeout: Duration) -> bool {
        self.set(Stage::Stopping);
        self.wait_until(Stage::Stopped, Some(timeout))
    }
}

#[cfg(windows)]
fn install_console_handler() -> Result<(), String> {
    use windows::core::BOOL;
    use windows::Win32::System::Console::{
        SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT,
        CTRL_SHUTDOWN_EVENT,
    };

    unsafe extern "system" fn on_console_event(event: u32) -> BOOL {
        match event {
            CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT => {
                SHUTDOWN.request_stop(CLEANUP_TIMEOUT);
                BOOL::from(true)
            }
            _ => BOOL::from(false),
        }
    }

    unsafe { SetConsoleCtrlHandler(Some(on_console_event), true) }
        .map_err(|error| format!("could not install the console stop handler: {error}"))
}

#[cfg(not(windows))]
fn install_console_handler() -> Result<(), String> {
    Ok(())
}

fn default_helper_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let directory = executable
        .parent()
        .ok_or("bridge executable has no parent directory")?;
    Ok(directory.join("navmut-helper.exe"))
}

fn run() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.is_empty() || arguments.len() > 2 {
        return Err("usage: navmut-bridge CONNECTION_FILE [HELPER_PATH]".to_owned());
    }
    let connection_path = PathBuf::from(&arguments[0]);
    let helper_path = match arguments.get(1) {
        Some(path) => PathBuf::from(path),
        None => default_helper_path()?,
    };
    if !cfg!(windows) {
        return Err("the live loopback bridge requires Windows".to_owned());
    }
    install_console_handler()?;
    let server = BridgeServer::bind(
        &connection_path,
        WindowsBridgeBackend::new(helper_path),
        capabilities(),
    )
    .map_err(|error| format!("{error}: {}", connection_path.display()))?;
    let handle = server.start().map_err(|error| error.to_string())?;
    eprintln!(
        "Navmut bridge listening on 127.0.0.1:{}. Press Ctrl+C to stop.",
        handle.address().port()
    );
    SHUTDOWN.wait_until(Stage::Stopping, None);
    eprintln!("Stopping Navmut bridge...");
    handle.stop();
    SHUTDOWN.set(Stage::Stopped);
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("navmut-bridge: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_request_waits_for_cleanup() {
        let shutdown: &'static Shutdown = Box::leak(Box::new(Shutdown::new()));
        let main = std::thread::spawn(move || {
            assert!(shutdown.wait_until(Stage::Stopping, None));
            shutdown.set(Stage::Stopped);
        });
        assert!(shutdown.request_stop(Duration::from_secs(5)));
        main.join().expect("main thread");
        assert!(shutdown.request_stop(Duration::ZERO));
    }

    #[test]
    fn stop_request_gives_up_when_cleanup_stalls() {
        let shutdown = Shutdown::new();
        assert!(!shutdown.request_stop(Duration::from_millis(20)));
        assert!(shutdown.wait_until(Stage::Stopping, Some(Duration::ZERO)));
    }
}
