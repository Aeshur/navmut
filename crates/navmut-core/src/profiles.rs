//! Map profiles and persisted named locations.

use crate::{absolute_without_io, atomic_replace, display_path, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapProfile {
    pub name: String,
    pub zone: Option<u16>,
    pub map_image: PathBuf,
    pub map_bounds: [f64; 4],
    pub observations: PathBuf,
    #[serde(default = "default_coordinate_space")]
    pub coordinate_space: String,
    #[serde(default = "default_availability_note")]
    pub availability_note: String,
    #[serde(default)]
    pub interaction_mode: Option<String>,
    pub profile_id: String,
    #[serde(default)]
    pub region: Option<u16>,
}

fn default_coordinate_space() -> String {
    "world".to_string()
}
fn default_availability_note() -> String {
    "Provisional alignment".to_string()
}

impl MapProfile {
    pub fn can_move(&self) -> bool {
        self.coordinate_space == "world"
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub name: String,
    pub zone: u16,
    pub position: [f64; 3],
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn finite(value: &Value, label: &str) -> Result<f64> {
    let number = value
        .as_f64()
        .ok_or_else(|| Error::invalid(format!("{label} must be a finite number")))?;
    if number.is_finite() {
        Ok(number)
    } else {
        Err(Error::invalid(format!("{label} must be a finite number")))
    }
}

fn number_value(value: f64, label: &str) -> Result<Value> {
    if value.is_finite() {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| Error::invalid(format!("{label} must be a finite number")))
    } else {
        Err(Error::invalid(format!("{label} must be a finite number")))
    }
}

fn string(value: Option<&Value>, label: &str) -> Result<String> {
    let text = value
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid(format!("{label} must be a non-empty string")))?;
    if text.trim().is_empty() {
        Err(Error::invalid(format!(
            "{label} must be a non-empty string"
        )))
    } else {
        Ok(text.to_string())
    }
}

fn zone(value: Option<&Value>, label: &str) -> Result<u16> {
    let number = value
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::invalid(format!("{label} must be a positive uint16")))?;
    u16::try_from(number)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| Error::invalid(format!("{label} must be a positive uint16")))
}

fn optional_zone(value: Option<&Value>, label: &str) -> Result<Option<u16>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => zone(Some(value), label).map(Some),
    }
}

fn position(value: Option<&Value>, label: &str) -> Result<[f64; 3]> {
    let values = value
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invalid(format!("{label} must contain exactly three coordinates")))?;
    if values.len() != 3 {
        return Err(Error::invalid(format!(
            "{label} must contain exactly three coordinates"
        )));
    }
    Ok([
        finite(&values[0], &format!("{label}[0]"))?,
        finite(&values[1], &format!("{label}[1]"))?,
        finite(&values[2], &format!("{label}[2]"))?,
    ])
}

fn bounds(value: Option<&Value>, label: &str) -> Result<[f64; 4]> {
    let values = value.and_then(Value::as_array).ok_or_else(|| {
        Error::invalid(format!(
            "{label} must contain min X, min Z, max X, and max Z"
        ))
    })?;
    if values.len() != 4 {
        return Err(Error::invalid(format!(
            "{label} must contain min X, min Z, max X, and max Z"
        )));
    }
    let result = [
        finite(&values[0], &format!("{label}[0]"))?,
        finite(&values[1], &format!("{label}[1]"))?,
        finite(&values[2], &format!("{label}[2]"))?,
        finite(&values[3], &format!("{label}[3]"))?,
    ];
    if result[0] >= result[2]
        || result[1] >= result[3]
        || !(result[2] - result[0]).is_finite()
        || !(result[3] - result[1]).is_finite()
    {
        return Err(Error::invalid(format!(
            "{label} must have increasing X and Z bounds"
        )));
    }
    Ok(result)
}

fn path(value: Option<&Value>, label: &str, base: &Path) -> Result<PathBuf> {
    let text = string(value, label)?;
    Ok(if Path::new(&text).is_absolute() {
        PathBuf::from(text)
    } else {
        absolute_without_io(&base.join(text))
    })
}

fn parse_profile(record: &Map<String, Value>, manifest: &Path, index: usize) -> Result<MapProfile> {
    let label = format!("{} maps[{index}]", display_path(manifest));
    let name = string(record.get("name"), &format!("{label}.name"))?;
    let zone_value = optional_zone(record.get("zone"), &format!("{label}.zone"))?;
    let map_image = path(
        record.get("map_image"),
        &format!("{label}.map_image"),
        manifest.parent().unwrap_or_else(|| Path::new(".")),
    )?;
    let map_bounds = bounds(record.get("map_bounds"), &format!("{label}.map_bounds"))?;
    let observations = path(
        record.get("observations"),
        &format!("{label}.observations"),
        manifest.parent().unwrap_or_else(|| Path::new(".")),
    )?;
    let coordinate_space = match record.get("coordinate_space") {
        None => default_coordinate_space(),
        Some(value) => string(Some(value), &format!("{label}.coordinate_space"))?,
    };
    if coordinate_space != "world" && coordinate_space != "image" {
        return Err(Error::invalid(format!(
            "{label}: coordinate_space must be world or image"
        )));
    }
    let availability_note = match record.get("availability_note") {
        None => default_availability_note(),
        Some(value) => string(Some(value), &format!("{label}.availability_note"))?,
    };
    let interaction_mode = match record.get("interaction_mode") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let mode = string(Some(value), &format!("{label}.interaction_mode"))?;
            if mode != "warp" && mode != "browse" {
                return Err(Error::invalid(format!(
                    "{label}: interaction_mode must be warp or browse"
                )));
            }
            Some(mode)
        }
    };
    let profile_id = string(record.get("profile_id"), &format!("{label}.profile_id"))?;
    let region = optional_zone(record.get("region"), &format!("{label}.region"))?;
    Ok(MapProfile {
        name,
        zone: zone_value,
        map_image,
        map_bounds,
        observations,
        coordinate_space,
        availability_note,
        interaction_mode,
        profile_id,
        region,
    })
}

pub fn load_profiles(path: impl AsRef<Path>) -> Result<Vec<MapProfile>> {
    let manifest = absolute_without_io(path.as_ref());
    let document: Value =
        serde_json::from_str(&fs::read_to_string(&manifest).map_err(|error| {
            Error::invalid(format!(
                "Could not read map manifest {}: {error}",
                display_path(&manifest)
            ))
        })?)?;
    let records = document
        .get("maps")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::invalid(format!(
                "Invalid map manifest {}: maps must be a non-empty list",
                display_path(&manifest)
            ))
        })?;
    if records.is_empty() {
        return Err(Error::invalid(format!(
            "Invalid map manifest {}: maps must be a non-empty list",
            display_path(&manifest)
        )));
    }
    let mut result = Vec::with_capacity(records.len());
    let mut ids = std::collections::HashSet::new();
    for (index, record) in records.iter().enumerate() {
        let object = record.as_object().ok_or_else(|| {
            Error::invalid(format!(
                "Invalid map profile at {} maps[{index}]: record must be an object",
                display_path(&manifest)
            ))
        })?;
        let profile = parse_profile(object, &manifest, index).map_err(|error| {
            Error::invalid(format!(
                "Invalid map profile at {} maps[{index}]: {error}",
                display_path(&manifest)
            ))
        })?;
        if !ids.insert(profile.profile_id.clone()) {
            return Err(Error::invalid(format!(
                "duplicate profile_id {:?}",
                profile.profile_id
            )));
        }
        result.push(profile);
    }
    Ok(result)
}

pub fn load_world_profiles(path: impl AsRef<Path>) -> Result<Vec<MapProfile>> {
    let manifest = absolute_without_io(path.as_ref());
    let profiles = load_profiles(&manifest)?;
    let world: Vec<_> = profiles
        .into_iter()
        .filter(|profile| profile.coordinate_space == "world")
        .collect();
    if world.is_empty() {
        return Err(Error::invalid(format!(
            "Map manifest {} has no world maps",
            display_path(&manifest)
        )));
    }
    Ok(world)
}

fn parse_locations(path: &Path, document: Value) -> Result<Vec<Location>> {
    let records = document
        .get("locations")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::invalid(format!(
                "Invalid locations file {}: locations must be a list",
                display_path(path)
            ))
        })?;
    let mut result = Vec::with_capacity(records.len());
    let mut seen = std::collections::HashSet::new();
    for (index, record) in records.iter().enumerate() {
        let object = record.as_object().ok_or_else(|| {
            Error::invalid(format!(
                "Invalid location at {} locations[{index}]: record must be an object",
                display_path(path)
            ))
        })?;
        let label = format!("{} locations[{index}]", display_path(path));
        let name = string(object.get("name"), &format!("{label}.name"))?;
        let zone_value = zone(object.get("zone"), &format!("{label}.zone"))?;
        let position_value = position(object.get("position"), &format!("{label}.position"))?;
        if !seen.insert((name.clone(), zone_value)) {
            return Err(Error::invalid(format!(
                "Invalid location at {label}: duplicate name in zone"
            )));
        }
        let extra = object
            .iter()
            .filter(|(key, _)| *key != "name" && *key != "zone" && *key != "position")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        result.push(Location {
            name,
            zone: zone_value,
            position: position_value,
            extra,
        });
    }
    Ok(result)
}

pub fn load_locations(path: impl AsRef<Path>) -> Result<Vec<Location>> {
    let path = absolute_without_io(path.as_ref());
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path).map_err(|error| {
        Error::invalid(format!(
            "Could not read locations file {}: {error}",
            display_path(&path)
        ))
    })?;
    let document: Value = serde_json::from_str(&text).map_err(|error| {
        Error::invalid(format!("Invalid JSON in {}: {error}", display_path(&path)))
    })?;
    parse_locations(&path, document)
}

struct OwnedTemporary {
    path: PathBuf,
    committed: bool,
}

impl OwnedTemporary {
    fn create(path: PathBuf) -> std::io::Result<(Self, fs::File)> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok((
            Self {
                path,
                committed: false,
            },
            file,
        ))
    }

    fn replace(mut self, destination: &Path) -> std::io::Result<()> {
        atomic_replace(&self.path, destination)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for OwnedTemporary {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn atomic_write_json(path: &Path, document: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile_path(path);
    let mut attempts = 0u32;
    while temporary.exists() {
        attempts += 1;
        temporary = parent.join(format!(
            ".{}.{}.tmp",
            path.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("locations.json"),
            attempts
        ));
    }
    let (temporary, file) = OwnedTemporary::create(temporary)?;
    let result = (|| -> Result<()> {
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, document)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        temporary.replace(path)?;
        Ok(())
    })();
    result
}

fn tempfile_path(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            ".{}.tmp",
            path.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("locations.json")
        ))
}

fn locations_document(locations: &[Location]) -> Result<Value> {
    let values = locations
        .iter()
        .map(|location| {
            let mut object = Map::new();
            object.insert("name".to_string(), Value::String(location.name.clone()));
            object.insert(
                "zone".to_string(),
                Value::Number(serde_json::Number::from(location.zone)),
            );
            object.insert(
                "position".to_string(),
                Value::Array(
                    location
                        .position
                        .iter()
                        .enumerate()
                        .map(|(index, value)| number_value(*value, &format!("position[{index}]")))
                        .collect::<Result<Vec<_>>>()?,
                ),
            );
            object.extend(location.extra.clone());
            Ok(Value::Object(object))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut root = Map::new();
    root.insert("locations".to_string(), Value::Array(values));
    Ok(Value::Object(root))
}

pub fn save_location(
    path: impl AsRef<Path>,
    name: &str,
    zone_value: u16,
    position_value: [f64; 3],
) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::invalid("name must be a non-empty string"));
    }
    if zone_value == 0 {
        return Err(Error::invalid("zone must be a positive uint16"));
    }
    if !position_value.iter().all(|value| value.is_finite()) {
        return Err(Error::invalid("position must contain finite coordinates"));
    }
    let path = absolute_without_io(path.as_ref());
    let mut locations = load_locations(&path)?;
    if locations
        .iter()
        .any(|location| location.name == name && location.zone == zone_value)
    {
        return Err(Error::invalid(format!(
            "A location named {name:?} already exists in zone {zone_value}"
        )));
    }
    locations.push(Location {
        name: name.to_string(),
        zone: zone_value,
        position: position_value,
        extra: BTreeMap::new(),
    });
    atomic_write_json(&path, &locations_document(&locations)?)
}

pub fn delete_location(path: impl AsRef<Path>, name: &str, zone_value: u16) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::invalid("name must be a non-empty string"));
    }
    if zone_value == 0 {
        return Err(Error::invalid("zone must be a positive uint16"));
    }
    let path = absolute_without_io(path.as_ref());
    let locations = load_locations(&path)?;
    let remaining: Vec<_> = locations
        .into_iter()
        .filter(|location| !(location.name == name && location.zone == zone_value))
        .collect();
    if remaining.len() == load_locations(&path)?.len() {
        return Err(Error::invalid(format!(
            "No location named {name:?} exists in zone {zone_value}"
        )));
    }
    atomic_write_json(&path, &locations_document(&remaining)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn profile_ids_are_path_free_and_locations_are_atomic() {
        let root = tempdir().unwrap();
        let manifest = root.path().join("maps.json");
        fs::write(&manifest, r#"{"maps":[{"name":"A","zone":128,"map_image":"art/map.png","map_bounds":[0,0,10,10],"observations":"obs.jsonl","profile_id":"profile"}]}"#).unwrap();
        let profile = load_profiles(&manifest).unwrap().remove(0);
        assert_eq!(profile.profile_id, "profile");
        let locations = root.path().join("nested/locations.json");
        save_location(&locations, "Gate", 128, [1.0, 2.0, 3.0]).unwrap();
        save_location(&locations, "Second gate", 128, [4.0, 5.0, 6.0]).unwrap();
        assert_eq!(load_locations(&locations).unwrap()[0].name, "Gate");
        delete_location(&locations, "Gate", 128).unwrap();
        assert_eq!(load_locations(&locations).unwrap()[0].name, "Second gate");
        assert!(!root.path().join("nested/.locations.json.tmp").exists());
    }

    #[test]
    fn world_profiles_without_zone_remain_movable() {
        let root = tempdir().unwrap();
        let manifest = root.path().join("maps.json");
        fs::write(
            &manifest,
            r#"{"maps":[{"name":"Live-zone world","zone":null,"coordinate_space":"world","map_image":"world.png","map_bounds":[0,0,10,10],"observations":"world.jsonl","profile_id":"world"},{"name":"Image","zone":null,"coordinate_space":"image","map_image":"image.png","map_bounds":[0,0,10,10],"observations":"image.jsonl","profile_id":"image"}]}"#,
        )
        .unwrap();
        let profiles = load_profiles(&manifest).unwrap();
        assert!(profiles[0].can_move());
        assert!(!profiles[1].can_move());
    }

    #[test]
    fn present_malformed_coordinate_space_is_rejected() {
        let root = tempdir().unwrap();
        let manifest = root.path().join("maps.json");
        for coordinate_space in [serde_json::Value::Null, serde_json::json!(42)] {
            let document = serde_json::json!({
                "maps": [{"name":"Map","zone":128,"coordinate_space":coordinate_space,"map_image":"map.png","map_bounds":[0,0,10,10],"observations":"map.jsonl","profile_id":"map"}]
            });
            fs::write(&manifest, serde_json::to_vec(&document).unwrap()).unwrap();

            let error = load_profiles(&manifest).unwrap_err().to_string();
            assert!(error.contains("coordinate_space"), "{error}");
        }
    }

    #[test]
    fn failed_owned_temporary_replacement_cleans_only_its_file() {
        let root = tempdir().unwrap();
        let temporary_path = root.path().join(".locations.json.tmp");
        let destination = root.path().join("missing/locations.json");
        let (temporary, file) = OwnedTemporary::create(temporary_path.clone()).unwrap();
        drop(file);

        assert!(temporary.replace(&destination).is_err());
        assert!(!temporary_path.exists());
    }
}
