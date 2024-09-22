mod db;

use crate::db::Db;
use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

const DB_DIR: &str = "db";
const BUILD_DIR: &str = "build";
const JSON_PATH: &str = "db/db.json";
const SVG_PATH: &str = "db/plot.svg";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Opt {
    /// No output printed to stdout
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Update(OptUpdate),
    Check(OptCheck),
}

/// Update DB
#[derive(Args)]
pub struct OptUpdate;

/// Check
#[derive(Args)]
pub struct OptCheck {
    #[arg(long)]
    path: Option<PathBuf>,
    #[arg(long)]
    all: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let dir = PathBuf::from(DB_DIR);
    let path = PathBuf::from(JSON_PATH);

    if !dir.exists() {
        std::fs::create_dir(DB_DIR)?;
    }

    let mut db = if path.exists() {
        Db::load(&path)?
    } else {
        Db::default()
    };

    let opt = Opt::parse();

    match opt.command {
        Commands::Update(_) => {
            db.update().await?;
            db.build(PathBuf::from(BUILD_DIR), None).await?;
            db.save(PathBuf::from(JSON_PATH))?;
            db.plot(PathBuf::from(SVG_PATH))?;
        }
        Commands::Check(x) => {
            db.build(PathBuf::from(BUILD_DIR), Some(x)).await?;
        }
    }

    Ok(())
}
