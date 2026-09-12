use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::Duration;

use crate::{GameWindow, PlatformError};

pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_RESPONSE_BYTES: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubmitStatus {
    Ok,
    Error,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelperResponse {
    pub status: SubmitStatus,
    pub reason: String,
}

impl HelperResponse {
    fn ok() -> Self {
        Self {
            status: SubmitStatus::Ok,
            reason: String::new(),
        }
    }
}

/// Parse one response line from the native helper.
pub fn parse_helper_response(line: &str) -> HelperResponse {
    let line = line.trim_end_matches(['\r', '\n']);
    if line == "OK" {
        return HelperResponse::ok();
    }
    if let Some(reason) = line.strip_prefix("ERROR ") {
        return HelperResponse {
            status: SubmitStatus::Error,
            reason: reason.to_owned(),
        };
    }
    if let Some(reason) = line.strip_prefix("UNKNOWN ") {
        return HelperResponse {
            status: SubmitStatus::Unknown,
            reason: reason.to_owned(),
        };
    }
    HelperResponse {
        status: SubmitStatus::Unknown,
        reason: "helper returned a malformed response".to_owned(),
    }
}

pub fn validate_position(x: f64, y: f64, z: f64, zone: u16) -> Result<(), PlatformError> {
    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
        return Err(PlatformError::invalid(
            "position",
            "coordinates must be finite",
        ));
    }
    if [x, y, z].iter().any(|value| value.abs() > 1_000_000.0) {
        return Err(PlatformError::invalid(
            "position",
            "coordinates are outside [-1000000,1000000]",
        ));
    }
    if zone == 0 {
        return Err(PlatformError::invalid("zone", "must be positive"));
    }
    Ok(())
}

pub fn format_position_command(x: f64, y: f64, z: f64, zone: u16) -> Result<String, PlatformError> {
    validate_position(x, y, z, zone)?;
    Ok(format!(
        "POS {:.3} {:.3} {:.3} {}",
        if x == 0.0 { 0.0 } else { x },
        if y == 0.0 { 0.0 } else { y },
        if z == 0.0 { 0.0 } else { z },
        zone
    ))
}

struct HelperInner {
    helper_path: PathBuf,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    responses: Option<Receiver<Result<String, String>>>,
    process_id: Option<u32>,
    handle: Option<u64>,
    closed: bool,
}

/// A hidden helper session accessed through a mutex.
pub struct HelperSession {
    inner: Mutex<HelperInner>,
}

impl HelperSession {
    pub fn new(helper_path: impl Into<PathBuf>) -> Self {
        Self {
            inner: Mutex::new(HelperInner {
                helper_path: helper_path.into(),
                child: None,
                stdin: None,
                responses: None,
                process_id: None,
                handle: None,
                closed: false,
            }),
        }
    }

    pub fn start(&self, window: &GameWindow) -> Result<(), PlatformError> {
        if !cfg!(windows) {
            return Err(PlatformError::Unsupported("silent position helper"));
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PlatformError::Helper("helper session lock was poisoned".to_owned()))?;
        Self::ensure_not_closed(&inner)?;
        if inner.process_id == Some(window.process_id) && inner.handle == Some(window.handle) {
            return Ok(());
        }
        Self::close_inner(&mut inner, true);
        Self::spawn_inner(&mut inner, window)
    }

    pub fn list_windows(&self) -> Result<Vec<GameWindow>, PlatformError> {
        crate::enumerate_game_windows()
    }

    pub fn submit(
        &self,
        window: &GameWindow,
        x: f64,
        y: f64,
        z: f64,
        zone: u16,
    ) -> Result<HelperResponse, PlatformError> {
        let command = format_position_command(x, y, z, zone)?;
        if !cfg!(windows) {
            return Err(PlatformError::Unsupported("silent position helper"));
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PlatformError::Helper("helper session lock was poisoned".to_owned()))?;
        Self::ensure_not_closed(&inner)?;
        if inner.process_id != Some(window.process_id) || inner.handle != Some(window.handle) {
            Self::close_inner(&mut inner, true);
            Self::spawn_inner(&mut inner, window)?;
        }
        let result = Self::write_and_wait(&mut inner, &command);
        match result {
            Ok(response) if response.status == SubmitStatus::Unknown => {
                Self::close_inner(&mut inner, false);
                Ok(response)
            }
            Ok(response) => Ok(response),
            Err(error) => {
                Self::close_inner(&mut inner, false);
                Err(PlatformError::Unknown(error))
            }
        }
    }

    pub fn close(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if inner.closed {
                return;
            }
            inner.closed = true;
            Self::close_inner(&mut inner, true);
        }
    }

    fn ensure_not_closed(inner: &HelperInner) -> Result<(), PlatformError> {
        if inner.closed {
            Err(PlatformError::Helper(
                "silent helper session is closed".to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    fn spawn_inner(inner: &mut HelperInner, window: &GameWindow) -> Result<(), PlatformError> {
        if window.process_id == 0 || window.handle == 0 {
            return Err(PlatformError::invalid(
                "window",
                "handle and process id must be positive",
            ));
        }
        let mut command = std::process::Command::new(&inner.helper_path);
        command
            .arg("--pid")
            .arg(window.process_id.to_string())
            .arg("--hwnd")
            .arg(window.handle.to_string())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command
            .spawn()
            .map_err(|error| PlatformError::Helper(format!("could not start helper: {error}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PlatformError::Helper("helper has no stdin".to_owned()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PlatformError::Helper("helper has no stdout".to_owned()))?;
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || read_lines(stdout, sender));
        inner.child = Some(child);
        inner.stdin = Some(stdin);
        inner.responses = Some(receiver);
        inner.process_id = Some(window.process_id);
        inner.handle = Some(window.handle);
        match Self::receive(inner, STARTUP_TIMEOUT) {
            Ok(line) if line == "READY" => Ok(()),
            Ok(line) if line.starts_with("ERROR ") => {
                Self::close_inner(inner, false);
                Err(PlatformError::Helper(line[6..].to_owned()))
            }
            Ok(_) => {
                Self::close_inner(inner, false);
                Err(PlatformError::Helper(
                    "helper did not become ready".to_owned(),
                ))
            }
            Err(error) => {
                // A timeout or EOF during readiness is a failed session, not a
                // usable one. Drop all handles before reporting the failure so
                // the next request cannot reuse stale process/window state.
                Self::close_inner(inner, false);
                Err(error)
            }
        }
    }

    fn write_and_wait(inner: &mut HelperInner, command: &str) -> Result<HelperResponse, String> {
        let stdin = inner
            .stdin
            .as_mut()
            .ok_or_else(|| "helper has no stdin".to_owned())?;
        stdin
            .write_all(format!("{command}\n").as_bytes())
            .and_then(|_| stdin.flush())
            .map_err(|error| error.to_string())?;
        let line = Self::receive(inner, COMMAND_TIMEOUT).map_err(platform_message)?;
        Ok(parse_helper_response(&line))
    }

    fn receive(inner: &mut HelperInner, timeout: Duration) -> Result<String, PlatformError> {
        let receiver = inner
            .responses
            .as_ref()
            .ok_or_else(|| PlatformError::Helper("helper has no output reader".to_owned()))?;
        match receiver.recv_timeout(timeout) {
            Ok(Ok(line)) => Ok(line),
            Ok(Err(error)) => Err(PlatformError::Helper(error)),
            Err(RecvTimeoutError::Timeout) => Err(PlatformError::Helper(
                "helper did not respond before the five-second deadline".to_owned(),
            )),
            Err(RecvTimeoutError::Disconnected) => Err(PlatformError::Unknown(
                "helper output closed before the command result".to_owned(),
            )),
        }
    }

    fn close_inner(inner: &mut HelperInner, send_quit: bool) {
        if let Some(mut stdin) = inner.stdin.take() {
            if send_quit {
                let _ = stdin.write_all(b"QUIT\n");
                let _ = stdin.flush();
            }
            drop(stdin);
        }
        inner.responses = None;
        if let Some(mut child) = inner.child.take() {
            match child.wait_timeout(SHUTDOWN_TIMEOUT) {
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
        inner.process_id = None;
        inner.handle = None;
    }
}

impl Drop for HelperSession {
    fn drop(&mut self) {
        self.close();
    }
}

fn read_lines<R: std::io::Read>(stream: R, sender: Sender<Result<String, String>>) {
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {
                if line.len() > MAX_RESPONSE_BYTES {
                    let _ = sender.send(Err("helper response was too long".to_owned()));
                    return;
                }
                if !line.is_ascii() {
                    let _ = sender.send(Err("helper response was not ASCII".to_owned()));
                    return;
                }
                let _ = sender.send(Ok(line.trim_end_matches(['\r', '\n']).to_owned()));
            }
            Err(error) => {
                let _ = sender.send(Err(error.to_string()));
                return;
            }
        }
    }
}

fn platform_message(error: PlatformError) -> String {
    error.to_string()
}

trait ChildWaitTimeout {
    fn wait_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl ChildWaitTimeout for Child {
    fn wait_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(Some(status));
            }
            if std::time::Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_commands_are_canonical_and_validated_before_spawn() {
        assert_eq!(
            format_position_command(1.25, -2.0, 0.0, 128).expect("valid position"),
            "POS 1.250 -2.000 0.000 128"
        );
        assert!(format_position_command(f64::NAN, 2.0, 3.0, 128).is_err());
        assert!(format_position_command(1.0, 2.0, 3.0, 0).is_err());
    }

    #[test]
    fn malformed_and_unknown_helper_responses_are_uncertain() {
        assert_eq!(parse_helper_response("OK"), HelperResponse::ok());
        assert_eq!(
            parse_helper_response("MAYBE"),
            HelperResponse {
                status: SubmitStatus::Unknown,
                reason: "helper returned a malformed response".to_owned(),
            }
        );
        assert_eq!(
            parse_helper_response("UNKNOWN access violation").status,
            SubmitStatus::Unknown
        );
    }

    #[test]
    fn timeout_constants_are_five_seconds() {
        assert_eq!(STARTUP_TIMEOUT, Duration::from_secs(5));
        assert_eq!(COMMAND_TIMEOUT, Duration::from_secs(5));
    }
}
