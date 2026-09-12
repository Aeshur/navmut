use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    Windows,
    PlayerState,
    SilentPosition,
}

impl Capability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::PlayerState => "player-state",
            Self::SilentPosition => "silent-position",
        }
    }
}

/// Capabilities shared by the UI and bridge handshake.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlatformCapabilities {
    pub windows: bool,
    pub player_state: bool,
    pub silent_position: bool,
    pub bridge_protocol: u16,
}

impl PlatformCapabilities {
    pub const fn current() -> Self {
        Self {
            windows: cfg!(windows),
            player_state: cfg!(windows),
            silent_position: cfg!(windows),
            bridge_protocol: 2,
        }
    }

    pub const fn supports(self, capability: Capability) -> bool {
        match capability {
            Capability::Windows => self.windows,
            Capability::PlayerState => self.player_state,
            Capability::SilentPosition => self.silent_position,
        }
    }

    pub fn enabled(self) -> Vec<Capability> {
        [
            (self.windows, Capability::Windows),
            (self.player_state, Capability::PlayerState),
            (self.silent_position, Capability::SilentPosition),
        ]
        .into_iter()
        .filter_map(|(enabled, capability)| enabled.then_some(capability))
        .collect()
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilitySet {
    pub windows: bool,
    pub player_state: bool,
    pub silent_position: bool,
}

impl From<PlatformCapabilities> for CapabilitySet {
    fn from(value: PlatformCapabilities) -> Self {
        Self {
            windows: value.windows,
            player_state: value.player_state,
            silent_position: value.silent_position,
        }
    }
}

impl CapabilitySet {
    pub const fn requires_all(self) -> bool {
        self.windows && self.player_state && self.silent_position
    }
}
