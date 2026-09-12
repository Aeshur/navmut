use serde::{Deserialize, Serialize};

use crate::PlatformError;

/// A visible retail game window and its owning process.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GameWindow {
    pub handle: u64,
    pub process_id: u32,
    pub title: String,
}

impl GameWindow {
    pub const fn new(handle: u64, process_id: u32, title: String) -> Self {
        Self {
            handle,
            process_id,
            title,
        }
    }
}

#[cfg(not(windows))]
pub fn enumerate_game_windows() -> Result<Vec<GameWindow>, PlatformError> {
    Err(PlatformError::Unsupported("Windows game windows"))
}

#[cfg(windows)]
pub fn enumerate_game_windows() -> Result<Vec<GameWindow>, PlatformError> {
    use std::path::Path;
    use windows::core::{BOOL, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        IsWindowVisible,
    };

    struct Enumeration<'a> {
        windows: &'a mut Vec<GameWindow>,
    }

    unsafe extern "system" fn visit(window: HWND, context: LPARAM) -> BOOL {
        let enumeration = &mut *(context.0 as *mut Enumeration<'_>);
        if !IsWindowVisible(window).as_bool() {
            return BOOL(1);
        }

        let mut process_id = 0u32;
        let thread_id = GetWindowThreadProcessId(window, Some(&mut process_id));
        if thread_id == 0 || process_id == 0 {
            return BOOL(1);
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id);
        let Ok(process) = process else {
            return BOOL(1);
        };
        let mut path = vec![0u16; 32_768];
        let mut path_length = path.len() as u32;
        let identified = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(path.as_mut_ptr()),
            &mut path_length,
        )
        .is_ok();
        let _ = CloseHandle(process);
        if !identified {
            return BOOL(1);
        }
        let image_path = String::from_utf16_lossy(&path[..path_length as usize]);
        if !Path::new(&image_path)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("ffxivgame.exe"))
        {
            return BOOL(1);
        }

        let text_length = GetWindowTextLengthW(window);
        let mut title = vec![0u16; text_length.max(0) as usize + 1];
        let copied = GetWindowTextW(window, &mut title);
        title.truncate(copied.max(0) as usize);
        enumeration.windows.push(GameWindow::new(
            window.0 as usize as u64,
            process_id,
            String::from_utf16_lossy(&title),
        ));
        BOOL(1)
    }

    let mut result = Vec::new();
    let mut enumeration = Enumeration {
        windows: &mut result,
    };
    unsafe {
        EnumWindows(
            Some(visit),
            LPARAM((&mut enumeration as *mut Enumeration<'_>) as isize),
        )
        .map_err(|error| {
            PlatformError::Windows(format!("could not enumerate game windows: {error}"))
        })?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_window_round_trips_json() {
        let value = GameWindow::new(101, 202, "FINAL FANTASY XIV".to_owned());
        let encoded = serde_json::to_string(&value).expect("window serializes");
        let decoded: GameWindow = serde_json::from_str(&encoded).expect("window deserializes");
        assert_eq!(decoded, value);
    }
}
