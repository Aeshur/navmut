use clap::Parser;
use navmut_tools::build_poi_catalog;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Build a Navmut POI catalog")]
struct Args {
    #[arg(long = "aetheryte-lua")]
    aetheryte_lua: PathBuf,
    #[arg(long = "aetheryte-yaml")]
    aetheryte_yaml: PathBuf,
    #[arg(long = "server-spawns")]
    server_spawns: PathBuf,
    #[arg(long = "quest-conditions")]
    quest_conditions: PathBuf,
    #[arg(long)]
    actorclass: PathBuf,
    #[arg(long = "display-names")]
    display_names: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

fn main() {
    let args = Args::parse();
    match build_poi_catalog(
        args.aetheryte_lua,
        args.aetheryte_yaml,
        args.server_spawns,
        args.quest_conditions,
        args.actorclass,
        args.display_names,
        args.output,
    ) {
        Ok(document) => {
            let coverage = document.get("coverage").expect("coverage");
            println!(
                "POI catalog: {} aetherytes, {} public quest NPC placements",
                coverage["aetherytes"], coverage["quest_npc_placements"]
            );
        }
        Err(error) => {
            eprintln!("POI catalog: {error}");
            std::process::exit(1);
        }
    }
}
