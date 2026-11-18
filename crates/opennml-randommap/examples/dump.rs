//! Excerpt of a random map for comparison with the original.
//!
//! Writes little-endian, in the order of the reading script: `DIM_X`, `DIM_Z` (`u32`), the heights
//! of the grid points (`f32`, `(DIM + 1)^2`, row by row, x innermost), the tile GUIDs (`u32`, `DIM^2`)
//! and the steepness flags (`u8`, `DIM^2`). The file contains game data and belongs outside the
//! repository.
//!
//! `--extra-out` adds a second file: `DIM_X`, `DIM_Z` (`u32`), then for each grid point (row by row,
//! x innermost, `(DIM_X + 1) * (DIM_Z + 1)` points) the lit vertex color as a D3D color value
//! `0x00RRGGBB` (`TerrainVertex+0x24`, `lit_color` from `Terrain::new` via `to_scene`) and the water alpha
//! 0 ... 255 (`TerrainVertex+0x2C`), both as `u32`.
//!
//! Without `--load-path` the excerpt is generated like the restart (with the object step); with
//! `--load-path` like the load path of the original (without the object step).
//!
//! `--objects-out` (not together with `--load-path`) adds a third file: for each placed object in
//! placement order GUID (`u32`), position x, y, z (`f32`, y = 0), rotation about the vertical axis (`f32`) and
//! owner (`i32`), followed by the blocking map at the end of the object step (`u8`, `DIM^2`, steepness plus
//! footprints).
//!
//! `cargo run -p opennml-randommap --example dump -- --random-map ZKA --out zka.bin`

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use opennml_randommap::{
    RandomMapSpec, generate, generate_load_path, load_types, map_dim, script_and_defines, to_scene,
};
use opennml_rda::{Vfs, find_game_dir, read_remembered, remembered_file};

/// Switches like the viewer switches of the random map.
#[derive(Parser)]
struct Cli {
    /// Only this location as the original installation; without it, search as in `docs/programs.md`,
    /// section "Game directory"
    #[arg(long)]
    game_dir: Option<PathBuf>,
    /// Seed text
    #[arg(long = "random-map", value_name = "SEED")]
    seed: String,
    /// Size 0/1/2 (64/128/256)
    #[arg(long = "random-size", default_value_t = 1)]
    size: i32,
    /// Number of players 2 ... 8
    #[arg(long = "random-players", default_value_t = 4)]
    players: i32,
    /// Map type (index into `MSCRIPT`)
    #[arg(long = "random-terrain", default_value_t = 0)]
    terrain: usize,
    /// Landscape (index into `LTYPE`)
    #[arg(long = "random-landscape", default_value_t = 4)]
    landscape: usize,
    /// Time of day (index into `DTYPE`)
    #[arg(long = "random-daytime", default_value_t = 0)]
    daytime: usize,
    /// Game mode (index into `GTYPE`)
    #[arg(long = "random-game-type", default_value_t = 0)]
    game_type: usize,
    /// Output file
    #[arg(long)]
    out: PathBuf,
    /// Second output file: vertex color and water alpha for each grid point of the scene terrain
    #[arg(long)]
    extra_out: Option<PathBuf>,
    /// Third output file: placed objects and blocking map of the object step
    #[arg(long, conflicts_with = "load_path")]
    objects_out: Option<PathBuf>,
    /// Load path of the original: generate without the object step
    #[arg(long)]
    load_path: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let dir = find_game_dir(
        cli.game_dir.as_deref(),
        remembered_file(None, false)
            .as_deref()
            .and_then(read_remembered),
    )?
    .path;
    let vfs = Vfs::open_game_dir(&dir)
        .with_context(|| format!("opening installation {}", dir.display()))?;
    let types = load_types(&vfs)?;
    let spec = RandomMapSpec {
        seed_text: cli.seed,
        size: cli.size,
        players: cli.players,
        terrain: cli.terrain,
        landscape: cli.landscape,
        daytime: cli.daytime,
        game_type: cli.game_type,
    };
    // Resolved menu values, so the reference run shows which settings the indexes produce.
    let (script, defines) = script_and_defines(&spec, &types)?;
    let word = |list: &[String], i: usize| list.get(i).cloned().unwrap_or_default();
    let game_type = types
        .game_types
        .get(spec.game_type)
        .map_or_else(String::new, |g| g.gtype.clone());
    eprintln!(
        "Map settings: seed \"{}\", script {script}, size {} ({}-tile), {} players",
        spec.seed_text,
        spec.size,
        map_dim(spec.size),
        spec.players
    );
    eprintln!(
        "  Landscape {} = {}, time of day {} = {}, game mode {} = {game_type}, defines {}",
        spec.landscape,
        word(&types.landscape_types, spec.landscape),
        spec.daytime,
        word(&types.daytime_types, spec.daytime),
        spec.game_type,
        defines.join(",")
    );
    let map = if cli.load_path {
        generate_load_path(&vfs, &types, &spec)?
    } else {
        generate(&vfs, &types, &spec)?
    };
    let dim = map.dim as u32;
    let mut out = Vec::with_capacity(8 + map.heights.len() * 4 + map.tiles.len() * 5);
    out.extend(dim.to_le_bytes());
    out.extend(dim.to_le_bytes());
    out.extend(map.heights.iter().flat_map(|h| h.to_le_bytes()));
    out.extend(map.tiles.iter().flat_map(|g| g.to_le_bytes()));
    out.extend(&map.steep);
    std::fs::write(&cli.out, &out).with_context(|| format!("writing {}", cli.out.display()))?;
    if let Some(extra_path) = &cli.extra_out {
        let scene = to_scene(&vfs, &map)?;
        let terrain = &scene.terrain;
        let mut extra = Vec::with_capacity(8 + terrain.vertices.len() * 8);
        extra.extend((terrain.dim_x() as u32).to_le_bytes());
        extra.extend((terrain.dim_z() as u32).to_le_bytes());
        for v in &terrain.vertices {
            let [r, g, b] = v.lit_color;
            let color = u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b);
            extra.extend(color.to_le_bytes());
            extra.extend(u32::from(v.water_alpha).to_le_bytes());
        }
        std::fs::write(extra_path, &extra)
            .with_context(|| format!("writing {}", extra_path.display()))?;
    }
    if let Some(objects_path) = &cli.objects_out {
        let mut objects = Vec::with_capacity(map.objects.len() * 24 + map.object_block.len());
        for o in &map.objects {
            objects.extend(o.guid.to_le_bytes());
            for p in o.position {
                objects.extend(p.to_le_bytes());
            }
            objects.extend(o.rotation_y.to_le_bytes());
            objects.extend(o.owner.to_le_bytes());
        }
        objects.extend(&map.object_block);
        std::fs::write(objects_path, &objects)
            .with_context(|| format!("writing {}", objects_path.display()))?;
        println!(
            "{}: {} bytes, {} objects",
            objects_path.display(),
            objects.len(),
            map.objects.len()
        );
    }
    println!(
        "{}: {} bytes, {} ({dim}-tile), hash {}, rand_state {}, {:?}, {:?}",
        cli.out.display(),
        out.len(),
        map.script_path,
        map.hash,
        map.rand_state,
        map.rand_counts,
        map.digests()
    );
    Ok(())
}
