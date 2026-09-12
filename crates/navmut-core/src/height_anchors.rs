//! Conservative height selection without a navmesh.

use crate::{Error, Result};
use serde_json::Value;

pub const DEFAULT_ANCHOR_RADIUS: f64 = 4.0;

fn finite(value: &Value, label: &str) -> Option<f64> {
    let number = value.as_f64()?;
    number.is_finite().then_some(number).or_else(|| {
        let _ = label;
        None
    })
}

fn position(value: &Value) -> Option<[f64; 3]> {
    let value = value.get("position").unwrap_or(value);
    if let Some(values) = value.as_array() {
        if values.len() != 3 {
            return None;
        }
        return Some([
            finite(&values[0], "position[0]")?,
            finite(&values[1], "position[1]")?,
            finite(&values[2], "position[2]")?,
        ]);
    }
    Some([
        finite(value.get("x")?, "position.x")?,
        finite(value.get("y")?, "position.y")?,
        finite(value.get("z")?, "position.z")?,
    ])
}

fn valid_identifier(value: &Value) -> bool {
    if value.is_null() || value.is_boolean() {
        return false;
    }
    value.as_str().is_some_and(|text| !text.trim().is_empty())
        || value.as_f64().is_some_and(f64::is_finite)
}

fn matches_context(
    record: &Value,
    zone: Option<&Value>,
    profile_id: Option<&str>,
    layer: Option<&str>,
) -> bool {
    fn check(record: &Value, name: &str, expected: Option<&Value>) -> bool {
        let Some(candidate) = record.get(name) else {
            return true;
        };
        valid_identifier(candidate) && expected.is_none_or(|expected| candidate == expected)
    }
    let profile = profile_id.map(|value| Value::String(value.to_string()));
    let active_layer = layer.map(|value| Value::String(value.to_string()));
    check(record, "zone", zone)
        && check(record, "profile_id", profile.as_ref())
        && check(record, "layer", active_layer.as_ref())
}

fn trusted_observation(record: &Value) -> bool {
    if record
        .get("trusted")
        .is_some_and(|value| value != &Value::Bool(true))
    {
        return false;
    }
    match record.get("subject_type").and_then(Value::as_str) {
        Some("npc") | Some("monster") => true,
        Some(_) | None => false,
    }
}

fn records(records: Option<&[Value]>) -> &[Value] {
    records.unwrap_or(&[])
}

/// Resolve a destination Y from nearby trusted records.
///
/// Distances are horizontal X/Z distances.  Observations, locations, and
/// explicit POI anchors only affect equal distance ties in that order. A nearer
/// location therefore beats a farther observation.
#[allow(clippy::too_many_arguments)]
pub fn resolve_height_anchor(
    current_position: &Value,
    observations: Option<&[Value]>,
    locations: Option<&[Value]>,
    pois: Option<&[Value]>,
    zone: Option<&Value>,
    profile_id: Option<&str>,
    layer: Option<&str>,
    radius: f64,
) -> Result<f64> {
    let current = position(current_position)
        .ok_or_else(|| Error::invalid("current_position must contain finite x, y, and z"))?;
    if !radius.is_finite() || radius < 0.0 {
        return Err(Error::invalid("radius must be finite and not negative"));
    }
    for (value, label) in [(zone, "zone")] {
        if let Some(value) = value {
            if !valid_identifier(value) {
                return Err(Error::invalid(format!(
                    "{label} must be a valid identifier"
                )));
            }
        }
    }
    if profile_id.is_some_and(|value| value.trim().is_empty()) {
        return Err(Error::invalid("profile_id must be a valid identifier"));
    }
    if layer.is_some_and(|value| value.trim().is_empty()) {
        return Err(Error::invalid("layer must be a valid identifier"));
    }
    let radius_squared = radius * radius;
    if !radius_squared.is_finite() {
        return Err(Error::invalid("radius must be finite"));
    }
    let mut best_y = current[1];
    let mut best_distance = radius_squared;
    let mut best_order: Option<(usize, usize)> = None;
    for (source_index, (source, kind)) in [
        (records(observations), 0u8),
        (records(locations), 1u8),
        (records(pois), 2u8),
    ]
    .into_iter()
    .enumerate()
    {
        for (record_index, record) in source.iter().enumerate() {
            let accepted = match kind {
                0 => trusted_observation(record),
                2 => record.get("height_anchor") == Some(&Value::Bool(true)),
                _ => true,
            };
            if !accepted || !matches_context(record, zone, profile_id, layer) {
                continue;
            }
            let Some(candidate) = position(record) else {
                continue;
            };
            let dx = candidate[0] - current[0];
            let dz = candidate[2] - current[2];
            let distance_squared = dx.mul_add(dx, dz * dz);
            if !distance_squared.is_finite() || distance_squared > radius_squared {
                continue;
            }
            let order = (source_index, record_index);
            if distance_squared < best_distance
                || (distance_squared == best_distance
                    && best_order.is_none_or(|previous| order < previous))
            {
                best_distance = distance_squared;
                best_order = Some(order);
                best_y = candidate[1];
            }
        }
    }
    Ok(best_y)
}

/// Convenience form with no context filters and the conservative default radius.
pub fn resolve_height_anchor_default(
    current_position: &Value,
    observations: Option<&[Value]>,
    locations: Option<&[Value]>,
    pois: Option<&[Value]>,
) -> Result<f64> {
    resolve_height_anchor(
        current_position,
        observations,
        locations,
        pois,
        None,
        None,
        None,
        DEFAULT_ANCHOR_RADIUS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nearest_wins_and_priority_only_breaks_ties() {
        let observations = vec![json!({"subject_type":"npc","position":[3.0,20.0,0.0]})];
        let locations = vec![json!({"name":"near","position":[1.0,10.0,0.0]})];
        assert_eq!(
            resolve_height_anchor(
                &json!([0.0, 99.0, 0.0]),
                Some(&observations),
                Some(&locations),
                None,
                None,
                None,
                None,
                4.0
            )
            .unwrap(),
            10.0
        );
        let locations = vec![json!({"name":"tie","position":[3.0,30.0,0.0]})];
        assert_eq!(
            resolve_height_anchor(
                &json!([0.0, 99.0, 0.0]),
                Some(&observations),
                Some(&locations),
                None,
                None,
                None,
                None,
                3.0
            )
            .unwrap(),
            20.0
        );
    }

    #[test]
    fn boundary_and_explicit_poi_opt_in() {
        let pois = vec![json!({"height_anchor":true,"position":[4.0,12.0,0.0]})];
        assert_eq!(
            resolve_height_anchor(
                &json!([0.0, 99.0, 0.0]),
                None,
                None,
                Some(&pois),
                None,
                None,
                None,
                4.0
            )
            .unwrap(),
            12.0
        );
    }
}
