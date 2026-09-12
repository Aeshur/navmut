use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Capability, CapabilitySet, GameWindow, HelperSession, PlatformCapabilities, PlatformError,
    PlayerState, PlayerStateReader, SubmitStatus,
};

pub const BRIDGE_PROTOCOL_VERSION: u16 = 2;
const MAX_TITLE_BYTES: usize = 512;
const MAX_COORDINATE: f64 = 1_000_000.0;
const MAX_REQUEST_BODY_BYTES: usize = 4 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// Bridge operations supported by protocol version 2.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BridgeEndpoint {
    Windows,
    PlayerState,
    SilentPosition,
}

impl BridgeEndpoint {
    pub const fn path(self) -> &'static str {
        match self {
            Self::Windows => "/windows",
            Self::PlayerState => "/player-state",
            Self::SilentPosition => "/silent-position",
        }
    }

    pub fn from_path(path: &str) -> Result<Self, BridgeError> {
        match path {
            "/windows" => Ok(Self::Windows),
            "/player-state" => Ok(Self::PlayerState),
            "/silent-position" => Ok(Self::SilentPosition),
            _ => Err(BridgeError::UnknownEndpoint(path.to_owned())),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeWindow {
    pub handle: u64,
    pub process_id: u32,
    pub title: String,
}

impl TryFrom<GameWindow> for BridgeWindow {
    type Error = BridgeError;

    fn try_from(value: GameWindow) -> Result<Self, Self::Error> {
        let result = Self {
            handle: value.handle,
            process_id: value.process_id,
            title: value.title,
        };
        result.validate()?;
        Ok(result)
    }
}

impl BridgeWindow {
    pub fn validate(&self) -> Result<(), BridgeError> {
        if self.handle == 0 {
            return Err(BridgeError::Invalid(
                "window handle must be positive".to_owned(),
            ));
        }
        if self.process_id == 0 {
            return Err(BridgeError::Invalid(
                "process id must be positive".to_owned(),
            ));
        }
        if self.title.len() > MAX_TITLE_BYTES {
            return Err(BridgeError::Invalid("window title is too long".to_owned()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BridgePlayerState {
    pub process_id: u32,
    pub actor_id: u32,
    pub region: u16,
    pub zone: u16,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub rotation: f64,
}

impl From<PlayerState> for BridgePlayerState {
    fn from(value: PlayerState) -> Self {
        Self {
            process_id: value.process_id,
            actor_id: value.actor_id,
            region: value.region,
            zone: value.zone,
            x: value.x,
            y: value.y,
            z: value.z,
            rotation: value.rotation,
        }
    }
}

impl BridgePlayerState {
    pub fn validate(&self) -> Result<(), BridgeError> {
        if self.process_id == 0 {
            return Err(BridgeError::Protocol(
                "player-state process id is invalid".to_owned(),
            ));
        }
        if self.actor_id == 0 {
            return Err(BridgeError::Protocol(
                "player-state actor id is invalid".to_owned(),
            ));
        }
        if self.region == 0 || self.zone == 0 {
            return Err(BridgeError::Protocol(
                "player-state region or zone is invalid".to_owned(),
            ));
        }
        if ![self.x, self.y, self.z, self.rotation]
            .iter()
            .all(|value| value.is_finite())
        {
            return Err(BridgeError::Protocol(
                "player-state values are not finite".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeRequest {
    pub version: u16,
    pub endpoint: BridgeEndpoint,
    #[serde(default)]
    pub window: Option<BridgeWindow>,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub z: Option<f64>,
    #[serde(default)]
    pub zone: Option<u16>,
}

impl BridgeRequest {
    pub fn windows() -> Self {
        Self {
            version: BRIDGE_PROTOCOL_VERSION,
            endpoint: BridgeEndpoint::Windows,
            window: None,
            x: None,
            y: None,
            z: None,
            zone: None,
        }
    }

    pub fn player_state(window: BridgeWindow) -> Self {
        Self {
            version: BRIDGE_PROTOCOL_VERSION,
            endpoint: BridgeEndpoint::PlayerState,
            window: Some(window),
            x: None,
            y: None,
            z: None,
            zone: None,
        }
    }

    pub fn silent_position(window: BridgeWindow, x: f64, y: f64, z: f64, zone: u16) -> Self {
        Self {
            version: BRIDGE_PROTOCOL_VERSION,
            endpoint: BridgeEndpoint::SilentPosition,
            window: Some(window),
            x: Some(x),
            y: Some(y),
            z: Some(z),
            zone: Some(zone),
        }
    }

    pub fn validate(&self) -> Result<(), BridgeError> {
        if self.version != BRIDGE_PROTOCOL_VERSION {
            return Err(BridgeError::Version(self.version));
        }
        if let Some(window) = &self.window {
            window.validate()?;
        }
        match self.endpoint {
            BridgeEndpoint::Windows => {
                if self.window.is_some()
                    || self.x.is_some()
                    || self.y.is_some()
                    || self.z.is_some()
                    || self.zone.is_some()
                {
                    return Err(BridgeError::Invalid(
                        "windows does not accept a request payload".to_owned(),
                    ));
                }
            }
            BridgeEndpoint::PlayerState => {
                if self.window.is_none() {
                    return Err(BridgeError::Invalid(
                        "player-state requires a selected window".to_owned(),
                    ));
                }
                if self.x.is_some() || self.y.is_some() || self.z.is_some() || self.zone.is_some() {
                    return Err(BridgeError::Invalid(
                        "player-state does not accept coordinates".to_owned(),
                    ));
                }
            }
            BridgeEndpoint::SilentPosition => {
                if self.window.is_none() {
                    return Err(BridgeError::Invalid(
                        "silent-position requires a selected window".to_owned(),
                    ));
                }
                let coordinates = [self.x, self.y, self.z];
                if coordinates.iter().any(|value| {
                    value.is_none()
                        || value.is_some_and(|number| {
                            !number.is_finite() || number.abs() > MAX_COORDINATE
                        })
                }) {
                    return Err(BridgeError::Invalid(
                        "silent-position coordinates must be finite and bounded".to_owned(),
                    ));
                }
                if self.zone.is_none_or(|zone| zone == 0) {
                    return Err(BridgeError::Invalid(
                        "silent-position zone must be positive".to_owned(),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeResponse {
    pub version: u16,
    pub endpoint: BridgeEndpoint,
    #[serde(default)]
    pub capabilities: Option<CapabilitySet>,
    #[serde(default)]
    pub windows: Option<Vec<BridgeWindow>>,
    #[serde(default)]
    pub player_state: Option<BridgePlayerState>,
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub uncertain: bool,
}

impl BridgeResponse {
    pub fn error(endpoint: BridgeEndpoint, error: impl Into<String>, uncertain: bool) -> Self {
        Self {
            version: BRIDGE_PROTOCOL_VERSION,
            endpoint,
            capabilities: None,
            windows: None,
            player_state: None,
            ok: Some(false),
            error: Some(error.into()),
            uncertain,
        }
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeConnection {
    pub version: u16,
    pub port: u16,
    pub token: String,
    pub capabilities: PlatformCapabilities,
}

impl fmt::Debug for BridgeConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BridgeConnection")
            .field("version", &self.version)
            .field("port", &self.port)
            .field("token", &"<redacted>")
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

impl BridgeConnection {
    pub fn new(port: u16, token: String, capabilities: PlatformCapabilities) -> Self {
        Self {
            version: BRIDGE_PROTOCOL_VERSION,
            port,
            token,
            capabilities,
        }
    }

    pub fn validate(&self) -> Result<(), BridgeError> {
        if self.version != BRIDGE_PROTOCOL_VERSION {
            return Err(BridgeError::Version(self.version));
        }
        if self.port == 0 {
            return Err(BridgeError::Invalid(
                "connection port must be positive".to_owned(),
            ));
        }
        if self.token.len() < 32 || self.token.len() > 128 || !self.token.is_ascii() {
            return Err(BridgeError::Invalid(
                "connection token has an invalid shape".to_owned(),
            ));
        }
        if self.capabilities.bridge_protocol != BRIDGE_PROTOCOL_VERSION {
            return Err(BridgeError::Invalid(
                "connection capabilities do not advertise protocol v2".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn write_to(&self, path: impl AsRef<Path>) -> Result<(), BridgeError> {
        self.validate()?;
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| BridgeError::ConnectionFile)?;
        }
        let bytes = serde_json::to_vec(self).map_err(|_| BridgeError::ConnectionFile)?;
        let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4().as_simple()));
        write_private_file(&temporary, &bytes)?;
        // A hard link creates the destination atomically. It also prevents an
        // existing connection file from being overwritten.
        if fs::hard_link(&temporary, path).is_err() {
            let _ = fs::remove_file(&temporary);
            return Err(BridgeError::ConnectionFile);
        }
        let _ = fs::remove_file(&temporary);
        Ok(())
    }

    pub fn read_from(path: impl AsRef<Path>) -> Result<Self, BridgeError> {
        let bytes = fs::read(path).map_err(|_| BridgeError::ConnectionFile)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| BridgeError::Json("connection file is malformed".to_owned()))?;
        let connection: Self = serde_json::from_value(value)
            .map_err(|_| BridgeError::Json("connection file is malformed".to_owned()))?;
        connection.validate()?;
        Ok(connection)
    }
}

fn connection_file_matches_owner(path: &Path, owner: &BridgeConnection) -> bool {
    BridgeConnection::read_from(path).is_ok_and(|current| {
        current.version == owner.version
            && current.port == owner.port
            && current.token == owner.token
    })
}

fn remove_owned_connection_file(path: &Path, owner: &BridgeConnection) {
    if connection_file_matches_owner(path, owner) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), BridgeError> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| BridgeError::ConnectionFile)?;
    file.write_all(bytes)
        .map_err(|_| BridgeError::ConnectionFile)
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), BridgeError> {
    // Windows connection files inherit the user's profile ACL. The bearer
    // token is never printed or included in error text.
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|_| BridgeError::ConnectionFile)?;
    file.write_all(bytes)
        .map_err(|_| BridgeError::ConnectionFile)
}

#[derive(Debug)]
pub struct BridgeBackendError {
    message: String,
    pub uncertain: bool,
}

impl BridgeBackendError {
    pub fn rejected(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            uncertain: false,
        }
    }

    pub fn uncertain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            uncertain: true,
        }
    }
}

impl std::fmt::Display for BridgeBackendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// Operations the loopback server may delegate to.
pub trait BridgeBackend: Send + Sync + 'static {
    fn windows(&self) -> Result<Vec<BridgeWindow>, BridgeBackendError>;
    fn player_state(&self, window: &BridgeWindow) -> Result<BridgePlayerState, BridgeBackendError>;
    fn silent_position(
        &self,
        window: &BridgeWindow,
        x: f64,
        y: f64,
        z: f64,
        zone: u16,
    ) -> Result<(), BridgeBackendError>;
}

pub struct WindowsBridgeBackend {
    helper: HelperSession,
}

impl WindowsBridgeBackend {
    pub fn new(helper_path: impl Into<PathBuf>) -> Self {
        Self {
            helper: HelperSession::new(helper_path),
        }
    }
}

impl BridgeBackend for WindowsBridgeBackend {
    fn windows(&self) -> Result<Vec<BridgeWindow>, BridgeBackendError> {
        crate::enumerate_game_windows()
            .map_err(|error| BridgeBackendError::rejected(error.to_string()))?
            .into_iter()
            .map(BridgeWindow::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| BridgeBackendError::rejected(error.to_string()))
    }

    fn player_state(&self, window: &BridgeWindow) -> Result<BridgePlayerState, BridgeBackendError> {
        let game_window = GameWindow::new(window.handle, window.process_id, window.title.clone());
        let mut reader = PlayerStateReader::open(&game_window)
            .map_err(|error| BridgeBackendError::rejected(error.to_string()))?;
        reader
            .snapshot()
            .map(BridgePlayerState::from)
            .map_err(|error| BridgeBackendError::rejected(error.to_string()))
    }

    fn silent_position(
        &self,
        window: &BridgeWindow,
        x: f64,
        y: f64,
        z: f64,
        zone: u16,
    ) -> Result<(), BridgeBackendError> {
        let game_window = GameWindow::new(window.handle, window.process_id, window.title.clone());
        let response = self
            .helper
            .submit(&game_window, x, y, z, zone)
            .map_err(|error| BridgeBackendError::uncertain(error.to_string()))?;
        match response.status {
            SubmitStatus::Ok => Ok(()),
            SubmitStatus::Error => Err(BridgeBackendError::rejected(response.reason)),
            SubmitStatus::Unknown => Err(BridgeBackendError::uncertain(response.reason)),
        }
    }
}

pub struct BridgeServer<B> {
    listener: TcpListener,
    connection: BridgeConnection,
    backend: Arc<B>,
    shutdown: Arc<AtomicBool>,
    active_stream: Arc<Mutex<Option<TcpStream>>>,
    finished: Arc<AtomicBool>,
    connection_path: PathBuf,
}

impl<B: BridgeBackend> BridgeServer<B> {
    pub fn bind(
        connection_path: impl Into<PathBuf>,
        backend: B,
        capabilities: PlatformCapabilities,
    ) -> Result<Self, BridgeError> {
        if capabilities.bridge_protocol != BRIDGE_PROTOCOL_VERSION {
            return Err(BridgeError::Invalid(
                "bridge capabilities must advertise protocol v2".to_owned(),
            ));
        }
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| BridgeError::Bind)?;
        let port = listener.local_addr().map_err(|_| BridgeError::Bind)?.port();
        let token = Uuid::new_v4().as_simple().to_string();
        let connection = BridgeConnection::new(port, token, capabilities);
        let connection_path = connection_path.into();
        connection.write_to(&connection_path)?;
        Ok(Self {
            listener,
            connection,
            backend: Arc::new(backend),
            shutdown: Arc::new(AtomicBool::new(false)),
            active_stream: Arc::new(Mutex::new(None)),
            finished: Arc::new(AtomicBool::new(false)),
            connection_path,
        })
    }

    pub fn connection(&self) -> &BridgeConnection {
        &self.connection
    }

    pub fn connection_path(&self) -> &Path {
        &self.connection_path
    }

    pub fn start(self) -> Result<BridgeHandle, BridgeError> {
        if self.listener.set_nonblocking(true).is_err() {
            remove_owned_connection_file(&self.connection_path, &self.connection);
            return Err(BridgeError::Bind);
        }
        let address = match self.listener.local_addr() {
            Ok(address) => address,
            Err(_) => {
                remove_owned_connection_file(&self.connection_path, &self.connection);
                return Err(BridgeError::Bind);
            }
        };
        let connection = self.connection.clone();
        let connection_path = self.connection_path.clone();
        let shutdown = Arc::clone(&self.shutdown);
        let active_stream = Arc::clone(&self.active_stream);
        let finished = Arc::clone(&self.finished);
        let thread = std::thread::Builder::new()
            .name("navmut-bridge".to_owned())
            .spawn(move || self.serve())
            .map_err(|_| BridgeError::Thread)?;
        Ok(BridgeHandle {
            address,
            connection,
            connection_path,
            shutdown,
            active_stream,
            finished,
            thread: Some(thread),
        })
    }

    fn serve(self) {
        while !self.shutdown.load(Ordering::Acquire) {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    // Windows sockets inherit the listener's nonblocking mode.
                    if stream.set_nonblocking(false).is_err() {
                        continue;
                    }
                    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                    if let Ok(mut active_stream) = self.active_stream.lock() {
                        if self.shutdown.load(Ordering::Acquire) {
                            let _ = stream.shutdown(Shutdown::Both);
                            continue;
                        }
                        *active_stream = stream.try_clone().ok();
                    }
                    let _ = handle_connection(&mut stream, &self.connection, self.backend.as_ref());
                    if let Ok(mut active_stream) = self.active_stream.lock() {
                        active_stream.take();
                    }
                    let _ = stream.shutdown(Shutdown::Both);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        self.finished.store(true, Ordering::Release);
    }
}

impl<B> Drop for BridgeServer<B> {
    fn drop(&mut self) {
        remove_owned_connection_file(&self.connection_path, &self.connection);
    }
}

pub struct BridgeHandle {
    address: std::net::SocketAddr,
    connection: BridgeConnection,
    connection_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    active_stream: Arc<Mutex<Option<TcpStream>>>,
    finished: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl BridgeHandle {
    pub fn address(&self) -> std::net::SocketAddr {
        self.address
    }

    pub fn connection(&self) -> &BridgeConnection {
        &self.connection
    }

    pub fn connection_path(&self) -> &Path {
        &self.connection_path
    }

    pub fn stop(mut self) {
        stop_bridge_thread(
            &self.shutdown,
            &self.active_stream,
            &self.finished,
            &mut self.thread,
        );
        remove_owned_connection_file(&self.connection_path, &self.connection);
    }
}

impl Drop for BridgeHandle {
    fn drop(&mut self) {
        stop_bridge_thread(
            &self.shutdown,
            &self.active_stream,
            &self.finished,
            &mut self.thread,
        );
        remove_owned_connection_file(&self.connection_path, &self.connection);
    }
}

fn stop_bridge_thread(
    shutdown: &AtomicBool,
    active_stream: &Mutex<Option<TcpStream>>,
    finished: &AtomicBool,
    thread: &mut Option<std::thread::JoinHandle<()>>,
) {
    shutdown.store(true, Ordering::Release);
    if let Ok(active_stream) = active_stream.lock() {
        if let Some(stream) = active_stream.as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
    let Some(handle) = thread.take() else {
        return;
    };
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    while !finished.load(Ordering::Acquire) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if finished.load(Ordering::Acquire) {
        let _ = handle.join();
    }
    // A blocked or panicked worker must not make shutdown unbounded. Dropping
    // its JoinHandle detaches it. Closing the active stream above normally lets
    // the worker finish immediately.
}

pub struct BridgeClient {
    connection: BridgeConnection,
}

impl BridgeClient {
    pub fn from_connection_file(path: impl AsRef<Path>) -> Result<Self, BridgeError> {
        Ok(Self {
            connection: BridgeConnection::read_from(path)?,
        })
    }

    pub fn from_connection(connection: BridgeConnection) -> Result<Self, BridgeError> {
        connection.validate()?;
        Ok(Self { connection })
    }

    pub fn connection(&self) -> &BridgeConnection {
        &self.connection
    }

    pub fn windows(&self) -> Result<Vec<BridgeWindow>, BridgeError> {
        let response = self.request(BridgeRequest::windows())?;
        let windows = response
            .windows
            .ok_or_else(|| BridgeError::Protocol("windows response was malformed".to_owned()))?;
        for window in &windows {
            window.validate()?;
        }
        Ok(windows)
    }

    pub fn player_state(&self, window: BridgeWindow) -> Result<BridgePlayerState, BridgeError> {
        let process_id = window.process_id;
        let response = self.request(BridgeRequest::player_state(window))?;
        let state = response.player_state.ok_or_else(|| {
            BridgeError::Protocol("player-state response was malformed".to_owned())
        })?;
        state.validate()?;
        if state.process_id != process_id {
            return Err(BridgeError::Protocol(
                "player-state process id does not match the selected window".to_owned(),
            ));
        }
        Ok(state)
    }

    pub fn silent_position(
        &self,
        window: BridgeWindow,
        x: f64,
        y: f64,
        z: f64,
        zone: u16,
    ) -> Result<(), BridgeError> {
        let response = self.request(BridgeRequest::silent_position(window, x, y, z, zone))?;
        if response.ok == Some(true) && !response.uncertain {
            Ok(())
        } else if response.uncertain {
            Err(BridgeError::Uncertain(response.error.unwrap_or_else(
                || "silent-position outcome is unknown".to_owned(),
            )))
        } else {
            Err(BridgeError::Protocol(response.error.unwrap_or_else(|| {
                "silent-position was rejected".to_owned()
            })))
        }
    }

    pub fn request(&self, request: BridgeRequest) -> Result<BridgeResponse, BridgeError> {
        request.validate()?;
        let mut stream = TcpStream::connect(("127.0.0.1", self.connection.port))
            .map_err(|error| BridgeError::Transport(format!("connect: {error}")))?;
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .map_err(|error| BridgeError::Transport(format!("set client read timeout: {error}")))?;
        stream
            .set_write_timeout(Some(IO_TIMEOUT))
            .map_err(|error| {
                BridgeError::Transport(format!("set client write timeout: {error}"))
            })?;
        let body = serde_json::to_vec(&request)
            .map_err(|_| BridgeError::Protocol("request serialization failed".to_owned()))?;
        let header = format!(
            "POST {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            request.endpoint.path(),
            self.connection.port,
            self.connection.token,
            body.len()
        );
        let write_deadline = Instant::now() + IO_TIMEOUT;
        write_all_with_deadline(&mut stream, header.as_bytes(), write_deadline)?;
        write_all_with_deadline(&mut stream, &body, write_deadline)?;
        let (status, response) = read_http_response(&mut stream)?;
        if status == 401 {
            return Err(BridgeError::Authentication);
        }
        if response.endpoint != request.endpoint {
            return Err(BridgeError::Protocol(
                "bridge response endpoint does not match request".to_owned(),
            ));
        }
        if (200..300).contains(&status) {
            validate_success_response(&request, &response)?;
            return Ok(response);
        }
        if !(200..300).contains(&status) {
            if response.uncertain {
                return Err(BridgeError::Uncertain(response.error.unwrap_or_else(
                    || "silent-position outcome is unknown".to_owned(),
                )));
            }
            return Err(BridgeError::Http(
                status,
                response
                    .error
                    .unwrap_or_else(|| "bridge request was rejected".to_owned()),
            ));
        }
        unreachable!("non-success bridge responses return above")
    }
}

fn handle_connection<B: BridgeBackend>(
    stream: &mut TcpStream,
    connection: &BridgeConnection,
    backend: &B,
) -> Result<(), BridgeError> {
    let request = match read_http_request(stream, connection.port) {
        Ok(request) => request,
        Err(error) => {
            let status = if matches!(error, BridgeError::RequestTooLarge) {
                413
            } else {
                400
            };
            write_http_response(
                stream,
                status,
                BridgeResponse::error(BridgeEndpoint::Windows, error.to_string(), false),
            )?;
            return Ok(());
        }
    };
    if !token_matches(
        request.authorization.as_deref().unwrap_or_default(),
        &connection.token,
    ) {
        write_http_response(
            stream,
            401,
            BridgeResponse::error(
                BridgeEndpoint::Windows,
                "bridge authentication failed",
                false,
            ),
        )?;
        return Ok(());
    }
    let endpoint = match BridgeEndpoint::from_path(&request.path) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            write_http_response(
                stream,
                404,
                BridgeResponse::error(BridgeEndpoint::Windows, error.to_string(), false),
            )?;
            return Ok(());
        }
    };
    if request.method != "POST" {
        write_http_response(
            stream,
            405,
            BridgeResponse::error(endpoint, "bridge requests must use POST", false),
        )?;
        return Ok(());
    }
    let parsed = match parse_request(&request.body) {
        Ok(parsed) if parsed.endpoint == endpoint => parsed,
        Ok(_) => {
            write_http_response(
                stream,
                400,
                BridgeResponse::error(endpoint, "request endpoint does not match path", false),
            )?;
            return Ok(());
        }
        Err(error) => {
            write_http_response(
                stream,
                400,
                BridgeResponse::error(endpoint, error.to_string(), false),
            )?;
            return Ok(());
        }
    };
    if !connection.capabilities.supports(capability_for(endpoint)) {
        write_http_response(
            stream,
            503,
            BridgeResponse::error(
                endpoint,
                "requested bridge capability is unavailable",
                false,
            ),
        )?;
        return Ok(());
    }
    let response = match parsed.endpoint {
        BridgeEndpoint::Windows => match backend.windows() {
            Ok(windows) => BridgeResponse {
                version: BRIDGE_PROTOCOL_VERSION,
                endpoint,
                capabilities: Some(connection.capabilities.into()),
                windows: Some(windows),
                player_state: None,
                ok: Some(true),
                error: None,
                uncertain: false,
            },
            Err(error) => BridgeResponse::error(endpoint, error.to_string(), error.uncertain),
        },
        BridgeEndpoint::PlayerState => {
            let window = parsed.window.expect("validated player-state window");
            match backend.player_state(&window) {
                Ok(player_state) => BridgeResponse {
                    version: BRIDGE_PROTOCOL_VERSION,
                    endpoint,
                    capabilities: Some(connection.capabilities.into()),
                    windows: None,
                    player_state: Some(player_state),
                    ok: Some(true),
                    error: None,
                    uncertain: false,
                },
                Err(error) => BridgeResponse::error(endpoint, error.to_string(), error.uncertain),
            }
        }
        BridgeEndpoint::SilentPosition => {
            let window = parsed.window.expect("validated silent-position window");
            match backend.silent_position(
                &window,
                parsed.x.expect("validated x"),
                parsed.y.expect("validated y"),
                parsed.z.expect("validated z"),
                parsed.zone.expect("validated zone"),
            ) {
                Ok(()) => BridgeResponse {
                    version: BRIDGE_PROTOCOL_VERSION,
                    endpoint,
                    capabilities: Some(connection.capabilities.into()),
                    windows: None,
                    player_state: None,
                    ok: Some(true),
                    error: None,
                    uncertain: false,
                },
                Err(error) => BridgeResponse::error(endpoint, error.to_string(), error.uncertain),
            }
        }
    };
    write_http_response(
        stream,
        if response.ok == Some(true) {
            200
        } else if response.uncertain {
            502
        } else {
            503
        },
        response,
    )
}

fn validate_success_response(
    request: &BridgeRequest,
    response: &BridgeResponse,
) -> Result<(), BridgeError> {
    let capabilities = response.capabilities.ok_or_else(|| {
        BridgeError::Protocol("bridge response omitted advertised capabilities".to_owned())
    })?;
    let supports_endpoint = match request.endpoint {
        BridgeEndpoint::Windows => capabilities.windows,
        BridgeEndpoint::PlayerState => capabilities.player_state,
        BridgeEndpoint::SilentPosition => capabilities.silent_position,
    };
    if !supports_endpoint {
        return Err(BridgeError::Protocol(
            "bridge response does not advertise the requested capability".to_owned(),
        ));
    }
    if response.uncertain {
        return Err(BridgeError::Uncertain(
            response
                .error
                .clone()
                .unwrap_or_else(|| "bridge operation outcome is unknown".to_owned()),
        ));
    }
    if response.ok != Some(true) {
        return Err(BridgeError::Protocol(
            response
                .error
                .clone()
                .unwrap_or_else(|| "bridge operation was rejected".to_owned()),
        ));
    }
    match request.endpoint {
        BridgeEndpoint::Windows => {
            let windows = response.windows.as_ref().ok_or_else(|| {
                BridgeError::Protocol("windows response was malformed".to_owned())
            })?;
            for window in windows {
                window.validate()?;
            }
            if response.player_state.is_some() {
                return Err(BridgeError::Protocol(
                    "windows response contained player state".to_owned(),
                ));
            }
        }
        BridgeEndpoint::PlayerState => {
            let player_state = response.player_state.as_ref().ok_or_else(|| {
                BridgeError::Protocol("player-state response was malformed".to_owned())
            })?;
            player_state.validate()?;
            if response.windows.is_some() {
                return Err(BridgeError::Protocol(
                    "player-state response contained windows".to_owned(),
                ));
            }
        }
        BridgeEndpoint::SilentPosition => {
            if response.windows.is_some() || response.player_state.is_some() {
                return Err(BridgeError::Protocol(
                    "silent-position response contained an unexpected payload".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn capability_for(endpoint: BridgeEndpoint) -> Capability {
    match endpoint {
        BridgeEndpoint::Windows => Capability::Windows,
        BridgeEndpoint::PlayerState => Capability::PlayerState,
        BridgeEndpoint::SilentPosition => Capability::SilentPosition,
    }
}

fn token_matches(actual: &str, expected: &str) -> bool {
    let mut difference = actual.len() ^ expected.len();
    for (left, right) in actual.bytes().zip(expected.bytes()) {
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

struct HttpRequest {
    method: String,
    path: String,
    authorization: Option<String>,
    body: String,
}

fn remaining_timeout(deadline: Instant) -> Result<Duration, BridgeError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(BridgeError::Transport(
            "I/O deadline expired before the operation started".to_owned(),
        ))
    } else {
        Ok(remaining)
    }
}

fn read_with_deadline(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    deadline: Instant,
) -> Result<usize, BridgeError> {
    stream
        .set_read_timeout(Some(remaining_timeout(deadline)?))
        .map_err(|error| BridgeError::Transport(format!("set read timeout: {error}")))?;
    stream
        .read(buffer)
        .map_err(|error| BridgeError::Transport(format!("read: {error}")))
}

fn write_all_with_deadline(
    stream: &mut TcpStream,
    bytes: &[u8],
    deadline: Instant,
) -> Result<(), BridgeError> {
    let mut written = 0;
    while written < bytes.len() {
        stream
            .set_write_timeout(Some(remaining_timeout(deadline)?))
            .map_err(|error| BridgeError::Transport(format!("set write timeout: {error}")))?;
        let count = stream
            .write(&bytes[written..])
            .map_err(|error| BridgeError::Transport(format!("write: {error}")))?;
        if count == 0 {
            return Err(BridgeError::Transport(
                "write returned zero bytes".to_owned(),
            ));
        }
        written += count;
    }
    Ok(())
}

fn read_http_request(
    stream: &mut TcpStream,
    expected_port: u16,
) -> Result<HttpRequest, BridgeError> {
    read_http_request_with_deadline(stream, expected_port, Instant::now() + IO_TIMEOUT)
}

fn read_http_request_with_deadline(
    stream: &mut TcpStream,
    expected_port: u16,
    deadline: Instant,
) -> Result<HttpRequest, BridgeError> {
    let mut bytes = Vec::new();
    let header_end;
    loop {
        let mut chunk = [0u8; 1024];
        let received = read_with_deadline(stream, &mut chunk, deadline)?;
        if received == 0 {
            return Err(BridgeError::MalformedRequest);
        }
        bytes.extend_from_slice(&chunk[..received]);
        if bytes.len() > MAX_HEADER_BYTES + MAX_REQUEST_BODY_BYTES {
            return Err(BridgeError::RequestTooLarge);
        }
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = position + 4;
            if header_end > MAX_HEADER_BYTES {
                return Err(BridgeError::RequestTooLarge);
            }
            break;
        }
    }
    let header_text =
        std::str::from_utf8(&bytes[..header_end - 4]).map_err(|_| BridgeError::MalformedRequest)?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().ok_or(BridgeError::MalformedRequest)?;
    let mut request_parts = request_line.split_ascii_whitespace();
    let method = request_parts
        .next()
        .ok_or(BridgeError::MalformedRequest)?
        .to_owned();
    let raw_path = request_parts
        .next()
        .ok_or(BridgeError::MalformedRequest)?
        .to_owned();
    if request_parts.next() != Some("HTTP/1.1")
        || raw_path.contains('?')
        || !raw_path.starts_with('/')
    {
        return Err(BridgeError::MalformedRequest);
    }
    let mut authorization = None;
    let mut content_length = None;
    let mut host = None;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(BridgeError::MalformedRequest)?;
        if name.eq_ignore_ascii_case("authorization") {
            if authorization.is_some() {
                return Err(BridgeError::MalformedRequest);
            }
            authorization = value.trim().strip_prefix("Bearer ").map(ToOwned::to_owned);
            if authorization.is_none() {
                return Err(BridgeError::MalformedRequest);
            }
        } else if name.eq_ignore_ascii_case("host") {
            if host.is_some() {
                return Err(BridgeError::MalformedRequest);
            }
            host = Some(value.trim().to_owned());
        } else if name.eq_ignore_ascii_case("origin") {
            return Err(BridgeError::MalformedRequest);
        } else if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(BridgeError::MalformedRequest);
            }
            let length = value
                .trim()
                .parse::<usize>()
                .map_err(|_| BridgeError::MalformedRequest)?;
            if length > MAX_REQUEST_BODY_BYTES {
                return Err(BridgeError::RequestTooLarge);
            }
            content_length = Some(length);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(BridgeError::MalformedRequest);
        }
    }
    let host = host.ok_or(BridgeError::MalformedRequest)?;
    if !is_loopback_host(&host, expected_port) {
        return Err(BridgeError::MalformedRequest);
    }
    let length = content_length.ok_or(BridgeError::MalformedRequest)?;
    while bytes.len() - header_end < length {
        let mut chunk = [0u8; 1024];
        let received = read_with_deadline(stream, &mut chunk, deadline)?;
        if received == 0 {
            return Err(BridgeError::MalformedRequest);
        }
        bytes.extend_from_slice(&chunk[..received]);
        if bytes.len() > header_end + length {
            // Each connection carries exactly one request.
            return Err(BridgeError::MalformedRequest);
        }
    }
    if bytes.len() != header_end + length {
        return Err(BridgeError::MalformedRequest);
    }
    let body = std::str::from_utf8(&bytes[header_end..header_end + length])
        .map_err(|_| BridgeError::MalformedRequest)?
        .to_owned();
    Ok(HttpRequest {
        method,
        path: raw_path,
        authorization,
        body,
    })
}

fn is_loopback_host(host: &str, expected_port: u16) -> bool {
    host == "127.0.0.1"
        || host
            .strip_prefix("127.0.0.1:")
            .and_then(|port| port.parse::<u16>().ok())
            .is_some_and(|port| port == expected_port)
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    response: BridgeResponse,
) -> Result<(), BridgeError> {
    let body = serde_json::to_vec(&response)
        .map_err(|_| BridgeError::Protocol("response serialization failed".to_owned()))?;
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        410 => "Gone",
        413 => "Payload Too Large",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if header.len() > MAX_HEADER_BYTES
        || body.len() > MAX_RESPONSE_BYTES
        || header.len() + body.len() > MAX_RESPONSE_BYTES
    {
        return Err(BridgeError::ResponseTooLarge);
    }
    let deadline = Instant::now() + IO_TIMEOUT;
    write_all_with_deadline(stream, header.as_bytes(), deadline)?;
    write_all_with_deadline(stream, &body, deadline)
}

// Content-Length frames the body. A probe for connection EOF can report a
// timeout or reset after a complete response on an otherwise valid peer.
fn read_http_response(stream: &mut TcpStream) -> Result<(u16, BridgeResponse), BridgeError> {
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0u8; 1024];
        let received = read_with_deadline(stream, &mut chunk, deadline)?;
        if received == 0 {
            return Err(BridgeError::MalformedResponse);
        }
        bytes.extend_from_slice(&chunk[..received]);
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = position + 4;
            if header_end > MAX_HEADER_BYTES {
                return Err(BridgeError::ResponseTooLarge);
            }
            break header_end;
        }
        if bytes.len() > MAX_HEADER_BYTES {
            return Err(BridgeError::ResponseTooLarge);
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end - 4])
        .map_err(|_| BridgeError::MalformedResponse)?;
    let mut lines = headers.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(BridgeError::MalformedResponse)?;
    let content_length = lines
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
        })
        .ok_or(BridgeError::MalformedResponse)?;
    let expected_total = header_end
        .checked_add(content_length)
        .ok_or(BridgeError::ResponseTooLarge)?;
    if content_length > MAX_RESPONSE_BYTES || expected_total > MAX_RESPONSE_BYTES {
        return Err(BridgeError::ResponseTooLarge);
    }
    while bytes.len() < expected_total {
        let remaining = expected_total - bytes.len();
        let read_size = remaining.min(1024);
        let mut chunk = [0u8; 1024];
        let received = read_with_deadline(stream, &mut chunk[..read_size], deadline)?;
        if received == 0 {
            return Err(BridgeError::MalformedResponse);
        }
        bytes.extend_from_slice(&chunk[..received]);
    }
    if bytes.len() != expected_total {
        return Err(BridgeError::MalformedResponse);
    }
    let response: BridgeResponse =
        serde_json::from_slice(&bytes[header_end..]).map_err(|_| BridgeError::MalformedResponse)?;
    if response.version != BRIDGE_PROTOCOL_VERSION {
        return Err(BridgeError::Version(response.version));
    }
    Ok((status, response))
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum BridgeError {
    #[error("unknown bridge endpoint: {0}")]
    UnknownEndpoint(String),
    #[error("bridge protocol version {0} is not supported")]
    Version(u16),
    #[error("invalid bridge request: {0}")]
    Invalid(String),
    #[error("bridge request JSON is malformed: {0}")]
    Json(String),
    #[error("bridge connection file could not be read")]
    ConnectionFile,
    #[error("bridge listener could not bind")]
    Bind,
    #[error("bridge server thread could not start")]
    Thread,
    #[error("bridge request was malformed")]
    MalformedRequest,
    #[error("bridge request exceeded its size limit")]
    RequestTooLarge,
    #[error("bridge response was malformed")]
    MalformedResponse,
    #[error("bridge response exceeded its size limit")]
    ResponseTooLarge,
    #[error("bridge transport failed: {0}")]
    Transport(String),
    #[error("bridge authentication failed")]
    Authentication,
    #[error("bridge returned HTTP status {0}: {1}")]
    Http(u16, String),
    #[error("bridge protocol response was malformed: {0}")]
    Protocol(String),
    #[error("silent-position outcome is uncertain: {0}")]
    Uncertain(String),
}

pub fn parse_request(raw: &str) -> Result<BridgeRequest, BridgeError> {
    let request: BridgeRequest =
        serde_json::from_str(raw).map_err(|error| BridgeError::Json(error.to_string()))?;
    request.validate()?;
    Ok(request)
}

impl From<BridgeError> for PlatformError {
    fn from(value: BridgeError) -> Self {
        Self::Bridge(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tempfile::tempdir;

    use super::*;

    fn window() -> BridgeWindow {
        BridgeWindow {
            handle: 101,
            process_id: 202,
            title: "FINAL FANTASY XIV".to_owned(),
        }
    }

    fn all_capabilities() -> PlatformCapabilities {
        PlatformCapabilities {
            windows: true,
            player_state: true,
            silent_position: true,
            bridge_protocol: BRIDGE_PROTOCOL_VERSION,
        }
    }

    struct MockBackend {
        windows_calls: AtomicUsize,
        uncertain: bool,
        invalid_window: bool,
        wrong_player_process: bool,
    }

    impl BridgeBackend for MockBackend {
        fn windows(&self) -> Result<Vec<BridgeWindow>, BridgeBackendError> {
            self.windows_calls.fetch_add(1, Ordering::Relaxed);
            Ok(vec![if self.invalid_window {
                BridgeWindow {
                    handle: 0,
                    process_id: 202,
                    title: "invalid".to_owned(),
                }
            } else {
                window()
            }])
        }

        fn player_state(
            &self,
            _window: &BridgeWindow,
        ) -> Result<BridgePlayerState, BridgeBackendError> {
            Ok(BridgePlayerState {
                process_id: if self.wrong_player_process { 303 } else { 202 },
                actor_id: 7,
                region: 104,
                zone: 170,
                x: 1.0,
                y: 2.0,
                z: 3.0,
                rotation: 0.5,
            })
        }

        fn silent_position(
            &self,
            _window: &BridgeWindow,
            _x: f64,
            _y: f64,
            _z: f64,
            _zone: u16,
        ) -> Result<(), BridgeBackendError> {
            if self.uncertain {
                Err(BridgeBackendError::uncertain(
                    "helper result was not observed",
                ))
            } else {
                Ok(())
            }
        }
    }

    fn start_server(
        capabilities: PlatformCapabilities,
        uncertain: bool,
    ) -> (tempfile::TempDir, BridgeHandle) {
        let directory = tempdir().expect("temporary bridge directory");
        let path = directory.path().join("connection.json");
        let server = BridgeServer::bind(
            &path,
            MockBackend {
                windows_calls: AtomicUsize::new(0),
                uncertain,
                invalid_window: false,
                wrong_player_process: false,
            },
            capabilities,
        )
        .expect("bridge binds");
        (directory, server.start().expect("bridge starts"))
    }

    fn start_server_with_validation_flags(
        invalid_window: bool,
        wrong_player_process: bool,
    ) -> (tempfile::TempDir, BridgeHandle) {
        let directory = tempdir().expect("temporary bridge directory");
        let path = directory.path().join("connection.json");
        let server = BridgeServer::bind(
            &path,
            MockBackend {
                windows_calls: AtomicUsize::new(0),
                uncertain: false,
                invalid_window,
                wrong_player_process,
            },
            all_capabilities(),
        )
        .expect("bridge binds");
        (directory, server.start().expect("bridge starts"))
    }

    #[test]
    fn bridge_accepts_only_the_three_capability_endpoints() {
        assert_eq!(
            BridgeEndpoint::from_path("/windows"),
            Ok(BridgeEndpoint::Windows)
        );
        assert_eq!(
            BridgeEndpoint::from_path("/player-state"),
            Ok(BridgeEndpoint::PlayerState)
        );
        assert_eq!(
            BridgeEndpoint::from_path("/silent-position"),
            Ok(BridgeEndpoint::SilentPosition)
        );
        assert!(matches!(
            BridgeEndpoint::from_path("/position"),
            Err(BridgeError::UnknownEndpoint(_))
        ));
        assert!(matches!(
            BridgeEndpoint::from_path("/diagnostic"),
            Err(BridgeError::UnknownEndpoint(_))
        ));
    }

    #[test]
    fn silent_position_rejects_non_finite_and_zero_zone() {
        assert!(BridgeRequest::silent_position(window(), 1.0, 2.0, 3.0, 128)
            .validate()
            .is_ok());
        assert!(
            BridgeRequest::silent_position(window(), f64::NAN, 2.0, 3.0, 128)
                .validate()
                .is_err()
        );
        assert!(BridgeRequest::silent_position(window(), 1.0, 2.0, 3.0, 0)
            .validate()
            .is_err());
    }

    #[test]
    fn request_parser_rejects_wrong_version_and_unknown_fields() {
        let wrong = r#"{"version":1,"endpoint":"windows"}"#;
        assert!(matches!(parse_request(wrong), Err(BridgeError::Version(1))));
        let unknown = r#"{"version":2,"endpoint":"windows","extra":true}"#;
        assert!(matches!(parse_request(unknown), Err(BridgeError::Json(_))));
    }

    #[test]
    fn authenticated_client_dispatches_all_v2_operations() {
        let (directory, server) = start_server(all_capabilities(), false);
        let client = BridgeClient::from_connection_file(directory.path().join("connection.json"))
            .expect("client reads connection file");
        assert_eq!(client.windows().expect("windows response"), vec![window()]);
        assert_eq!(
            client
                .player_state(window())
                .expect("player state response")
                .zone,
            170
        );
        client
            .silent_position(window(), 1.0, 2.0, 3.0, 128)
            .expect("silent position response");
        server.stop();
    }

    #[test]
    fn accepted_connection_waits_for_request_bytes() {
        let (_directory, server) = start_server(all_capabilities(), false);
        let mut stream = TcpStream::connect(server.address()).expect("connect bridge");
        // A client may be scheduled after the server accepts its connection.
        std::thread::sleep(Duration::from_millis(100));
        let body = serde_json::to_string(&BridgeRequest::windows()).expect("request body");
        let request = format!(
            "POST /windows HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{}",
            server.address().port(), server.connection().token, body.len(), body
        );
        stream
            .write_all(request.as_bytes())
            .expect("write delayed request");
        let (status, response) = read_http_response(&mut stream).expect("delayed response");
        assert_eq!(status, 200);
        assert_eq!(response.ok, Some(true));
        server.stop();
    }

    #[test]
    fn wrong_token_is_rejected_without_echoing_secret() {
        let (directory, server) = start_server(all_capabilities(), false);
        let mut connection = BridgeConnection::read_from(directory.path().join("connection.json"))
            .expect("connection file");
        let secret = connection.token.clone();
        assert!(!format!("{connection:?}").contains(&secret));
        connection.token = "x".repeat(32);
        let client = BridgeClient::from_connection(connection).expect("client config");
        let error = client.windows().expect_err("wrong token rejected");
        assert_eq!(error, BridgeError::Authentication);
        assert!(!error.to_string().contains(&secret));
        server.stop();
    }

    #[test]
    fn malformed_and_oversized_requests_are_rejected_before_dispatch() {
        let (directory, server) = start_server(all_capabilities(), false);
        let connection = BridgeConnection::read_from(directory.path().join("connection.json"))
            .expect("connection file");
        let mut stream = TcpStream::connect(server.address()).expect("connect bridge");
        let request = format!(
            "POST /windows HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{",
            connection.token
        );
        stream
            .write_all(request.as_bytes())
            .expect("write malformed request");
        stream
            .shutdown(std::net::Shutdown::Write)
            .expect("finish malformed request");
        let (status, _) = read_http_response(&mut stream).expect("read malformed response");
        assert_eq!(status, 400);
        let mut oversized = TcpStream::connect(server.address()).expect("connect bridge");
        let request = format!(
            "POST /windows HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Length: 5000\r\nConnection: close\r\n\r\n",
            connection.token
        );
        oversized
            .write_all(request.as_bytes())
            .expect("write oversized request");
        let (status, _) = read_http_response(&mut oversized).expect("read oversized response");
        assert_eq!(status, 413);
        server.stop();
    }

    #[test]
    fn response_reader_accepts_complete_content_length_without_eof() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind response peer");
        let address = listener.local_addr().expect("response peer address");
        let response = BridgeResponse::error(BridgeEndpoint::Windows, "test response", false);
        let body = serde_json::to_vec(&response).expect("serialize response");
        let header = format!(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
            body.len()
        );
        let mut stream = TcpStream::connect(address).expect("connect response peer");
        let (mut peer, _) = listener.accept().expect("accept response peer");
        peer.write_all(header.as_bytes())
            .expect("write response header");
        peer.write_all(&body).expect("write response body");
        let actual = read_http_response(&mut stream).expect("read complete response");
        assert_eq!(actual, (503, response));
    }

    #[test]
    fn response_reader_rejects_truncated_content_length() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind response peer");
        let address = listener.local_addr().expect("response peer address");
        let response = BridgeResponse::error(BridgeEndpoint::Windows, "test response", false);
        let body = serde_json::to_vec(&response).expect("serialize response");
        let header = format!(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len() + 1
        );
        let mut stream = TcpStream::connect(address).expect("connect response peer");
        let (mut peer, _) = listener.accept().expect("accept response peer");
        peer.write_all(header.as_bytes())
            .expect("write response header");
        peer.write_all(&body).expect("write response body");
        peer.shutdown(Shutdown::Write)
            .expect("finish truncated response");
        drop(peer);
        let result = read_http_response(&mut stream);
        assert!(matches!(result, Err(BridgeError::MalformedResponse)));
    }

    #[test]
    fn transport_errors_keep_operation_and_os_cause_without_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind transport peer");
        let address = listener.local_addr().expect("transport peer address");
        let mut stream = TcpStream::connect(address).expect("connect transport peer");
        let (_peer, _) = listener.accept().expect("accept transport peer");
        stream
            .set_nonblocking(true)
            .expect("make idle reads fail immediately");
        let mut buffer = [0u8; 1];
        let result = read_with_deadline(&mut stream, &mut buffer, Instant::now() + IO_TIMEOUT);
        let error = result.expect_err("idle nonblocking peer should fail to read");
        let BridgeError::Transport(message) = error else {
            panic!("unexpected transport result: {error:?}");
        };
        assert!(
            message.starts_with("read: "),
            "missing operation: {message}"
        );
        assert!(
            message.len() > "read: ".len(),
            "missing OS cause: {message}"
        );
        assert!(!message.contains("Bearer"), "credential leaked: {message}");
    }

    #[test]
    fn hostile_host_and_browser_origin_are_rejected_before_authentication() {
        let (directory, server) = start_server(all_capabilities(), false);
        let connection = BridgeConnection::read_from(directory.path().join("connection.json"))
            .expect("connection file");
        for headers in [
            format!(
                "Host: localhost\r\nAuthorization: Bearer {}",
                connection.token
            ),
            format!(
                "Host: 127.0.0.1:{}\r\nOrigin: https://evil.example\r\nAuthorization: Bearer {}",
                server.address().port(),
                connection.token
            ),
        ] {
            let mut stream = TcpStream::connect(server.address()).expect("connect bridge");
            let request = format!(
                "POST /windows HTTP/1.1\r\n{}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}",
                headers
            );
            stream
                .write_all(request.as_bytes())
                .expect("write hostile request");
            let (status, _) = read_http_response(&mut stream).expect("read hostile response");
            assert_eq!(status, 400);
        }
        server.stop();
    }

    #[test]
    fn connection_files_are_exclusive_and_teardown_is_owner_safe() {
        let directory = tempdir().expect("temporary bridge directory");
        let path = directory.path().join("connection.json");
        let existing = BridgeConnection::new(45_001, "e".repeat(32), all_capabilities());
        existing.write_to(&path).expect("write existing connection");
        assert!(matches!(
            BridgeConnection::new(45_002, "n".repeat(32), all_capabilities()).write_to(&path),
            Err(BridgeError::ConnectionFile)
        ));
        assert_eq!(
            BridgeConnection::read_from(&path)
                .expect("existing file")
                .port,
            45_001
        );

        let (directory, server) = start_server(all_capabilities(), false);
        let path = directory.path().join("connection.json");
        let replacement = BridgeConnection::new(45_003, "r".repeat(32), all_capabilities());
        fs::write(
            &path,
            serde_json::to_vec(&replacement).expect("replacement JSON"),
        )
        .expect("replace connection file");
        server.stop();
        assert_eq!(
            BridgeConnection::read_from(&path)
                .expect("replacement survives teardown")
                .token,
            replacement.token
        );
    }

    #[test]
    fn client_rejects_invalid_windows_and_mismatched_player_process() {
        let (directory, server) = start_server_with_validation_flags(true, false);
        let client = BridgeClient::from_connection_file(directory.path().join("connection.json"))
            .expect("client reads connection file");
        let error = client.windows().expect_err("invalid window response");
        assert!(
            matches!(error, BridgeError::Invalid(_)),
            "unexpected bridge error: {error}"
        );
        server.stop();

        let (directory, server) = start_server_with_validation_flags(false, true);
        let client = BridgeClient::from_connection_file(directory.path().join("connection.json"))
            .expect("client reads connection file");
        assert!(matches!(
            client.player_state(window()),
            Err(BridgeError::Protocol(_))
        ));
        server.stop();
    }

    #[test]
    fn client_rejects_unadvertised_success_capability() {
        let request = BridgeRequest::windows();
        let response = BridgeResponse {
            version: BRIDGE_PROTOCOL_VERSION,
            endpoint: BridgeEndpoint::Windows,
            capabilities: Some(CapabilitySet {
                windows: false,
                player_state: true,
                silent_position: true,
            }),
            windows: Some(vec![window()]),
            player_state: None,
            ok: Some(true),
            error: None,
            uncertain: false,
        };
        let error =
            validate_success_response(&request, &response).expect_err("capability mismatch");
        assert!(matches!(error, BridgeError::Protocol(_)));
    }

    #[test]
    fn capabilities_are_enforced_and_uncertainty_is_propagated() {
        let limited = PlatformCapabilities {
            windows: true,
            player_state: false,
            silent_position: false,
            bridge_protocol: BRIDGE_PROTOCOL_VERSION,
        };
        let (directory, server) = start_server(limited, false);
        let client = BridgeClient::from_connection_file(directory.path().join("connection.json"))
            .expect("client reads connection file");
        let error = client
            .player_state(window())
            .expect_err("capability is unavailable");
        assert!(matches!(error, BridgeError::Http(503, _)));
        server.stop();

        let (directory, server) = start_server(all_capabilities(), true);
        let client = BridgeClient::from_connection_file(directory.path().join("connection.json"))
            .expect("client reads connection file");
        let error = client
            .silent_position(window(), 1.0, 2.0, 3.0, 128)
            .expect_err("uncertain helper result");
        assert!(matches!(error, BridgeError::Uncertain(_)));
        server.stop();
    }
}
