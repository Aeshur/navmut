use clap::Parser;
use navmut_tools::build_map_catalog;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Build a Navmut map catalog")]
struct Args {
    #[arg(long)]
    crosswalk: PathBuf,
    #[arg(long)]
    images: PathBuf,
    #[arg(long = "image-output")]
    image_output: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[arg(long = "map-navi")]
    map_navi: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    match build_map_catalog(
        args.crosswalk,
        args.images,
        args.image_output,
        args.output,
        args.map_navi,
    ) {
        Ok(document) => {
            let coverage = document.get("coverage").expect("coverage");
            println!(
                "Catalog: {} entries, {} images, {} world; excluded {} image-only",
                coverage["profiles"],
                coverage["images"],
                coverage["world_profiles"],
                coverage["excluded_image_profiles"]
            );
        }
        Err(error) => {
            eprintln!("Catalog: {error}");
            std::process::exit(1);
        }
    }
}
