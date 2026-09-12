import { describe, expect, it } from "vitest";
import { autoAttachPid, compactStatusMessage, createMapTransform, enqueueMovement, errorMessage, gameConnectionStatus, gameStateFailure, isBrowserShortcut, isCurrentMovementContext, isDeletablePoint, loadProfileSelection, movementContext, movementDirectionForKey, moveDelta, observationPayload, panelForNavigation, parsePositionFields, poiLabel, profileLabel, profileLabels, selectProfileForLiveZone, settingsForPersistence, UiError } from "./main";
import type { GameState, JournalEntry, MapProfile, PointSelection, UiSettings } from "./types";

const profile: MapProfile = { id: "test", name: "Test", zone: 128, region: 104, bounds: { minX: -100, maxX: 100, minZ: -50, maxZ: 50 }, pois: [], anchors: [] };

describe("movement semantics", () => {
  it("keeps diagonal movement components unnormalized", () => {
    expect(moveDelta("ne", 10)).toEqual({ x: 10, y: 0, z: -10 });
    expect(moveDelta("up", 15)).toEqual({ x: 0, y: 15, z: 0 });
    expect(moveDelta("down", 5)).toEqual({ x: 0, y: -5, z: 0 });
  });

  it("maps movement shortcut keys without consuming unrelated keys", () => {
    expect(movementDirectionForKey("ArrowUp")).toBe("n");
    expect(movementDirectionForKey("PageDown")).toBe("down");
    expect(movementDirectionForKey("Tab")).toBeNull();
  });

  it("fits the complete map with aspect-ratio-preserving offsets", () => {
    const transform = createMapTransform(profile, 800, 600, 400, 200);
    expect(transform.scale).toBe(1.96);
    expect(transform.offsetX).toBe(8);
    expect(transform.offsetY).toBe(104);
  });

  it("serializes movement requests", async () => {
    const state = { movementQueue: Promise.resolve() };
    const events: string[] = [];
    enqueueMovement(state, async () => {
      events.push("first-start");
      await Promise.resolve();
      events.push("first-end");
    });
    enqueueMovement(state, async () => {
      events.push("second");
    });
    await state.movementQueue;
    expect(events).toEqual(["first-start", "first-end", "second"]);
  });
});

describe("runtime contracts", () => {
  it("wraps tab navigation and supports Home and End", () => {
    expect(panelForNavigation("warp", "ArrowLeft")).toBe("journal");
    expect(panelForNavigation("journal", "ArrowRight")).toBe("warp");
    expect(panelForNavigation("journal", "Home")).toBe("warp");
    expect(panelForNavigation("warp", "End")).toBe("journal");
    expect(panelForNavigation("warp", "Tab")).toBeNull();
  });

  it("hides internal map artwork names from profile labels", () => {
    expect(profileLabel({ name: "Central Shroud - map01201", zone: 150 })).toBe("Central Shroud [150]");
    expect(profileLabel({ name: "Navmut map", zone: null })).toBe("Navmut map");
  });

  it("letters duplicate zone labels in stable profile order", () => {
    const duplicateB = { ...profile, id: "zone-map-b", name: "Limsa Lominsa - map00002", zone: 230 };
    const duplicateA = { ...profile, id: "zone-map-a", name: "Limsa Lominsa - map00001", zone: 230 };
    const unique = { ...profile, id: "unique", name: "Central Shroud - map01201", zone: 150 };
    const labels = profileLabels([duplicateB, unique, duplicateA]);
    expect(labels.get(duplicateA.id)).toBe("Limsa Lominsa [230] - A");
    expect(labels.get(duplicateB.id)).toBe("Limsa Lominsa [230] - B");
    expect(labels.get(unique.id)).toBe("Central Shroud [150]");
  });

  it("capitalizes packaged NPC labels and identifies Aetherytes", () => {
    expect(poiLabel({ name: "muscle-bound deckhand", category: "quest_npc" })).toBe("Muscle-bound Deckhand");
    expect(poiLabel({ name: "pearly-toothed porter", category: "quest_npc" })).toBe("Pearly-toothed Porter");
    expect(poiLabel({ name: "N'mmulika", category: "quest_npc" })).toBe("N'mmulika");
    expect(poiLabel({ name: "Camp Bearded Rock", category: "aetheryte" })).toBe("Camp Bearded Rock Aetheryte");
  });

  it("blocks browser chrome shortcuts without breaking editor shortcuts", () => {
    const key = (value: string, overrides = {}) => ({ key: value, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...overrides });
    expect(isBrowserShortcut(key("F5"), false)).toBe(true);
    expect(isBrowserShortcut(key("ArrowLeft", { altKey: true }), false)).toBe(true);
    expect(isBrowserShortcut(key("r", { ctrlKey: true }), false)).toBe(true);
    expect(isBrowserShortcut(key("i", { ctrlKey: true, shiftKey: true }), false)).toBe(true);
    expect(isBrowserShortcut(key("Backspace"), false)).toBe(true);
    expect(isBrowserShortcut(key("c", { ctrlKey: true }), true)).toBe(false);
    expect(isBrowserShortcut(key("Tab"), false)).toBe(false);
  });

  it("keeps session-only settings out of persisted fields", () => {
    const settings: UiSettings = { selectedPid: 123, profileId: "map", overrideZone: true, activePanel: "warp", stepSize: 15, mapVisible: true, alwaysOnTop: true, opacity: 75 };
    const persisted = settingsForPersistence(settings);
    expect(persisted).not.toHaveProperty("selectedPid");
    expect(persisted).not.toHaveProperty("overrideZone");
    expect(persisted).not.toHaveProperty("stepSize");
    expect(persisted).toMatchObject({ profileId: "map", mapVisible: true, alwaysOnTop: true, opacity: 75 });
  });

  it("parses finite Set Position coordinates", () => {
    expect(parsePositionFields("1.25, -2 3")).toEqual({ x: 1.25, y: -2, z: 3 });
    expect(parsePositionFields("1, nope, 3")).toBeNull();
    expect(parsePositionFields("1, 2, 3, 4")).toBeNull();
  });

  it("replaces backend details with short user messages", () => {
    expect(errorMessage("bridge failed", "fallback")).toBe("fallback");
    expect(errorMessage({ message: "helper failed" }, "fallback")).toBe("fallback");
    expect(errorMessage({ reason: "unknown" }, "fallback")).toBe("fallback");
    expect(errorMessage("bridge authentication failed", "fallback")).toBe("Could not authenticate with the bridge. Check its connection file.");
    expect(errorMessage("selected game window is no longer valid", "fallback")).toBe("The selected game is no longer available.");
    expect(errorMessage("FFXIV executable SHA-256 does not match retail 1.23b", "fallback")).toBe("This game version is not supported.");
    expect(errorMessage("configured bridge lacks silent-position", "fallback")).toBe("The bridge is incompatible with this version of Navmut.");
    expect(errorMessage("movement outcome was uncertain: silent-position outcome is uncertain", "fallback")).toBe("The position outcome is unknown. Check the game before trying again.");
    expect(errorMessage(new UiError("Select a live game before saving a point."), "fallback")).toBe("Select a live game before saving a point.");
    expect(errorMessage(new Error("Select a live game before saving a point."), "fallback")).toBe("fallback");
  });

  it("presents an unavailable player state as a neutral connection wait", () => {
    expect(gameStateFailure("game connection failed: could not find live FFXIV player state")).toEqual({
      message: "Waiting for a logged in character...",
      isError: false,
      persistent: false,
    });
    expect(gameStateFailure("bridge request failed: could not find live FFXIV player state").isError).toBe(false);
    expect(gameStateFailure("bridge authentication failed")).toEqual({
      message: "Could not authenticate with the bridge. Check its connection file.",
      isError: true,
      persistent: true,
    });
  });

  it("describes disconnected game states without platform language", () => {
    expect(gameConnectionStatus({ connected: false, pid: null })).toBe("Waiting for a game...");
    expect(gameConnectionStatus({ connected: false, pid: 123 })).toBe("Waiting for a logged in character...");
    expect(gameConnectionStatus({ connected: true, pid: 123 })).toBe("Ready");
  });

  it("bounds status text and removes layout-breaking whitespace", () => {
    expect(compactStatusMessage("one\n\n two\tthree")).toBe("one two three");
    const compact = compactStatusMessage("x".repeat(500));
    expect(compact).toHaveLength(180);
    expect(compact.endsWith("...")).toBe(true);
  });

  it("builds the observation form payload without trimming notes", () => {
    expect(observationPayload("  Aetheryte / id-1  ", "npc", "line one\nline two")).toEqual({ name: "Aetheryte / id-1", type: "npc", notes: "line one\nline two" });
  });

  it("attaches only when exactly one game process exists", () => {
    expect(autoAttachPid([{ pid: 10 }])).toBe(10);
    expect(autoAttachPid([])).toBeNull();
    expect(autoAttachPid([{ pid: 10 }, { pid: 11 }])).toBeNull();
  });

  it("refresh selection follows live zone unless override is set", () => {
    const profiles = [profile, { ...profile, id: "other", name: "Other", zone: 140 }];
    const game: GameState = { connected: true, pid: 1, character: "", zone: "", zoneId: 140, regionId: 104, position: { x: 0, y: 0, z: 0 }, rotation: 0, mapId: "" };
    expect(selectProfileForLiveZone(profiles, game, false, profile.id)).toBe("other");
    expect(selectProfileForLiveZone(profiles, game, true, profile.id)).toBe(profile.id);
  });

  it("loads artwork and journal for an override-selected profile", async () => {
    const calls: string[] = [];
    const loaded = { ...profile, artwork: { mime: "image/png", data: "art", width: 1, height: 1 } };
    const result = await loadProfileSelection(
      {
        loadProfile: async (profileId) => {
          calls.push(`profile:${profileId}`);
          return loaded;
        },
        listJournal: async (profileId) => {
          calls.push(`journal:${profileId}`);
          return [];
        },
      },
      profile,
      profile.id,
      () => true,
    );
    expect(result).toEqual({ profile: loaded, journal: [] });
    expect(calls).toEqual(["profile:test", "journal:test"]);
  });

  it("discards a journal result after a newer profile selection", async () => {
    const journalResolvers = new Map<string, (entries: JournalEntry[]) => void>();
    const bridge = {
      loadProfile: async (profileId: string) => ({ ...profile, id: profileId, artwork: { mime: "image/png", data: "art", width: 1, height: 1 } }),
      listJournal: (profileId: string) => new Promise<JournalEntry[]>((resolve) => journalResolvers.set(profileId, resolve)),
    };
    const oldProfile = { ...profile, id: "old", artwork: { mime: "image/png", data: "old", width: 1, height: 1 } };
    const newProfile = { ...profile, id: "new", artwork: { mime: "image/png", data: "new", width: 1, height: 1 } };
    let selectedProfile = oldProfile.id;
    const oldSelection = loadProfileSelection(bridge, oldProfile, oldProfile.id, () => selectedProfile === oldProfile.id);
    selectedProfile = newProfile.id;
    const newSelection = loadProfileSelection(bridge, newProfile, newProfile.id, () => selectedProfile === newProfile.id);
    const entry: JournalEntry = { id: "new-entry", capturedAt: "2026-09-12T00:00:00Z", zone: 128, profile: newProfile.id, position: { x: 0, y: 0, z: 0 }, rotation: 0, name: "New", type: "misc", notes: "" };
    journalResolvers.get(newProfile.id)?.([]);
    expect(await newSelection).toEqual({ profile: newProfile, journal: [] });
    journalResolvers.get(oldProfile.id)?.([entry]);
    expect(await oldSelection).toBeNull();
  });

  it("requires the live position to be inside candidate map bounds", () => {
    const profiles = [profile, { ...profile, id: "far", name: "Far", bounds: { minX: 500, maxX: 600, minZ: 500, maxZ: 600 } }];
    const game: GameState = { connected: true, pid: 1, character: "", zone: "", zoneId: 128, regionId: 104, position: { x: 550, y: 0, z: 550 }, rotation: 0, mapId: "" };
    expect(selectProfileForLiveZone(profiles, game, false, profile.id)).toBe("far");
  });

  it("chooses the broadest map when live maps overlap", () => {
    const stale = { ...profile, id: "stale", bounds: { minX: 500, maxX: 600, minZ: 500, maxZ: 600 } };
    const broad = { ...profile, id: "broad", bounds: { minX: -100, maxX: 100, minZ: -100, maxZ: 100 } };
    const detail = { ...profile, id: "detail", bounds: { minX: -10, maxX: 10, minZ: -10, maxZ: 10 } };
    const game: GameState = { connected: true, pid: 1, character: "", zone: "", zoneId: 128, regionId: 104, position: { x: 0, y: 0, z: 0 }, rotation: 0, mapId: "" };
    expect(selectProfileForLiveZone([stale, broad, detail], game, false, detail.id)).toBe("broad");
  });

  it("uses stable catalog order for matching maps with identical bounds", () => {
    const stale = { ...profile, id: "stale", bounds: { minX: 500, maxX: 600, minZ: 500, maxZ: 600 } };
    const lower = { ...profile, id: "lower" };
    const upper = { ...profile, id: "upper" };
    const game: GameState = { connected: true, pid: 1, character: "", zone: "", zoneId: 128, regionId: 104, position: { x: 0, y: 0, z: 0 }, rotation: 0, mapId: "" };
    expect(selectProfileForLiveZone([stale, lower, upper], game, false, stale.id)).toBe("upper");
  });

  it("captures and validates the current movement context", () => {
    const state = {
      settings: { selectedPid: 42, profileId: "test", overrideZone: false },
      game: { connected: true },
      profiles: [profile],
    } as unknown as Parameters<typeof movementContext>[0];
    const context = movementContext(state);
    expect(context).toEqual({ pid: 42, profileId: "test", overrideZone: false });
    expect(context && isCurrentMovementContext(state, context)).toBe(true);
    const changed = { ...state, settings: { ...state.settings, selectedPid: 43 } };
    expect(context && isCurrentMovementContext(changed, context)).toBe(false);
  });

  it("rejects deletion for packaged POIs", () => {
    const poi: PointSelection = { kind: "poi", id: "poi:one", name: "One", x: 0, y: 0, z: 0, zone: 1, packaged: true };
    const saved: PointSelection = { ...poi, kind: "saved", id: "saved:one", packaged: false };
    expect(isDeletablePoint(poi)).toBe(false);
    expect(isDeletablePoint(saved)).toBe(true);
  });
});
