//! Build the POI catalog from decoded retail source tables.

use navmut_core::{atomic_replace, validate_poi_document, Error, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub const EXPECTED_AETHERYTES: usize = 88;

fn excerpt(line: &str) -> String {
    line.chars().take(80).collect()
}

fn digest(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}
fn finite(value: &Value, label: &str) -> Result<f64> {
    let number = value
        .as_f64()
        .ok_or_else(|| Error::invalid(format!("{label} must be numeric")))?;
    if number.is_finite() {
        Ok(number)
    } else {
        Err(Error::invalid(format!("{label} must be finite")))
    }
}

#[derive(PartialEq)]
enum SqlToken {
    Open,
    Close,
    Comma,
    End,
    Value(Value),
}

fn sql_token(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<Option<SqlToken>> {
    while chars
        .peek()
        .is_some_and(|character| character.is_whitespace())
    {
        chars.next();
    }
    let Some(character) = chars.next() else {
        return Ok(None);
    };
    Ok(Some(match character {
        '(' => SqlToken::Open,
        ')' => SqlToken::Close,
        ',' => SqlToken::Comma,
        ';' => SqlToken::End,
        '\'' => {
            let mut text = String::new();
            loop {
                match chars.next() {
                    Some('\'') if chars.peek() == Some(&'\'') => {
                        chars.next();
                        text.push('\'');
                    }
                    Some('\'') => break,
                    Some('\\') => {
                        let escaped = chars
                            .next()
                            .ok_or_else(|| Error::invalid("Unterminated SQL escape"))?;
                        match escaped {
                            '0' => text.push('\0'),
                            'b' => text.push('\u{0008}'),
                            'n' => text.push('\n'),
                            'r' => text.push('\r'),
                            't' => text.push('\t'),
                            'Z' => text.push('\u{001a}'),
                            '%' | '_' => {
                                text.push('\\');
                                text.push(escaped);
                            }
                            _ => text.push(escaped),
                        }
                    }
                    Some(character) => text.push(character),
                    None => return Err(Error::invalid("Unterminated SQL string")),
                }
            }
            SqlToken::Value(Value::String(text))
        }
        _ => {
            let mut text = String::from(character);
            while chars.peek().is_some_and(|character| {
                !character.is_whitespace() && !['(', ')', ',', ';', '\''].contains(character)
            }) {
                text.push(chars.next().expect("peeked character"));
            }
            let value = if text.eq_ignore_ascii_case("NULL") {
                Value::Null
            } else if let Ok(number) = text.parse::<i64>() {
                Value::Number(number.into())
            } else if let Ok(number) = text.parse::<u64>() {
                Value::Number(number.into())
            } else {
                text.parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number)
                    .ok_or_else(|| Error::invalid(format!("Invalid SQL value: {text:?}")))?
            };
            SqlToken::Value(value)
        }
    }))
}

fn sql_values(line: &str) -> Result<Vec<Vec<Value>>> {
    let malformed = || Error::invalid(format!("Malformed SQL INSERT: {:?}", excerpt(line)));
    let (_, body) = line.split_once(" VALUES").ok_or_else(malformed)?;
    let mut chars = body.chars().peekable();
    let mut rows = Vec::new();
    loop {
        if sql_token(&mut chars)? != Some(SqlToken::Open) {
            return Err(malformed());
        }
        let mut row = Vec::new();
        loop {
            let Some(SqlToken::Value(value)) = sql_token(&mut chars)? else {
                return Err(malformed());
            };
            row.push(value);
            match sql_token(&mut chars)? {
                Some(SqlToken::Comma) => {}
                Some(SqlToken::Close) => break,
                _ => return Err(malformed()),
            }
        }
        rows.push(row);
        match sql_token(&mut chars)? {
            Some(SqlToken::Comma) => {}
            Some(SqlToken::End) if sql_token(&mut chars)?.is_none() => return Ok(rows),
            _ => return Err(malformed()),
        }
    }
}

fn parse_aetherytes(path: &Path) -> Result<HashMap<u64, (u16, [f64; 3])>> {
    let text = fs::read_to_string(path)?;
    let body = text
        .split_once("teleportPositions")
        .and_then(|(_, tail)| {
            tail.split_once('{')
                .and_then(|(_, body)| body.split_once("\n}").map(|(body, _)| body))
        })
        .ok_or_else(|| {
            Error::invalid(format!(
                "aetheryte source has no teleportPositions table: {}",
                path.display()
            ))
        })?;
    let mut result = HashMap::new();
    for row in body.split('}').filter_map(|row| {
        row.split_once("[")
            .and_then(|(_, value)| value.split_once(']'))
            .and_then(|(id, rest)| rest.split_once('{').map(|(_, values)| (id, values)))
    }) {
        let id = row
            .0
            .parse::<u64>()
            .map_err(|_| Error::invalid("invalid aetheryte ID"))?;
        if result.contains_key(&id) {
            return Err(Error::invalid(format!("duplicate aetheryte ID {id}")));
        }
        let values: Vec<_> = row.1.split(',').map(str::trim).collect();
        if values.len() != 4 {
            return Err(Error::invalid(format!(
                "aetheryte {id} must have zone and XYZ"
            )));
        }
        let zone = values[0]
            .parse::<u16>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| Error::invalid(format!("aetheryte {id} has invalid zone")))?;
        let position = [
            values[1]
                .parse::<f64>()
                .map_err(|_| Error::invalid(format!("invalid aetheryte {id}")))?,
            values[2]
                .parse::<f64>()
                .map_err(|_| Error::invalid(format!("invalid aetheryte {id}")))?,
            values[3]
                .parse::<f64>()
                .map_err(|_| Error::invalid(format!("invalid aetheryte {id}")))?,
        ];
        if !position.iter().all(|value| value.is_finite()) {
            return Err(Error::invalid(format!("invalid aetheryte {id}")));
        }
        result.insert(id, (zone, position));
    }
    if result.is_empty() {
        return Err(Error::invalid(format!(
            "aetheryte source has no coordinate-backed entries: {}",
            path.display()
        )));
    }
    Ok(result)
}

fn parse_aetheryte_names(path: &Path) -> Result<HashMap<u64, String>> {
    let mut result = HashMap::new();
    let mut symbols = std::collections::HashSet::new();
    let mut in_values = false;
    for line in fs::read_to_string(path)?.lines() {
        if line.trim() == "values:" {
            in_values = true;
            continue;
        }
        if !in_values || line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((symbol, raw_id)) = line.trim().split_once(':') else {
            return Err(Error::invalid(format!(
                "malformed aetheryte name row in {}: {line:?}",
                path.display()
            )));
        };
        let symbol = symbol.trim();
        if symbol.is_empty()
            || !symbol
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_uppercase())
            || !symbol.chars().all(|character| {
                character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
            })
        {
            return Err(Error::invalid(format!(
                "malformed aetheryte name row in {}: {line:?}",
                path.display()
            )));
        }
        let id = raw_id
            .split('#')
            .next()
            .unwrap_or(raw_id)
            .trim()
            .parse::<u64>()
            .map_err(|_| {
                Error::invalid(format!(
                    "malformed aetheryte name row in {}: {line:?}",
                    path.display()
                ))
            })?;
        if !symbols.insert(symbol.to_string()) || result.insert(id, symbol.to_string()).is_some() {
            return Err(Error::invalid(format!(
                "duplicate aetheryte name or ID in {}",
                path.display()
            )));
        }
    }
    if result.is_empty() {
        return Err(Error::invalid(format!(
            "aetheryte names source has no values: {}",
            path.display()
        )));
    }
    Ok(result)
}

fn parse_conditions(path: &Path) -> Result<HashMap<(u64, String), Value>> {
    let document: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    let entries = document
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::invalid(format!(
                "quest condition source has no entries list: {}",
                path.display()
            ))
        })?;
    let mut result = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let object = entry
            .as_object()
            .ok_or_else(|| Error::invalid(format!("quest condition {index} must be an object")))?;
        let Some(actor) = object.get("actorClassId").and_then(Value::as_u64) else {
            continue;
        };
        let unique = object
            .get("uniqueId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::invalid(format!("quest condition {index} has no uniqueId")))?;
        let mut ids = vec![unique.to_string()];
        if let Some(alts) = object.get("altUniqueIds") {
            let values = alts.as_array().ok_or_else(|| {
                Error::invalid(format!("quest condition {index} has invalid altUniqueIds"))
            })?;
            for value in values {
                ids.push(
                    value
                        .as_str()
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| {
                            Error::invalid(format!(
                                "quest condition {index} has invalid altUniqueIds"
                            ))
                        })?
                        .to_string(),
                );
            }
        }
        for id in ids {
            if result.insert((actor, id.clone()), entry.clone()).is_some() {
                return Err(Error::invalid("duplicate quest condition identity"));
            }
        }
    }
    if result.is_empty() {
        return Err(Error::invalid(format!(
            "quest condition source has no numeric identities: {}",
            path.display()
        )));
    }
    Ok(result)
}

fn parse_actor_classes(path: &Path) -> Result<HashMap<u64, (u64, u64)>> {
    let mut result = HashMap::new();
    for line in fs::read_to_string(path)?.lines().filter(|line| {
        line.trim_start()
            .starts_with("INSERT INTO actorclass VALUES (")
    }) {
        for row in sql_values(line)? {
            let actor = row
                .first()
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::invalid("malformed actorclass row"))?;
            let display = row
                .get(2)
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::invalid("malformed actorclass row"))?;
            let flags = row
                .get(3)
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::invalid("malformed actorclass row"))?;
            if result.insert(actor, (display, flags)).is_some() {
                return Err(Error::invalid(format!("duplicate actor class ID {actor}")));
            }
        }
    }
    if result.is_empty() {
        return Err(Error::invalid(format!(
            "actorclass source has no rows: {}",
            path.display()
        )));
    }
    Ok(result)
}

fn parse_display_names(path: &Path) -> Result<HashMap<u64, String>> {
    let document: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    let rows = document
        .get("displayNames")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::invalid(format!(
                "display name source has no displayNames list: {}",
                path.display()
            ))
        })?;
    let mut result = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        let object = row
            .as_object()
            .ok_or_else(|| Error::invalid(format!("malformed display name row {index}")))?;
        let id = object
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::invalid(format!("malformed display name row {index}")))?;
        let name = object
            .get("en")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::invalid(format!("malformed display name row {index}")))?;
        if result.insert(id, name.to_string()).is_some() {
            return Err(Error::invalid(format!("duplicate display name ID {id}")));
        }
    }
    Ok(result)
}

fn symbol_name(symbol: &str) -> String {
    symbol
        .replace('_', " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build and validate a deterministic POI catalog.
pub fn build_catalog(
    aetheryte_lua: impl AsRef<Path>,
    aetheryte_yaml: impl AsRef<Path>,
    server_spawns: impl AsRef<Path>,
    quest_conditions: impl AsRef<Path>,
    actorclass: impl AsRef<Path>,
    display_names: impl AsRef<Path>,
    output: impl AsRef<Path>,
) -> Result<Value> {
    let aetheryte_lua = aetheryte_lua.as_ref();
    let aetheryte_yaml = aetheryte_yaml.as_ref();
    let server_spawns = server_spawns.as_ref();
    let quest_conditions = quest_conditions.as_ref();
    let actorclass = actorclass.as_ref();
    let display_names_path = display_names.as_ref();
    let output = output.as_ref();
    let positions = parse_aetherytes(aetheryte_lua)?;
    let names = parse_aetheryte_names(aetheryte_yaml)?;
    if positions.len() != EXPECTED_AETHERYTES {
        return Err(Error::invalid(format!(
            "expected exactly {EXPECTED_AETHERYTES} coordinate-backed aetherytes, got {}",
            positions.len()
        )));
    }
    let missing: Vec<_> = positions
        .keys()
        .filter(|id| !names.contains_key(id))
        .copied()
        .collect();
    if !missing.is_empty() {
        return Err(Error::invalid(format!(
            "aetherytes missing names: {missing:?}"
        )));
    }
    let conditions = parse_conditions(quest_conditions)?;
    let classes = parse_actor_classes(actorclass)?;
    let display_names = parse_display_names(display_names_path)?;
    let mut pois = Vec::new();
    let mut aetheryte_ids: Vec<_> = positions.keys().copied().collect();
    aetheryte_ids.sort_unstable();
    for id in aetheryte_ids {
        let (zone, position) = positions[&id];
        let symbol = &names[&id];
        pois.push(serde_json::json!({"poi_id":format!("aetheryte-{id}"),"kind":"aetheryte","name":symbol_name(symbol),"zone":zone,"position":position,"height_anchor":true,"aetheryte_id":id,"source_symbol":symbol}));
    }
    let mut seen_spawn = std::collections::HashSet::new();
    let mut parsed_rows = 0;
    for line in fs::read_to_string(server_spawns)?.lines().filter(|line| {
        line.trim_start().starts_with("INSERT INTO ") && line.contains("server_spawn_locations")
    }) {
        for row in sql_values(line)? {
            if row.len() < 13 {
                return Err(Error::invalid("malformed server spawn row"));
            }
            let spawn = row[0]
                .as_u64()
                .ok_or_else(|| Error::invalid("malformed server spawn row"))?;
            let actor = row[1]
                .as_u64()
                .ok_or_else(|| Error::invalid("malformed server spawn row"))?;
            let zone = row[3]
                .as_u64()
                .and_then(|value| u16::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| Error::invalid("malformed server spawn row"))?;
            if !seen_spawn.insert(spawn) {
                return Err(Error::invalid(format!(
                    "duplicate server spawn row ID {spawn}"
                )));
            }
            parsed_rows += 1;
            let unique_id = row[2].as_str().ok_or_else(|| {
                Error::invalid(format!("server spawn {spawn} has invalid unique_id"))
            })?;
            if !row
                .get(4)
                .is_some_and(|value| value.is_null() || value.as_str() == Some(""))
            {
                continue;
            }
            let Some(condition) = conditions.get(&(actor, unique_id.to_string())) else {
                continue;
            };
            let Some((display_id, _flags)) = classes.get(&actor).copied() else {
                return Err(Error::invalid(format!(
                    "quest spawn {spawn} has unknown actor class {actor}"
                )));
            };
            let Some(name) = display_names
                .get(&display_id)
                .filter(|name| !name.trim().is_empty())
            else {
                continue;
            };
            if [6, 7, 8]
                .iter()
                .any(|index| row.get(*index).is_none_or(Value::is_null))
            {
                continue;
            }
            let position = [
                finite(&row[6], &format!("server spawn {spawn} position"))?,
                finite(&row[7], &format!("server spawn {spawn} position"))?,
                finite(&row[8], &format!("server spawn {spawn} position"))?,
            ];
            let rotation = finite(&row[9], &format!("server spawn {spawn} rotation"))?;
            let quest_unique_id = condition
                .get("uniqueId")
                .and_then(Value::as_str)
                .unwrap_or(unique_id);
            pois.push(serde_json::json!({"poi_id":format!("quest-npc-{spawn}"),"kind":"quest_npc","name":name,"zone":zone,"position":position,"height_anchor":true,"spawn_id":spawn,"actor_class_id":actor,"unique_id":unique_id,"display_name_id":display_id,"rotation":rotation,"quest_unique_id":quest_unique_id}));
        }
    }
    if parsed_rows == 0 {
        return Err(Error::invalid(
            "server spawn source has no recognized INSERT rows",
        ));
    }
    let document = serde_json::json!({"schema_version":1,"pois":pois,"coverage":{"aetherytes":pois.iter().filter(|poi| poi.get("kind").and_then(Value::as_str)==Some("aetheryte")).count(),"quest_npc_placements":pois.iter().filter(|poi| poi.get("kind").and_then(Value::as_str)==Some("quest_npc")).count(),"total":pois.len()},"sources":{"aetheryte_lua_sha256":digest(aetheryte_lua)?,"aetheryte_yaml_sha256":digest(aetheryte_yaml)?,"server_spawns_sha256":digest(server_spawns)?,"quest_conditions_sha256":digest(quest_conditions)?,"actorclass_sha256":digest(actorclass)?,"display_names_sha256":digest(display_names_path)?}});
    validate_poi_document(&document)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary =
        tempfile::NamedTempFile::new_in(output.parent().unwrap_or_else(|| Path::new(".")))?;
    serde_json::to_writer_pretty(temporary.as_file(), &document)?;
    temporary.as_file().sync_all()?;
    let temp_path = temporary.into_temp_path();
    atomic_replace(&temp_path, output)?;
    Ok(document)
}

pub use build_catalog as build_poi_catalog;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn sql_preserves_quoted_numbers_as_strings() {
        assert_eq!(
            sql_values("INSERT INTO t VALUES ('123',123,' 123 ');").unwrap(),
            vec![vec![
                serde_json::json!("123"),
                serde_json::json!(123),
                serde_json::json!(" 123 ")
            ]]
        );
    }

    #[test]
    fn sql_distinguishes_quoted_null_from_bare_null() {
        assert_eq!(
            sql_values("INSERT INTO t VALUES ('NULL',NULL,null,'null');").unwrap(),
            vec![vec![
                serde_json::json!("NULL"),
                Value::Null,
                Value::Null,
                serde_json::json!("null")
            ]]
        );
    }

    #[test]
    fn sql_decodes_mysql_backslash_and_doubled_quote_escapes() {
        let rows = sql_values(
            r"INSERT INTO t VALUES ('it\'s, (here);','C:\\maps','it''s','line\nnext\r\t\0\b\Z');",
        )
        .unwrap();
        assert_eq!(
            rows,
            vec![vec![
                serde_json::json!("it's, (here);"),
                serde_json::json!("C:\\maps"),
                serde_json::json!("it's"),
                serde_json::json!("line\nnext\r\t\0\u{0008}\u{001a}")
            ]]
        );
    }

    #[test]
    fn sql_parses_multiple_values_rows_and_actor_classes() {
        let root = tempdir().unwrap();
        let path = root.path().join("actorclass.sql");
        let statement = "INSERT INTO actorclass VALUES (10,'123',100,0), (11,'NULL',101,1);";
        fs::write(&path, statement).unwrap();
        let rows = sql_values(statement).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][1], "123");
        assert_eq!(rows[1][1], "NULL");
        let classes = parse_actor_classes(&path).unwrap();
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[&10], (100, 0));
        assert_eq!(classes[&11], (101, 1));
    }

    #[test]
    fn sql_rejects_incomplete_strings_rows_and_trailing_tokens() {
        for statement in [
            "('unterminated);",
            r"('unterminated\",
            "(1,);",
            "(1),;",
            "(1); (2);",
            "(1)",
        ] {
            assert!(
                sql_values(&format!("INSERT INTO t VALUES {statement}")).is_err(),
                "{statement}"
            );
        }
    }

    #[test]
    fn builds_deterministic_aetherytes_and_quest_npc() {
        let root = tempdir().unwrap();
        let lua = root.path().join("aetheryte.lua");
        let mut lua_text = String::from("teleportPositions = {\n");
        for id in 1..=88 {
            lua_text.push_str(&format!("  [{id}] = {{128, {id}, 5, {id}}},\n"));
        }
        lua_text.push_str("}\n");
        fs::write(&lua, lua_text).unwrap();
        let yaml = root.path().join("aetheryte.yaml");
        let mut yaml_text = String::from("values:\n");
        for id in 1..=88 {
            yaml_text.push_str(&format!("  CAMP_{id}: {id}\n"));
        }
        fs::write(&yaml, yaml_text).unwrap();
        let spawns = root.path().join("spawns.sql");
        fs::write(
            &spawns,
            concat!(
                "INSERT INTO `server_spawn_locations` VALUES ",
                "(1, 1000001, 'quest_npc', 128, '', 0, 4, 5, 6, 0, 0, 0, NULL), ",
                "(2, 1000001, '123', 128, NULL, 0, 4, 5, 6, 0, 0, 0, NULL), ",
                "(3, 1000001, 'npc\\'s\\\\place', 128, NULL, 0, 4, 5, 6, 0, 0, 0, NULL), ",
                "(4, 1000001, '123', 128, 'NULL', 0, 4, 5, 6, 0, 0, 0, NULL);\n"
            ),
        )
        .unwrap();
        let conditions = root.path().join("conditions.json");
        fs::write(
            &conditions,
            serde_json::json!({"entries":[
                {"actorClassId":1000001,"uniqueId":"quest_npc"},
                {"actorClassId":1000001,"uniqueId":"123"},
                {"actorClassId":1000001,"uniqueId":"npc's\\place"}
            ]})
            .to_string(),
        )
        .unwrap();
        let actorclass = root.path().join("actorclass.sql");
        fs::write(
            &actorclass,
            "INSERT INTO actorclass VALUES (1000001, '/Npc', 100, 0);\n",
        )
        .unwrap();
        let display_names = root.path().join("names.json");
        fs::write(
            &display_names,
            serde_json::json!({"displayNames":[{"id":100,"en":"Quest NPC"}]}).to_string(),
        )
        .unwrap();
        let document = build_catalog(
            &lua,
            &yaml,
            &spawns,
            &conditions,
            &actorclass,
            &display_names,
            root.path().join("pois.json"),
        )
        .unwrap();
        assert_eq!(document["coverage"]["aetherytes"], 88);
        assert_eq!(document["coverage"]["quest_npc_placements"], 3);
        let pois = document["pois"].as_array().unwrap();
        assert_eq!(pois[89]["unique_id"], "123");
        assert_eq!(pois[90]["unique_id"], "npc's\\place");
        build_catalog(
            &lua,
            &yaml,
            &spawns,
            &conditions,
            &actorclass,
            &display_names,
            root.path().join("pois.json"),
        )
        .unwrap();
    }

    #[test]
    fn malformed_sql_excerpt_handles_multibyte_text() {
        let line = format!("{}é", "x".repeat(79));
        assert!(sql_values(&line).is_err());
    }

    #[test]
    fn symbol_name_preserves_supported_source_capitalization() {
        assert_eq!(symbol_name("CAMP_BEARDED_ROCK"), "CAMP BEARDED ROCK");
    }
}
