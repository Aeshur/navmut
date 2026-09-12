//! Domain logic shared by Navmut's UI and platform code.

pub mod exporting;
pub mod geometry;
pub mod height_anchors;
pub mod observations;
pub mod pois;
pub mod profiles;

pub use exporting::{export_observations, export_observations_one, ARCHIVE_NAMES};
pub use geometry::{
    map_image_transform, pixel_is_eligible, position_command, rgba_pixel_is_eligible,
};
pub use height_anchors::{
    resolve_height_anchor, resolve_height_anchor_default, DEFAULT_ANCHOR_RADIUS,
};
pub use observations::{
    append_observation, load_observations, load_observations_for_profile, validate_observation,
};
pub use observations::{SCHEMA_VERSION, SUBJECT_TYPES};
pub use pois::{
    filter_pois, load_poi_catalog, load_pois, poi_label, validate_poi, validate_poi_document,
    POI_KINDS, POI_SCHEMA_VERSION,
};
pub use profiles::{
    delete_location, load_locations, load_profiles, load_world_profiles, save_location, Location,
    MapProfile,
};

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),
}

impl Error {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn display_path(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn absolute_without_io(path: &std::path::Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

/// Replace a same-directory temporary file without deleting the destination.
///
/// Windows' `std::fs::rename` refuses to replace an existing file, so the
/// Windows path uses the kernel's `ReplaceFileW`. The destination remains in
/// place if replacement fails.
pub fn atomic_replace(
    temporary: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        if destination.exists() {
            match replace_file_windows(temporary, destination) {
                Ok(()) => return Ok(()),
                Err(_error) if !destination.exists() => {}
                Err(error) => return Err(error),
            }
        }
    }
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_file_windows(
    temporary: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    let mut temporary_wide: Vec<u16> = temporary.as_os_str().encode_wide().collect();
    let mut destination_wide: Vec<u16> = destination.as_os_str().encode_wide().collect();
    if temporary_wide.contains(&0) || destination_wide.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "paths cannot contain NUL characters",
        ));
    }
    temporary_wide.push(0);
    destination_wide.push(0);
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced_file_name: *const u16,
            replacement_file_name: *const u16,
            backup_file_name: *const u16,
            replace_flags: u32,
            exclude: *const core::ffi::c_void,
            reserved: *const core::ffi::c_void,
        ) -> i32;
    }
    let result = unsafe {
        ReplaceFileW(
            destination_wide.as_ptr(),
            temporary_wide.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Serialize JSON with Python `json.dumps(..., ensure_ascii=True)` escaping.
pub(crate) fn ascii_json(value: &serde_json::Value) -> Result<String> {
    let encoded = serde_json::to_string(value)?;
    let mut output = String::with_capacity(encoded.len());
    for character in encoded.chars() {
        if character.is_ascii() {
            output.push(character);
        } else {
            let mut units = [0u16; 2];
            for unit in character.encode_utf16(&mut units) {
                output.push_str(&format!(r"\u{unit:04x}"));
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::atomic_replace;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn failed_replacement_preserves_the_prior_file() {
        let root = tempdir().unwrap();
        let destination = root.path().join("locations.json");
        fs::write(&destination, "prior\n").unwrap();
        let missing_temporary = root.path().join("missing.tmp");

        assert!(atomic_replace(&missing_temporary, &destination).is_err());
        assert_eq!(fs::read_to_string(destination).unwrap(), "prior\n");
    }
}
