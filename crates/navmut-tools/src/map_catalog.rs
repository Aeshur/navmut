//! Build a portable world map profile catalog from an explicit crosswalk.

use image::ImageReader;
use navmut_core::{atomic_replace, load_profiles, Error, Result};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn digest(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn required_array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invalid(format!("crosswalk must contain a {key} list")))
}

fn number(value: &Value, label: &str) -> Result<u64> {
    value
        .as_u64()
        .ok_or_else(|| Error::invalid(format!("{label} must be a positive integer")))
}
fn optional_zone(value: Option<&Value>, label: &str) -> Result<Option<u16>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let number = value
                .as_u64()
                .ok_or_else(|| Error::invalid(format!("{label} must be a positive uint16")))?;
            Ok(Some(
                u16::try_from(number)
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| Error::invalid(format!("{label} must be a positive uint16")))?,
            ))
        }
    }
}
fn finite_bounds(value: &Value, label: &str) -> Result<Value> {
    let values = value
        .as_array()
        .ok_or_else(|| Error::invalid(format!("{label} must contain four finite numbers")))?;
    if values.len() != 4
        || !values
            .iter()
            .all(|value| value.as_f64().is_some_and(f64::is_finite))
    {
        return Err(Error::invalid(format!(
            "{label} must contain four finite numbers"
        )));
    }
    let bounds: Vec<_> = values
        .iter()
        .map(|value| value.as_f64().expect("finite"))
        .collect();
    if bounds[0] >= bounds[2] || bounds[1] >= bounds[3] {
        return Err(Error::invalid(format!(
            "{label} must have increasing bounds"
        )));
    }
    Ok(value.clone())
}

fn profile_id(row: &Map<String, Value>) -> Result<String> {
    let encoded = format!(
        "{{\"piece\":{},\"zone\":{}}}",
        serde_json::to_string(
            row.get("piece")
                .ok_or_else(|| Error::invalid("crosswalk profile is missing piece"))?
        )?,
        serde_json::to_string(
            row.get("zone")
                .ok_or_else(|| Error::invalid("crosswalk profile is missing zone"))?
        )?
    );
    Ok(format!("profile-{:x}", Sha256::digest(encoded.as_bytes()))[..24].to_string())
}

fn validate_crosswalk_profiles(profiles: &[Value]) -> Result<()> {
    for (index, profile) in profiles.iter().enumerate() {
        let record = profile.as_object().ok_or_else(|| {
            Error::invalid(format!("crosswalk profile {index} must be an object"))
        })?;
        match record.get("coordinate_space") {
            Some(Value::String(value)) if value == "world" || value == "image" => {}
            _ => {
                return Err(Error::invalid(format!(
                    "crosswalk profile {index} coordinate_space must be world or image"
                )));
            }
        }
    }
    Ok(())
}

fn map_navi_regions(path: &Path) -> Result<HashMap<(u64, u64), i64>> {
    let mut regions = HashMap::new();
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_path(path)
        .map_err(|error| Error::invalid(format!("Could not read mapNavi input: {error}")))?;
    for row in reader.records() {
        let row =
            row.map_err(|error| Error::invalid(format!("Could not read mapNavi row: {error}")))?;
        if row.len() < 8 {
            continue;
        }
        let region = row
            .get(1)
            .and_then(|value| value.trim().parse::<i64>().ok());
        let map_id = row
            .get(2)
            .and_then(|value| value.trim().parse::<u64>().ok());
        let piece = row
            .get(7)
            .and_then(|value| value.trim().parse::<u64>().ok());
        let (Some(region), Some(map_id), Some(piece)) = (region, map_id, piece) else {
            continue;
        };
        if let Some(previous) = regions.insert((piece, map_id), region) {
            if previous != region {
                return Err(Error::invalid(format!(
                    "Conflicting mapNavi region for piece {piece}, map {map_id}"
                )));
            }
        }
    }
    if regions.is_empty() {
        return Err(Error::invalid("mapNavi input has no numeric rows"));
    }
    Ok(regions)
}

fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), value)?;
    temporary.as_file_mut().write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    let temporary_path = temporary.into_temp_path();
    atomic_replace(&temporary_path, path).map_err(|error| {
        Error::invalid(format!(
            "Could not replace catalog {}: {error}",
            path.display()
        ))
    })?;
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

/// Build the map catalog and copy only explicitly listed PNG artwork.
pub fn build_catalog(
    crosswalk_path: impl AsRef<Path>,
    images: impl AsRef<Path>,
    image_output: impl AsRef<Path>,
    output: impl AsRef<Path>,
    map_navi: Option<PathBuf>,
) -> Result<Value> {
    let crosswalk_path = crosswalk_path.as_ref();
    let images = images.as_ref();
    let image_output = absolute_path(image_output.as_ref())?;
    let output = output.as_ref();
    let output_base = absolute_path(output.parent().unwrap_or_else(|| Path::new(".")))?;
    let crosswalk: Value = serde_json::from_str(&fs::read_to_string(crosswalk_path)?)?;
    let profiles = required_array(&crosswalk, "profiles")?;
    validate_crosswalk_profiles(profiles)?;
    let regions = map_navi.as_deref().map(map_navi_regions).transpose()?;
    let mut sources = BTreeMap::new();
    for entry in fs::read_dir(images)? {
        let path = entry?.path();
        if let Some(stem) = path.file_name().and_then(|value| value.to_str()) {
            if let Some(piece) = stem
                .strip_prefix("map")
                .and_then(|value| value.strip_suffix(".png"))
                .and_then(|value| (value.len() == 5).then(|| value.parse::<u64>().ok()))
                .flatten()
            {
                sources.insert(piece, path);
            }
        }
    }
    let represented: HashSet<u64> = profiles
        .iter()
        .filter_map(|profile| profile.get("piece").and_then(Value::as_u64))
        .collect();
    let source_pieces: HashSet<u64> = sources.keys().copied().collect();
    if represented != source_pieces {
        return Err(Error::invalid(format!(
            "Crosswalk image coverage mismatch: missing {:?}, unknown {:?}",
            source_pieces.difference(&represented).collect::<Vec<_>>(),
            represented.difference(&source_pieces).collect::<Vec<_>>()
        )));
    }
    let world_pieces: HashSet<u64> = profiles
        .iter()
        .filter(|profile| profile.get("coordinate_space").and_then(Value::as_str) == Some("world"))
        .filter_map(|profile| profile.get("piece").and_then(Value::as_u64))
        .collect();
    let mut old_observations = HashMap::new();
    if output.exists() {
        if let Ok(old) = load_profiles(output) {
            for profile in old {
                if let (Some(zone), Some(name)) = (profile.zone, profile.map_image.file_name()) {
                    old_observations.insert((zone, name.to_os_string()), profile.observations);
                }
            }
        }
    }
    fs::create_dir_all(&image_output)?;
    for source in sources
        .iter()
        .filter(|(piece, _)| world_pieces.contains(piece))
        .map(|(_, source)| source)
    {
        ImageReader::open(source)
            .map_err(|error| {
                Error::invalid(format!("invalid artwork {}: {error}", source.display()))
            })?
            .decode()
            .map_err(|error| {
                Error::invalid(format!("invalid artwork {}: {error}", source.display()))
            })?;
        let target = image_output.join(source.file_name().expect("source filename"));
        if target.exists() {
            if fs::read(&target)? != fs::read(source)? {
                return Err(Error::invalid(format!(
                    "Existing artwork differs: {}",
                    target.display()
                )));
            }
        } else {
            fs::copy(source, &target)?;
        }
    }
    let mut result = Vec::new();
    for record in profiles
        .iter()
        .filter_map(Value::as_object)
        .filter(|record| record.get("coordinate_space").and_then(Value::as_str) == Some("world"))
    {
        let piece = number(
            record
                .get("piece")
                .ok_or_else(|| Error::invalid("crosswalk profile is missing piece"))?,
            "piece",
        )?;
        let zone = optional_zone(record.get("zone"), "zone")?;
        let evidence = record.get("evidence").and_then(Value::as_object);
        let mut region = record.get("region").cloned();
        if let Some(regions) = &regions {
            let mut candidates = HashSet::new();
            if let Some(map_ids) = evidence
                .and_then(|evidence| evidence.get("map_ids"))
                .and_then(Value::as_array)
            {
                for map_id in map_ids.iter().filter_map(Value::as_u64) {
                    if let Some(region) = regions.get(&(piece, map_id)) {
                        candidates.insert(*region);
                    }
                }
            }
            if candidates.is_empty() {
                return Err(Error::invalid(format!(
                    "Crosswalk profile {:?} has no mapNavi region",
                    record.get("name")
                )));
            }
            if candidates.len() > 1 {
                return Err(Error::invalid(format!(
                    "Crosswalk profile {:?} has conflicting mapNavi regions",
                    record.get("name")
                )));
            }
            let mapped = *candidates.iter().next().expect("one candidate");
            if region
                .as_ref()
                .and_then(Value::as_i64)
                .is_some_and(|value| value != mapped)
            {
                return Err(Error::invalid(format!(
                    "Crosswalk profile {:?} contradicts mapNavi region",
                    record.get("name")
                )));
            }
            region = Some(Value::Number(mapped.into()));
        }
        let place = evidence
            .and_then(|evidence| evidence.get("place_name_display"))
            .and_then(Value::as_str);
        let name = place.map_or_else(
            || {
                record
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Unnamed")
                    .to_string()
            },
            |place| format!("{place} - map{piece:05}"),
        );
        let artwork_name = sources
            .get(&piece)
            .and_then(|path| path.file_name())
            .expect("covered artwork");
        let observations = zone
            .and_then(|value| {
                old_observations
                    .get(&(value, artwork_name.to_os_string()))
                    .cloned()
            })
            .unwrap_or_else(|| {
                output_base.join("observations").join(format!(
                    "zone-{}-{piece}.jsonl",
                    zone.map_or_else(|| "unmapped".to_string(), |value| value.to_string())
                ))
            });
        let map_image = path_relative(&output_base, &image_output.join(artwork_name));
        let mut row = Map::new();
        row.insert("profile_id".to_string(), Value::String(profile_id(record)?));
        row.insert("name".to_string(), Value::String(name));
        row.insert(
            "zone".to_string(),
            zone.map_or(Value::Null, |value| Value::Number(value.into())),
        );
        row.insert("map_image".to_string(), Value::String(map_image));
        row.insert(
            "map_bounds".to_string(),
            finite_bounds(
                record
                    .get("bounds")
                    .ok_or_else(|| Error::invalid("crosswalk profile is missing bounds"))?,
                "bounds",
            )?,
        );
        row.insert(
            "coordinate_space".to_string(),
            Value::String("world".to_string()),
        );
        row.insert("region".to_string(), region.unwrap_or(Value::Null));
        row.insert(
            "availability_note".to_string(),
            Value::String(format!(
                "Warp available: zone {}",
                zone.map_or_else(
                    || "the attached player's live zone".to_string(),
                    |value| value.to_string()
                )
            )),
        );
        row.insert(
            "observations".to_string(),
            Value::String(path_relative(&output_base, &absolute_path(&observations)?)),
        );
        result.push(Value::Object(row));
    }
    result.sort_by_key(|record| {
        let object = record.as_object().expect("object");
        (
            object.get("zone").and_then(Value::as_u64) != Some(128),
            object
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase(),
        )
    });
    let world_count = result.len();
    let excluded = profiles.len().saturating_sub(world_count);
    let mut source_evidence = Map::new();
    source_evidence.insert(
        "crosswalk_sha256".to_string(),
        Value::String(digest(crosswalk_path)?),
    );
    if let Some(path) = map_navi {
        source_evidence.insert("map_navi_sha256".to_string(), Value::String(digest(&path)?));
    }
    let zone_bound_profiles = result
        .iter()
        .filter(|row| row.get("zone").is_some_and(|zone| !zone.is_null()))
        .count();
    let document = serde_json::json!({"maps":result,"coverage":{"images":sources.len(),"profiles":world_count,"world_profiles":world_count,"excluded_image_profiles":excluded,"zone_bound_profiles":zone_bound_profiles},"sources":source_evidence});
    // Validate before replacing an existing catalog so incomplete crosswalks do
    // not destroy the last valid document.
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary =
        tempfile::NamedTempFile::new_in(output.parent().unwrap_or_else(|| Path::new(".")))?;
    serde_json::to_writer_pretty(temporary.as_file(), &document)?;
    navmut_core::load_profiles(temporary.path())?;
    drop(temporary);
    atomic_json(output, &document)?;
    Ok(document)
}

fn path_relative(base: &Path, target: &Path) -> String {
    pathdiff(base, target).to_string_lossy().replace('\\', "/")
}
fn pathdiff(base: &Path, target: &Path) -> PathBuf {
    pathdiff_inner(base, target)
}
fn pathdiff_inner(base: &Path, target: &Path) -> PathBuf {
    let base: Vec<_> = base.components().collect();
    let target: Vec<_> = target.components().collect();
    let common = base
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    let mut result = PathBuf::new();
    for _ in common..base.len() {
        result.push("..");
    }
    for component in target.iter().skip(common) {
        result.push(component.as_os_str());
    }
    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn builds_world_only_catalog_and_copies_valid_artwork() {
        let root = tempdir().unwrap();
        let images = root.path().join("images");
        fs::create_dir_all(&images).unwrap();
        for piece in 1..=2 {
            let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 255, 255, 255]));
            image
                .save(images.join(format!("map{piece:05}.png")))
                .unwrap();
        }
        let crosswalk = root.path().join("crosswalk.json");
        fs::write(&crosswalk, serde_json::json!({"profiles":[
            {"name":"World","zone":128,"piece":1,"coordinate_space":"world","bounds":[0,0,10,10]},
            {"name":"Image","zone":null,"piece":2,"coordinate_space":"image","bounds":[0,0,10,10]}
        ]}).to_string()).unwrap();
        let output = root.path().join("local/maps.json");
        let document =
            build_catalog(&crosswalk, &images, root.path().join("art"), &output, None).unwrap();
        assert_eq!(document["coverage"]["world_profiles"], 1);
        assert!(output.exists());
        assert!(root.path().join("art/map00001.png").exists());
        assert!(!root.path().join("art/map00002.png").exists());
        build_catalog(&crosswalk, &images, root.path().join("art"), &output, None).unwrap();
    }

    #[test]
    fn rejects_unknown_coordinate_space_before_replacing_catalog() {
        let root = tempdir().unwrap();
        let images = root.path().join("images");
        fs::create_dir_all(&images).unwrap();
        for piece in 1..=2 {
            let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 255, 255, 255]));
            image
                .save(images.join(format!("map{piece:05}.png")))
                .unwrap();
        }
        let crosswalk = root.path().join("crosswalk.json");
        fs::write(
            &crosswalk,
            serde_json::json!({"profiles":[
                {"name":"World A","zone":128,"piece":1,"coordinate_space":"world","bounds":[0,0,10,10]},
                {"name":"World B","zone":129,"piece":2,"coordinate_space":"world","bounds":[0,0,10,10]}
            ]})
            .to_string(),
        )
        .unwrap();
        let output = root.path().join("local/maps.json");
        build_catalog(&crosswalk, &images, root.path().join("art"), &output, None).unwrap();
        let before = fs::read(&output).unwrap();

        fs::write(
            &crosswalk,
            serde_json::json!({"profiles":[
                {"name":"World A","zone":128,"piece":1,"coordinate_space":"world","bounds":[0,0,10,10]},
                {"name":"World B","zone":129,"piece":2,"coordinate_space":"wrold","bounds":[0,0,10,10]}
            ]})
            .to_string(),
        )
        .unwrap();
        let error = build_catalog(
            &crosswalk,
            &images,
            root.path().join("bad-art"),
            &output,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("coordinate_space"), "{error}");
        assert_eq!(fs::read(&output).unwrap(), before);
        assert!(!root.path().join("bad-art").exists());
    }

    #[test]
    fn relativizes_relative_generation_paths_from_one_absolute_base() {
        let base = absolute_path(Path::new("out/catalog")).unwrap();
        let target = absolute_path(Path::new("out/art/map00001.png")).unwrap();
        assert_eq!(path_relative(&base, &target), "../art/map00001.png");
    }
}
