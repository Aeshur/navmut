//! Observation validation and JSONL persistence for the journal format.

use crate::{absolute_without_io, ascii_json, display_path, Error, Result};
use serde_json::{Map, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use time::OffsetDateTime;

pub const SCHEMA_VERSION: u64 = 2;
pub const SUBJECT_TYPES: [&str; 4] = ["npc", "monster", "misc", "unknown"];

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

fn vector(value: Option<&Value>, label: &str) -> Result<()> {
    let values = value
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invalid(format!("{label} must contain exactly three coordinates")))?;
    if values.len() != 3 {
        return Err(Error::invalid(format!(
            "{label} must contain exactly three coordinates"
        )));
    }
    for (index, component) in values.iter().enumerate() {
        finite(component, &format!("{label}[{index}]"))?;
    }
    Ok(())
}

fn non_empty_string(value: Option<&Value>, label: &str) -> Result<String> {
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

fn uint16(value: Option<&Value>, label: &str) -> Result<u16> {
    let number = value
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::invalid(format!("{label} must be a positive uint16")))?;
    u16::try_from(number)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| Error::invalid(format!("{label} must be a positive uint16")))
}

fn validate_provenance(value: Option<&Value>) -> Result<()> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| Error::invalid("provenance must be an object"))?;
    for (key, item) in object {
        let lower = key.to_ascii_lowercase();
        if lower.ends_with("_sha256") || lower.ends_with("_sha512") {
            non_empty_string(Some(item), &format!("provenance.{key}"))?;
        }
        if lower.contains("path") || ["file", "filename", "asset"].contains(&lower.as_str()) {
            return Err(Error::invalid(
                "provenance cannot contain private paths or assets",
            ));
        }
        if item.is_array() || item.is_object() {
            return Err(Error::invalid(
                "provenance values must be hashes or scalar metadata",
            ));
        }
    }
    Ok(())
}

fn validate_v2(record: &Map<String, Value>) -> Result<()> {
    const REQUIRED: [&str; 9] = [
        "schema_version",
        "observation_id",
        "subject_name",
        "subject_type",
        "zone",
        "position",
        "notes",
        "profile_id",
        "created_at",
    ];
    let missing: Vec<_> = REQUIRED
        .iter()
        .filter(|field| !record.contains_key(**field))
        .copied()
        .collect();
    if !missing.is_empty() {
        return Err(Error::invalid(format!(
            "schema-v2 observation is missing {}",
            missing.join(", ")
        )));
    }
    if record.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return Err(Error::invalid("schema_version must be 2"));
    }
    non_empty_string(record.get("observation_id"), "observation_id")?;
    non_empty_string(record.get("subject_name"), "subject_name")?;
    let subject_type = record
        .get("subject_type")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid("subject_type must be npc, monster, misc, or unknown"))?;
    if !SUBJECT_TYPES.contains(&subject_type) {
        return Err(Error::invalid(
            "subject_type must be npc, monster, misc, or unknown",
        ));
    }
    uint16(record.get("zone"), "zone")?;
    vector(record.get("position"), "position")?;
    if let Some(rotation) = record.get("rotation") {
        if !rotation.is_null() {
            finite(rotation, "rotation")?;
        }
    }
    if !record.get("notes").is_some_and(Value::is_string) {
        return Err(Error::invalid("notes must be a string"));
    }
    non_empty_string(record.get("profile_id"), "profile_id")?;
    let created_at = non_empty_string(record.get("created_at"), "created_at")?;
    OffsetDateTime::parse(&created_at, &time::format_description::well_known::Rfc3339)
        .map_err(|_| Error::invalid("created_at must be an ISO-8601 timestamp"))?;
    match record.get("provenance") {
        Some(value) => validate_provenance(Some(value)),
        None => Ok(()),
    }
}

/// Validate and clone one version 2 observation.
pub fn validate_observation(record: &Value) -> Result<Value> {
    let object = record
        .as_object()
        .ok_or_else(|| Error::invalid("observation must be an object"))?;
    if object.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return Err(Error::invalid("schema_version must be 2"));
    }
    validate_v2(object)?;
    Ok(record.clone())
}

pub fn load_observations(
    path: impl AsRef<Path>,
    zone: u16,
    profile_id: Option<&str>,
) -> Result<Vec<Value>> {
    load_observations_filtered(path, Some(zone), profile_id)
}

/// Load only version 2 observations belonging to a profile without inventing
/// a zone for profiles whose catalog entry intentionally has none.
pub fn load_observations_for_profile(
    path: impl AsRef<Path>,
    profile_id: &str,
) -> Result<Vec<Value>> {
    load_observations_filtered(path, None, Some(profile_id))
}

fn load_observations_filtered(
    path: impl AsRef<Path>,
    zone: Option<u16>,
    profile_id: Option<&str>,
) -> Result<Vec<Value>> {
    let path = absolute_without_io(path.as_ref());
    if zone == Some(0) {
        return Err(Error::invalid("zone must be a positive uint16"));
    }
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path)?;
    let mut result = Vec::new();
    for (line_number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line).map_err(|error| {
            Error::invalid(format!(
                "Invalid observation at {}:{}: {error}",
                display_path(&path),
                line_number + 1
            ))
        })?;
        let record = validate_observation(&record).map_err(|error| {
            Error::invalid(format!(
                "Invalid observation at {}:{}: {error}",
                display_path(&path),
                line_number + 1
            ))
        })?;
        let object = record.as_object().expect("validated object");
        let matches = if profile_id.is_some() {
            profile_id.is_some_and(|expected| {
                object.get("profile_id").and_then(Value::as_str) == Some(expected)
            })
        } else {
            zone.is_some_and(|expected| {
                object.get("zone").and_then(Value::as_u64) == Some(u64::from(expected))
            })
        };
        if matches {
            result.push(record);
        }
    }
    Ok(result)
}

pub fn append_observation(path: impl AsRef<Path>, point: &Value) -> Result<()> {
    let checked = validate_observation(point)?;
    let encoded = ascii_json(&checked).map_err(|error| {
        Error::invalid(format!("Observation is not JSON serializable: {error}"))
    })?;
    let path = absolute_without_io(path.as_ref());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(&path)?;
    let metadata = file.metadata()?;
    if metadata.len() > 0 {
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::End(-1))?;
        let mut last = [0u8; 1];
        std::io::Read::read_exact(&mut file, &mut last)?;
        if last[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }
    file.write_all(encoded.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn compact() -> Value {
        serde_json::json!({"schema_version":2,"observation_id":"obs","subject_name":"Goblin","subject_type":"monster","zone":128,"position":[1.0,2.0,3.0],"notes":"","profile_id":"profile","created_at":"2026-01-02T03:04:05Z","provenance":{"map_sha256":"hash"}})
    }

    #[test]
    fn schema_v2_records_round_trip() {
        assert_eq!(validate_observation(&compact()).unwrap(), compact());
        let root = tempdir().unwrap();
        let path = root.path().join("obs.jsonl");
        fs::write(&path, serde_json::to_string(&compact()).unwrap()).unwrap();
        append_observation(&path, &compact()).unwrap();
        assert_eq!(load_observations(&path, 128, None).unwrap().len(), 2);
        assert_eq!(
            load_observations(&path, 128, Some("profile"))
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            load_observations_for_profile(&path, "profile")
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn schema_v2_does_not_require_image_hashes() {
        let mut record = compact();
        record.as_object_mut().unwrap().remove("provenance");
        assert_eq!(validate_observation(&record).unwrap(), record);
    }

    #[test]
    fn schema_v1_records_are_rejected() {
        let old_record =
            serde_json::json!({"schema_version":1,"mob":"Old","zone":128,"position":[1,2,3]});
        let error = validate_observation(&old_record).unwrap_err().to_string();
        assert!(error.contains("schema_version must be 2"));
    }

    #[test]
    fn append_uses_ascii_json_while_round_tripping_unicode() {
        let root = tempdir().unwrap();
        let path = root.path().join("obs.jsonl");
        let mut point = compact();
        point["subject_name"] = Value::String("Cafe\u{00e9} \u{1f600}".to_string());
        append_observation(&path, &point).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains(r#"Cafe\u00e9 \ud83d\ude00"#));
        assert_eq!(
            load_observations(&path, 128, Some("profile")).unwrap(),
            vec![point]
        );
    }
}
