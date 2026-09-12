use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine as _;
use navmut_core::{
    append_observation, export_observations_one, load_locations, load_observations,
    load_observations_for_profile, load_poi_catalog, load_world_profiles, resolve_height_anchor,
    validate_poi_document, MapProfile as CoreMapProfile,
};
use navmut_platform::{
    enumerate_game_windows, BridgeClient, BridgeWindow, Capability, GameWindow, HelperSession,
    PlayerState, PlayerStateReader, SubmitStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State, Window};
use tauri_plugin_dialog::DialogExt;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

const PACKAGED_POINTS_OF_INTEREST: &str = include_str!("../../data/points_of_interest.json");

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MovementContext {
    pub pid: u32,
    pub profile_id: String,
    pub override_zone: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameState {
    pub connected: bool,
    pub pid: Option<u32>,
    pub character: String,
    pub zone: String,
    pub zone_id: Option<u16>,
    pub region_id: Option<u16>,
    pub position: Position,
    pub rotation: f64,
    pub map_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapBounds {
    pub min_x: f64,
    pub max_x: f64,
    pub min_z: f64,
    pub max_z: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapArtwork {
    pub mime: String,
    pub data: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PointOfInterest {
    pub id: String,
    pub name: String,
    pub category: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub zone: u16,
    pub trusted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeightAnchor {
    pub x: f64,
    pub z: f64,
    pub y: f64,
    pub source: String,
    pub trusted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapProfile {
    pub id: String,
    pub name: String,
    pub zone: Option<u16>,
    pub region: Option<u16>,
    pub bounds: MapBounds,
    pub pois: Vec<PointOfInterest>,
    pub anchors: Vec<HeightAnchor>,
    pub artwork: Option<MapArtwork>,
    pub can_move: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPoint {
    pub id: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub zone: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    pub id: String,
    pub captured_at: String,
    pub zone: u16,
    pub profile: String,
    pub position: Position,
    pub rotation: f64,
    pub name: String,
    pub r#type: String,
    pub notes: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationForm {
    pub name: String,
    pub r#type: String,
    pub notes: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiSettings {
    pub selected_pid: Option<u32>,
    pub profile_id: String,
    pub override_zone: bool,
    pub map_visible: bool,
    pub active_panel: String,
    pub step_size: u32,
    #[serde(default)]
    pub catalog_path: Option<String>,
    #[serde(default)]
    pub poi_catalog_path: Option<String>,
    #[serde(default)]
    pub helper_path: Option<String>,
    #[serde(default)]
    pub locations_path: Option<String>,
    #[serde(default)]
    pub observations_path: Option<String>,
    #[serde(default)]
    pub bridge_path: Option<String>,
    pub always_on_top: bool,
    pub opacity: u8,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct PersistedSettings {
    pub profile_id: String,
    pub map_visible: bool,
    pub active_panel: String,
    pub catalog_path: Option<String>,
    pub poi_catalog_path: Option<String>,
    pub helper_path: Option<String>,
    pub locations_path: Option<String>,
    pub observations_path: Option<String>,
    pub bridge_path: Option<String>,
    pub always_on_top: bool,
    pub opacity: u8,
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            profile_id: String::new(),
            map_visible: true,
            active_panel: "warp".to_string(),
            catalog_path: None,
            poi_catalog_path: None,
            helper_path: None,
            locations_path: None,
            observations_path: None,
            bridge_path: None,
            always_on_top: false,
            opacity: 100,
        }
    }
}

impl From<&UiSettings> for PersistedSettings {
    fn from(settings: &UiSettings) -> Self {
        Self {
            profile_id: settings.profile_id.clone(),
            map_visible: settings.map_visible,
            active_panel: settings.active_panel.clone(),
            catalog_path: settings.catalog_path.clone(),
            poi_catalog_path: settings.poi_catalog_path.clone(),
            helper_path: settings.helper_path.clone(),
            locations_path: settings.locations_path.clone(),
            observations_path: settings.observations_path.clone(),
            bridge_path: settings.bridge_path.clone(),
            always_on_top: settings.always_on_top,
            opacity: settings.opacity,
        }
    }
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            selected_pid: None,
            profile_id: String::new(),
            override_zone: false,
            map_visible: true,
            active_panel: "warp".to_string(),
            step_size: 5,
            catalog_path: None,
            poi_catalog_path: None,
            helper_path: None,
            locations_path: None,
            observations_path: None,
            bridge_path: None,
            always_on_top: false,
            opacity: 100,
        }
    }
}

struct Runtime {
    profiles: Vec<CoreMapProfile>,
    manifest: Option<PathBuf>,
    windows: Vec<GameWindow>,
    selected_window: Option<GameWindow>,
    reader: ReaderCache,
    input: Option<HelperSession>,
    bridge: Option<BridgeClient>,
    bridge_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
struct StartupArgs {
    catalog: Option<PathBuf>,
    map_image: Option<PathBuf>,
    map_bounds: Option<[f64; 4]>,
    zone: Option<u16>,
    name: Option<String>,
    observations: Option<PathBuf>,
    locations: Option<PathBuf>,
    pois: Option<PathBuf>,
    bridge: Option<PathBuf>,
    native_helper: Option<PathBuf>,
}

// PlayerStateReader owns a Windows process HANDLE and the platform crate keeps
// that handle deliberately non-Send. Runtime serializes every access through
// its mutex, so this narrow wrapper preserves the reader cache without exposing
// it to concurrent callers.
struct ReaderCache(Option<PlayerStateReader>);

unsafe impl Send for ReaderCache {}
unsafe impl Sync for ReaderCache {}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            profiles: Vec::new(),
            manifest: None,
            windows: Vec::new(),
            selected_window: None,
            reader: ReaderCache(None),
            input: None,
            bridge: None,
            bridge_path: None,
        }
    }
}

pub struct AppState {
    runtime: Mutex<Runtime>,
    settings: Mutex<UiSettings>,
    startup: StartupArgs,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new(StartupArgs::default())
    }
}

impl AppState {
    fn new(startup: StartupArgs) -> Self {
        Self {
            runtime: Mutex::new(Runtime::default()),
            settings: Mutex::new(UiSettings::default()),
            startup,
        }
    }
}

fn state_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

async fn run_blocking<T, F>(app: AppHandle, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(AppHandle) -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || operation(app))
        .await
        .map_err(state_error)?
}

fn effective_movement_zone(
    profile: &CoreMapProfile,
    live_zone: u16,
    override_zone: bool,
) -> Result<u16, String> {
    if override_zone {
        if let Some(zone) = profile.zone {
            return Ok(zone);
        }
    }
    if live_zone == 0 {
        Err("live player zone is unavailable".to_string())
    } else {
        Ok(live_zone)
    }
}

fn validate_warp_intent(
    intent: &str,
    target_zone: Option<u16>,
    effective_zone: u16,
) -> Result<(), String> {
    match intent {
        "map" => Ok(()),
        "point" if target_zone == Some(effective_zone) => Ok(()),
        "point" => Err("selected point does not belong to the effective movement zone".to_string()),
        _ => Err("warp intent must be map or point".to_string()),
    }
}

fn parse_startup_args() -> Result<StartupArgs, String> {
    let mut startup = StartupArgs::default();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut next_value = || {
            args.next()
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        match flag.as_str() {
            "--catalog" => startup.catalog = Some(PathBuf::from(next_value()?)),
            "--map-image" => startup.map_image = Some(PathBuf::from(next_value()?)),
            "--map-bounds" | "--bounds" => {
                let mut values = Vec::with_capacity(4);
                for _ in 0..4 {
                    values.push(next_value()?.parse::<f64>().map_err(|_| {
                        format!("{flag} must be four finite numbers: MIN_X MIN_Z MAX_X MAX_Z")
                    })?);
                }
                if !values.iter().all(|value| value.is_finite()) {
                    return Err(format!(
                        "{flag} must be four finite numbers: MIN_X MIN_Z MAX_X MAX_Z"
                    ));
                }
                startup.map_bounds = Some([values[0], values[1], values[2], values[3]]);
            }
            "--zone" => {
                startup.zone = Some(
                    next_value()?
                        .parse::<u16>()
                        .map_err(|_| "--zone must be a positive uint16".to_string())?,
                );
            }
            "--name" => startup.name = Some(next_value()?),
            "--observations" => startup.observations = Some(PathBuf::from(next_value()?)),
            "--locations" => startup.locations = Some(PathBuf::from(next_value()?)),
            "--pois" => startup.pois = Some(PathBuf::from(next_value()?)),
            "--bridge" | "--connection" | "--bridge-config" => {
                startup.bridge = Some(PathBuf::from(next_value()?))
            }
            "--native-helper" => startup.native_helper = Some(PathBuf::from(next_value()?)),
            _ => return Err(format!("unsupported startup argument: {flag}")),
        }
    }
    let direct_any = startup.map_image.is_some()
        || startup.map_bounds.is_some()
        || startup.zone.is_some()
        || startup.name.is_some();
    if startup.catalog.is_some() && direct_any {
        return Err("--catalog cannot be combined with direct map arguments".to_string());
    }
    if direct_any
        && (startup.map_image.is_none()
            || startup.map_bounds.is_none()
            || startup.zone.is_none()
            || startup.observations.is_none())
    {
        return Err(
            "direct map startup requires --map-image, --map-bounds, --zone, and --observations"
                .to_string(),
        );
    }
    Ok(startup)
}

fn startup_settings(settings: &mut UiSettings, startup: &StartupArgs) {
    if let Some(path) = &startup.catalog {
        settings.catalog_path = Some(path.display().to_string());
    }
    if let Some(path) = &startup.locations {
        settings.locations_path = Some(path.display().to_string());
    }
    if let Some(path) = &startup.pois {
        settings.poi_catalog_path = Some(path.display().to_string());
    }
    if let Some(path) = &startup.observations {
        settings.observations_path = Some(path.display().to_string());
    }
    if let Some(path) = &startup.native_helper {
        settings.helper_path = Some(path.display().to_string());
    }
    if let Some(path) = &startup.bridge {
        settings.bridge_path = Some(path.display().to_string());
    }
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(user_data_dir(app)?.join("navmut.json"))
}

fn user_data_dir(_app: &AppHandle) -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        let executable = std::env::current_exe().map_err(state_error)?;
        executable
            .parent()
            .map(|parent| parent.join("data"))
            .ok_or_else(|| "Navmut executable has no parent directory".to_string())
    }
    #[cfg(not(windows))]
    {
        _app.path().app_config_dir().map_err(state_error)
    }
}

fn read_settings(app: &AppHandle) -> Result<UiSettings, String> {
    let path = config_path(app)?;
    if !path.is_file() {
        return Ok(UiSettings::default());
    }
    let text = fs::read_to_string(path).map_err(state_error)?;
    let persisted: PersistedSettings = serde_json::from_str(&text).map_err(state_error)?;
    Ok(UiSettings {
        profile_id: persisted.profile_id,
        map_visible: persisted.map_visible,
        active_panel: persisted.active_panel,
        catalog_path: persisted.catalog_path,
        poi_catalog_path: persisted.poi_catalog_path,
        helper_path: persisted.helper_path,
        locations_path: persisted.locations_path,
        observations_path: persisted.observations_path,
        bridge_path: persisted.bridge_path,
        always_on_top: persisted.always_on_top,
        opacity: persisted.opacity.clamp(35, 100),
        ..UiSettings::default()
    })
}

fn write_settings(app: &AppHandle, settings: &UiSettings) -> Result<(), String> {
    let path = config_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(state_error)?;
    }
    let encoded =
        serde_json::to_string_pretty(&PersistedSettings::from(settings)).map_err(state_error)?;
    fs::write(path, format!("{encoded}\n")).map_err(state_error)
}

fn resolve_path(value: Option<&str>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|path| path.is_file())
}

fn candidates(app: &AppHandle, names: &[&str]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(resource) = app.path().resource_dir() {
        for name in names {
            paths.push(resource.join(name));
            paths.push(resource.join("data").join(name));
        }
    }
    if cfg!(debug_assertions) {
        if let Ok(current) = std::env::current_dir() {
            for name in names {
                paths.push(current.join("local").join(name));
                paths.push(current.join("data").join(name));
                paths.push(current.join(name));
            }
        }
    }
    paths
}

fn resolve_manifest(app: &AppHandle, settings: &UiSettings) -> Result<Option<PathBuf>, String> {
    if let Some(path) = resolve_path(settings.catalog_path.as_deref()) {
        return Ok(Some(path));
    }
    let bundled = candidates(app, &["maps.json", "map-catalog.json"])
        .into_iter()
        .find(|path| path.is_file());
    if settings.catalog_path.is_some() {
        return bundled
            .map(Some)
            .ok_or_else(|| "configured map catalog does not exist".to_string());
    }
    Ok(bundled)
}

fn resolve_poi_catalog(
    app: &AppHandle,
    settings: &UiSettings,
    manifest: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    if let Some(path) = resolve_path(settings.poi_catalog_path.as_deref()) {
        return Ok(Some(path));
    }
    if settings.poi_catalog_path.is_some() {
        return Err("configured POI catalog does not exist".to_string());
    }
    let mut paths = Vec::new();
    if let Some(manifest) = manifest {
        if let Some(parent) = manifest.parent() {
            paths.push(parent.join("points_of_interest.json"));
            paths.push(parent.join("pois.json"));
            paths.push(parent.join("poi-catalog.json"));
        }
    }
    paths.extend(candidates(
        app,
        &["points_of_interest.json", "pois.json", "poi-catalog.json"],
    ));
    Ok(paths.into_iter().find(|path| path.is_file()))
}

fn resolve_locations_path(app: &AppHandle, settings: &UiSettings) -> Result<PathBuf, String> {
    if let Some(path) = settings.locations_path.as_deref() {
        return Ok(PathBuf::from(path));
    }
    if cfg!(debug_assertions) {
        if let Ok(current) = std::env::current_dir() {
            let local = current.join("local").join("locations.json");
            if local.is_file() {
                return Ok(local);
            }
        }
    }
    Ok(user_data_dir(app)?.join("locations.json"))
}

fn resolve_observations_path(app: &AppHandle, settings: &UiSettings) -> Result<PathBuf, String> {
    if let Some(path) = settings.observations_path.as_deref() {
        return Ok(PathBuf::from(path));
    }
    Ok(user_data_dir(app)?.join("observations.jsonl"))
}

fn resolve_helper_path(app: &AppHandle, settings: &UiSettings) -> Result<PathBuf, String> {
    if let Some(path) = resolve_path(settings.helper_path.as_deref()) {
        return Ok(path);
    }
    if settings.helper_path.is_some() {
        return Err("configured silent helper does not exist".to_string());
    }
    let mut paths = Vec::new();
    if let Ok(resource) = app.path().resource_dir() {
        paths.push(resource.join("navmut-helper.exe"));
        paths.push(resource.join("navmut").join("navmut-helper.exe"));
    }
    if cfg!(debug_assertions) {
        if let Ok(current) = std::env::current_dir() {
            paths.push(current.join("out").join("native").join("navmut-helper.exe"));
            paths.push(current.join("native-build").join("navmut-helper.exe"));
        }
    }
    paths
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| "no bundled or configured silent helper was found".to_string())
}

fn prepare_bridge(runtime: &mut Runtime, settings: &UiSettings) -> Result<(), String> {
    if let Some(path) = settings.bridge_path.as_deref() {
        let path_value = PathBuf::from(path);
        let changed = runtime.bridge.is_none() || runtime.bridge_path.as_ref() != Some(&path_value);
        if changed {
            runtime.bridge = Some(BridgeClient::from_connection_file(path).map_err(state_error)?);
            runtime.bridge_path = Some(path_value);
            runtime.reader.0 = None;
            runtime.selected_window = None;
        }
    } else {
        runtime.bridge = None;
        runtime.bridge_path = None;
    }
    Ok(())
}

fn strip_png_gamma(bytes: &[u8]) -> Vec<u8> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < SIGNATURE.len() || &bytes[..SIGNATURE.len()] != SIGNATURE {
        return bytes.to_vec();
    }
    let mut output = Vec::with_capacity(bytes.len());
    output.extend_from_slice(SIGNATURE);
    let mut offset = SIGNATURE.len();
    while offset + 12 <= bytes.len() {
        let length = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        let Some(end) = offset.checked_add(12 + length) else {
            return bytes.to_vec();
        };
        if end > bytes.len() {
            return bytes.to_vec();
        }
        if &bytes[offset + 4..offset + 8] != b"gAMA" {
            output.extend_from_slice(&bytes[offset..end]);
        }
        offset = end;
    }
    if offset != bytes.len() {
        return bytes.to_vec();
    }
    output
}

fn map_artwork(path: &Path) -> Option<MapArtwork> {
    let mut bytes = fs::read(path).ok()?;
    let format = image::guess_format(&bytes).ok()?;
    if format == image::ImageFormat::Png {
        bytes = strip_png_gamma(&bytes);
    }
    let (width, height) = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format)
        .into_dimensions()
        .ok()?;
    Some(MapArtwork {
        mime: format.to_mime_type().to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        width,
        height,
    })
}

fn value_position(value: &Value) -> Option<Position> {
    let values = value.get("position")?.as_array()?;
    Some(Position {
        x: values.first()?.as_f64()?,
        y: values.get(1)?.as_f64()?,
        z: values.get(2)?.as_f64()?,
    })
}

fn poi_from_value(value: &Value) -> Option<PointOfInterest> {
    let position = value_position(value)?;
    let object = value.as_object()?;
    Some(PointOfInterest {
        id: object.get("poi_id")?.as_str()?.to_string(),
        name: object.get("name")?.as_str()?.to_string(),
        category: object.get("kind")?.as_str()?.to_string(),
        x: position.x,
        y: position.y,
        z: position.z,
        zone: object.get("zone")?.as_u64()? as u16,
        trusted: object
            .get("height_anchor")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn ui_profile(profile: &CoreMapProfile, pois: &[Value], include_artwork: bool) -> MapProfile {
    let [min_x, min_z, max_x, max_z] = profile.map_bounds;
    let pois = pois
        .iter()
        .filter(|poi| {
            profile
                .zone
                .is_none_or(|zone| poi.get("zone").and_then(Value::as_u64) == Some(u64::from(zone)))
        })
        .filter_map(poi_from_value)
        .collect::<Vec<_>>();
    let anchors = pois
        .iter()
        .filter(|poi| poi.trusted)
        .map(|poi| HeightAnchor {
            x: poi.x,
            z: poi.z,
            y: poi.y,
            source: "poi".to_string(),
            trusted: true,
        })
        .collect();
    MapProfile {
        id: profile.profile_id.clone(),
        name: profile.name.clone(),
        zone: profile.zone,
        region: profile.region,
        bounds: MapBounds {
            min_x,
            max_x,
            min_z,
            max_z,
        },
        pois,
        anchors,
        artwork: include_artwork
            .then(|| map_artwork(&profile.map_image))
            .flatten(),
        can_move: profile.can_move(),
    }
}

fn load_catalog(
    app: &AppHandle,
    settings: &UiSettings,
) -> Result<(Vec<CoreMapProfile>, Option<PathBuf>), String> {
    let Some(manifest) = resolve_manifest(app, settings)? else {
        return Ok((Vec::new(), None));
    };
    let profiles = load_world_profiles(&manifest).map_err(state_error)?;
    Ok((profiles, Some(manifest)))
}

fn direct_profile(
    app: &AppHandle,
    startup: &StartupArgs,
) -> Result<Option<CoreMapProfile>, String> {
    let (Some(zone), Some(map_image), Some(map_bounds)) =
        (startup.zone, &startup.map_image, startup.map_bounds)
    else {
        return Ok(None);
    };
    validate_startup_map(zone, map_bounds)?;
    let observations = startup
        .observations
        .clone()
        .unwrap_or(user_data_dir(app)?.join("observations.jsonl"));
    let name = startup.name.as_deref().unwrap_or("Navmut map");
    let profile_id = name
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    Ok(Some(CoreMapProfile {
        name: name.to_string(),
        zone: Some(zone),
        map_image: map_image.clone(),
        map_bounds,
        observations,
        coordinate_space: "world".to_string(),
        availability_note: "Startup map profile".to_string(),
        interaction_mode: None,
        profile_id,
        region: None,
    }))
}

fn validate_startup_map(zone: u16, bounds: [f64; 4]) -> Result<(), String> {
    if zone == 0 {
        return Err("startup zone must be a positive uint16".to_string());
    }
    if bounds[0] >= bounds[2] || bounds[1] >= bounds[3] {
        return Err("startup map bounds must have increasing X and Z limits".to_string());
    }
    Ok(())
}

fn load_pois(
    app: &AppHandle,
    settings: &UiSettings,
    manifest: Option<&Path>,
) -> Result<Vec<Value>, String> {
    match resolve_poi_catalog(app, settings, manifest)? {
        Some(path) => load_poi_catalog(path).map_err(state_error),
        None => packaged_pois(),
    }
}

fn packaged_pois() -> Result<Vec<Value>, String> {
    let document = serde_json::from_str(PACKAGED_POINTS_OF_INTEREST).map_err(state_error)?;
    validate_poi_document(&document).map_err(state_error)
}

fn select_profile<'a>(
    runtime: &'a Runtime,
    profile_id: &str,
) -> Result<&'a CoreMapProfile, String> {
    runtime
        .profiles
        .iter()
        .find(|profile| profile.profile_id == profile_id)
        .ok_or_else(|| "selected map profile is unavailable".to_string())
}

fn prepare_profiles(app: &AppHandle, state: &State<'_, AppState>) -> Result<(), String> {
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let mut runtime = state.runtime.lock().map_err(state_error)?;
    if runtime.profiles.is_empty() {
        let (profiles, manifest) = if let Some(profile) = direct_profile(app, &state.startup)? {
            (vec![profile], None)
        } else {
            load_catalog(app, &settings)?
        };
        runtime.profiles = profiles;
        runtime.manifest = manifest;
    }
    Ok(())
}

fn disconnected(pid: Option<u32>) -> GameState {
    GameState {
        connected: false,
        pid,
        character: String::new(),
        zone: String::new(),
        zone_id: None,
        region_id: None,
        position: Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        rotation: 0.0,
        map_id: String::new(),
    }
}

fn game_state(player: &PlayerState, window: &GameWindow, profiles: &[CoreMapProfile]) -> GameState {
    let profile = profiles
        .iter()
        .find(|profile| profile.zone == Some(player.zone));
    GameState {
        connected: true,
        pid: Some(player.process_id),
        character: window.title.clone(),
        zone: format!("Zone {}", player.zone),
        zone_id: Some(player.zone),
        region_id: Some(player.region),
        position: Position {
            x: player.x,
            y: player.y,
            z: player.z,
        },
        rotation: player.rotation,
        map_id: profile.map_or_else(String::new, |value| value.profile_id.clone()),
    }
}

fn valid_zone_profile(profile: &MapProfile, game: &GameState) -> bool {
    let Some(zone) = game.zone_id else {
        return false;
    };
    let Some(region) = game.region_id else {
        return false;
    };
    profile.zone == Some(zone)
        && profile.region == Some(region)
        && profile.bounds.max_x > profile.bounds.min_x
        && profile.bounds.max_z > profile.bounds.min_z
        && game.position.x >= profile.bounds.min_x
        && game.position.x <= profile.bounds.max_x
        && game.position.z >= profile.bounds.min_z
        && game.position.z <= profile.bounds.max_z
}

fn select_refresh_profile(profiles: &[MapProfile], game: &GameState) -> Option<String> {
    profiles
        .iter()
        .filter(|profile| valid_zone_profile(profile, game))
        .fold(None, |best: Option<(&MapProfile, f64)>, profile| {
            let area = (profile.bounds.max_x - profile.bounds.min_x)
                * (profile.bounds.max_z - profile.bounds.min_z);
            match best {
                Some((_, best_area)) if area < best_area => best,
                _ => Some((profile, area)),
            }
        })
        .map(|(profile, _)| profile.id.clone())
}

fn snapshot(
    runtime: &mut Runtime,
    pid: Option<u32>,
) -> Result<Option<(PlayerState, GameWindow)>, String> {
    if let Some(client) = runtime.bridge.as_ref() {
        let bridge_windows = client.windows().map_err(state_error)?;
        runtime.windows = bridge_windows
            .iter()
            .map(|window| GameWindow::new(window.handle, window.process_id, window.title.clone()))
            .collect();
        let Some(pid) = pid else {
            runtime.selected_window = None;
            return Ok(None);
        };
        let Some(window) = runtime
            .windows
            .iter()
            .find(|window| window.process_id == pid)
            .cloned()
        else {
            runtime.selected_window = None;
            return Ok(None);
        };
        let bridge_window = BridgeWindow {
            handle: window.handle,
            process_id: window.process_id,
            title: window.title.clone(),
        };
        let player = client.player_state(bridge_window).map_err(state_error)?;
        return Ok(Some((
            PlayerState {
                process_id: player.process_id,
                actor_id: player.actor_id,
                region: player.region,
                zone: player.zone,
                x: player.x,
                y: player.y,
                z: player.z,
                rotation: player.rotation,
            },
            window,
        )));
    }
    let windows = enumerate_game_windows().map_err(state_error)?;
    runtime.windows = windows;
    let Some(pid) = pid else {
        runtime.selected_window = None;
        runtime.reader.0 = None;
        return Ok(None);
    };
    let Some(window) = runtime
        .windows
        .iter()
        .find(|window| window.process_id == pid)
        .cloned()
    else {
        runtime.selected_window = None;
        runtime.reader.0 = None;
        return Ok(None);
    };
    let changed = runtime.selected_window.as_ref().is_none_or(|selected| {
        selected.handle != window.handle || selected.process_id != window.process_id
    });
    if changed || runtime.reader.0.is_none() {
        runtime.reader.0 = Some(PlayerStateReader::open(&window).map_err(state_error)?);
        runtime.selected_window = Some(window.clone());
    }
    let Some(reader) = runtime.reader.0.as_mut() else {
        return Err("player state reader is unavailable".to_string());
    };
    match reader.snapshot() {
        Ok(player) => Ok(Some((player, window))),
        Err(error) => {
            runtime.reader.0 = None;
            Err(state_error(error))
        }
    }
}

fn submit_position(
    runtime: &mut Runtime,
    app: &AppHandle,
    settings: &UiSettings,
    window: &GameWindow,
    position: &Position,
    zone: u16,
) -> Result<(), String> {
    if let Some(client) = runtime.bridge.as_ref() {
        if !client
            .connection()
            .capabilities
            .supports(Capability::SilentPosition)
        {
            return Err(
                "configured bridge does not support position control for this version of Navmut"
                    .to_string(),
            );
        }
        let bridge_window = BridgeWindow {
            handle: window.handle,
            process_id: window.process_id,
            title: window.title.clone(),
        };
        return client
            .silent_position(bridge_window, position.x, position.y, position.z, zone)
            .map_err(state_error);
    }
    let helper_path = resolve_helper_path(app, settings)?;
    let helper = runtime
        .input
        .get_or_insert_with(|| HelperSession::new(helper_path));
    let response = helper
        .submit(window, position.x, position.y, position.z, zone)
        .map_err(state_error)?;
    if response.status != SubmitStatus::Ok {
        return Err(format!(
            "movement outcome was uncertain: {}",
            response.reason
        ));
    }
    Ok(())
}

type AnchorRecords = (Vec<Value>, Vec<Value>, Vec<Value>);

fn core_records(
    app: &AppHandle,
    settings: &UiSettings,
    profile: &CoreMapProfile,
    manifest: Option<&Path>,
    zone: u16,
) -> Result<AnchorRecords, String> {
    let observations_path = resolve_observations_path(app, settings)?;
    let observations = load_observations(observations_path, zone, Some(&profile.profile_id))
        .map_err(state_error)?;
    let locations_path = resolve_locations_path(app, settings)?;
    let locations = load_locations(locations_path).map_err(state_error)?;
    let locations = locations
        .into_iter()
        .filter(|location| location.zone == zone)
        .filter_map(|location| serde_json::to_value(location).ok())
        .collect();
    let pois = match resolve_poi_catalog(app, settings, manifest)? {
        Some(path) => load_poi_catalog(path).map_err(state_error)?,
        None => packaged_pois()?,
    };
    Ok((observations, locations, pois))
}

fn resolve_destination_y(
    app: &AppHandle,
    settings: &UiSettings,
    profile: &CoreMapProfile,
    manifest: Option<&Path>,
    current: &Position,
    target: &Position,
    zone: u16,
) -> Result<f64, String> {
    let (observations, locations, mut pois) = core_records(app, settings, profile, manifest, zone)?;
    pois.retain(|poi| poi.get("zone").and_then(Value::as_u64) == Some(u64::from(zone)));
    let zone_value = json!(zone);
    resolve_height_anchor(
        &json!({"x": target.x, "y": current.y, "z": target.z}),
        Some(&observations),
        Some(&locations),
        Some(&pois),
        Some(&zone_value),
        Some(&profile.profile_id),
        None,
        navmut_core::DEFAULT_ANCHOR_RADIUS,
    )
    .map_err(state_error)
}

fn archive_entry(value: &Value, profile: &CoreMapProfile) -> Option<JournalEntry> {
    let object = value.as_object()?;
    let position = value_position(value)?;
    Some(JournalEntry {
        id: object.get("observation_id")?.as_str()?.to_string(),
        captured_at: object.get("created_at")?.as_str()?.to_string(),
        zone: object.get("zone")?.as_u64()? as u16,
        profile: profile.name.clone(),
        position,
        rotation: object
            .get("rotation")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        name: object
            .get("subject_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        r#type: match object
            .get("subject_type")
            .and_then(Value::as_str)
            .unwrap_or("misc")
        {
            "monster" => "mob".to_string(),
            value => value.to_string(),
        },
        notes: object
            .get("notes")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

fn list_processes_blocking(state: State<'_, AppState>) -> Result<Vec<ProcessInfo>, String> {
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let mut runtime = state.runtime.lock().map_err(state_error)?;
    prepare_bridge(&mut runtime, &settings)?;
    if let Some(client) = runtime.bridge.as_ref() {
        let windows = client.windows().map_err(state_error)?;
        return Ok(windows
            .into_iter()
            .map(|window| ProcessInfo {
                pid: window.process_id,
            })
            .collect());
    }
    if !navmut_platform::capabilities().windows {
        return Ok(Vec::new());
    }
    runtime.windows = enumerate_game_windows().map_err(state_error)?;
    Ok(runtime
        .windows
        .iter()
        .map(|window| ProcessInfo {
            pid: window.process_id,
        })
        .collect())
}

fn get_game_state_blocking(
    pid: Option<u32>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<GameState, String> {
    prepare_profiles(&app, &state)?;
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let mut runtime = state.runtime.lock().map_err(state_error)?;
    prepare_bridge(&mut runtime, &settings)?;
    if runtime.bridge.is_none() && !navmut_platform::capabilities().player_state {
        return Ok(disconnected(pid));
    }
    let Some((player, window)) = snapshot(&mut runtime, pid)? else {
        return Ok(disconnected(pid));
    };
    Ok(game_state(&player, &window, &runtime.profiles))
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UiCapabilities {
    windows: bool,
    player_state: bool,
    silent_position: bool,
    bridge_protocol: u16,
}

fn platform_capabilities_blocking(state: State<'_, AppState>) -> UiCapabilities {
    let local = navmut_platform::capabilities();
    let capabilities = state
        .settings
        .lock()
        .ok()
        .and_then(|settings| settings.bridge_path.clone())
        .and_then(|path| BridgeClient::from_connection_file(path).ok())
        .map_or(local, |client| client.connection().capabilities);
    UiCapabilities {
        windows: capabilities.windows,
        player_state: capabilities.player_state,
        silent_position: capabilities.silent_position,
        bridge_protocol: capabilities.bridge_protocol,
    }
}

fn list_profiles_blocking(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<MapProfile>, String> {
    prepare_profiles(&app, &state)?;
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let runtime = state.runtime.lock().map_err(state_error)?;
    let pois = load_pois(&app, &settings, runtime.manifest.as_deref())?;
    Ok(runtime
        .profiles
        .iter()
        .map(|profile| ui_profile(profile, &pois, false))
        .collect())
}

fn load_profile_blocking(
    profile_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<MapProfile, String> {
    prepare_profiles(&app, &state)?;
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let runtime = state.runtime.lock().map_err(state_error)?;
    let pois = load_pois(&app, &settings, runtime.manifest.as_deref())?;
    Ok(ui_profile(
        select_profile(&runtime, &profile_id)?,
        &pois,
        true,
    ))
}

fn select_catalog_blocking(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<MapProfile>, String> {
    let Some(chosen) = app
        .dialog()
        .file()
        .add_filter("Map catalog", &["json"])
        .blocking_pick_file()
    else {
        return Ok(Vec::new());
    };
    let manifest = chosen.into_path().map_err(state_error)?;
    let profiles = load_world_profiles(&manifest).map_err(state_error)?;
    if profiles.is_empty() {
        return Err("the selected catalog has no world coordinate maps".to_string());
    }
    let mut settings = state.settings.lock().map_err(state_error)?.clone();
    settings.catalog_path = Some(manifest.display().to_string());
    let pois = load_pois(&app, &settings, Some(&manifest))?;
    let result = profiles
        .iter()
        .map(|profile| ui_profile(profile, &pois, false))
        .collect::<Vec<_>>();
    write_settings(&app, &settings)?;
    *state.settings.lock().map_err(state_error)? = settings;
    let mut runtime = state.runtime.lock().map_err(state_error)?;
    runtime.profiles = profiles;
    runtime.manifest = Some(manifest);
    Ok(result)
}

fn move_player_blocking(
    delta: Position,
    context: MovementContext,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<GameState, String> {
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let mut runtime = state.runtime.lock().map_err(state_error)?;
    prepare_bridge(&mut runtime, &settings)?;
    let Some((player, window)) = snapshot(&mut runtime, Some(context.pid))? else {
        return Err("selected game window is unavailable".to_string());
    };
    let selected = select_profile(&runtime, &context.profile_id)?;
    if !selected.can_move() {
        return Err("selected profile is not a world coordinate map".to_string());
    }
    let zone = effective_movement_zone(selected, player.zone, context.override_zone)?;
    let target = Position {
        x: player.x + delta.x,
        y: player.y + delta.y,
        z: player.z + delta.z,
    };
    submit_position(&mut runtime, &app, &settings, &window, &target, zone)?;
    Ok(game_state(
        &PlayerState {
            x: target.x,
            y: target.y,
            z: target.z,
            ..player
        },
        &window,
        &runtime.profiles,
    ))
}

fn warp_to_blocking(
    position: Position,
    context: MovementContext,
    intent: String,
    target_zone: Option<u16>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<GameState, String> {
    prepare_profiles(&app, &state)?;
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let mut runtime = state.runtime.lock().map_err(state_error)?;
    prepare_bridge(&mut runtime, &settings)?;
    let profile = select_profile(&runtime, &context.profile_id)?.clone();
    if !profile.can_move() {
        return Err("selected profile is not a world coordinate map".to_string());
    }
    let Some((player, window)) = snapshot(&mut runtime, Some(context.pid))? else {
        return Err("selected game window is unavailable".to_string());
    };
    let zone = effective_movement_zone(&profile, player.zone, context.override_zone)?;
    validate_warp_intent(&intent, target_zone, zone)?;
    let target = if intent == "map" {
        let y = resolve_destination_y(
            &app,
            &settings,
            &profile,
            runtime.manifest.as_deref(),
            &Position {
                x: player.x,
                y: player.y,
                z: player.z,
            },
            &position,
            zone,
        )?;
        Position {
            x: position.x,
            y,
            z: position.z,
        }
    } else {
        position
    };
    submit_position(&mut runtime, &app, &settings, &window, &target, zone)?;
    Ok(game_state(
        &PlayerState {
            x: target.x,
            y: target.y,
            z: target.z,
            ..player
        },
        &window,
        &runtime.profiles,
    ))
}

fn list_saved_points_blocking(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<SavedPoint>, String> {
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let path = resolve_locations_path(&app, &settings)?;
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let locations = load_locations(path).map_err(state_error)?;
    Ok(locations
        .into_iter()
        .enumerate()
        .map(|(index, location)| SavedPoint {
            id: format!("location-{index}"),
            name: location.name,
            x: location.position[0],
            y: location.position[1],
            z: location.position[2],
            zone: location.zone,
        })
        .collect())
}

fn save_point_blocking(
    name: String,
    pid: u32,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SavedPoint, String> {
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let mut runtime = state.runtime.lock().map_err(state_error)?;
    prepare_bridge(&mut runtime, &settings)?;
    let Some((player, _window)) = snapshot(&mut runtime, Some(pid))? else {
        return Err("selected game window is unavailable".to_string());
    };
    if name.trim().is_empty() {
        return Err("point name must not be empty".to_string());
    }
    let path = resolve_locations_path(&app, &settings)?;
    navmut_core::save_location(&path, &name, player.zone, [player.x, player.y, player.z])
        .map_err(state_error)?;
    Ok(SavedPoint {
        id: format!("location-{}", name),
        name,
        x: player.x,
        y: player.y,
        z: player.z,
        zone: player.zone,
    })
}

fn delete_saved_point_blocking(
    point: SavedPoint,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    reject_packaged_point(&point)?;
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let path = resolve_locations_path(&app, &settings)?;
    navmut_core::delete_location(path, &point.name, point.zone).map_err(state_error)
}

fn reject_packaged_point(point: &SavedPoint) -> Result<(), String> {
    if point.id.starts_with("poi:") {
        Err("packaged POIs cannot be deleted".to_string())
    } else {
        Ok(())
    }
}

fn set_position_blocking(
    position: Position,
    context: MovementContext,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<GameState, String> {
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let mut runtime = state.runtime.lock().map_err(state_error)?;
    prepare_bridge(&mut runtime, &settings)?;
    let Some((player, window)) = snapshot(&mut runtime, Some(context.pid))? else {
        return Err("selected game window is unavailable".to_string());
    };
    let profile = select_profile(&runtime, &context.profile_id)?.clone();
    if !profile.can_move() {
        return Err("selected profile is not a world coordinate map".to_string());
    }
    let zone = effective_movement_zone(&profile, player.zone, context.override_zone)?;
    submit_position(&mut runtime, &app, &settings, &window, &position, zone)?;
    Ok(game_state(
        &PlayerState {
            x: position.x,
            y: position.y,
            z: position.z,
            ..player
        },
        &window,
        &runtime.profiles,
    ))
}

fn capture_observation_blocking(
    profile_id: String,
    form: ObservationForm,
    pid: u32,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<JournalEntry, String> {
    prepare_profiles(&app, &state)?;
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let mut runtime = state.runtime.lock().map_err(state_error)?;
    prepare_bridge(&mut runtime, &settings)?;
    let profile = select_profile(&runtime, &profile_id)?.clone();
    let Some((player, _window)) = snapshot(&mut runtime, Some(pid))? else {
        return Err("selected game window is unavailable".to_string());
    };
    if form.name.trim().is_empty() {
        return Err("observation name must not be empty".to_string());
    }
    let subject_type = match form.r#type.as_str() {
        "npc" => "npc",
        "mob" => "monster",
        "misc" => "misc",
        _ => return Err("observation type must be npc, mob, or misc".to_string()),
    };
    let created_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(state_error)?;
    let [min_x, min_z, max_x, max_z] = profile.map_bounds;
    let record = json!({
        "schema_version": 2,
        "observation_id": Uuid::new_v4().to_string(),
        "subject_name": form.name.trim(),
        "subject_type": subject_type,
        "zone": player.zone,
        "position": [player.x, player.y, player.z],
        "rotation": player.rotation,
        "notes": form.notes,
        "profile_id": profile.profile_id,
        "created_at": created_at,
        "map_bounds": [min_x, min_z, max_x, max_z]
    });
    let observations = resolve_observations_path(&app, &settings)?;
    append_observation(&observations, &record).map_err(state_error)?;
    archive_entry(&record, &profile).ok_or_else(|| "captured observation was malformed".to_string())
}

fn list_journal_blocking(
    profile_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<JournalEntry>, String> {
    prepare_profiles(&app, &state)?;
    let runtime = state.runtime.lock().map_err(state_error)?;
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let profile = select_profile(&runtime, &profile_id)?;
    let observations = resolve_observations_path(&app, &settings)?;
    let records =
        match profile.zone {
            Some(zone) => load_observations(observations, zone, Some(&profile.profile_id))
                .map_err(state_error)?,
            None => load_observations_for_profile(observations, &profile.profile_id)
                .map_err(state_error)?,
        };
    Ok(records
        .iter()
        .filter_map(|record| archive_entry(record, profile))
        .collect())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedArchive {
    pub filename: String,
    pub mime: String,
    pub data: Option<String>,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshZoneResult {
    pub game: GameState,
    pub profile_id: Option<String>,
    pub profiles: Vec<MapProfile>,
}

fn export_journal_blocking(
    profile_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ExportedArchive, String> {
    prepare_profiles(&app, &state)?;
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let observations = {
        let runtime = state.runtime.lock().map_err(state_error)?;
        select_profile(&runtime, &profile_id)?;
        resolve_observations_path(&app, &settings)?
    };
    let chosen = app
        .dialog()
        .file()
        .set_file_name("navmut-observations.zip")
        .add_filter("ZIP archive", &["zip"])
        .blocking_save_file()
        .ok_or_else(|| "export cancelled".to_string())?;
    let output = chosen.into_path().map_err(state_error)?;
    let output = export_observations_one(&observations, &output).map_err(state_error)?;
    Ok(ExportedArchive {
        filename: "navmut-observations.zip".to_string(),
        mime: "application/zip".to_string(),
        data: None,
        path: Some(output.display().to_string()),
    })
}

fn refresh_zone_blocking(
    pid: Option<u32>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<RefreshZoneResult, String> {
    prepare_profiles(&app, &state)?;
    let settings = state.settings.lock().map_err(state_error)?.clone();
    let mut runtime = state.runtime.lock().map_err(state_error)?;
    prepare_bridge(&mut runtime, &settings)?;
    let game = snapshot(&mut runtime, pid)?.map_or_else(
        || disconnected(pid),
        |(player, window)| game_state(&player, &window, &runtime.profiles),
    );
    let pois = load_pois(&app, &settings, runtime.manifest.as_deref())?;
    let profiles = runtime
        .profiles
        .iter()
        .map(|profile| ui_profile(profile, &pois, false))
        .collect::<Vec<_>>();
    let selected = select_refresh_profile(&profiles, &game);
    Ok(RefreshZoneResult {
        game,
        profile_id: selected,
        profiles,
    })
}

fn load_settings_blocking(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UiSettings, String> {
    let mut settings = read_settings(&app)?;
    startup_settings(&mut settings, &state.startup);
    *state.settings.lock().map_err(state_error)? = settings.clone();
    Ok(settings)
}

fn save_settings_blocking(
    settings: PersistedSettings,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut current = state.settings.lock().map_err(state_error)?;
    current.profile_id = settings.profile_id;
    current.map_visible = settings.map_visible;
    current.active_panel = settings.active_panel;
    current.catalog_path = settings.catalog_path;
    current.poi_catalog_path = settings.poi_catalog_path;
    current.helper_path = settings.helper_path;
    current.locations_path = settings.locations_path;
    current.observations_path = settings.observations_path;
    current.bridge_path = settings.bridge_path;
    current.always_on_top = settings.always_on_top;
    current.opacity = settings.opacity.clamp(35, 100);
    write_settings(&app, &current)
}

#[tauri::command]
async fn list_processes(app: AppHandle) -> Result<Vec<ProcessInfo>, String> {
    run_blocking(app, |app| {
        let state = app.state::<AppState>();
        list_processes_blocking(state)
    })
    .await
}

#[tauri::command]
async fn get_game_state(pid: Option<u32>, app: AppHandle) -> Result<GameState, String> {
    run_blocking(app, move |app| {
        let state = app.state::<AppState>();
        get_game_state_blocking(pid, app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn platform_capabilities(app: AppHandle) -> Result<UiCapabilities, String> {
    run_blocking(app, |app| {
        let state = app.state::<AppState>();
        Ok(platform_capabilities_blocking(state))
    })
    .await
}

#[tauri::command]
async fn list_profiles(app: AppHandle) -> Result<Vec<MapProfile>, String> {
    run_blocking(app, |app| {
        let state = app.state::<AppState>();
        list_profiles_blocking(app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn load_profile(profile_id: String, app: AppHandle) -> Result<MapProfile, String> {
    run_blocking(app, move |app| {
        let state = app.state::<AppState>();
        load_profile_blocking(profile_id, app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn select_catalog(app: AppHandle) -> Result<Vec<MapProfile>, String> {
    run_blocking(app, |app| {
        let state = app.state::<AppState>();
        select_catalog_blocking(app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn move_player(
    delta: Position,
    context: MovementContext,
    app: AppHandle,
) -> Result<GameState, String> {
    run_blocking(app, move |app| {
        let state = app.state::<AppState>();
        move_player_blocking(delta, context, app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn warp_to(
    position: Position,
    context: MovementContext,
    intent: String,
    target_zone: Option<u16>,
    app: AppHandle,
) -> Result<GameState, String> {
    run_blocking(app, move |app| {
        let state = app.state::<AppState>();
        warp_to_blocking(position, context, intent, target_zone, app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn list_saved_points(app: AppHandle) -> Result<Vec<SavedPoint>, String> {
    run_blocking(app, |app| {
        let state = app.state::<AppState>();
        list_saved_points_blocking(app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn save_point(name: String, pid: u32, app: AppHandle) -> Result<SavedPoint, String> {
    run_blocking(app, move |app| {
        let state = app.state::<AppState>();
        save_point_blocking(name, pid, app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn delete_saved_point(point: SavedPoint, app: AppHandle) -> Result<(), String> {
    run_blocking(app, move |app| {
        let state = app.state::<AppState>();
        delete_saved_point_blocking(point, app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn set_position(
    position: Position,
    context: MovementContext,
    app: AppHandle,
) -> Result<GameState, String> {
    run_blocking(app, move |app| {
        let state = app.state::<AppState>();
        set_position_blocking(position, context, app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn capture_observation(
    profile_id: String,
    form: ObservationForm,
    pid: u32,
    app: AppHandle,
) -> Result<JournalEntry, String> {
    run_blocking(app, move |app| {
        let state = app.state::<AppState>();
        capture_observation_blocking(profile_id, form, pid, app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn list_journal(profile_id: String, app: AppHandle) -> Result<Vec<JournalEntry>, String> {
    run_blocking(app, move |app| {
        let state = app.state::<AppState>();
        list_journal_blocking(profile_id, app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn export_journal(profile_id: String, app: AppHandle) -> Result<ExportedArchive, String> {
    run_blocking(app, move |app| {
        let state = app.state::<AppState>();
        export_journal_blocking(profile_id, app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn refresh_zone(pid: Option<u32>, app: AppHandle) -> Result<RefreshZoneResult, String> {
    run_blocking(app, move |app| {
        let state = app.state::<AppState>();
        refresh_zone_blocking(pid, app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn load_settings(app: AppHandle) -> Result<UiSettings, String> {
    run_blocking(app, |app| {
        let state = app.state::<AppState>();
        load_settings_blocking(app.clone(), state)
    })
    .await
}

#[tauri::command]
async fn save_settings(settings: PersistedSettings, app: AppHandle) -> Result<(), String> {
    run_blocking(app, move |app| {
        let state = app.state::<AppState>();
        save_settings_blocking(settings, app.clone(), state)
    })
    .await
}

#[tauri::command]
fn minimize_window(window: Window) -> Result<(), String> {
    window.minimize().map_err(state_error)
}

#[tauri::command]
fn close_window(window: Window) -> Result<(), String> {
    window.close().map_err(state_error)
}

#[tauri::command]
fn resize_window(width: f64, window: Window) -> Result<(), String> {
    window
        .set_size(tauri::Size::Logical(tauri::LogicalSize::new(width, 710.0)))
        .map_err(state_error)
}

#[tauri::command]
fn set_window_opacity(opacity: u8, window: Window) -> Result<(), String> {
    #[cfg(windows)]
    {
        let value = opacity.clamp(35, 100);
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongW, SetLayeredWindowAttributes, SetWindowLongW, GWL_EXSTYLE, LWA_ALPHA,
            WS_EX_LAYERED,
        };
        let raw_hwnd = window.hwnd().map_err(state_error)?;
        let hwnd = windows::Win32::Foundation::HWND(raw_hwnd.0 as _);
        unsafe {
            let style = GetWindowLongW(hwnd, GWL_EXSTYLE);
            if style & WS_EX_LAYERED.0 as i32 == 0 {
                SetWindowLongW(hwnd, GWL_EXSTYLE, style | WS_EX_LAYERED.0 as i32);
            }
            SetLayeredWindowAttributes(
                hwnd,
                windows::Win32::Foundation::COLORREF(0),
                ((u16::from(value) * 255) / 100) as u8,
                LWA_ALPHA,
            )
            .map_err(state_error)?;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (opacity, window);
        Ok(())
    }
}

#[tauri::command]
fn set_always_on_top(always_on_top: bool, window: Window) -> Result<(), String> {
    window.set_always_on_top(always_on_top).map_err(state_error)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let startup = parse_startup_args()
        .unwrap_or_else(|error| panic!("invalid Navmut startup arguments: {error}"));
    tauri::Builder::default()
        .manage(AppState::new(startup))
        .plugin(tauri_plugin_dialog::init())
        .setup(|_app| {
            #[cfg(windows)]
            {
                use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
                use windows_core::Interface;

                let main_webview = _app
                    .get_webview_window("main")
                    .ok_or_else(|| "main webview is unavailable".to_string())?;
                main_webview.with_webview(|webview| unsafe {
                    let configure = || -> Result<(), windows_core::Error> {
                        let core = webview.controller().CoreWebView2()?;
                        let settings = core.Settings()?;
                        settings.SetAreDefaultContextMenusEnabled(false)?;
                        settings.SetAreDevToolsEnabled(false)?;
                        settings.SetIsStatusBarEnabled(false)?;
                        if let Ok(settings3) = settings.cast::<ICoreWebView2Settings3>() {
                            settings3.SetAreBrowserAcceleratorKeysEnabled(false)?;
                        }
                        Ok(())
                    };
                    if let Err(error) = configure() {
                        eprintln!("could not harden the WebView2 surface: {error}");
                    }
                })?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_processes,
            platform_capabilities,
            get_game_state,
            list_profiles,
            load_profile,
            select_catalog,
            move_player,
            warp_to,
            list_saved_points,
            save_point,
            delete_saved_point,
            set_position,
            capture_observation,
            list_journal,
            export_journal,
            refresh_zone,
            load_settings,
            save_settings,
            minimize_window,
            close_window,
            resize_window,
            set_window_opacity,
            set_always_on_top,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Navmut");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_png_copy_omits_gamma_metadata() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&4u32.to_be_bytes());
        png.extend_from_slice(b"gAMA");
        png.extend_from_slice(&100_000u32.to_be_bytes());
        png.extend_from_slice(&[0, 0, 0, 0]);
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IEND");
        png.extend_from_slice(&[0, 0, 0, 0]);

        let stripped = strip_png_gamma(&png);
        assert!(!stripped.windows(4).any(|value| value == b"gAMA"));
        assert!(stripped.windows(4).any(|value| value == b"IEND"));
    }

    #[test]
    fn map_artwork_preserves_dimensions_for_supported_formats() {
        for (format, mime) in [
            (image::ImageFormat::Png, "image/png"),
            (image::ImageFormat::Jpeg, "image/jpeg"),
            (image::ImageFormat::Gif, "image/gif"),
            (image::ImageFormat::WebP, "image/webp"),
        ] {
            let path = std::env::temp_dir().join(format!("navmut-artwork-{}", Uuid::new_v4()));
            image::DynamicImage::new_rgb8(7, 3)
                .save_with_format(&path, format)
                .unwrap();
            let artwork = map_artwork(&path).expect("supported artwork");
            fs::remove_file(&path).unwrap();
            assert_eq!((artwork.width, artwork.height), (7, 3), "{format:?}");
            assert_eq!(artwork.mime, mime);
        }
    }

    #[test]
    fn packaged_points_are_not_deletable() {
        let point = SavedPoint {
            id: "poi:camp".to_string(),
            name: "Camp".to_string(),
            x: 0.0,
            y: 0.0,
            z: 0.0,
            zone: 1,
        };
        assert!(reject_packaged_point(&point).is_err());
    }

    #[test]
    fn embedded_pois_include_lower_la_noscea_aetherytes() {
        let points = packaged_pois().expect("embedded POI catalog");
        let names = points
            .iter()
            .filter(|point| point.get("zone").and_then(Value::as_u64) == Some(128))
            .filter_map(|point| point.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "Camp Bearded Rock",
                "Cedarwood",
                "Widow Cliffs",
                "Moraby Bay"
            ]
        );
    }

    #[test]
    fn movement_zone_prefers_override_profile_and_live_fallback() {
        let profile = CoreMapProfile {
            name: "Map".to_string(),
            zone: Some(140),
            map_image: PathBuf::from("map.png"),
            map_bounds: [0.0, 0.0, 10.0, 10.0],
            observations: PathBuf::from("obs.jsonl"),
            coordinate_space: "world".to_string(),
            availability_note: String::new(),
            interaction_mode: None,
            profile_id: "map".to_string(),
            region: Some(104),
        };
        assert_eq!(effective_movement_zone(&profile, 128, true).unwrap(), 140);
        assert_eq!(effective_movement_zone(&profile, 128, false).unwrap(), 128);
    }

    #[test]
    fn point_warp_requires_the_effective_zone() {
        assert!(validate_warp_intent("map", None, 128).is_ok());
        assert!(validate_warp_intent("point", Some(128), 128).is_ok());
        assert!(validate_warp_intent("point", Some(140), 128).is_err());
        assert!(validate_warp_intent("other", None, 128).is_err());
    }

    #[test]
    fn direct_startup_map_requires_positive_zone_and_increasing_bounds() {
        assert!(validate_startup_map(0, [0.0, 0.0, 10.0, 10.0]).is_err());
        assert!(validate_startup_map(128, [10.0, 0.0, 0.0, 10.0]).is_err());
        assert!(validate_startup_map(128, [0.0, 0.0, 10.0, 10.0]).is_ok());
    }

    #[test]
    fn refresh_requires_zone_region_and_bounds_match() {
        let profile = MapProfile {
            id: "map".to_string(),
            name: "Map".to_string(),
            zone: Some(128),
            region: Some(104),
            bounds: MapBounds {
                min_x: 0.0,
                max_x: 10.0,
                min_z: 0.0,
                max_z: 10.0,
            },
            pois: Vec::new(),
            anchors: Vec::new(),
            artwork: None,
            can_move: true,
        };
        let game = GameState {
            connected: true,
            pid: Some(1),
            character: String::new(),
            zone: String::new(),
            zone_id: Some(128),
            region_id: Some(104),
            position: Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            rotation: 0.0,
            map_id: String::new(),
        };
        assert_eq!(
            select_refresh_profile(std::slice::from_ref(&profile), &game).as_deref(),
            Some("map")
        );
        let outside = GameState {
            position: Position {
                x: 11.0,
                y: 0.0,
                z: 5.0,
            },
            ..game
        };
        assert_eq!(
            select_refresh_profile(std::slice::from_ref(&profile), &outside),
            None
        );
    }

    #[test]
    fn refresh_chooses_broadest_live_match() {
        let broad = MapProfile {
            id: "broad".to_string(),
            name: "Broad".to_string(),
            zone: Some(128),
            region: Some(104),
            bounds: MapBounds {
                min_x: -100.0,
                max_x: 100.0,
                min_z: -100.0,
                max_z: 100.0,
            },
            pois: Vec::new(),
            anchors: Vec::new(),
            artwork: None,
            can_move: true,
        };
        let detail = MapProfile {
            id: "detail".to_string(),
            name: "Detail".to_string(),
            bounds: MapBounds {
                min_x: -10.0,
                max_x: 10.0,
                min_z: -10.0,
                max_z: 10.0,
            },
            ..broad.clone()
        };
        let stale = MapProfile {
            id: "stale".to_string(),
            name: "Stale".to_string(),
            bounds: MapBounds {
                min_x: 500.0,
                max_x: 600.0,
                min_z: 500.0,
                max_z: 600.0,
            },
            ..broad.clone()
        };
        let game = GameState {
            connected: true,
            pid: Some(1),
            character: String::new(),
            zone: String::new(),
            zone_id: Some(128),
            region_id: Some(104),
            position: Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            rotation: 0.0,
            map_id: String::new(),
        };
        assert_eq!(
            select_refresh_profile(&[stale, broad, detail], &game).as_deref(),
            Some("broad")
        );
    }

    #[test]
    fn refresh_uses_stable_catalog_order_for_equal_bounds() {
        let profile = MapProfile {
            id: "lower".to_string(),
            name: "Lower".to_string(),
            zone: Some(128),
            region: Some(104),
            bounds: MapBounds {
                min_x: -10.0,
                max_x: 10.0,
                min_z: -10.0,
                max_z: 10.0,
            },
            pois: Vec::new(),
            anchors: Vec::new(),
            artwork: None,
            can_move: true,
        };
        let upper = MapProfile {
            id: "upper".to_string(),
            name: "Upper".to_string(),
            ..profile.clone()
        };
        let game = GameState {
            connected: true,
            pid: Some(1),
            character: String::new(),
            zone: String::new(),
            zone_id: Some(128),
            region_id: Some(104),
            position: Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            rotation: 0.0,
            map_id: String::new(),
        };
        assert_eq!(
            select_refresh_profile(&[profile, upper], &game).as_deref(),
            Some("upper")
        );
    }
}
