use serde::{Deserialize, Serialize};

use crate::{GameWindow, PlatformError};

#[cfg(windows)]
pub const SUPPORTED_CLIENT_SIZE: u64 = 15_996_808;
#[cfg(windows)]
pub const SUPPORTED_CLIENT_SHA256: &str =
    "9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9";
#[cfg(windows)]
pub const SCENE_VTABLE: u32 = 0x00F8CC1C;
#[cfg(windows)]
pub const SCENE_TICK: u32 = 0x004B3C50;
#[cfg(windows)]
pub const MAP_VTABLE: u32 = 0x00FAAD88;
#[cfg(windows)]
pub const IMAGE_BASE: u32 = 0x00400000;
#[cfg(windows)]
pub const IMAGE_SIZE: u32 = 0x00F99000;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PlayerState {
    pub process_id: u32,
    pub actor_id: u32,
    pub region: u16,
    pub zone: u16,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub rotation: f64,
}

impl PlayerState {
    pub fn validate(&self) -> Result<(), PlatformError> {
        if self.process_id == 0 {
            return Err(PlatformError::invalid("process_id", "must be positive"));
        }
        if self.actor_id == 0 {
            return Err(PlatformError::invalid("actor_id", "must be positive"));
        }
        if self.region == 0 || self.zone == 0 {
            return Err(PlatformError::invalid(
                "zone",
                "region and zone must be positive",
            ));
        }
        if ![self.x, self.y, self.z, self.rotation]
            .iter()
            .all(|value| value.is_finite())
        {
            return Err(PlatformError::invalid(
                "player_state",
                "values must be finite",
            ));
        }
        Ok(())
    }
}

#[cfg(not(windows))]
pub struct PlayerStateReader;

#[cfg(not(windows))]
impl PlayerStateReader {
    pub fn open(_window: &GameWindow) -> Result<Self, PlatformError> {
        Err(PlatformError::Unsupported("live player state"))
    }

    pub fn snapshot(&mut self) -> Result<PlayerState, PlatformError> {
        Err(PlatformError::Unsupported("live player state"))
    }
}

#[cfg(windows)]
mod live {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::cmp::{max, min};
    use std::ffi::OsString;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStringExt;
    use std::path::Path;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
    use windows::Win32::System::Memory::{
        VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE,
        PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, PAGE_WRITECOPY,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_FORMAT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReadProcessMemory(
            hprocess: HANDLE,
            lpbaseaddress: *const core::ffi::c_void,
            lpbuffer: *mut core::ffi::c_void,
            nsize: usize,
            lpnumberofbytesread: *mut usize,
        ) -> i32;
    }

    const INVALID_POINTERS: [u32; 4] = [0, 0xFFFF_FFFF, 0xC000_0000, 0xCDCD_CDCD];
    const SCAN_LIMIT: u32 = 0x8000_0000;
    const SCAN_CHUNK: usize = 1024 * 1024;
    const PROCESS_VM_READ: u32 = 0x0010;
    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    struct ProcessReader {
        handle: HANDLE,
    }

    impl ProcessReader {
        fn scannable(info: &MEMORY_BASIC_INFORMATION) -> bool {
            let access = info.Protect.0 & 0xff;
            info.State == MEM_COMMIT
                && info.Protect.0 & PAGE_GUARD.0 == 0
                && matches!(
                    access,
                    value if value == PAGE_READWRITE.0
                        || value == PAGE_WRITECOPY.0
                        || value == PAGE_EXECUTE_READWRITE.0
                        || value == PAGE_EXECUTE_WRITECOPY.0
                )
        }

        fn readable(&self, address: u32, size: usize) -> bool {
            if address < 0x10000
                || size == 0
                || u64::from(address) + size as u64 > u64::from(SCAN_LIMIT)
            {
                return false;
            }
            let mut info = MEMORY_BASIC_INFORMATION::default();
            let queried = unsafe {
                VirtualQueryEx(
                    self.handle,
                    Some(address as usize as *const core::ffi::c_void),
                    &mut info,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if queried == 0 {
                return false;
            }
            let base = info.BaseAddress as usize;
            let end = base.saturating_add(info.RegionSize);
            info.State == MEM_COMMIT
                && info.Protect.0 & PAGE_GUARD.0 == 0
                && info.Protect.0 & 0xff != PAGE_NOACCESS.0
                && address as usize >= base
                && (address as usize).saturating_add(size) <= end
        }

        fn read(&self, address: u32, destination: &mut [u8]) -> bool {
            if !self.readable(address, destination.len()) {
                return false;
            }
            let mut received = 0usize;
            unsafe {
                ReadProcessMemory(
                    self.handle,
                    address as usize as *const core::ffi::c_void,
                    destination.as_mut_ptr() as *mut core::ffi::c_void,
                    destination.len(),
                    &mut received,
                ) != 0
                    && received == destination.len()
            }
        }

        fn read_u32(&self, address: u32) -> Option<u32> {
            let mut bytes = [0; 4];
            self.read(address, &mut bytes)
                .then(|| u32::from_le_bytes(bytes))
        }

        fn read_f32(&self, address: u32) -> Option<f32> {
            let mut bytes = [0; 4];
            self.read(address, &mut bytes)
                .then(|| f32::from_le_bytes(bytes))
        }

        fn scan_dword(&self, needle: u32, mut visitor: impl FnMut(u32) -> bool) -> Option<u32> {
            let mut address = 0x10000u32;
            let mut chunk = vec![0; SCAN_CHUNK];
            while address < SCAN_LIMIT {
                let mut info = MEMORY_BASIC_INFORMATION::default();
                let queried = unsafe {
                    VirtualQueryEx(
                        self.handle,
                        Some(address as usize as *const core::ffi::c_void),
                        &mut info,
                        size_of::<MEMORY_BASIC_INFORMATION>(),
                    )
                };
                if queried == 0 {
                    address = address.saturating_add(0x1000);
                    continue;
                }
                let base = info.BaseAddress as usize;
                let end64 = base
                    .saturating_add(info.RegionSize)
                    .min(SCAN_LIMIT as usize);
                if Self::scannable(&info) {
                    let mut cursor = max(address as usize, base);
                    while cursor < end64 {
                        let amount = min(SCAN_CHUNK, end64 - cursor) & !3;
                        if amount != 0 && self.read(cursor as u32, &mut chunk[..amount]) {
                            for offset in (0..amount).step_by(4) {
                                if u32::from_le_bytes([
                                    chunk[offset],
                                    chunk[offset + 1],
                                    chunk[offset + 2],
                                    chunk[offset + 3],
                                ]) == needle
                                {
                                    let candidate = cursor as u32 + offset as u32;
                                    if visitor(candidate) {
                                        return Some(candidate);
                                    }
                                }
                            }
                        }
                        cursor = cursor.saturating_add(amount.max(4));
                    }
                }
                address = if end64 <= address as usize {
                    address.saturating_add(0x1000)
                } else {
                    end64 as u32
                };
            }
            None
        }
    }

    impl Drop for ProcessReader {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }

    pub struct Reader {
        process_id: u32,
        process: ProcessReader,
        scene_address: Option<u32>,
    }

    impl Reader {
        pub fn open(window: &GameWindow) -> Result<Self, PlatformError> {
            if window.process_id == 0 || window.handle == 0 {
                return Err(PlatformError::invalid(
                    "window",
                    "handle and process id must be positive",
                ));
            }
            let hwnd = HWND(window.handle as usize as *mut core::ffi::c_void);
            let mut owner = 0u32;
            let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
            if !unsafe { IsWindow(Some(hwnd)).as_bool() }
                || thread == 0
                || owner != window.process_id
            {
                return Err(PlatformError::Windows(
                    "selected game window is no longer valid".to_owned(),
                ));
            }
            let process = unsafe {
                OpenProcess(
                    PROCESS_ACCESS_RIGHTS(
                        PROCESS_VM_READ
                            | PROCESS_QUERY_INFORMATION
                            | PROCESS_QUERY_LIMITED_INFORMATION,
                    ),
                    false,
                    window.process_id,
                )
            }
            .map_err(|error| {
                PlatformError::Windows(format!("could not open FFXIV process: {error}"))
            })?;
            let reader = Self {
                process_id: window.process_id,
                process: ProcessReader { handle: process },
                scene_address: None,
            };
            reader.check_identity()?;
            if reader.process.read_u32(SCENE_VTABLE + 4) != Some(SCENE_TICK) {
                return Err(PlatformError::Windows(
                    "running client does not match the supported 1.23b layout".to_owned(),
                ));
            }
            Ok(reader)
        }

        fn check_identity(&self) -> Result<(), PlatformError> {
            let mut buffer = vec![0u16; 32_768];
            let mut length = buffer.len() as u32;
            unsafe {
                QueryFullProcessImageNameW(
                    self.process.handle,
                    PROCESS_NAME_FORMAT(0),
                    PWSTR(buffer.as_mut_ptr()),
                    &mut length,
                )
            }
            .map_err(|error| {
                PlatformError::Windows(format!("could not identify FFXIV process: {error}"))
            })?;
            let path = OsString::from_wide(&buffer[..length as usize]);
            let path = Path::new(&path);
            let file_name = path.file_name().and_then(|value| value.to_str());
            let metadata = std::fs::metadata(path).map_err(|error| {
                PlatformError::Windows(format!("could not inspect FFXIV executable: {error}"))
            })?;
            if !file_name.is_some_and(|value| value.eq_ignore_ascii_case("ffxivgame.exe"))
                || metadata.len() != SUPPORTED_CLIENT_SIZE
            {
                return Err(PlatformError::Windows(
                    "live state supports only the retail FFXIV 1.23b executable".to_owned(),
                ));
            }
            let mut hash = Sha256::new();
            let bytes = std::fs::read(path).map_err(|error| {
                PlatformError::Windows(format!("could not hash FFXIV executable: {error}"))
            })?;
            hash.update(bytes);
            let digest = format!("{:x}", hash.finalize());
            if digest != SUPPORTED_CLIENT_SHA256 {
                return Err(PlatformError::Windows(
                    "FFXIV executable SHA-256 does not match retail 1.23b".to_owned(),
                ));
            }
            Ok(())
        }

        fn world_position(&self, actor: u32, depth: u8) -> Result<(f64, f64, f64), PlatformError> {
            if INVALID_POINTERS.contains(&actor) || depth >= 32 {
                return Err(PlatformError::Windows(
                    "invalid player position chain".to_owned(),
                ));
            }
            let parent = self.process.read_u32(actor + 0x98).ok_or_else(|| {
                PlatformError::Windows("could not read player position parent".to_owned())
            })?;
            let local =
                (
                    self.process.read_f32(actor + 0x9c).ok_or_else(|| {
                        PlatformError::Windows("could not read player X".to_owned())
                    })? as f64,
                    self.process.read_f32(actor + 0xa0).ok_or_else(|| {
                        PlatformError::Windows("could not read player Y".to_owned())
                    })? as f64,
                    self.process.read_f32(actor + 0xa4).ok_or_else(|| {
                        PlatformError::Windows("could not read player Z".to_owned())
                    })? as f64,
                );
            if parent == 0 {
                return Ok(local);
            }
            if INVALID_POINTERS.contains(&parent) {
                return Err(PlatformError::Windows(
                    "invalid player position parent".to_owned(),
                ));
            }
            let inherited = self.world_position(parent, depth + 1)?;
            Ok((
                inherited.0 + local.0,
                inherited.1 + local.1,
                inherited.2 + local.2,
            ))
        }

        fn world_facing(&self, actor: u32, depth: u8) -> Result<f64, PlatformError> {
            if INVALID_POINTERS.contains(&actor) || depth >= 32 {
                return Err(PlatformError::Windows(
                    "invalid player facing chain".to_owned(),
                ));
            }
            let parent = self.process.read_u32(actor + 0x98).ok_or_else(|| {
                PlatformError::Windows("could not read player facing parent".to_owned())
            })?;
            let local = self.process.read_f32(actor + 0xac).ok_or_else(|| {
                PlatformError::Windows("could not read player rotation".to_owned())
            })? as f64;
            if parent == 0 {
                return Ok(local);
            }
            if INVALID_POINTERS.contains(&parent) {
                return Err(PlatformError::Windows(
                    "invalid player facing parent".to_owned(),
                ));
            }
            let inherited = self.world_facing(parent, depth + 1)? + local;
            Ok(normalise_facing(inherited))
        }

        fn snapshot_at(&self, scene: u32) -> Result<PlayerState, PlatformError> {
            if self.process.read_u32(scene) != Some(SCENE_VTABLE) {
                return Err(PlatformError::Windows("scene object changed".to_owned()));
            }
            let outer = self
                .process
                .read_u32(scene + 0x64)
                .filter(|value| !INVALID_POINTERS.contains(value))
                .ok_or_else(|| {
                    PlatformError::Windows("player container is unavailable".to_owned())
                })?;
            let container = outer + 0x10;
            let actor = self
                .process
                .read_u32(container + 0x17838)
                .ok_or_else(|| PlatformError::Windows("player actor is unavailable".to_owned()))?;
            let actor_id = self.process.read_u32(container + 0x1783c).ok_or_else(|| {
                PlatformError::Windows("player actor id is unavailable".to_owned())
            })?;
            let map_layout = self
                .process
                .read_u32(container + 0x4e0)
                .ok_or_else(|| PlatformError::Windows("map layout is unavailable".to_owned()))?;
            if INVALID_POINTERS.contains(&actor)
                || INVALID_POINTERS.contains(&actor_id)
                || INVALID_POINTERS.contains(&map_layout)
            {
                return Err(PlatformError::Windows(
                    "player actor or map layout is unavailable".to_owned(),
                ));
            }
            let actor_vtable = self.process.read_u32(actor).ok_or_else(|| {
                PlatformError::Windows("player actor layout is unavailable".to_owned())
            })?;
            if !(IMAGE_BASE..IMAGE_BASE + IMAGE_SIZE).contains(&actor_vtable) {
                return Err(PlatformError::Windows(
                    "player actor layout does not match supported client".to_owned(),
                ));
            }
            if self.process.read_u32(map_layout) != Some(MAP_VTABLE) {
                return Err(PlatformError::Windows(
                    "map layout does not match supported client".to_owned(),
                ));
            }
            let region = self
                .process
                .read_u32(map_layout + 0x94)
                .filter(|value| *value > 0 && *value <= u32::from(u16::MAX))
                .ok_or_else(|| PlatformError::Windows("player region unavailable".to_owned()))?
                as u16;
            let zone = self
                .process
                .read_u32(map_layout + 0x98)
                .filter(|value| *value > 0 && *value <= u32::from(u16::MAX))
                .ok_or_else(|| PlatformError::Windows("player zone unavailable".to_owned()))?
                as u16;
            let (x, y, z) = self.world_position(actor, 0)?;
            let rotation = self.world_facing(actor, 0)?;
            let state = PlayerState {
                process_id: self.process_id,
                actor_id,
                region,
                zone,
                x,
                y,
                z,
                rotation,
            };
            state.validate()?;
            if self.process.read_u32(scene) != Some(SCENE_VTABLE)
                || self.process.read_u32(scene + 0x64) != Some(outer)
                || self.process.read_u32(container + 0x17838) != Some(actor)
                || self.process.read_u32(container + 0x1783c) != Some(actor_id)
                || self.process.read_u32(container + 0x4e0) != Some(map_layout)
                || self.process.read_u32(map_layout + 0x94) != Some(u32::from(region))
                || self.process.read_u32(map_layout + 0x98) != Some(u32::from(zone))
            {
                return Err(PlatformError::Windows(
                    "player state changed while it was sampled".to_owned(),
                ));
            }
            Ok(state)
        }

        pub fn snapshot(&mut self) -> Result<PlayerState, PlatformError> {
            if let Some(scene) = self.scene_address {
                for _ in 0..2 {
                    if let Ok(state) = self.snapshot_at(scene) {
                        return Ok(state);
                    }
                }
                self.scene_address = None;
            }
            let mut found = None;
            self.process.scan_dword(SCENE_VTABLE, |scene| {
                if let Ok(state) = self.snapshot_at(scene) {
                    found = Some((scene, state));
                    true
                } else {
                    false
                }
            });
            if let Some((scene, state)) = found {
                self.scene_address = Some(scene);
                return Ok(state);
            }
            Err(PlatformError::Windows(
                "could not find live FFXIV player state".to_owned(),
            ))
        }
    }

    pub fn open(window: &GameWindow) -> Result<Reader, PlatformError> {
        Reader::open(window)
    }

    pub fn normalise_facing(value: f64) -> f64 {
        if !value.is_finite() {
            return value;
        }
        let adjusted = if value < 0.0 {
            value - std::f64::consts::PI
        } else {
            value + std::f64::consts::PI
        };
        value - (adjusted / (2.0 * std::f64::consts::PI)).trunc() * (2.0 * std::f64::consts::PI)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows::Win32::System::Memory::PAGE_READONLY;

        #[test]
        fn scene_scan_accepts_only_writable_memory() {
            let mut info = MEMORY_BASIC_INFORMATION {
                State: MEM_COMMIT,
                Protect: PAGE_READWRITE,
                ..Default::default()
            };
            assert!(ProcessReader::scannable(&info));
            info.Protect = PAGE_EXECUTE_READWRITE;
            assert!(ProcessReader::scannable(&info));
            info.Protect = PAGE_READONLY;
            assert!(!ProcessReader::scannable(&info));
            info.Protect = PAGE_READWRITE | PAGE_GUARD;
            assert!(!ProcessReader::scannable(&info));
        }
    }
}

#[cfg(windows)]
pub struct PlayerStateReader {
    inner: live::Reader,
}

#[cfg(windows)]
impl PlayerStateReader {
    pub fn open(window: &GameWindow) -> Result<Self, PlatformError> {
        Ok(Self {
            inner: live::open(window)?,
        })
    }

    pub fn snapshot(&mut self) -> Result<PlayerState, PlatformError> {
        self.inner.snapshot()
    }
}

pub fn normalise_facing(value: f64) -> f64 {
    #[cfg(windows)]
    {
        live::normalise_facing(value)
    }
    #[cfg(not(windows))]
    {
        if !value.is_finite() {
            return value;
        }
        let adjusted = if value < 0.0 {
            value - std::f64::consts::PI
        } else {
            value + std::f64::consts::PI
        };
        value - (adjusted / (2.0 * std::f64::consts::PI)).trunc() * (2.0 * std::f64::consts::PI)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_validation_rejects_non_finite_values() {
        let state = PlayerState {
            process_id: 42,
            actor_id: 123,
            region: 104,
            zone: 170,
            x: f64::NAN,
            y: 2.0,
            z: 3.0,
            rotation: 0.75,
        };
        assert!(state.validate().is_err());
    }

    #[test]
    fn facing_normalisation_matches_retail_reader() {
        let positive = normalise_facing(4.0);
        let negative = normalise_facing(-4.0);
        assert!((positive - (4.0 - 2.0 * std::f64::consts::PI)).abs() < 1e-12);
        assert!((negative - (-4.0 + 2.0 * std::f64::consts::PI)).abs() < 1e-12);
    }
}
