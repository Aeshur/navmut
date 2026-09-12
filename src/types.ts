export interface Position {
  x: number;
  y: number;
  z: number;
}

export interface GameState {
  connected: boolean;
  pid: number | null;
  character: string;
  zone: string;
  zoneId: number | null;
  regionId: number | null;
  position: Position;
  rotation: number;
  mapId: string;
}

export interface ProcessInfo {
  pid: number;
}

export interface PlatformCapabilities {
  windows: boolean;
  playerState: boolean;
  silentPosition: boolean;
  bridgeProtocol: number;
}

export interface MapBounds {
  minX: number;
  maxX: number;
  minZ: number;
  maxZ: number;
}

export interface PointOfInterest {
  id: string;
  name: string;
  category: "aetheryte" | "quest_npc" | "landmark" | "poi";
  x: number;
  y: number;
  z: number;
  zone: number;
  trusted: boolean;
}

export interface SavedPoint {
  id: string;
  name: string;
  x: number;
  y: number;
  z: number;
  zone: number;
}

export interface PointSelection {
  kind: "poi" | "saved";
  id: string;
  name: string;
  x: number;
  y: number;
  z: number;
  zone: number;
  packaged: boolean;
}

export interface HeightAnchor {
  x: number;
  z: number;
  y: number;
  source: "observation" | "saved" | "poi";
  trusted?: boolean;
}

export interface MapProfile {
  id: string;
  name: string;
  zone: number | null;
  region: number | null;
  bounds: MapBounds;
  pois: PointOfInterest[];
  anchors: HeightAnchor[];
  artwork?: MapArtwork;
  canMove?: boolean;
}

export interface MapArtwork {
  mime: string;
  data: string;
  width: number;
  height: number;
}

export interface JournalEntry {
  id: string;
  capturedAt: string;
  zone: number;
  profile: string;
  position: Position;
  rotation: number;
  name: string;
  type: "npc" | "mob" | "misc";
  notes: string;
}

export interface ObservationForm {
  name: string;
  type: "npc" | "mob" | "misc";
  notes: string;
}

export type WarpIntent = "map" | "point";

export interface MovementContext {
  pid: number;
  profileId: string;
  overrideZone: boolean;
}

export interface UiSettings {
  selectedPid: number | null;
  profileId: string;
  overrideZone: boolean;
  activePanel: "warp" | "journal";
  stepSize: 5 | 10 | 15 | 20;
  locationsPath?: string | null;
  observationsPath?: string | null;
  catalogPath?: string | null;
  poiCatalogPath?: string | null;
  helperPath?: string | null;
  bridgePath?: string | null;
  mapVisible: boolean;
  alwaysOnTop: boolean;
  opacity: number;
}

export type PersistedSettings = Omit<UiSettings, "selectedPid" | "overrideZone" | "stepSize">;

export interface ExportedArchive {
  filename: string;
  mime: string;
  data?: string;
  path?: string;
}

export interface NavmutBridge {
  getCapabilities(): Promise<PlatformCapabilities>;
  listProcesses(): Promise<ProcessInfo[]>;
  getGameState(pid: number | null): Promise<GameState>;
  listProfiles(): Promise<MapProfile[]>;
  loadProfile(profileId: string): Promise<MapProfile>;
  selectCatalog(): Promise<MapProfile[]>;
  move(delta: Position, context: MovementContext): Promise<GameState>;
  warp(position: Position, context: MovementContext, intent: WarpIntent, targetZone?: number | null): Promise<GameState>;
  listSavedPoints(): Promise<SavedPoint[]>;
  savePoint(name: string, pid: number): Promise<SavedPoint>;
  deletePoint(point: SavedPoint): Promise<void>;
  setPosition(position: Position, context: MovementContext): Promise<GameState>;
  refreshZone(pid: number | null): Promise<{ game: GameState; profileId: string | null; profiles: MapProfile[] }>;
  captureObservation(profileId: string, form: ObservationForm, pid: number): Promise<JournalEntry>;
  listJournal(profileId: string): Promise<JournalEntry[]>;
  exportJournal(profileId: string): Promise<ExportedArchive>;
  loadSettings(): Promise<UiSettings>;
  saveSettings(settings: PersistedSettings): Promise<void>;
  minimizeWindow(): Promise<void>;
  closeWindow(): Promise<void>;
  resizeWindow(width: number): Promise<void>;
  setWindowOpacity(opacity: number): Promise<void>;
  setAlwaysOnTop(alwaysOnTop: boolean): Promise<void>;
}
