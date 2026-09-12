//! Point of interest validation.

use crate::{absolute_without_io, display_path, Error, Result};
use serde_json::Value;
use std::fs;
use std::path::Path;

pub const POI_SCHEMA_VERSION: u64 = 1;
pub const POI_KINDS: [&str; 2] = ["aetheryte", "quest_npc"];

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

fn identifier(value: Option<&Value>, label: &str) -> Result<String> {
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

fn positive_uint(value: Option<&Value>, label: &str) -> Result<u64> {
    let number = value
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::invalid(format!("{label} must be a positive integer")))?;
    if number > 0 {
        Ok(number)
    } else {
        Err(Error::invalid(format!(
            "{label} must be a positive integer"
        )))
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

/// Validate one POI and return an owned JSON value.
pub fn validate_poi(record: &Value) -> Result<Value> {
    validate_poi_labeled(record, "POI")
}

fn validate_poi_labeled(record: &Value, label: &str) -> Result<Value> {
    let object = record
        .as_object()
        .ok_or_else(|| Error::invalid(format!("Invalid {label}: record must be an object")))?;
    const REQUIRED: [&str; 6] = [
        "poi_id",
        "kind",
        "name",
        "zone",
        "position",
        "height_anchor",
    ];
    let missing: Vec<_> = REQUIRED
        .iter()
        .filter(|field| !object.contains_key(**field))
        .copied()
        .collect();
    if !missing.is_empty() {
        return Err(Error::invalid(format!(
            "Invalid {label}: missing {}",
            missing.join(", ")
        )));
    }
    let poi_id = identifier(object.get("poi_id"), &format!("{label}.poi_id"))?;
    let kind = identifier(object.get("kind"), &format!("{label}.kind"))?;
    if !POI_KINDS.contains(&kind.as_str()) {
        return Err(Error::invalid(format!(
            "Invalid {label}: kind must be aetheryte or quest_npc"
        )));
    }
    let name = identifier(object.get("name"), &format!("{label}.name"))?;
    let zone = uint16(object.get("zone"), &format!("{label}.zone"))?;
    let position = position(object.get("position"), &format!("{label}.position"))?;
    let height_anchor = object
        .get("height_anchor")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            Error::invalid(format!("Invalid {label}: height_anchor must be a boolean"))
        })?;
    let mut result = object.clone();
    result.insert("poi_id".to_string(), Value::String(poi_id));
    result.insert("kind".to_string(), Value::String(kind.clone()));
    result.insert("name".to_string(), Value::String(name));
    result.insert(
        "zone".to_string(),
        Value::Number(serde_json::Number::from(zone)),
    );
    result.insert(
        "position".to_string(),
        Value::Array(
            position
                .iter()
                .map(|value| {
                    serde_json::Number::from_f64(*value)
                        .map(Value::Number)
                        .ok_or_else(|| Error::invalid("position must contain finite coordinates"))
                })
                .collect::<Result<Vec<_>>>()?,
        ),
    );
    result.insert("height_anchor".to_string(), Value::Bool(height_anchor));
    if kind == "aetheryte" {
        let id = positive_uint(object.get("aetheryte_id"), &format!("{label}.aetheryte_id"))?;
        result.insert(
            "aetheryte_id".to_string(),
            Value::Number(serde_json::Number::from(id)),
        );
    } else {
        for key in ["spawn_id", "actor_class_id", "display_name_id"] {
            let id = positive_uint(object.get(key), &format!("{label}.{key}"))?;
            result.insert(key.to_string(), Value::Number(serde_json::Number::from(id)));
        }
        let unique_id = object
            .get("unique_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "Invalid {label}: {label}.unique_id must be a string"
                ))
            })?;
        result.insert(
            "unique_id".to_string(),
            Value::String(unique_id.to_string()),
        );
    }
    Ok(Value::Object(result))
}

pub fn validate_poi_document(document: &Value) -> Result<Vec<Value>> {
    let object = document
        .as_object()
        .ok_or_else(|| Error::invalid("Invalid POI catalog: document must be an object"))?;
    if object.get("schema_version").and_then(Value::as_u64) != Some(POI_SCHEMA_VERSION) {
        return Err(Error::invalid(format!(
            "Invalid POI catalog: schema_version must be {POI_SCHEMA_VERSION}"
        )));
    }
    let records = object
        .get("pois")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invalid("Invalid POI catalog: pois must be a list"))?;
    let mut result = Vec::with_capacity(records.len());
    let mut ids = std::collections::HashSet::new();
    let mut sources = std::collections::HashSet::new();
    for (index, record) in records.iter().enumerate() {
        let checked = validate_poi_labeled(record, &format!("catalog pois[{index}]"))?;
        let object = checked.as_object().expect("validated object");
        let poi_id = object
            .get("poi_id")
            .and_then(Value::as_str)
            .expect("validated id");
        if !ids.insert(poi_id.to_string()) {
            return Err(Error::invalid(format!("duplicate poi_id {poi_id:?}")));
        }
        let kind = object
            .get("kind")
            .and_then(Value::as_str)
            .expect("validated kind");
        let source_key = match kind {
            "aetheryte" => "aetheryte_id",
            "quest_npc" => "spawn_id",
            _ => unreachable!("validated kind"),
        };
        let source = object
            .get(source_key)
            .and_then(Value::as_u64)
            .expect("validated source");
        if !sources.insert((kind.to_string(), source)) {
            return Err(Error::invalid(format!(
                "duplicate source identity ({kind:?}, {source})"
            )));
        }
        result.push(checked);
    }
    Ok(result)
}

pub fn load_pois(path: impl AsRef<Path>) -> Result<Vec<Value>> {
    let path = absolute_without_io(path.as_ref());
    let text = fs::read_to_string(&path).map_err(|error| {
        Error::invalid(format!(
            "Could not read POI catalog {}: {error}",
            display_path(&path)
        ))
    })?;
    let document: Value = serde_json::from_str(&text).map_err(|error| {
        Error::invalid(format!(
            "Invalid POI JSON in {}: {error}",
            display_path(&path)
        ))
    })?;
    validate_poi_document(&document)
}

pub fn load_poi_catalog(path: impl AsRef<Path>) -> Result<Vec<Value>> {
    load_pois(path)
}

pub fn filter_pois(pois: &[Value], query: &str, zone: Option<u16>) -> Vec<Value> {
    let folded = query.trim().to_lowercase();
    pois.iter()
        .filter(|poi| {
            let object = poi.as_object();
            if let Some(zone) = zone {
                if object
                    .and_then(|object| object.get("zone"))
                    .and_then(Value::as_u64)
                    != Some(u64::from(zone))
                {
                    return false;
                }
            }
            let search = ["name", "kind", "poi_id", "unique_id", "aetheryte_id"]
                .iter()
                .map(|key| {
                    object
                        .and_then(|object| object.get(*key))
                        .map_or_else(String::new, Value::to_string)
                })
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            search.contains(&folded)
        })
        .cloned()
        .collect()
}

pub fn poi_label(poi: &Value) -> Result<String> {
    let object = poi
        .as_object()
        .ok_or_else(|| Error::invalid("POI must be an object"))?;
    Ok(format!(
        "{} [{}]",
        object
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::invalid("POI name is missing"))?,
        object
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::invalid("POI kind is missing"))?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_kind_specific_identity_and_filters_deterministically() {
        let poi = validate_poi(&json!({"poi_id":"aetheryte-1","kind":"aetheryte","name":"Camp","zone":128,"position":[1,2,3],"height_anchor":true,"aetheryte_id":1})).unwrap();
        assert_eq!(poi_label(&poi).unwrap(), "Camp [aetheryte]");
        assert_eq!(filter_pois(&[poi], "camp", Some(128)).len(), 1);
    }

    #[test]
    fn duplicate_identity_uses_the_kind_specific_source() {
        let document = json!({
            "schema_version": POI_SCHEMA_VERSION,
            "pois": [
                {"poi_id":"quest-npc-1","kind":"quest_npc","name":"One","zone":128,"position":[1,2,3],"height_anchor":true,"spawn_id":1,"actor_class_id":10,"display_name_id":20,"unique_id":"one","aetheryte_id":99},
                {"poi_id":"quest-npc-2","kind":"quest_npc","name":"Two","zone":128,"position":[4,5,6],"height_anchor":true,"spawn_id":2,"actor_class_id":11,"display_name_id":21,"unique_id":"two","aetheryte_id":99}
            ]
        });

        assert_eq!(validate_poi_document(&document).unwrap().len(), 2);
    }
}
