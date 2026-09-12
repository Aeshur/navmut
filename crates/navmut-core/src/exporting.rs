//! Observation ZIP exports with local paths removed.

use crate::{absolute_without_io, ascii_json, observations::validate_observation, Error, Result};
use csv::WriterBuilder;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

pub const ARCHIVE_NAMES: [&str; 4] = [
    "manifest.json",
    "observations.jsonl",
    "observations.csv",
    "report.md",
];

const CURRENT_CSV_FIELDS: [&str; 13] = [
    "schema_version",
    "observation_id",
    "subject_name",
    "subject_type",
    "zone",
    "position_x",
    "position_y",
    "position_z",
    "rotation",
    "notes",
    "profile_id",
    "created_at",
    "map_sha256",
];
const PATH_KEYS: [&str; 22] = [
    "asset",
    "assets",
    "file",
    "filename",
    "image",
    "image_file",
    "image_path",
    "map_image",
    "artwork",
    "artwork_file",
    "artwork_path",
    "mesh",
    "mesh_sha256",
    "mesh_file",
    "mesh_path",
    "obj",
    "obj_file",
    "obj_path",
    "observations",
    "path",
    "source_file",
    "source_path",
];

const REQUIRED_V2_FIELDS: [&str; 10] = [
    "schema_version",
    "observation_id",
    "subject_name",
    "subject_type",
    "zone",
    "position",
    "notes",
    "profile_id",
    "created_at",
    "provenance",
];

fn path_like(value: &str) -> bool {
    let file_uri = value
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"));
    let normalized = value.replace('\\', "/");
    let drive = value.len() >= 3
        && value.as_bytes().get(1) == Some(&b':')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
        && value
            .as_bytes()
            .get(2)
            .is_some_and(|character| *character == b'/' || *character == b'\\');
    file_uri
        || drive
        || normalized.starts_with('/')
        || normalized.starts_with("//")
        || normalized.split('/').any(|part| {
            [
                ".dds", ".fbx", ".glb", ".jpg", ".jpeg", ".obj", ".png", ".tga",
            ]
            .iter()
            .any(|suffix| part.to_ascii_lowercase().ends_with(suffix))
        })
}

fn safe_record(value: &Value, key: Option<&str>) -> Option<Value> {
    if let Some(key) = key {
        let lower = key.to_ascii_lowercase();
        if PATH_KEYS.contains(&lower.as_str())
            || lower.ends_with("_file")
            || lower.ends_with("_filename")
            || lower.ends_with("_path")
        {
            return None;
        }
    }
    match value {
        Value::Object(object) => {
            let mut safe = Map::new();
            for (child_key, child_value) in object {
                if let Some(cleaned) = safe_record(child_value, Some(child_key)) {
                    safe.insert(child_key.clone(), cleaned);
                } else if key.is_none() && REQUIRED_V2_FIELDS.contains(&child_key.as_str()) {
                    // Required fields may contain text that resembles a local path.
                    // Removing them would make the exported record invalid.
                    safe.insert(child_key.clone(), child_value.clone());
                }
            }
            Some(Value::Object(safe))
        }
        Value::Array(values) => Some(Value::Array(
            values
                .iter()
                .filter_map(|value| safe_record(value, None))
                .collect(),
        )),
        Value::String(text) if path_like(text) => None,
        _ => Some(value.clone()),
    }
}

fn sanitize_record(record: Value) -> Result<Value> {
    let safe = safe_record(&record, None)
        .ok_or_else(|| Error::invalid("Observation record must be a JSON object"))?;
    validate_observation(&safe)?;
    Ok(safe)
}

fn output_aliases_source(sources: &[PathBuf], output: &Path) -> Result<bool> {
    let output = absolute_without_io(output);
    let output_identity = canonical_destination(&output)?;
    let output_file_identity = output.exists().then(|| fs::metadata(&output)).transpose()?;
    Ok(sources.iter().any(|source| {
        let canonical_match = fs::canonicalize(source)
            .ok()
            .zip(output_identity.as_ref())
            .is_some_and(|(canonical, output)| same_path(&canonical, output));
        let file_match = output_file_identity
            .as_ref()
            .zip(fs::metadata(source).ok())
            .is_some_and(|(output, source)| same_file_identity(output, &source));
        canonical_match || file_match
    }))
}

fn same_path(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (left, right);
        false
    }
}

fn canonical_destination(path: &Path) -> Result<Option<PathBuf>> {
    if path.exists() {
        return Ok(Some(fs::canonicalize(path)?));
    }
    let Some(name) = path.file_name() else {
        return Ok(None);
    };
    let mut missing = Vec::new();
    let mut parent = path.parent().unwrap_or_else(|| Path::new("."));
    while !parent.exists() {
        missing.push(parent.to_path_buf());
        let Some(next) = parent.parent() else {
            return Ok(None);
        };
        parent = next;
    }
    let mut canonical = fs::canonicalize(parent)?;
    for component in missing.iter().rev().filter_map(|value| value.file_name()) {
        canonical.push(component);
    }
    canonical.push(name);
    Ok(Some(canonical))
}

struct OwnedArchiveTemporary {
    path: PathBuf,
    committed: bool,
}

impl OwnedArchiveTemporary {
    fn create(destination: &Path) -> Result<(Self, fs::File)> {
        let parent = destination
            .parent()
            .ok_or_else(|| Error::invalid("export path has no parent directory"))?;
        fs::create_dir_all(parent)?;
        let filename = destination
            .file_name()
            .ok_or_else(|| Error::invalid("export path has no file name"))?
            .to_string_lossy();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        for attempt in 0..16u8 {
            let path = parent.join(format!(
                ".{filename}.{}.{}-{attempt}.tmp",
                std::process::id(),
                stamp
            ));
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(file) => {
                    return Ok((
                        Self {
                            path,
                            committed: false,
                        },
                        file,
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(Error::invalid("could not create an owned export temporary"))
    }

    fn commit(mut self, destination: &Path) -> Result<()> {
        crate::atomic_replace(&self.path, destination)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for OwnedArchiveTemporary {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let sorted: BTreeMap<_, _> = object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_value(value)))
                .collect();
            let mut normalized = Map::new();
            for (key, value) in sorted {
                normalized.insert(key, value);
            }
            Value::Object(normalized)
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
        _ => value.clone(),
    }
}

fn canonical(record: &Value) -> String {
    ascii_json(&canonical_value(record)).unwrap_or_default()
}

fn sort_key(record: &Value) -> (String, String, String, String, String) {
    let object = record.as_object();
    let identifier = object
        .and_then(|object| object.get("observation_id"))
        .map_or_else(String::new, value_text);
    let created = object
        .and_then(|object| object.get("created_at"))
        .map_or_else(String::new, value_text);
    let subject = object
        .and_then(|object| object.get("subject_name"))
        .map_or_else(String::new, value_text);
    let zone = object
        .and_then(|object| object.get("zone"))
        .map_or_else(String::new, value_text);
    (identifier, created, subject, zone, canonical(record))
}

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), ToString::to_string)
}

fn position_component(record: &Map<String, Value>, index: usize) -> String {
    record
        .get("position")
        .and_then(Value::as_array)
        .and_then(|position| position.get(index))
        .map_or_else(String::new, value_text)
}
fn field(record: &Map<String, Value>, key: &str) -> String {
    record.get(key).map_or_else(String::new, value_text)
}
fn hash_field(record: &Map<String, Value>, key: &str) -> String {
    record
        .get(key)
        .or_else(|| {
            record
                .get("provenance")
                .and_then(Value::as_object)
                .and_then(|object| object.get(key))
        })
        .map_or_else(String::new, value_text)
}

fn csv_payload(records: &[Value]) -> Result<Vec<u8>> {
    let fields: Vec<&str> = CURRENT_CSV_FIELDS.to_vec();
    let mut writer = WriterBuilder::new()
        .has_headers(true)
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    writer
        .write_record(&fields)
        .map_err(|error| Error::invalid(format!("CSV export failed: {error}")))?;
    for record in records {
        let object = record
            .as_object()
            .ok_or_else(|| Error::invalid("Observation record must be an object"))?;
        let row = vec![
            field(object, "schema_version"),
            field(object, "observation_id"),
            field(object, "subject_name"),
            field(object, "subject_type"),
            field(object, "zone"),
            position_component(object, 0),
            position_component(object, 1),
            position_component(object, 2),
            field(object, "rotation"),
            field(object, "notes"),
            field(object, "profile_id"),
            field(object, "created_at"),
            hash_field(object, "map_sha256"),
        ];
        writer
            .write_record(row)
            .map_err(|error| Error::invalid(format!("CSV export failed: {error}")))?;
    }
    writer
        .into_inner()
        .map_err(|error| Error::invalid(format!("CSV export failed: {error}")))
}

fn markdown_report(records: &[Value]) -> Vec<u8> {
    let mut grouped: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for record in records {
        let zone = record
            .as_object()
            .and_then(|object| object.get("zone"))
            .map_or_else(|| "unknown".to_string(), value_text);
        grouped.entry(zone).or_default().push(record);
    }
    let mut lines = vec![
        "# Observation export".to_string(),
        String::new(),
        format!("Observation count: {}", records.len()),
        String::new(),
    ];
    for (zone, values) in grouped {
        lines.extend([format!("## Zone {zone}"), String::new()]);
        lines.extend([
            "| Subject | Type | X | Y | Z | Rotation | Notes | Recorded at |".to_string(),
            "| --- | --- | ---: | ---: | ---: | ---: | --- | --- |".to_string(),
        ]);
        for record in values {
            let object = record.as_object().expect("record object");
            let position = object.get("position").and_then(Value::as_array);
            let component = |index| {
                position
                    .and_then(|position| position.get(index))
                    .map_or_else(String::new, value_text)
            };
            let mut row = vec![
                field(object, "subject_name"),
                object
                    .get("subject_type")
                    .map_or_else(|| "unknown".to_string(), value_text),
                component(0),
                component(1),
                component(2),
                field(object, "rotation"),
            ];
            row.extend([field(object, "notes"), field(object, "created_at")]);
            lines.push(format!(
                "| {} |",
                row.into_iter()
                    .map(|item| item.replace('|', "\\|").replace('\n', " "))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
        }
        lines.push(String::new());
    }
    lines.push(String::new());
    lines.join("\n").into_bytes()
}

fn hash_bytes(payload: &[u8]) -> String {
    format!("{:x}", Sha256::digest(payload))
}

/// Export one or more JSONL files into a deterministic observations ZIP.
pub fn export_observations<I, P, O>(inputs: I, output: O) -> Result<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
    O: AsRef<Path>,
{
    let sources: Vec<PathBuf> = inputs
        .into_iter()
        .map(|path| absolute_without_io(path.as_ref()))
        .collect();
    if sources.is_empty() {
        return Err(Error::invalid("At least one JSONL input is required"));
    }
    let mut records = Vec::new();
    for path in &sources {
        if !path.is_file() {
            return Err(Error::invalid(format!(
                "Observation input is not a file: {}",
                path.display()
            )));
        }
        for (line_number, line) in fs::read_to_string(path)?.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(line).map_err(|error| {
                Error::invalid(format!(
                    "Invalid observation at {}:{}: {error}",
                    path.display(),
                    line_number + 1
                ))
            })?;
            records.push(validate_observation(&value).map_err(|error| {
                Error::invalid(format!(
                    "Invalid observation at {}:{}: {error}",
                    path.display(),
                    line_number + 1
                ))
            })?);
        }
    }
    let mut safe_records = records
        .into_iter()
        .map(sanitize_record)
        .collect::<Result<Vec<_>>>()?;
    safe_records.sort_by_key(sort_key);
    let jsonl = format!(
        "{}\n",
        safe_records
            .iter()
            .map(canonical)
            .collect::<Vec<_>>()
            .join("\n")
    )
    .into_bytes();
    let csv = csv_payload(&safe_records)?;
    let report = markdown_report(&safe_records);
    let manifest = serde_json::json!({"format":"navmut-observations","schema_version":2,"observation_count":safe_records.len(),"input_count":sources.len(),"members":ARCHIVE_NAMES,"observations_sha256":hash_bytes(&jsonl)});
    let manifest_payload = format!("{}\n", ascii_json(&manifest)?).into_bytes();
    let payloads = [manifest_payload, jsonl, csv, report];
    let output = absolute_without_io(output.as_ref());
    if output_aliases_source(&sources, &output)? {
        return Err(Error::invalid(
            "Export output must not overwrite an observation input",
        ));
    }
    let (temporary, file) = OwnedArchiveTemporary::create(&output)?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(
            DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)
                .map_err(|error| Error::invalid(format!("invalid ZIP date: {error}")))?,
        )
        .unix_permissions(0o600);
    for (name, payload) in ARCHIVE_NAMES.iter().zip(payloads) {
        archive.start_file(name, options)?;
        std::io::Write::write_all(&mut archive, &payload)?;
    }
    archive.finish()?;
    temporary.commit(&output)?;
    Ok(output)
}

pub fn export_observations_one(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
) -> Result<PathBuf> {
    export_observations([input], output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::tempdir;

    #[test]
    fn export_has_fixed_members_and_compact_columns() {
        let root = tempdir().unwrap();
        let source = root.path().join("obs.jsonl");
        fs::write(&source, r#"{"schema_version":2,"observation_id":"a","subject_name":"Ant","subject_type":"monster","zone":128,"position":[1.0,2.0,3.0],"notes":"","profile_id":"p","created_at":"2026-01-02T03:04:05Z","provenance":{"map_sha256":"hash"},"obj_path":"C:\\private\\a.obj"}
"#).unwrap();
        let first = export_observations_one(&source, root.path().join("a.zip")).unwrap();
        let second = export_observations_one(&source, root.path().join("b.zip")).unwrap();
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
        let mut archive = zip::ZipArchive::new(fs::File::open(second).unwrap()).unwrap();
        assert_eq!(
            (0..archive.len())
                .map(|index| archive.by_index(index).unwrap().name().to_string())
                .collect::<Vec<_>>(),
            ARCHIVE_NAMES
        );
        let mut manifest = String::new();
        archive
            .by_name("manifest.json")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        assert!(manifest.contains(r#""format":"navmut-observations""#));
        let mut csv = String::new();
        archive
            .by_name("observations.csv")
            .unwrap()
            .read_to_string(&mut csv)
            .unwrap();
        assert!(csv.starts_with("schema_version,observation_id"));
        assert!(!csv.contains("mesh_sha256"));
    }

    #[test]
    fn export_matches_ascii_and_path_redaction_rules() {
        let root = tempdir().unwrap();
        let source = root.path().join("obs.jsonl");
        let record = serde_json::json!({
            "schema_version": 2,
            "observation_id": "unicode",
            "subject_name": "Cafe\u{00e9} \u{1f600}",
            "subject_type": "monster",
            "zone": 128,
            "position": [1.0, 2.0, 3.0],
            "notes": "", "profile_id": "profile",
            "created_at": "2026-01-02T03:04:05Z",
            "provenance": {"map_sha256": "map-hash"},
            "obj_path": "C:\\private\\ground.obj",
            "source": "C:relative",
            "artifact": "maps/foo.obj/child",
            "capture_uri": "FILE:///C:/private/secret",
            "evidence": {"reference": "https://example.test/capture"}
        });
        fs::write(
            &source,
            format!("{}\n", serde_json::to_string(&record).unwrap()),
        )
        .unwrap();
        let output = export_observations_one(&source, root.path().join("export.zip")).unwrap();
        let mut archive = zip::ZipArchive::new(fs::File::open(output).unwrap()).unwrap();
        let mut jsonl = String::new();
        archive
            .by_name("observations.jsonl")
            .unwrap()
            .read_to_string(&mut jsonl)
            .unwrap();
        assert!(jsonl.contains(r#"Cafe\u00e9 \ud83d\ude00"#));
        assert!(!jsonl.contains("ground.obj"));
        assert!(!jsonl.contains("maps/foo.obj/child"));
        assert!(!jsonl.contains("private/secret"));
        assert!(jsonl.contains("C:relative"));
        assert!(jsonl.contains("https://example.test/capture"));
    }

    #[test]
    fn export_keeps_required_fields_when_their_text_looks_like_a_path() {
        let root = tempdir().unwrap();
        let source = root.path().join("obs.jsonl");
        let record = serde_json::json!({
            "schema_version": 2,
            "observation_id": "path-text",
            "subject_name": "C:\\research\\subject",
            "subject_type": "monster",
            "zone": 128,
            "position": [1.0, 2.0, 3.0],
            "notes": "C:\\research\\notes.txt",
            "profile_id": "profile",
            "created_at": "2026-01-02T03:04:05Z",
            "provenance": {"map_sha256": "map-hash"}
        });
        fs::write(
            &source,
            format!("{}\n", serde_json::to_string(&record).unwrap()),
        )
        .unwrap();
        let output =
            export_observations_one(&source, root.path().join("nested/export.zip")).unwrap();
        let mut archive = zip::ZipArchive::new(fs::File::open(output).unwrap()).unwrap();
        let mut jsonl = String::new();
        archive
            .by_name("observations.jsonl")
            .unwrap()
            .read_to_string(&mut jsonl)
            .unwrap();
        let exported: Value = serde_json::from_str(jsonl.trim()).unwrap();
        assert_eq!(
            exported.get("subject_name").and_then(Value::as_str),
            Some("C:\\research\\subject")
        );
        assert_eq!(
            exported.get("notes").and_then(Value::as_str),
            Some("C:\\research\\notes.txt")
        );
        validate_observation(&exported).unwrap();
    }

    #[test]
    fn export_rejects_input_alias_without_changing_source() {
        let root = tempdir().unwrap();
        let source = root.path().join("obs.jsonl");
        fs::write(
            &source,
            format!(
                "{}\n",
                serde_json::to_string(&serde_json::json!({
                    "schema_version": 2,
                    "observation_id": "alias",
                    "subject_name": "Alias",
                    "subject_type": "monster",
                    "zone": 128,
                    "position": [1.0, 2.0, 3.0],
                    "notes": "",
                    "profile_id": "profile",
                    "created_at": "2026-01-02T03:04:05Z",
                    "provenance": {"map_sha256": "map-hash"}
                }))
                .unwrap()
            ),
        )
        .unwrap();
        let before = fs::read(&source).unwrap();
        let error = export_observations_one(&source, &source)
            .unwrap_err()
            .to_string();
        assert!(error.contains("must not overwrite"));
        assert_eq!(fs::read(&source).unwrap(), before);
    }

    #[test]
    fn failed_export_replacement_leaves_existing_destination_untouched() {
        let root = tempdir().unwrap();
        let source = root.path().join("obs.jsonl");
        fs::write(
            &source,
            format!(
                "{}\n",
                serde_json::to_string(&serde_json::json!({
                    "schema_version": 2,
                    "observation_id": "failure",
                    "subject_name": "Failure",
                    "subject_type": "monster",
                    "zone": 128,
                    "position": [1.0, 2.0, 3.0],
                    "notes": "",
                    "profile_id": "profile",
                    "created_at": "2026-01-02T03:04:05Z",
                    "provenance": {"map_sha256": "map-hash"}
                }))
                .unwrap()
            ),
        )
        .unwrap();
        let destination = root.path().join("destination");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("sentinel"), b"keep").unwrap();
        assert!(export_observations_one(&source, &destination).is_err());
        assert_eq!(fs::read(destination.join("sentinel")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    }
}
