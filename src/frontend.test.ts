import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { enqueueMovement, handleMovementKey, isBrowserShortcut, render, setupCapture } from "./main";
import type { AppState } from "./main";
import { defaultSettings } from "./ipc";
import type { JournalEntry, NavmutBridge, SavedPoint } from "./types";

// Fake DOM limited to what these handlers use. Replacing children resets scroll
// and focus, as a real redraw does.
class Element extends EventTarget {
  children: Element[] = [];
  textContent = "";
  className = "";
  value = "";
  disabled = false;
  hidden = false;
  checked = false;
  scrollTop = 0;
  isContentEditable = false;
  attributes = new Map<string, string>();
  classes = new Set<string>();
  classList = {
    contains: (name: string) => this.classes.has(name),
    toggle: (name: string, force: boolean) => force ? this.classes.add(name) : this.classes.delete(name),
  };
  set innerHTML(_html: string) {
    if (this.children.includes(activeElement!)) activeElement = null;
    this.children = [];
    this.value = "";
    this.scrollTop = 0;
  }
  append(...items: Element[]): void {
    this.children.push(...items);
    if (this instanceof Select && !this.value) this.value = items[0]?.value ?? "";
  }
  getAttribute(name: string): string | null { return this.attributes.get(name) ?? null; }
  setAttribute(name: string, value: string): void { this.attributes.set(name, value); }
  focus(): void { activeElement = this; }
  blur(): void { if (activeElement === this) activeElement = null; }
}
class Select extends Element {}
class Button extends Element {
  // Browsers do not dispatch clicks to disabled buttons.
  override dispatchEvent(event: Event): boolean {
    return this.disabled && event.type === "click" ? false : super.dispatchEvent(event);
  }
}
class Input extends Element {}
class Textarea extends Element {}
class Canvas extends Element {}

let activeElement: Element | null;
let elements: Map<string, Element>;

function element(id: string): Element {
  const found = elements.get(id);
  if (!found) throw new Error(`Missing test element: ${id}`);
  return found;
}

function appState(): AppState {
  return {
    settings: { ...defaultSettings(), selectedPid: 42, profileId: "map" },
    game: { connected: true, pid: 42, character: "", zone: "", zoneId: 128, regionId: 104, position: { x: 0, y: 0, z: 0 }, rotation: 0, mapId: "map" },
    profiles: [{ id: "map", name: "Map", zone: 128, region: 104, bounds: { minX: -100, maxX: 100, minZ: -100, maxZ: 100 }, pois: [], anchors: [] }],
    journal: [], savedPoints: [], movementQueue: Promise.resolve(),
    capabilities: { windows: true, playerState: true, silentPosition: true, bridgeProtocol: 2 },
    settingsWritable: true, capturePending: false,
  };
}

const observation: JournalEntry = { id: "one", name: "One", type: "npc", capturedAt: "2026-10-11T12:00:00Z", zone: 128, profile: "Map", position: { x: 1, y: 2, z: 3 }, rotation: 0, notes: "Notes" };
const point: SavedPoint = { id: "one", name: "One", x: 1, y: 2, z: 3, zone: 128 };

beforeEach(() => {
  activeElement = null;
  elements = new Map();
  for (const id of ["map-shell", "warp-panel", "journal-panel", "point-coords", "journal-list", "opacity-value", "status-message", "map-canvas"]) elements.set(id, new Element());
  for (const id of ["point-select", "step-select", "pid-select", "observation-type"]) elements.set(id, new Select());
  for (const id of ["capture-button", "delete-point", "refresh-pid", "refresh-zone", "add-point", "warp-point", "to-position"]) elements.set(id, new Button());
  for (const id of ["show-map", "override-zone", "opacity-range", "always-on-top", "observation-name-id"]) elements.set(id, new Input());
  elements.set("observation-notes", new Textarea());
  element("observation-name-id").value = "One";
  element("observation-type").value = "npc";
  vi.stubGlobal("HTMLElement", Element);
  vi.stubGlobal("HTMLSelectElement", Select);
  vi.stubGlobal("HTMLButtonElement", Button);
  vi.stubGlobal("HTMLInputElement", Input);
  vi.stubGlobal("HTMLTextAreaElement", Textarea);
  vi.stubGlobal("HTMLCanvasElement", Canvas);
  vi.stubGlobal("document", {
    getElementById: (id: string) => elements.get(id),
    createElement: () => new Element(),
    querySelector: () => null,
    querySelectorAll: () => [],
  });
  vi.spyOn(console, "error").mockImplementation(() => undefined);
});

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

function keyEvent(key: string, repeat: boolean, target: Element): KeyboardEvent {
  const event = new Event("keydown", { cancelable: true });
  Object.defineProperties(event, { key: { value: key }, repeat: { value: repeat }, target: { value: target } });
  return event as KeyboardEvent;
}

describe("movement key handler", () => {
  it.each(["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "PageUp", "PageDown"])("queues one move per press of %s", async (key) => {
    const state = appState();
    const move = vi.fn(async () => undefined);
    const sendMove = () => { void enqueueMovement(state, move); };
    const target = new Element();
    target.classes.add("compass-section");
    handleMovementKey(keyEvent(key, false, target), sendMove);
    for (let index = 0; index < 20; index++) {
      const repeated = keyEvent(key, true, target);
      handleMovementKey(repeated, sendMove);
      expect(repeated.defaultPrevented).toBe(true);
    }
    await state.movementQueue;
    expect(move).toHaveBeenCalledTimes(1);
    handleMovementKey(keyEvent(key, false, target), sendMove);
    await state.movementQueue;
    expect(move).toHaveBeenCalledTimes(2);
  });

  it("leaves editor and tab navigation alone and keeps Alt navigation blocked", () => {
    const move = vi.fn();
    const tab = new Element();
    tab.setAttribute("role", "tab");
    for (const target of [new Input(), new Select(), new Textarea(), tab]) {
      const event = keyEvent("ArrowRight", false, target);
      handleMovementKey(event, move);
      expect(event.defaultPrevented).toBe(false);
    }
    expect(move).not.toHaveBeenCalled();
    expect(isBrowserShortcut({ key: "ArrowLeft", altKey: true, ctrlKey: false, metaKey: false, shiftKey: false }, false)).toBe(true);
  });
});

describe("capture handler", () => {
  it("ignores overlapping clicks through capture and settings saves, including polling renders", async () => {
    const state = appState();
    let completeCapture!: (entry: JournalEntry) => void;
    let completeSettings!: () => void;
    const captureObservation = vi.fn(() => new Promise<JournalEntry>((resolve) => { completeCapture = resolve; }));
    const saveSettings = vi.fn(() => new Promise<void>((resolve) => { completeSettings = resolve; }));
    setupCapture(state, { captureObservation, saveSettings } as unknown as NavmutBridge);
    const button = element("capture-button");
    button.dispatchEvent(new Event("click"));
    expect(button.disabled).toBe(true);
    render(state);
    expect(button.disabled).toBe(true);
    button.dispatchEvent(new Event("click"));
    expect(captureObservation).toHaveBeenCalledTimes(1);
    completeCapture(observation);
    await vi.waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(element("journal-list").children[0]?.children[0]?.textContent).toBe("One");
    expect(button.disabled).toBe(true);
    button.dispatchEvent(new Event("click"));
    expect(captureObservation).toHaveBeenCalledTimes(1);
    completeSettings();
    await vi.waitFor(() => expect(button.disabled).toBe(false));
    expect(element("status-message").textContent).toBe("Observation saved.");
    expect(state.journal).toEqual([observation]);
  });

  it("keeps the saved observation visible when settings fail", async () => {
    const state = appState();
    setupCapture(state, {
      captureObservation: vi.fn(async () => observation),
      saveSettings: vi.fn(async () => { throw new Error("disk full"); }),
    } as unknown as NavmutBridge);
    element("capture-button").dispatchEvent(new Event("click"));
    await vi.waitFor(() => expect(state.capturePending).toBe(false));
    expect(state.journal).toEqual([observation]);
    expect(state.settings.activePanel).toBe("journal");
    expect(element("journal-list").children[0]?.children[0]?.textContent).toBe("One");
    expect(element("status-message").textContent).toBe("Observation saved, but could not save settings.");
    expect(element("capture-button").disabled).toBe(false);
  });

  it("reports a capture failure and allows a retry without clearing the form", async () => {
    const state = appState();
    const captureObservation = vi.fn().mockRejectedValueOnce(new Error("disk full")).mockResolvedValue(observation);
    const saveSettings = vi.fn(async () => undefined);
    setupCapture(state, { captureObservation, saveSettings } as unknown as NavmutBridge);
    element("capture-button").dispatchEvent(new Event("click"));
    await vi.waitFor(() => expect(state.capturePending).toBe(false));
    expect(element("status-message").textContent).toBe("Could not save observation.");
    expect(state.journal).toEqual([]);
    expect(saveSettings).not.toHaveBeenCalled();
    expect(element("observation-name-id").value).toBe("One");
    expect(element("capture-button").disabled).toBe(false);
    element("capture-button").dispatchEvent(new Event("click"));
    await vi.waitFor(() => expect(state.journal).toEqual([observation]));
    expect(captureObservation).toHaveBeenCalledTimes(2);
  });
});

describe("list rendering during polling", () => {
  it("keeps point nodes, scroll, focus and selection until point data changes", () => {
    const state = appState();
    state.savedPoints = [point, { ...point, id: "two", name: "Two" }];
    render(state);
    const select = element("point-select");
    select.value = "saved:two";
    select.scrollTop = 60;
    select.focus();
    const options = [...select.children];
    for (let poll = 0; poll < 4; poll++) {
      state.game.position.x++;
      state.savedPoints = state.savedPoints.map((saved) => ({ ...saved }));
      render(state);
      expect(select.children[0]).toBe(options[0]);
      expect(select.children[1]).toBe(options[1]);
    }
    expect(select.value).toBe("saved:two");
    expect(select.scrollTop).toBe(60);
    expect(activeElement).toBe(select);
    select.value = "saved:one";
    render(state);
    expect(select.children[0]).toBe(options[0]);
    expect(select.value).toBe("saved:one");
    select.value = "saved:two";
    state.savedPoints[1].name = "Updated";
    state.savedPoints[1].x = 40;
    render(state);
    expect(select.children[1]).not.toBe(options[1]);
    expect(select.value).toBe("saved:two");
    expect(element("point-coords").textContent).toContain("40.00");
    expect(select.scrollTop).toBe(60);
    expect(activeElement).toBe(select);
    state.savedPoints = [point];
    render(state);
    expect(select.value).toBe("saved:one");
    state.game.zoneId = 140;
    render(state);
    expect(select.children).toEqual([]);
    expect(element("delete-point").disabled).toBe(true);
  });

  it("defers hidden Journal changes and preserves unchanged nodes on visible polls", () => {
    const state = appState();
    state.journal = [observation];
    render(state);
    const list = element("journal-list");
    expect(list.children).toEqual([]);
    state.settings.activePanel = "journal";
    render(state);
    const item = list.children[0];
    const name = item.children[0];
    name.focus();
    list.scrollTop = 90;
    state.journal = [{ ...observation, position: { ...observation.position } }];
    render(state);
    expect(list.children[0]).toBe(item);
    expect(list.children[0].children[0]).toBe(name);
    expect(activeElement).toBe(name);
    expect(list.scrollTop).toBe(90);
    state.settings.activePanel = "warp";
    state.journal[0].notes = "Changed";
    render(state);
    expect(list.children[0]).toBe(item);
    state.settings.activePanel = "journal";
    render(state);
    expect(list.children[0]).not.toBe(item);
    expect(list.children[0].children[3].textContent).toBe("Changed");
    expect(list.scrollTop).toBe(90);
    state.journal = [];
    render(state);
    expect(list.children[0].textContent).toBe("No saved observations.");
  });
});
