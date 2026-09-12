import "./styles.css";
import { createBridge, defaultSettings, isTauriRuntime } from "./ipc";
import type { GameState, JournalEntry, MapProfile, MovementContext, NavmutBridge, ObservationForm, PersistedSettings, PlatformCapabilities, PointOfInterest, PointSelection, Position, SavedPoint, UiSettings, WarpIntent } from "./types";

export const POLL_INTERVAL_MS = 250;
const MAP_INSET = 16;
const WINDOW_WIDTH = 972;
const RAIL_WIDTH = 290;
const MOVE_STEPS = [5, 10, 15, 20] as const;
const BRAND_ICON_URL = new URL("../src-tauri/icons/32x32.png", import.meta.url).href;
const STATUS_MESSAGE_LIMIT = 180;
const PLAYER_STATE_NOT_READY = "could not find live FFXIV player state";

export class UiError extends Error {}

interface MovementQueueState {
  movementQueue: Promise<void>;
}

export interface AppState extends MovementQueueState {
  settings: UiSettings;
  game: GameState;
  profiles: MapProfile[];
  journal: JournalEntry[];
  savedPoints: SavedPoint[];
  movementQueue: Promise<void>;
  polling: number | null;
  capabilities: PlatformCapabilities;
}

function liveActionsAvailable(state: AppState): boolean {
  return state.capabilities.windows && state.capabilities.playerState && state.capabilities.silentPosition;
}

const UNSUPPORTED_MESSAGE = "Live game controls are unavailable on this platform. Maps, catalogs, saved data, and Journal exports remain available.";

export function enqueueMovement(state: MovementQueueState, movement: () => Promise<void>): Promise<void> {
  state.movementQueue = state.movementQueue.then(movement, movement);
  return state.movementQueue;
}

export function parsePositionFields(input: string): Position | null {
  const values = input.trim().split(/[\s,]+/).filter(Boolean).map((value) => Number(value));
  return values.length === 3 && values.every((value) => Number.isFinite(value)) ? { x: values[0], y: values[1], z: values[2] } : null;
}

function rawErrorMessage(error: unknown): string {
  if (typeof error === "string") return error.trim();
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") return message.trim();
  }
  return "";
}

export function errorMessage(error: unknown, fallback: string): string {
  const message = rawErrorMessage(error);
  if (error instanceof UiError) return message;
  const detail = message.toLowerCase();
  if (detail.includes("outcome is unknown") || detail.includes("outcome was uncertain") || detail.includes("outcome is uncertain")) {
    return "The position outcome is unknown. Check the game before trying again.";
  }
  if (detail.includes("authentication failed")) return "Could not authenticate with the bridge. Check its connection file.";
  if (detail.includes("connection file")) return "Could not read the bridge connection file.";
  if (detail.includes("bridge protocol") || detail.includes("bridge version") || detail.includes("lacks silent-position") || detail.includes("bridge does not support position control")) {
    return "The bridge is incompatible with this version of Navmut.";
  }
  if (detail.includes("selected game window is unavailable") || detail.includes("selected game window is no longer valid")) {
    return "The selected game is no longer available.";
  }
  if (detail.includes("supported 1.23b") || detail.includes("retail ffxiv 1.23b") || detail.includes("sha-256 does not match")) {
    return "This game version is not supported.";
  }
  return fallback;
}

export function compactStatusMessage(message: string): string {
  const normalized = message.replace(/\s+/g, " ").trim();
  return normalized.length <= STATUS_MESSAGE_LIMIT
    ? normalized
    : `${normalized.slice(0, STATUS_MESSAGE_LIMIT - 3)}...`;
}

export function gameStateFailure(error: unknown): { message: string; isError: boolean; persistent: boolean } {
  if (rawErrorMessage(error).toLowerCase().includes(PLAYER_STATE_NOT_READY.toLowerCase())) {
    return { message: "Waiting for a logged in character...", isError: false, persistent: false };
  }
  return { message: errorMessage(error, "Could not read game state."), isError: true, persistent: true };
}

export function gameConnectionStatus(game: Pick<GameState, "connected" | "pid">): string {
  if (game.connected) return "Ready";
  return game.pid === null ? "Waiting for a game..." : "Waiting for a logged in character...";
}

export function observationPayload(name: string, type: ObservationForm["type"], notes: string): ObservationForm {
  return { name: name.trim(), type, notes };
}

export function settingsForPersistence(settings: UiSettings): PersistedSettings {
  const { selectedPid: _selectedPid, overrideZone: _overrideZone, stepSize: _stepSize, ...persisted } = settings;
  return persisted;
}

export function autoAttachPid(processes: { pid: number }[]): number | null {
  return processes.length === 1 ? processes[0].pid : null;
}

function disconnectedGameState(pid: number | null): GameState {
  return {
    connected: false,
    pid,
    character: "",
    zone: "",
    zoneId: null,
    regionId: null,
    position: { x: 0, y: 0, z: 0 },
    rotation: 0,
    mapId: "",
  };
}

export function profileLabel(profile: Pick<MapProfile, "name" | "zone">): string {
  const zoneName = profile.name.replace(/\s+-\s+map\d+$/i, "");
  return `${zoneName}${profile.zone === null || profile.zone === undefined ? "" : ` [${profile.zone}]`}`;
}

function alphabeticSuffix(index: number): string {
  let value = index + 1;
  let result = "";
  while (value > 0) {
    value -= 1;
    result = String.fromCharCode(65 + (value % 26)) + result;
    value = Math.floor(value / 26);
  }
  return result;
}

export function profileLabels(profiles: MapProfile[]): Map<string, string> {
  const groups = new Map<string, MapProfile[]>();
  profiles.forEach((profile) => {
    const base = profileLabel(profile);
    groups.set(base, [...(groups.get(base) ?? []), profile]);
  });
  const labels = new Map<string, string>();
  groups.forEach((members, base) => {
    members.sort((left, right) => left.id.localeCompare(right.id));
    members.forEach((profile, index) => {
      labels.set(profile.id, members.length === 1 ? base : `${base} - ${alphabeticSuffix(index)}`);
    });
  });
  return labels;
}

export function poiLabel(poi: Pick<PointOfInterest, "name" | "category">): string {
  const capitalized = poi.name.replace(/(^|\s)([a-z])/g, (_match: string, spacing: string, letter: string) => `${spacing}${letter.toUpperCase()}`);
  return poi.category === "aetheryte" && !/\baetheryte$/i.test(capitalized)
    ? `${capitalized} Aetheryte`
    : capitalized;
}

interface ShortcutInput {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

export function isBrowserShortcut(event: ShortcutInput, editable: boolean): boolean {
  if (/^F(?:[1-9]|1[0-2])$/.test(event.key)) return true;
  if (event.altKey && (event.key === "ArrowLeft" || event.key === "ArrowRight")) return true;
  if (!editable && event.key === "Backspace") return true;
  if (!(event.ctrlKey || event.metaKey)) return false;
  const key = event.key.toLowerCase();
  if (["f", "l", "n", "o", "p", "r", "s", "t", "u", "w", "+", "-", "0", "="].includes(key)) return true;
  return event.shiftKey && ["c", "i", "j"].includes(key);
}

function isEditableTarget(target: EventTarget | null): boolean {
  return target instanceof HTMLInputElement
    || target instanceof HTMLSelectElement
    || target instanceof HTMLTextAreaElement
    || (target instanceof HTMLElement && target.isContentEditable);
}

export function selectProfileForLiveZone(profiles: MapProfile[], game: GameState, override: boolean, requestedId: string): string {
  const requested = profiles.find((profile) => profile.id === requestedId);
  if (override && requested) return requested.id;
  const valid = (profile: MapProfile) => profile.bounds.maxX > profile.bounds.minX
    && profile.bounds.maxZ > profile.bounds.minZ
    && profile.zone === game.zoneId
    && profile.region === game.regionId
    && game.position.x >= profile.bounds.minX
    && game.position.x <= profile.bounds.maxX
    && game.position.z >= profile.bounds.minZ
    && game.position.z <= profile.bounds.maxZ;
  const live = profiles.filter(valid);
  const selected = live.reduce<MapProfile | undefined>((best, profile) => {
    if (!best) return profile;
    const area = (profile.bounds.maxX - profile.bounds.minX) * (profile.bounds.maxZ - profile.bounds.minZ);
    const bestArea = (best.bounds.maxX - best.bounds.minX) * (best.bounds.maxZ - best.bounds.minZ);
    return area >= bestArea ? profile : best;
  }, undefined);
  return selected?.id ?? requested?.id ?? profiles[0]?.id ?? "";
}

export async function loadProfileSelection(
  bridge: Pick<NavmutBridge, "loadProfile" | "listJournal">,
  profile: MapProfile | undefined,
  profileId: string,
  isCurrent: () => boolean,
): Promise<{ profile: MapProfile | undefined; journal: JournalEntry[] } | null> {
  const loadedProfile = profile && !profile.artwork ? await bridge.loadProfile(profileId) : profile;
  if (!isCurrent()) return null;
  const journal = profileId ? await bridge.listJournal(profileId) : [];
  if (!isCurrent()) return null;
  return { profile: loadedProfile, journal };
}

function byId(id: string): HTMLElement {
  const element = document.getElementById(id);
  if (!element) throw new Error(`Missing UI element: ${id}`);
  return element;
}

function button(id: string, label: string, className?: string): HTMLButtonElement {
  const element = byId(id);
  if (!(element instanceof HTMLButtonElement)) throw new Error(`${id} is not a button`);
  element.setAttribute("aria-label", label);
  if (className) element.className = className;
  return element;
}

function formatNumber(value: number): string {
  return value.toFixed(2).replace("-0.00", "0.00");
}

function formatPosition(position: Position): string {
  return `${formatNumber(position.x)}, ${formatNumber(position.y)}, ${formatNumber(position.z)}`;
}

function formatTime(value: string): string {
  return new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" }).format(new Date(value));
}

function getProfile(state: AppState): MapProfile | undefined {
  return state.profiles.find((profile) => profile.id === state.settings.profileId) ?? state.profiles[0];
}

export function movementContext(state: Pick<AppState, "settings" | "game" | "profiles">): MovementContext | null {
  const profile = state.profiles.find((candidate) => candidate.id === state.settings.profileId) ?? state.profiles[0];
  if (!state.game.connected || state.settings.selectedPid === null || !profile || profile.canMove === false) return null;
  return { pid: state.settings.selectedPid, profileId: profile.id, overrideZone: state.settings.overrideZone };
}

export function isCurrentMovementContext(state: Pick<AppState, "settings" | "game" | "profiles">, context: MovementContext): boolean {
  const current = movementContext(state);
  return current !== null
    && current.pid === context.pid
    && current.profileId === context.profileId
    && current.overrideZone === context.overrideZone;
}

function effectiveZoneId(state: AppState): number | null {
  const profile = getProfile(state);
  return state.settings.overrideZone ? profile?.zone ?? state.game.zoneId : state.game.zoneId;
}

export interface MapTransform {
  scale: number;
  offsetX: number;
  offsetY: number;
  imageWidth: number;
  imageHeight: number;
}

const mapTransforms = new Map<string, MapTransform>();
const mapImages = new Map<string, HTMLImageElement>();
const sourceCanvases = new Map<string, HTMLCanvasElement>();

export function createMapTransform(_profile: MapProfile, width: number, height: number, imageWidth: number, imageHeight: number): MapTransform {
  const drawableWidth = Math.max(1, width - MAP_INSET);
  const drawableHeight = Math.max(1, height - MAP_INSET);
  const scale = Math.min(drawableWidth / imageWidth, drawableHeight / imageHeight);
  return { scale, offsetX: (width - imageWidth * scale) / 2, offsetY: (height - imageHeight * scale) / 2, imageWidth, imageHeight };
}

function worldToCanvas(profile: MapProfile, transform: MapTransform, x: number, z: number): { x: number; y: number } {
  const xRatio = (x - profile.bounds.minX) / (profile.bounds.maxX - profile.bounds.minX);
  const zRatio = (z - profile.bounds.minZ) / (profile.bounds.maxZ - profile.bounds.minZ);
  return { x: transform.offsetX + xRatio * transform.imageWidth * transform.scale, y: transform.offsetY + zRatio * transform.imageHeight * transform.scale };
}

function imageFor(profile: MapProfile): HTMLImageElement | null {
  const artwork = profile.artwork;
  if (!artwork) return null;
  const key = profile.id;
  const existing = mapImages.get(key);
  if (existing) return existing;
  const image = new Image();
  image.src = `data:${artwork.mime};base64,${artwork.data}`;
  mapImages.set(key, image);
  return image;
}

function drawPoi(context: CanvasRenderingContext2D, point: { x: number; y: number }, aetheryte: boolean): void {
  context.save();
  context.fillStyle = aetheryte ? "#5cc8ff" : "#d28cff";
  context.strokeStyle = aetheryte ? "#d7e4ee" : "#f0d8ff";
  context.lineWidth = 1;
  context.beginPath();
  if (aetheryte) {
    context.moveTo(point.x, point.y - 7);
    context.lineTo(point.x + 7, point.y);
    context.lineTo(point.x, point.y + 7);
    context.lineTo(point.x - 7, point.y);
    context.closePath();
  } else {
    context.arc(point.x, point.y, 5, 0, Math.PI * 2);
  }
  context.fill();
  context.stroke();
  context.restore();
}

function drawSquare(context: CanvasRenderingContext2D, point: { x: number; y: number }): void {
  context.fillStyle = "#8ed8ba";
  context.strokeStyle = "#101b27";
  context.fillRect(point.x - 4, point.y - 4, 8, 8);
  context.strokeRect(point.x - 4, point.y - 4, 8, 8);
}

function drawObservation(context: CanvasRenderingContext2D, point: { x: number; y: number }): void {
  context.fillStyle = "#e2ebf0";
  context.beginPath();
  context.arc(point.x, point.y, 3, 0, Math.PI * 2);
  context.fill();
}

function drawPlayer(context: CanvasRenderingContext2D, point: { x: number; y: number }, rotation: number): void {
  const dx = Math.sin(rotation);
  const dy = Math.cos(rotation);
  const sx = dy;
  const sy = -dx;
  context.fillStyle = "#72df8f";
  context.strokeStyle = "#102116";
  context.lineWidth = 1;
  context.beginPath();
  context.moveTo(point.x + dx * 12, point.y + dy * 12);
  context.lineTo(point.x - dx * 6 + sx * 6, point.y - dy * 6 + sy * 6);
  context.lineTo(point.x - dx * 6 - sx * 6, point.y - dy * 6 - sy * 6);
  context.closePath();
  context.fill();
  context.stroke();
}

function drawMap(canvas: HTMLCanvasElement, state: AppState): void {
  const profile = getProfile(state);
  const context = canvas.getContext("2d");
  if (!context || !profile) return;
  const ratio = window.devicePixelRatio || 1;
  const bounds = canvas.getBoundingClientRect();
  const width = Math.max(1, Math.floor(bounds.width));
  const height = Math.max(1, Math.floor(bounds.height));
  if (canvas.width !== width * ratio || canvas.height !== height * ratio) {
    canvas.width = width * ratio;
    canvas.height = height * ratio;
  }
  context.setTransform(ratio, 0, 0, ratio, 0, 0);
  context.clearRect(0, 0, width, height);
  context.fillStyle = "#16100f";
  context.fillRect(0, 0, width, height);
  context.fillStyle = "#261d1a";
  context.fillRect(MAP_INSET / 2, MAP_INSET / 2, width - MAP_INSET, height - MAP_INSET);
  const image = imageFor(profile);
  const imageWidth = profile.artwork?.width || 1;
  const imageHeight = profile.artwork?.height || 1;
  const transform = createMapTransform(profile, width, height, imageWidth, imageHeight);
  mapTransforms.set(profile.id, transform);
  if (image && image.complete && image.naturalWidth > 0) {
    context.drawImage(image, transform.offsetX, transform.offsetY, transform.imageWidth * transform.scale, transform.imageHeight * transform.scale);
  } else if (image) {
    image.onload = () => drawMap(canvas, state);
  }
  context.save();
  context.beginPath();
  context.rect(transform.offsetX, transform.offsetY, transform.imageWidth * transform.scale, transform.imageHeight * transform.scale);
  context.clip();
  context.strokeStyle = "#d7e4ee";
  context.lineWidth = 1;
  context.setLineDash([2, 6]);
  const worldPixelsPerUnit = transform.scale * transform.imageWidth / (profile.bounds.maxX - profile.bounds.minX);
  const gridSpacing = 10 ** Math.ceil(Math.log10(80 / Math.max(worldPixelsPerUnit, Number.MIN_VALUE)));
  const firstX = Math.ceil(profile.bounds.minX / gridSpacing) * gridSpacing;
  for (let x = firstX; x <= profile.bounds.maxX; x += gridSpacing) {
    const point = worldToCanvas(profile, transform, x, profile.bounds.minZ).x;
    const lineX = Math.round(point) + 0.5;
    context.beginPath();
    context.moveTo(lineX, transform.offsetY);
    context.lineTo(lineX, transform.offsetY + transform.imageHeight * transform.scale);
    context.stroke();
    context.fillStyle = "#ffffff";
    context.font = "12px 'Segoe UI', sans-serif";
    context.fillText(`X ${x}`, point + 3, transform.offsetY + 13);
  }
  const firstZ = Math.ceil(profile.bounds.minZ / gridSpacing) * gridSpacing;
  for (let z = firstZ; z <= profile.bounds.maxZ; z += gridSpacing) {
    const point = worldToCanvas(profile, transform, profile.bounds.minX, z).y;
    const lineY = Math.round(point) + 0.5;
    context.beginPath();
    context.moveTo(transform.offsetX, lineY);
    context.lineTo(transform.offsetX + transform.imageWidth * transform.scale, lineY);
    context.stroke();
    context.fillStyle = "#ffffff";
    context.font = "12px 'Segoe UI', sans-serif";
    context.fillText(`Z ${z}`, transform.offsetX + 3, point + 13);
  }
  context.restore();
  const zone = effectiveZoneId(state);
  profile.pois.filter((poi) => zone === null || poi.zone === zone).forEach((poi) => drawPoi(context, worldToCanvas(profile, transform, poi.x, poi.z), poi.category === "aetheryte"));
  state.savedPoints.filter((point) => zone === null || point.zone === zone).forEach((point) => drawSquare(context, worldToCanvas(profile, transform, point.x, point.z)));
  state.journal.filter((entry) => zone === null || entry.zone === zone).forEach((entry) => drawObservation(context, worldToCanvas(profile, transform, entry.position.x, entry.position.z)));
  if (state.game.connected && (profile.zone === null || profile.zone === state.game.zoneId)) {
    drawPlayer(context, worldToCanvas(profile, transform, state.game.position.x, state.game.position.z), state.game.rotation);
  }
}

let statusPersistent = false;

function setStatus(message: string, isError = false, persistent = false): void {
  const status = byId("status-message");
  status.textContent = compactStatusMessage(message);
  status.classList.toggle("is-error", isError);
  statusPersistent = persistent;
}

function setPollingStatus(message: string, isError = false): void {
  if (!statusPersistent) setStatus(message, isError);
}

function reportFailure(error: unknown, fallback: string): void {
  console.error(fallback, error);
  setStatus(errorMessage(error, fallback), true, true);
}

function applyWindowOpacity(opacity: number): void {
  document.documentElement.style.setProperty("--navmut-opacity", String(Math.max(35, Math.min(100, opacity)) / 100));
}

function renderProcesses(state: AppState, processes: { pid: number }[]): void {
  const select = byId("pid-select");
  select.innerHTML = "";
  processes.forEach((process) => {
    const option = document.createElement("option");
    option.value = String(process.pid);
    option.textContent = `PID ${process.pid}`;
    option.selected = process.pid === state.settings.selectedPid;
    select.append(option);
  });
  if (!processes.some((process) => process.pid === state.settings.selectedPid)) state.settings.selectedPid = autoAttachPid(processes);
}

function renderProfileOptions(state: AppState): void {
  const select = byId("profile-select");
  if (!(select instanceof HTMLSelectElement)) return;
  select.innerHTML = "";
  const labels = profileLabels(state.profiles);
  [...state.profiles].sort((left, right) => (labels.get(left.id) ?? "").localeCompare(labels.get(right.id) ?? "")).forEach((profile) => {
    const option = document.createElement("option");
    option.value = profile.id;
    option.textContent = labels.get(profile.id) ?? profileLabel(profile);
    option.selected = profile.id === state.settings.profileId;
    select.append(option);
  });
}

function pointSelections(profile: MapProfile | undefined, savedPoints: SavedPoint[], zone: number | null): PointSelection[] {
  const packaged = (profile?.pois ?? []).filter((poi) => zone === null || poi.zone === zone).map((poi) => ({ kind: "poi" as const, id: `poi:${poi.id}`, name: poiLabel(poi), x: poi.x, y: poi.y, z: poi.z, zone: poi.zone, packaged: true }));
  const saved = savedPoints.filter((point) => zone === null || point.zone === zone).map((point) => ({ kind: "saved" as const, id: `saved:${point.id}`, name: point.name, x: point.x, y: point.y, z: point.z, zone: point.zone, packaged: false }));
  return [...packaged, ...saved].sort((left, right) => left.name.localeCompare(right.name));
}

function renderPointManager(profile: MapProfile | undefined, savedPoints: SavedPoint[], zone: number | null = null): void {
  const select = byId("point-select");
  const coords = byId("point-coords");
  const remove = byId("delete-point");
  if (!(select instanceof HTMLSelectElement) || !(remove instanceof HTMLButtonElement)) return;
  const points = pointSelections(profile, savedPoints, zone);
  const selectedId = select.value;
  select.innerHTML = "";
  points.forEach((point) => {
    const option = document.createElement("option");
    option.value = point.id;
    option.textContent = point.name;
    select.append(option);
  });
  const selected = points.find((point) => point.id === selectedId) ?? points[0];
  if (selected) select.value = selected.id;
  coords.textContent = selected ? `(${formatPosition({ x: selected.x, y: selected.y, z: selected.z })})` : "(0.00, 0.00, 0.00)";
  remove.disabled = !selected || selected.packaged;
}

function selectedPoint(profile: MapProfile | undefined, savedPoints: SavedPoint[], zone: number | null = null): PointSelection | undefined {
  const select = byId("point-select");
  if (!(select instanceof HTMLSelectElement)) return undefined;
  return pointSelections(profile, savedPoints, zone).find((point) => point.id === select.value);
}

export function isDeletablePoint(point: PointSelection | undefined): boolean {
  return Boolean(point && !point.packaged && point.kind === "saved");
}

function renderJournal(entries: JournalEntry[]): void {
  const list = byId("journal-list");
  list.innerHTML = "";
  if (entries.length === 0) {
    const empty = document.createElement("li");
    empty.className = "empty-state";
    empty.textContent = "No observations captured.";
    list.append(empty);
    return;
  }
  entries.forEach((entry) => {
    const item = document.createElement("li");
    item.className = "journal-entry";
    const name = document.createElement("strong");
    name.textContent = entry.name;
    const identity = document.createElement("span");
    identity.textContent = `${entry.id} - ${entry.type.toUpperCase()}`;
    const details = document.createElement("small");
    details.textContent = `${formatPosition(entry.position)} - ${formatTime(entry.capturedAt)} - ${entry.profile}`;
    const notes = document.createElement("em");
    notes.textContent = entry.notes;
    item.append(name, identity, details, notes);
    list.append(item);
  });
}

function render(state: AppState): void {
  const profile = getProfile(state);
  document.querySelector(".app-window")?.classList.toggle("is-map-collapsed", !state.settings.mapVisible);
  byId("map-shell").classList.toggle("is-hidden", !state.settings.mapVisible);
  const showMap = byId("show-map");
  if (showMap instanceof HTMLInputElement) showMap.checked = state.settings.mapVisible;
  const override = byId("override-zone");
  if (override instanceof HTMLInputElement) override.checked = state.settings.overrideZone;
  byId("warp-panel").hidden = state.settings.activePanel !== "warp";
  byId("journal-panel").hidden = state.settings.activePanel !== "journal";
  document.querySelectorAll<HTMLButtonElement>("[data-panel]").forEach((tab) => {
    const active = tab.dataset.panel === state.settings.activePanel;
    tab.classList.toggle("is-active", active);
    tab.setAttribute("aria-selected", String(active));
    tab.tabIndex = active ? 0 : -1;
  });
  const stepSelect = byId("step-select");
  if (stepSelect instanceof HTMLSelectElement) stepSelect.value = String(state.settings.stepSize);
  const canvas = byId("map-canvas");
  if (canvas instanceof HTMLCanvasElement) drawMap(canvas, state);
  renderPointManager(profile, state.savedPoints, effectiveZoneId(state));
  renderJournal(state.journal);
  const opacity = byId("opacity-range");
  const opacityValue = byId("opacity-value");
  if (opacity instanceof HTMLInputElement) opacity.value = String(state.settings.opacity);
  opacityValue.textContent = `${state.settings.opacity}%`;
  const top = byId("always-on-top");
  if (top instanceof HTMLInputElement) top.checked = state.settings.alwaysOnTop;
  const liveAvailable = liveActionsAvailable(state);
  ["pid-select", "refresh-pid", "refresh-zone", "add-point", "warp-point", "to-position", "capture-button"].forEach((id) => {
    const control = document.getElementById(id);
    if (control instanceof HTMLButtonElement || control instanceof HTMLSelectElement || control instanceof HTMLInputElement) control.disabled = !liveAvailable;
  });
  document.querySelectorAll<HTMLButtonElement>("[data-direction]").forEach((control) => { control.disabled = !liveAvailable; });
}

async function persist(state: AppState, bridge: NavmutBridge): Promise<void> {
  await bridge.saveSettings(settingsForPersistence(state.settings));
}

async function refreshGame(state: AppState, bridge: NavmutBridge, changeProfile: (profileId: string, saveSettings?: boolean) => Promise<boolean>): Promise<void> {
  if (gameRefreshPending) return;
  if (!liveActionsAvailable(state)) {
    setPollingStatus(UNSUPPORTED_MESSAGE);
    return;
  }
  gameRefreshPending = true;
  const wasConnected = state.game.connected;
  try {
    state.game = await bridge.getGameState(state.settings.selectedPid);
    const nextProfileId = selectProfileForLiveZone(state.profiles, state.game, state.settings.overrideZone, state.settings.profileId);
    if (nextProfileId !== state.settings.profileId) {
      await changeProfile(nextProfileId, false);
    }
    if (!state.game.connected) setPollingStatus(gameConnectionStatus(state.game));
    else if (!wasConnected) setStatus("Ready");
    render(state);
  } catch (error) {
    const failure = gameStateFailure(error);
    if (failure.isError && !statusPersistent) console.error("Could not read game state.", error);
    state.game = disconnectedGameState(state.settings.selectedPid);
    setStatus(failure.message, failure.isError, failure.persistent);
    render(state);
  } finally {
    gameRefreshPending = false;
  }
}

let stateBridge: NavmutBridge | null = null;
let gameRefreshPending = false;

async function warpTo(state: AppState, position: Position, intent: WarpIntent = "map", targetZone: number | null = null): Promise<void> {
  const context = movementContext(state);
  if (!context || !stateBridge) {
    setStatus("Select a live game before warping.", true);
    return;
  }
  return enqueueMovement(state, async () => {
    try {
      if (!isCurrentMovementContext(state, context)) {
        throw new UiError("Warp cancelled because the selected game or map changed.");
      }
      state.game = await stateBridge!.warp(position, context, intent, targetZone);
      setStatus("Warp sent.");
      render(state);
    } catch (error) {
      reportFailure(error, "Could not warp.");
    }
  });
}

function pointSelectSetup(state: AppState, bridge: NavmutBridge): void {
  const pointSelect = byId("point-select");
  const warpSelected = () => {
    const point = selectedPoint(getProfile(state), state.savedPoints, effectiveZoneId(state));
    if (point) void warpTo(state, { x: point.x, y: point.y, z: point.z }, "point", point.zone);
  };
  pointSelect.addEventListener("change", () => renderPointManager(getProfile(state), state.savedPoints, effectiveZoneId(state)));
  pointSelect.addEventListener("dblclick", warpSelected);
  pointSelect.addEventListener("keydown", (event) => { if (event.key === "Enter") { event.preventDefault(); warpSelected(); } });
  button("add-point", "Add saved point", "outline-button").addEventListener("click", async () => {
    const input = byId("point-name");
    if (!(input instanceof HTMLInputElement) || !input.value.trim()) {
      setStatus("Enter a point name.", true);
      return;
    }
    try {
      const pid = state.settings.selectedPid;
      if (pid === null || !state.game.connected) throw new UiError("Select a live game before saving a point.");
      await bridge.savePoint(input.value.trim(), pid);
      state.savedPoints = await bridge.listSavedPoints();
      input.value = "";
      render(state);
      setStatus("Point saved.");
    } catch (error) {
      reportFailure(error, "Could not save point.");
    }
  });
  button("warp-point", "Warp to selected point", "outline-button").addEventListener("click", () => {
    warpSelected();
  });
  button("delete-point", "Delete selected saved point", "outline-button").addEventListener("click", async () => {
    const point = selectedPoint(getProfile(state), state.savedPoints, effectiveZoneId(state));
    if (!point || !isDeletablePoint(point)) return;
    const saved = state.savedPoints.find((candidate) => `saved:${candidate.id}` === point.id);
    if (!saved) return;
    try {
      await bridge.deletePoint(saved);
      state.savedPoints = state.savedPoints.filter((candidate) => candidate.id !== saved.id);
      render(state);
      setStatus("Point deleted.");
    } catch (error) {
      reportFailure(error, "Could not delete point.");
    }
  });
  button("to-position", "Set game position", "outline-button").addEventListener("click", async () => {
    const position = parsePositionFields((byId("position-input") as HTMLInputElement).value);
    const context = movementContext(state);
    if (!position || !context) {
      setStatus("Enter finite X, Y, and Z coordinates.", true);
      return;
    }
    void enqueueMovement(state, async () => {
      try {
        if (!isCurrentMovementContext(state, context)) throw new UiError("Position change cancelled because the selected game or map changed.");
        state.game = await bridge.setPosition(position, context);
        render(state);
        setStatus("Position sent.");
      } catch (error) {
        reportFailure(error, "Could not set the position.");
      }
    });
  });
}

export function moveDelta(direction: string, amount: number): Position {
  const values: Record<string, Position> = {
    nw: { x: -amount, y: 0, z: -amount }, n: { x: 0, y: 0, z: -amount }, ne: { x: amount, y: 0, z: -amount },
    w: { x: -amount, y: 0, z: 0 }, e: { x: amount, y: 0, z: 0 },
    sw: { x: -amount, y: 0, z: amount }, s: { x: 0, y: 0, z: amount }, se: { x: amount, y: 0, z: amount },
    up: { x: 0, y: amount, z: 0 }, down: { x: 0, y: -amount, z: 0 },
  };
  return values[direction] ?? { x: 0, y: 0, z: 0 };
}

const PANEL_ORDER: UiSettings["activePanel"][] = ["warp", "journal"];

export function panelForNavigation(current: UiSettings["activePanel"], key: string): UiSettings["activePanel"] | null {
  if (key === "Home") return PANEL_ORDER[0];
  if (key === "End") return PANEL_ORDER[PANEL_ORDER.length - 1];
  if (key !== "ArrowLeft" && key !== "ArrowRight") return null;
  const offset = key === "ArrowLeft" ? -1 : 1;
  const index = PANEL_ORDER.indexOf(current);
  return PANEL_ORDER[(index + offset + PANEL_ORDER.length) % PANEL_ORDER.length];
}

export function movementDirectionForKey(key: string): string | null {
  return ({
    ArrowUp: "n",
    ArrowDown: "s",
    ArrowLeft: "w",
    ArrowRight: "e",
    PageUp: "up",
    PageDown: "down",
  } as Record<string, string>)[key] ?? null;
}

function move(state: AppState, delta: Position): void {
  const context = movementContext(state);
  if (!stateBridge || !context) {
    setStatus("Select a live game before moving.", true);
    return;
  }
  void enqueueMovement(state, async () => {
    try {
      if (!isCurrentMovementContext(state, context)) throw new UiError("Movement cancelled because the selected game or map changed.");
      state.game = await stateBridge!.move(delta, context);
      setStatus("Movement sent.");
      render(state);
    } catch (error) {
      reportFailure(error, "Could not move.");
    }
  });
}

function sourcePixelIsOpaque(profile: MapProfile, image: HTMLImageElement, sourceX: number, sourceY: number): boolean {
  const key = profile.id;
  let source = sourceCanvases.get(key);
  const width = image.naturalWidth || profile.artwork?.width || 0;
  const height = image.naturalHeight || profile.artwork?.height || 0;
  if (!width || !height) return false;
  if (!source || source.width !== width || source.height !== height) {
    source = document.createElement("canvas");
    source.width = width;
    source.height = height;
    source.getContext("2d")?.drawImage(image, 0, 0, width, height);
    sourceCanvases.set(key, source);
  }
  const x = Math.max(0, Math.min(width - 1, Math.floor(sourceX)));
  const y = Math.max(0, Math.min(height - 1, Math.floor(sourceY)));
  return (source.getContext("2d")?.getImageData(x, y, 1, 1).data[3] ?? 0) !== 0;
}

function canvasPosition(event: MouseEvent, canvas: HTMLCanvasElement, profile: MapProfile): Position | null {
  const rect = canvas.getBoundingClientRect();
  const x = event.clientX - rect.left;
  const y = event.clientY - rect.top;
  const artwork = profile.artwork;
  const image = imageFor(profile);
  const transform = mapTransforms.get(profile.id) ?? (artwork ? createMapTransform(profile, rect.width, rect.height, artwork.width, artwork.height) : null);
  if (!artwork || !image || !transform || !image.complete || image.naturalWidth === 0) return null;
  const sourceX = (x - transform.offsetX) / transform.scale;
  const sourceY = (y - transform.offsetY) / transform.scale;
  if (sourceX < 0 || sourceY < 0 || sourceX >= transform.imageWidth || sourceY >= transform.imageHeight) return null;
  if (!sourcePixelIsOpaque(profile, image, sourceX, sourceY)) return null;
  const mapX = profile.bounds.minX + (sourceX / transform.imageWidth) * (profile.bounds.maxX - profile.bounds.minX);
  const mapZ = profile.bounds.minZ + (sourceY / transform.imageHeight) * (profile.bounds.maxZ - profile.bounds.minZ);
  return { x: mapX, y: 0, z: mapZ };
}

async function setup(): Promise<void> {
  const brandIcon = document.getElementById("brand-icon");
  if (brandIcon instanceof HTMLImageElement) brandIcon.src = BRAND_ICON_URL;
  const bridge = createBridge();
  stateBridge = bridge;
  // Keep the window controls available even when optional startup data fails.
  button("title-minimize", "Minimize window").addEventListener("click", () => void bridge.minimizeWindow().catch((error) => reportFailure(error, "Could not minimize Navmut.")));
  button("title-close", "Close Navmut").addEventListener("click", () => void bridge.closeWindow().catch((error) => reportFailure(error, "Could not close Navmut.")));
  setStatus("Loading settings...");
  const settings = { ...defaultSettings(), ...(await bridge.loadSettings()) };
  const capabilities = await bridge.getCapabilities();
  const nativeWindowOpacity = isTauriRuntime() && navigator.userAgent.includes("Windows");
  await bridge.resizeWindow(settings.mapVisible ? WINDOW_WIDTH : RAIL_WIDTH);
  if (nativeWindowOpacity) await bridge.setWindowOpacity(settings.opacity);
  else applyWindowOpacity(settings.opacity);
  await bridge.setAlwaysOnTop(settings.alwaysOnTop);
  setStatus("Loading game connections...");
  const [processes, savedPoints] = await Promise.all([bridge.listProcesses(), bridge.listSavedPoints()]);
  setStatus("Loading maps...");
  let unsortedProfiles = await bridge.listProfiles();
  if (unsortedProfiles.length === 0) unsortedProfiles = await bridge.selectCatalog();
  const profiles = [...unsortedProfiles];
  settings.selectedPid = settings.selectedPid ?? autoAttachPid(processes);
  setStatus("Connecting to game...");
  const startupGame = await bridge.getGameState(settings.selectedPid).then(
    (game) => ({ game, failure: null }),
    (error: unknown) => ({ game: disconnectedGameState(settings.selectedPid), failure: gameStateFailure(error) }),
  );
  const { game, failure: startupGameFailure } = startupGame;
  const profileId = selectProfileForLiveZone(profiles, game, settings.overrideZone, settings.profileId);
  if (profileId) {
    const index = profiles.findIndex((profile) => profile.id === profileId);
    if (index >= 0) profiles[index] = await bridge.loadProfile(profileId);
  }
  stateBridge = bridge;
  const journal = profileId ? await bridge.listJournal(profileId) : [];
  const state: AppState = { settings: { ...settings, profileId }, game, profiles, journal, savedPoints, movementQueue: Promise.resolve(), polling: null, capabilities };
  renderProcesses(state, processes);
  render(state);
  if (!liveActionsAvailable(state)) setStatus(UNSUPPORTED_MESSAGE);
  else if (startupGameFailure) setStatus(startupGameFailure.message, startupGameFailure.isError, startupGameFailure.persistent);
  else setStatus(gameConnectionStatus(game));

  let profileChangeSequence = 0;
  const completeProfileChange = async (profileId: string, sequence: number, saveSettings: boolean): Promise<boolean> => {
    const selection = await loadProfileSelection(
      bridge,
      state.profiles.find((profile) => profile.id === profileId),
      profileId,
      () => sequence === profileChangeSequence,
    );
    if (!selection) return false;
    if (selection.profile) {
      const index = state.profiles.findIndex((profile) => profile.id === profileId);
      if (index >= 0) state.profiles[index] = selection.profile;
    }
    state.journal = selection.journal;
    if (saveSettings) {
      await persist(state, bridge);
      if (sequence !== profileChangeSequence) return false;
    }
    renderProfileOptions(state);
    render(state);
    return true;
  };
  const changeProfile = (profileId: string, saveSettings = true): Promise<boolean> => {
    const sequence = ++profileChangeSequence;
    state.settings.profileId = profileId;
    renderProfileOptions(state);
    render(state);
    return completeProfileChange(profileId, sequence, saveSettings);
  };

  byId("pid-select").addEventListener("change", (event) => {
    const select = event.currentTarget;
    if (!(select instanceof HTMLSelectElement)) return;
    state.settings.selectedPid = Number(select.value) || null;
    void refreshGame(state, bridge, changeProfile);
  });
  button("refresh-pid", "Refresh game processes", "outline-button").addEventListener("click", () => void bridge.listProcesses().then((next) => { renderProcesses(state, next); render(state); }).catch((error) => reportFailure(error, "Could not refresh game processes.")));
  const selectPanel = (panel: UiSettings["activePanel"], focus: boolean): void => {
    if (state.settings.activePanel !== panel) {
      state.settings.activePanel = panel;
      void persist(state, bridge).catch((error) => reportFailure(error, "Could not save settings."));
      render(state);
    }
    if (focus) document.querySelector<HTMLButtonElement>(`[data-panel="${panel}"]`)?.focus();
  };
  document.querySelectorAll<HTMLButtonElement>("[data-panel]").forEach((tab) => {
    tab.addEventListener("click", () => {
      const panel = tab.dataset.panel;
      if (panel === "warp" || panel === "journal") selectPanel(panel, false);
    });
    tab.addEventListener("keydown", (event) => {
      const panel = tab.dataset.panel;
      if (panel !== "warp" && panel !== "journal") return;
      const next = panelForNavigation(panel, event.key);
      if (!next) return;
      event.preventDefault();
      selectPanel(next, true);
    });
  });
  byId("step-select").addEventListener("change", (event) => {
    const select = event.currentTarget;
    if (!(select instanceof HTMLSelectElement)) return;
    const amount = Number(select.value);
    if (!MOVE_STEPS.includes(amount as typeof MOVE_STEPS[number])) return;
    state.settings.stepSize = amount as UiSettings["stepSize"];
    render(state);
  });
  const override = byId("override-zone");
  override.addEventListener("change", () => {
    if (!(override instanceof HTMLInputElement)) return;
    state.settings.overrideZone = override.checked;
    const profileId = selectProfileForLiveZone(state.profiles, state.game, state.settings.overrideZone, state.settings.profileId);
    void changeProfile(profileId).catch((error) => reportFailure(error, "Could not load map profile."));
  });
  button("refresh-zone", "Refresh live zone", "outline-button").addEventListener("click", async () => {
    const sequence = ++profileChangeSequence;
    try {
      const refreshed = await bridge.refreshZone(state.settings.selectedPid);
      if (sequence !== profileChangeSequence) return;
      mapImages.clear();
      mapTransforms.clear();
      sourceCanvases.clear();
      state.game = refreshed.game;
      state.profiles = [...refreshed.profiles];
      const profileId = refreshed.profileId ?? state.settings.profileId;
      state.settings.profileId = profileId;
      renderProfileOptions(state);
      render(state);
      if (!await completeProfileChange(profileId, sequence, false)) return;
      setStatus("Zone refreshed.");
    } catch (error) {
      reportFailure(error, "Could not refresh zone.");
    }
  });
  const profileSelect = byId("profile-select");
  profileSelect.addEventListener("change", (event) => {
    const select = event.currentTarget;
    if (!(select instanceof HTMLSelectElement)) return;
    void changeProfile(select.value).catch((error) => reportFailure(error, "Could not load map profile."));
  });
  renderProfileOptions(state);
  pointSelectSetup(state, bridge);
  const showMap = byId("show-map");
  showMap.addEventListener("change", () => {
    if (!(showMap instanceof HTMLInputElement)) return;
    state.settings.mapVisible = showMap.checked;
    void persist(state, bridge).catch((error) => reportFailure(error, "Could not save settings."));
    void bridge.resizeWindow(state.settings.mapVisible ? WINDOW_WIDTH : RAIL_WIDTH).catch((error) => reportFailure(error, "Could not resize Navmut."));
    render(state);
  });
  document.querySelectorAll<HTMLButtonElement>("[data-direction]").forEach((control) => control.addEventListener("click", () => {
    const direction = control.dataset.direction;
    if (direction) move(state, moveDelta(direction, state.settings.stepSize));
  }));
  button("capture-button", "Save observation", "outline-button").addEventListener("click", async () => {
    try {
      const nameInput = byId("observation-name-id") as HTMLInputElement;
      const notesInput = byId("observation-notes") as HTMLTextAreaElement;
      const form = observationPayload(
        nameInput.value,
        (byId("observation-type") as HTMLSelectElement).value as ObservationForm["type"],
        notesInput.value,
      );
      if (!form.name) {
        setStatus("Observation name is required.", true);
        return;
      }
      const profile = getProfile(state);
      if (!profile) throw new UiError("Load a map catalog before saving an observation.");
      const pid = state.settings.selectedPid;
      if (pid === null || !state.game.connected) throw new UiError("Select a live game before saving an observation.");
      state.journal = [await bridge.captureObservation(profile.id, form, pid), ...state.journal];
      state.settings.activePanel = "journal";
      nameInput.value = "";
      notesInput.value = "";
      await persist(state, bridge);
      setStatus("Observation saved.");
      render(state);
    } catch (error) {
      reportFailure(error, "Could not capture observation.");
    }
  });
  button("export-button", "Export observations", "outline-button").addEventListener("click", async () => {
    try {
      const profile = getProfile(state);
      if (!profile) throw new UiError("Load a map catalog before exporting observations.");
      const archive = await bridge.exportJournal(profile.id);
      if (archive.path) {
        setStatus(`Observations saved to ${archive.path}.`);
      } else if (archive.data) {
        const binary = atob(archive.data);
        const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
        const url = URL.createObjectURL(new Blob([bytes], { type: archive.mime }));
        const link = document.createElement("a");
        link.href = url;
        link.download = archive.filename;
        link.click();
        URL.revokeObjectURL(url);
        setStatus("Observation archive exported.");
      }
    } catch (error) {
      reportFailure(error, "Could not export observations.");
    }
  });
  const canvas = byId("map-canvas");
  canvas.addEventListener("click", (event) => {
    if (!(event instanceof MouseEvent) || !(canvas instanceof HTMLCanvasElement)) return;
    if (!liveActionsAvailable(state)) {
      setPollingStatus(UNSUPPORTED_MESSAGE);
      return;
    }
    const profile = getProfile(state);
    const point = profile ? canvasPosition(event, canvas, profile) : null;
    if (point) void warpTo(state, point);
  });
  canvas.addEventListener("wheel", (event) => event.preventDefault(), { passive: false });
  canvas.addEventListener("pointerdown", (event) => { if (event.button === 1) event.preventDefault(); });
  const topmost = byId("always-on-top");
  topmost.addEventListener("change", () => {
    if (!(topmost instanceof HTMLInputElement)) return;
    state.settings.alwaysOnTop = topmost.checked;
    void bridge.setAlwaysOnTop(topmost.checked).catch((error) => reportFailure(error, "Could not change always on top mode."));
    void persist(state, bridge).catch((error) => reportFailure(error, "Could not save settings."));
  });
  const opacity = byId("opacity-range");
  opacity.addEventListener("input", () => {
    if (!(opacity instanceof HTMLInputElement)) return;
    state.settings.opacity = Math.max(35, Math.min(100, Number(opacity.value)));
    if (nativeWindowOpacity) void bridge.setWindowOpacity(state.settings.opacity).catch((error) => reportFailure(error, "Could not change window opacity."));
    else applyWindowOpacity(state.settings.opacity);
    render(state);
  });
  opacity.addEventListener("change", () => void persist(state, bridge).catch((error) => reportFailure(error, "Could not save settings.")));
  window.addEventListener("resize", () => render(state));
  document.addEventListener("contextmenu", (event) => event.preventDefault());
  document.addEventListener("selectstart", (event) => {
    if (!isEditableTarget(event.target)) event.preventDefault();
  });
  document.addEventListener("dragstart", (event) => {
    if (!isEditableTarget(event.target)) event.preventDefault();
  });
  window.addEventListener("keydown", (event) => {
    if (!isBrowserShortcut(event, isEditableTarget(event.target))) return;
    event.preventDefault();
    event.stopImmediatePropagation();
  }, { capture: true });
  window.addEventListener("keydown", (event) => {
    if (isEditableTarget(event.target)) return;
    if (event.target instanceof HTMLElement && event.target.getAttribute("role") === "tab") return;
    const direction = movementDirectionForKey(event.key);
    if (direction) {
      event.preventDefault();
      if (event.target instanceof HTMLElement && !event.target.classList.contains("compass-section")) event.target.blur();
      move(state, moveDelta(direction, state.settings.stepSize));
    }
  });
  state.polling = window.setInterval(() => void refreshGame(state, bridge, changeProfile), POLL_INTERVAL_MS);
}

if (typeof document !== "undefined") {
  const start = () => void setup().catch((error) => {
    console.error("Navmut could not finish starting.", error);
    const status = document.getElementById("status-message");
    if (status) {
      status.textContent = compactStatusMessage(errorMessage(error, "Navmut could not finish starting."));
      status.classList.add("is-error");
    }
  });
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
}
