//! Identity of the FFXIV 1.23b client executable that live state and the
//! native helper support.
//!
//! Launchers run a working copy named `ffxivgame.patched.exe` carrying five
//! in-place byte patches (encryption-time immediate, lobby host name, and
//! three Wine stability patches). Each patch keeps the file size and section
//! layout, and the lobby host bytes vary per server, so the identity check
//! hashes the file with those slots zeroed: retail and every patched copy
//! normalise to the same digest.

use sha2::{Digest, Sha256};

/// Byte size shared by the retail executable and every launcher-patched copy.
pub const SUPPORTED_CLIENT_SIZE: u64 = 15_996_808;

/// Raw SHA-256 of the unpatched retail executable, kept as provenance; the
/// check compares [`SUPPORTED_CLIENT_NORMALISED_SHA256`].
pub const RETAIL_CLIENT_SHA256: &str =
    "9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9";

/// SHA-256 of the retail executable after [`mask_launcher_patch_slots`].
pub const SUPPORTED_CLIENT_NORMALISED_SHA256: &str =
    "c8bd8e58bb48de41096e1f31b907e75ffe1ebc60e594bb0a897312ce7b99be65";

/// Executable file names the live endpoints accept, compared case-insensitively.
pub const SUPPORTED_CLIENT_NAMES: [&str; 2] = ["ffxivgame.exe", "ffxivgame.patched.exe"];

/// Launcher patch slots as `(file offset, length)`, ascending. The offsets equal
/// the launcher RVAs because every patched section in this image has its raw
/// pointer equal to its virtual address.
pub const LAUNCHER_PATCH_SLOTS: [(usize, usize); 5] = [
    (0x0049_2550, 29),   // null-this guard
    (0x0049_4B70, 4),    // null-member8 write NOP
    (0x0064_8BBF, 16),   // assert-log forwarder
    (0x009A_15E3, 5),    // encryption-time immediate
    (0x00B9_0110, 0x14), // lobby host name
];

/// Why an executable failed the identity check. The messages feed the UI's
/// "game version is not supported" mapping, so keep their wording stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientIdentityError {
    UnsupportedName,
    UnsupportedSize(u64),
    UnsupportedDigest(String),
}

impl std::fmt::Display for ClientIdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedName | Self::UnsupportedSize(_) => {
                f.write_str("live state supports only the retail FFXIV 1.23b executable")
            }
            Self::UnsupportedDigest(_) => {
                f.write_str("FFXIV executable SHA-256 does not match retail 1.23b")
            }
        }
    }
}

/// True when `name` is one of [`SUPPORTED_CLIENT_NAMES`] in any letter case.
pub fn is_supported_client_name(name: &str) -> bool {
    SUPPORTED_CLIENT_NAMES
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

/// Zero every [`LAUNCHER_PATCH_SLOTS`] range that lies inside `bytes`.
pub fn mask_launcher_patch_slots(bytes: &mut [u8]) {
    for (offset, length) in LAUNCHER_PATCH_SLOTS {
        if let Some(slot) = bytes.get_mut(offset..offset + length) {
            slot.fill(0);
        }
    }
}

/// Hex SHA-256 of `bytes` with the launcher patch slots zeroed, without
/// copying the image.
pub fn normalised_client_sha256(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    let mut cursor = 0;
    for (offset, length) in LAUNCHER_PATCH_SLOTS {
        let end = offset + length;
        if end > bytes.len() || offset < cursor {
            continue;
        }
        hash.update(&bytes[cursor..offset]);
        hash.update(vec![0u8; length]);
        cursor = end;
    }
    hash.update(&bytes[cursor..]);
    format!("{:x}", hash.finalize())
}

/// Check an executable's file name and bytes against the supported identity.
pub fn verify_client_image(
    file_name: Option<&str>,
    bytes: &[u8],
) -> Result<(), ClientIdentityError> {
    if !file_name.is_some_and(is_supported_client_name) {
        return Err(ClientIdentityError::UnsupportedName);
    }
    let size = bytes.len() as u64;
    if size != SUPPORTED_CLIENT_SIZE {
        return Err(ClientIdentityError::UnsupportedSize(size));
    }
    let digest = normalised_client_sha256(bytes);
    if digest != SUPPORTED_CLIENT_NORMALISED_SHA256 {
        return Err(ClientIdentityError::UnsupportedDigest(digest));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_image() -> Vec<u8> {
        (0..SUPPORTED_CLIENT_SIZE as usize)
            .map(|index| (index % 251) as u8)
            .collect()
    }

    #[test]
    fn accepts_retail_and_patched_names_case_insensitively() {
        assert!(is_supported_client_name("ffxivgame.exe"));
        assert!(is_supported_client_name("FFXIVGAME.EXE"));
        assert!(is_supported_client_name("ffxivgame.patched.exe"));
        assert!(is_supported_client_name("FFXIVGame.Patched.EXE"));
        assert!(!is_supported_client_name("ffxivboot.exe"));
        assert!(!is_supported_client_name("ffxivgame.patched.exe.bak"));
    }

    #[test]
    fn normalised_digest_ignores_patch_slots_only() {
        let retail = synthetic_image();
        let mut patched = retail.clone();
        for (offset, length) in LAUNCHER_PATCH_SLOTS {
            for byte in &mut patched[offset..offset + length] {
                *byte = !*byte;
            }
        }
        assert_eq!(
            normalised_client_sha256(&retail),
            normalised_client_sha256(&patched)
        );

        let mut masked = retail.clone();
        mask_launcher_patch_slots(&mut masked);
        assert_eq!(
            normalised_client_sha256(&retail),
            format!("{:x}", Sha256::digest(&masked))
        );

        let mut outside = retail.clone();
        let (offset, length) = LAUNCHER_PATCH_SLOTS[0];
        outside[offset + length] ^= 0xFF;
        assert_ne!(
            normalised_client_sha256(&retail),
            normalised_client_sha256(&outside)
        );
        let mut before = retail;
        before[offset - 1] ^= 0xFF;
        assert_ne!(
            normalised_client_sha256(&before),
            normalised_client_sha256(&outside)
        );
    }

    #[test]
    fn short_inputs_hash_without_panicking() {
        assert_eq!(
            normalised_client_sha256(&[]),
            format!("{:x}", Sha256::digest([]))
        );
        let mut short = vec![7u8; 16];
        mask_launcher_patch_slots(&mut short);
        assert_eq!(short, vec![7u8; 16]);
    }

    #[test]
    fn verify_rejects_name_size_and_digest_in_order() {
        let image = synthetic_image();
        assert_eq!(
            verify_client_image(None, &image),
            Err(ClientIdentityError::UnsupportedName)
        );
        assert_eq!(
            verify_client_image(Some("ffxivgame.patched.exe"), &image[..10]),
            Err(ClientIdentityError::UnsupportedSize(10))
        );
        let digest = normalised_client_sha256(&image);
        assert_eq!(
            verify_client_image(Some("ffxivgame.patched.exe"), &image),
            Err(ClientIdentityError::UnsupportedDigest(digest))
        );
    }

    /// Checks the real client when `NAVMUT_CLIENT_EXE` points at a retail or
    /// launcher-patched executable; skipped otherwise.
    #[test]
    fn verify_accepts_local_client_when_provided() {
        let Ok(path) = std::env::var("NAVMUT_CLIENT_EXE") else {
            return;
        };
        let bytes = std::fs::read(&path).expect("read NAVMUT_CLIENT_EXE");
        let name = std::path::Path::new(&path)
            .file_name()
            .and_then(|value| value.to_str());
        assert_eq!(verify_client_image(name, &bytes), Ok(()));
    }
}
