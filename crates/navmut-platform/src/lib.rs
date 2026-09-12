//! Windows game access for Navmut.
//!
//! Movement uses the native helper or local bridge. An uncertain result closes
//! the active session.

mod bridge;
mod capabilities;
mod error;
mod helper;
mod player_state;
mod windows;

pub use bridge::{
    parse_request, BridgeBackend, BridgeBackendError, BridgeClient, BridgeConnection,
    BridgeEndpoint, BridgeError, BridgeHandle, BridgeRequest, BridgeResponse, BridgeServer,
    BridgeWindow, WindowsBridgeBackend, BRIDGE_PROTOCOL_VERSION,
};
pub use capabilities::{Capability, CapabilitySet, PlatformCapabilities};
pub use error::PlatformError;
pub use helper::{
    format_position_command, parse_helper_response, validate_position, HelperResponse,
    HelperSession, SubmitStatus, COMMAND_TIMEOUT, STARTUP_TIMEOUT,
};
pub use player_state::{normalise_facing, PlayerState, PlayerStateReader};
pub use windows::{enumerate_game_windows, GameWindow};

/// Return the capabilities supported by this build.
pub fn capabilities() -> PlatformCapabilities {
    PlatformCapabilities::current()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_capabilities_are_conservative_on_non_windows() {
        let current = capabilities();
        if !cfg!(windows) {
            assert!(!current.windows);
            assert!(!current.player_state);
            assert!(!current.silent_position);
        }
    }

    #[test]
    fn capability_names_are_stable() {
        assert_eq!(Capability::Windows.as_str(), "windows");
        assert_eq!(Capability::PlayerState.as_str(), "player-state");
        assert_eq!(Capability::SilentPosition.as_str(), "silent-position");
    }

    #[test]
    fn enabled_capabilities_follow_the_advertised_flags() {
        let capabilities = PlatformCapabilities {
            windows: true,
            player_state: false,
            silent_position: true,
            bridge_protocol: 2,
        };
        assert_eq!(
            capabilities.enabled(),
            vec![Capability::Windows, Capability::SilentPosition]
        );
    }
}
