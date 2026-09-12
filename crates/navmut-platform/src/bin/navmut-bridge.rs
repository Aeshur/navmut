use std::path::PathBuf;

use navmut_platform::{capabilities, BridgeServer, WindowsBridgeBackend};

fn default_helper_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .ok_or("bridge executable has no parent directory")?;
    Ok(directory.join("navmut-helper.exe"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.is_empty() || arguments.len() > 2 {
        return Err("usage: navmut-bridge CONNECTION_FILE [HELPER_PATH]".into());
    }
    let connection_path = PathBuf::from(&arguments[0]);
    let helper_path = arguments
        .get(1)
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(default_helper_path)?;
    if !cfg!(windows) {
        return Err("the live loopback bridge requires Windows".into());
    }
    let server = BridgeServer::bind(
        connection_path,
        WindowsBridgeBackend::new(helper_path),
        capabilities(),
    )?;
    let handle = server.start()?;
    eprintln!(
        "Navmut bridge listening on 127.0.0.1:{}",
        handle.address().port()
    );
    std::thread::park();
    Ok(())
}
