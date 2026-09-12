import { invoke } from "@tauri-apps/api/core";
import type {
  GameState,
  JournalEntry,
  MapProfile,
  NavmutBridge,
  Position,
  ProcessInfo,
  SavedPoint,
  ExportedArchive,
  ObservationForm,
  PlatformCapabilities,
  PersistedSettings,
  WarpIntent,
  UiSettings,
  MovementContext,
} from "./types";

const DEFAULT_SETTINGS: UiSettings = {
  selectedPid: null,
  profileId: "",
  overrideZone: false,
  mapVisible: true,
  activePanel: "warp",
  stepSize: 5,
  alwaysOnTop: false,
  opacity: 100,
  catalogPath: null,
  locationsPath: null,
  poiCatalogPath: null,
  observationsPath: null,
  helperPath: null,
  bridgePath: null,
};

const MOCK_PROFILES: MapProfile[] = [
  {
    id: "middle-la-noscea",
    name: "Middle La Noscea",
    zone: 128,
    region: 104,
    bounds: { minX: -220, maxX: 220, minZ: -220, maxZ: 220 },
    pois: [
      { id: "limsa-gate", name: "Limsa Lominsa Gate", category: "aetheryte", x: 42, y: 7, z: -18, zone: 128, trusted: true },
      { id: "summerford", name: "Summerford Farms", category: "landmark", x: -78, y: 9, z: 62, zone: 128, trusted: true },
      { id: "zephyr", name: "Zephyr Gate", category: "poi", x: 112, y: 12, z: 116, zone: 128, trusted: false },
    ],
    anchors: [
      { x: 42, z: -18, y: 7, source: "poi" },
      { x: -78, z: 62, y: 9, source: "poi" },
      { x: 112, z: 116, y: 12, source: "poi" },
    ],
  },
  {
    id: "western-thanalan",
    name: "Western Thanalan",
    zone: 140,
    region: 104,
    bounds: { minX: -260, maxX: 260, minZ: -180, maxZ: 180 },
    pois: [
      { id: "horizon", name: "Horizon", category: "aetheryte", x: -28, y: 5, z: 24, zone: 140, trusted: true },
      { id: "sildih", name: "Sil'dih Excavation", category: "landmark", x: 156, y: 18, z: -92, zone: 140, trusted: true },
    ],
    anchors: [
      { x: -28, z: 24, y: 5, source: "poi" },
      { x: 156, z: -92, y: 18, source: "poi" },
    ],
  },
];

function mockGameState(): GameState {
  return {
    connected: true,
    pid: 4242,
    character: "Researcher",
    zone: "Middle La Noscea",
    zoneId: 128,
    regionId: 104,
    position: { x: 124.8, y: 8.2, z: -42.6 },
    rotation: 1.37,
    mapId: "middle-la-noscea",
  };
}

function cloneState(state: GameState): GameState {
  return { ...state, position: { ...state.position } };
}

function mockBridge(): NavmutBridge {
  let state = mockGameState();
  let settings = { ...DEFAULT_SETTINGS };
  let journal: JournalEntry[] = [];

  return {
    async getCapabilities(): Promise<PlatformCapabilities> {
      return { windows: true, playerState: true, silentPosition: true, bridgeProtocol: 2 };
    },
    async listProcesses(): Promise<ProcessInfo[]> {
      return [{ pid: 4242 }, { pid: 7712 }];
    },
    async getGameState(pid: number | null): Promise<GameState> {
      state = { ...state, pid, connected: pid !== null };
      return cloneState(state);
    },
    async listProfiles(): Promise<MapProfile[]> {
      return MOCK_PROFILES.map((profile) => ({ ...profile, pois: [...profile.pois], anchors: [...profile.anchors] }));
    },
    async loadProfile(profileId: string): Promise<MapProfile> {
      const profile = (await this.listProfiles()).find((item) => item.id === profileId);
      if (!profile) throw new Error("Map profile was not found.");
      return profile;
    },
    async selectCatalog(): Promise<MapProfile[]> {
      return this.listProfiles();
    },
    async move(delta: Position, _context: MovementContext): Promise<GameState> {
      state = { ...state, position: {
        x: state.position.x + delta.x,
        y: state.position.y + delta.y,
        z: state.position.z + delta.z,
      } };
      return cloneState(state);
    },
    async warp(position: Position, context: MovementContext, _intent = "map", _targetZone: number | null = null): Promise<GameState> {
      const profile = MOCK_PROFILES.find((candidate) => candidate.id === context.profileId) ?? MOCK_PROFILES[0];
      state = { ...state, position: { ...position }, zone: profile.name, zoneId: profile.zone, mapId: profile.id };
      return cloneState(state);
    },
    async captureObservation(profileId: string, form: ObservationForm, _pid: number): Promise<JournalEntry> {
      const profile = MOCK_PROFILES.find((candidate) => candidate.id === profileId) ?? MOCK_PROFILES[0];
      const entry: JournalEntry = {
        id: `journal-${journal.length + 1}`,
        capturedAt: new Date().toISOString(),
        zone: state.zoneId ?? 0,
        profile: profile.name,
        position: { ...state.position },
        rotation: state.rotation,
        name: form.name,
        type: form.type,
        notes: form.notes,
      };
      journal = [entry, ...journal];
      return { ...entry, position: { ...entry.position } };
    },
    async listSavedPoints(): Promise<SavedPoint[]> {
      return [{ id: "saved-dock", name: "Dock approach", x: 108.4, y: 8.2, z: -34.7, zone: 128 }];
    },
    async savePoint(name: string, _pid: number): Promise<SavedPoint> {
      return { id: `saved-${Date.now()}`, name, x: state.position.x, y: state.position.y, z: state.position.z, zone: state.zoneId ?? 0 };
    },
    async deletePoint(_point: SavedPoint): Promise<void> {
    },
    async setPosition(position: Position, _context: MovementContext): Promise<GameState> {
      state = { ...state, position: { ...position } };
      return cloneState(state);
    },
    async refreshZone(pid: number | null): Promise<{ game: GameState; profileId: string | null; profiles: MapProfile[] }> {
      state = { ...state, pid, connected: pid !== null };
      const next = MOCK_PROFILES.find((profile) => profile.zone === state.zoneId)?.id || MOCK_PROFILES[0]?.id || null;
      return { game: cloneState(state), profileId: next, profiles: await this.listProfiles() };
    },
    async listJournal(_profileId: string): Promise<JournalEntry[]> {
      return journal.map((entry) => ({ ...entry, position: { ...entry.position } }));
    },
    async exportJournal(_profileId: string): Promise<ExportedArchive> {
      const json = JSON.stringify({ identifier: "navmut-observations", version: 1, members: ["manifest.json", "observations.jsonl", "observations.csv", "report.md"], observations: journal }, null, 2);
      return { filename: "navmut-observations.zip", mime: "application/zip", data: btoa(json) };
    },
    async loadSettings(): Promise<UiSettings> {
      return { ...settings };
    },
    async saveSettings(next: PersistedSettings): Promise<void> {
      settings = { ...settings, ...next };
    },
    async minimizeWindow(): Promise<void> {
    },
    async closeWindow(): Promise<void> {
    },
    async resizeWindow(_width: number): Promise<void> {
    },
    async setWindowOpacity(_opacity: number): Promise<void> {
    },
    async setAlwaysOnTop(_alwaysOnTop: boolean): Promise<void> {
    },
  };
}

function tauriBridge(): NavmutBridge {
  return {
    getCapabilities: () => invoke<PlatformCapabilities>("platform_capabilities"),
    listProcesses: () => invoke<ProcessInfo[]>("list_processes"),
    getGameState: (pid) => invoke<GameState>("get_game_state", { pid }),
    listProfiles: () => invoke<MapProfile[]>("list_profiles"),
    loadProfile: (profileId) => invoke<MapProfile>("load_profile", { profileId }),
    selectCatalog: () => invoke<MapProfile[]>("select_catalog"),
    move: (delta, context) => invoke<GameState>("move_player", { delta, context }),
    warp: (position, context, intent: WarpIntent = "map", targetZone = null) => invoke<GameState>("warp_to", { position, context, intent, targetZone }),
    listSavedPoints: () => invoke<SavedPoint[]>("list_saved_points"),
    savePoint: (name, pid) => invoke<SavedPoint>("save_point", { name, pid }),
    deletePoint: (point) => invoke<void>("delete_saved_point", { point }),
    setPosition: (position, context) => invoke<GameState>("set_position", { position, context }),
    refreshZone: (pid) => invoke("refresh_zone", { pid }),
    captureObservation: (profileId, form, pid) => invoke<JournalEntry>("capture_observation", { profileId, form, pid }),
    listJournal: (profileId) => invoke<JournalEntry[]>("list_journal", { profileId }),
    exportJournal: (profileId) => invoke<ExportedArchive>("export_journal", { profileId }),
    loadSettings: () => invoke<UiSettings>("load_settings"),
    saveSettings: (settings) => invoke<void>("save_settings", { settings }),
    minimizeWindow: () => invoke<void>("minimize_window"),
    closeWindow: () => invoke<void>("close_window"),
    resizeWindow: (width) => invoke<void>("resize_window", { width }),
    setWindowOpacity: (opacity) => invoke<void>("set_window_opacity", { opacity }),
    setAlwaysOnTop: (alwaysOnTop) => invoke<void>("set_always_on_top", { alwaysOnTop }),
  };
}

export function isTauriRuntime(): boolean {
  const internals = typeof window === "undefined"
    ? undefined
    : (window as Window & { __TAURI_INTERNALS__?: { invoke?: unknown } }).__TAURI_INTERNALS__;
  return typeof internals?.invoke === "function";
}

export function createBridge(): NavmutBridge {
  const isTauri = isTauriRuntime();
  const browserMockAllowed = import.meta.env.DEV || import.meta.env.MODE === "test";
  return isTauri || !browserMockAllowed ? tauriBridge() : mockBridge();
}

export function defaultSettings(): UiSettings {
  return { ...DEFAULT_SETTINGS };
}

export { MOCK_PROFILES };
