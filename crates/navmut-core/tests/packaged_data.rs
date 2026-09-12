use std::path::PathBuf;

#[test]
fn packaged_poi_catalog_is_valid_and_asset_free() {
    let catalog = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("data")
        .join("points_of_interest.json");
    let points = navmut_core::load_poi_catalog(catalog).expect("packaged POI catalog");
    assert!(!points.is_empty());
    assert!(points.iter().all(|point| {
        point.as_object().is_some_and(|record| {
            !record.keys().any(|key| {
                let key = key.to_ascii_lowercase();
                key.contains("path") || key.contains("image") || key.contains("asset")
            })
        })
    }));
}

#[test]
fn packaged_map_catalog_has_all_artwork() {
    let data = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("data");
    let profiles =
        navmut_core::load_world_profiles(data.join("maps.json")).expect("packaged map catalog");

    assert_eq!(profiles.len(), 173);
    assert!(profiles.iter().all(|profile| profile.map_image.is_file()));
    assert!(profiles
        .iter()
        .all(|profile| profile.observations.ends_with("observations.jsonl")));
}
