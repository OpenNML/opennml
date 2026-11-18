//! Release helper: reads the shipped package from `cargo metadata` and checks an unpacked release folder.

mod check;
mod files;
mod pe;
mod release;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Release helper for OpenNML")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print machine, subsystem and imports of an exe.
    Inspect { exe: PathBuf },
    /// Check an unpacked release folder; every output line is one finding.
    Check { dir: PathBuf },
    /// Generate the readme and license files of the release archive.
    Files {
        /// Output folder; it is cleared first.
        #[arg(long, default_value = "target/release-files")]
        out: PathBuf,
    },
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    match cli.command {
        Command::Inspect { exe } => {
            let bytes =
                std::fs::read(&exe).with_context(|| format!("reading {}", exe.display()))?;
            let info = pe::read_pe(&bytes).with_context(|| format!("{}", exe.display()))?;
            println!("machine: {:#06x}", info.machine);
            println!("subsystem: {}", info.subsystem);
            println!("imports:");
            for import in &info.imports {
                println!("  {import}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Files { out } => {
            let release = release::load()?;
            let commit = files::git_commit(&release.workspace_root)?;
            files::write_release_files(&release, &commit, &out)?;
            println!(
                "wrote {} for {} {}",
                out.display(),
                release.package,
                release.version
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Check { dir } => {
            let release = release::load()?;
            let findings = check::check_folder(&dir, &release);
            for finding in &findings {
                println!("{finding}");
            }
            Ok(if findings.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(2)
        }
    }
}
