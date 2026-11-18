use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use opennml_rda::{Vfs, find_game_dir, read_remembered, remembered_file};

#[derive(Parser)]
#[command(
    name = "opennml-extract",
    about = "Tool for the RDA archives of No Man's Land"
)]
struct Cli {
    /// Only this location as the original installation; without it, search as in docs/programs.md, section "Game
    /// directory"
    #[arg(long, global = true)]
    game_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Lists all effective archive files (tab-separated; loose files are not listed)
    List {
        /// Glob pattern, e.g. "*.lvl" or "data/levels/*"
        #[arg(long)]
        filter: Option<String>,
    },
    /// Extracts files into a directory
    Extract {
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        filter: Option<String>,
    },
    /// Converts textures to PNG or models and levels to glTF (.glb)
    Convert {
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        filter: Option<String>,
        /// png: .tga -> .png; gltf: .rdo/.rdw/.lvl -> .glb
        #[arg(long, value_enum, default_value_t = Format::Png)]
        format: Format,
        /// Texels per tile edge of the baked terrain texture (gltf only)
        #[arg(long, default_value_t = 16)]
        pixels_per_tile: u32,
    },
    /// Shows an overview of an .rdo/.rdw/.rdm/.lvl
    Inspect {
        /// Game path, e.g. data/levels/kampagne1/kamp1-1/kamp1-1.lvl
        path: String,
    },
}

/// Target format of `convert`.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Format {
    /// Textures (.tga) as PNG
    Png,
    /// Models (.rdo/.rdw) and levels (.lvl) as glTF (.glb)
    Gltf,
}

fn main() -> Result<()> {
    // Logger so that warnings from opennml-world (skipped objects etc.) appear on the console.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    let game_dir = find_game_dir(
        cli.game_dir.as_deref(),
        remembered_file(None, false)
            .as_deref()
            .and_then(read_remembered),
    )?
    .path;
    let vfs = Vfs::open_game_dir(&game_dir)
        .with_context(|| format!("opening archives in {}", game_dir.display()))?;
    match cli.command {
        Command::List { filter } => {
            let filter = filter
                .as_deref()
                .map(opennml_extract::compile_filter)
                .transpose()?;
            let count =
                opennml_extract::list(&vfs, filter.as_ref(), &mut std::io::stdout().lock())?;
            eprintln!("{count} files");
        }
        Command::Extract { out, filter } => {
            let filter = filter
                .as_deref()
                .map(opennml_extract::compile_filter)
                .transpose()?;
            let count = opennml_extract::extract(&vfs, &out, filter.as_ref())?;
            eprintln!("{count} files extracted to {}", out.display());
        }
        Command::Convert {
            out,
            filter,
            format,
            pixels_per_tile,
        } => {
            let filter = filter
                .as_deref()
                .map(opennml_extract::compile_filter)
                .transpose()?;
            match format {
                Format::Png => {
                    let count = opennml_extract::convert(&vfs, &out, filter.as_ref())?;
                    eprintln!("{count} textures converted to {}", out.display());
                }
                Format::Gltf => {
                    let options = opennml_export::LevelExportOptions {
                        pixels_per_tile,
                        ..Default::default()
                    };
                    let summary =
                        opennml_extract::convert_gltf(&vfs, &out, filter.as_ref(), &options)?;
                    eprintln!(
                        "{} files exported to {} as glTF, {} skipped",
                        summary.exported,
                        out.display(),
                        summary.skipped
                    );
                }
            }
        }
        Command::Inspect { path } => {
            opennml_extract::inspect(&vfs, &path, &mut std::io::stdout().lock())?;
        }
    }
    Ok(())
}
