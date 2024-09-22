mod db;

use crate::db::{BuildLog, Db, Discovered, GithubRelease, Project};
use anyhow::Result;
use chrono::{TimeZone, Utc};
use clap::{Args, Parser, Subcommand};
use plotters::prelude::*;
use secrecy::SecretString;
use std::collections::HashSet;
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::time::Duration;
use tokio::time;
use url::Url;
use walkdir::WalkDir;

const DB_DIR: &str = "db";
const BUILD_DIR: &str = "build";
const JSON_PATH: &str = "db/db.json";
const SVG_PATH: &str = "db/plot.svg";
const VERYL_BINARY: &str =
    "https://github.com/veryl-lang/veryl/releases/latest/download/veryl-x86_64-linux.zip";
const VERYL_RELEASE_API: &str = "https://api.github.com/repos/veryl-lang/veryl/releases";

async fn update(db: &mut Db) -> Result<()> {
    let token = SecretString::from_str(&std::env::var("GITHUB_TOKEN").unwrap())?;
    let octocrab = octocrab::Octocrab::builder()
        .personal_token(token)
        .build()?;

    let page = octocrab.search().code("extension:veryl").send().await?;
    let sources = page.total_count.unwrap_or(0);

    time::sleep(Duration::from_secs(60)).await;

    let mut page = octocrab.search().code("filename:Veryl.toml").send().await?;
    let mut projects = HashSet::new();

    let items = page.take_items();
    for item in items {
        let repo = item.repository.full_name;
        if let Some(repo) = repo {
            let url = Url::parse(&format!("https://github.com/{}", repo)).unwrap();
            let project = Project {
                url,
                build_logs: vec![],
            };
            let id = db.insert_project(project);
            projects.insert(id);
        }
    }

    let mut projects: Vec<_> = projects.into_iter().collect();
    projects.sort();

    let discovered = Discovered {
        date: Utc::now(),
        sources,
        projects,
    };

    db.push_discovered(discovered);

    let client = reqwest::Client::builder()
        .user_agent("veryl-discovery/0.1.0")
        .build()?;
    let releases = client
        .get(VERYL_RELEASE_API)
        .send()
        .await?
        .json::<Vec<GithubRelease>>()
        .await?;

    db.push_release(&releases);

    db.save(PathBuf::from(JSON_PATH))?;

    Ok(())
}

fn plot(db: &Db) -> Result<()> {
    let mut src_plot = Vec::new();
    let mut prj_plot = Vec::new();
    let mut x_min = Utc.timestamp_opt(i32::MAX as i64, 0).unwrap().date_naive();
    let mut x_max = Utc.timestamp_opt(0, 0).unwrap().date_naive();
    let mut src_max = 0;
    let mut prj_max = 0;

    for discovered in &db.discovered {
        let x_val = discovered.date.date_naive();

        x_min = x_min.min(x_val);
        x_max = x_max.max(x_val);
        src_max = src_max.max(discovered.sources);
        prj_max = prj_max.max(discovered.projects.len());

        src_plot.push((x_val, discovered.sources));
        prj_plot.push((x_val, discovered.projects.len()));
    }

    src_max *= 2;
    prj_max *= 2;

    let backend = SVGBackend::new(SVG_PATH, (1200, 800));
    let root = backend.into_drawing_area();
    let _ = root.fill(&WHITE);
    let root = root.margin(10, 10, 10, 10);
    let mut chart = ChartBuilder::on(&root)
        .x_label_area_size(50)
        .y_label_area_size(50)
        .right_y_label_area_size(50)
        .build_cartesian_2d(x_min..x_max, 0..src_max)?
        .set_secondary_coord(x_min..x_max, 0..prj_max);

    chart
        .configure_mesh()
        .disable_x_mesh()
        .disable_y_mesh()
        .y_label_formatter(&|x| format!("{}", x))
        .y_desc("Source")
        .draw()?;

    chart.configure_secondary_axes().y_desc("Project").draw()?;

    let src_style = ShapeStyle {
        color: GREEN.into(),
        filled: true,
        stroke_width: 2,
    };

    let prj_style = ShapeStyle {
        color: BLUE.into(),
        filled: true,
        stroke_width: 2,
    };

    let anno = chart.draw_series(LineSeries::new(src_plot, src_style))?;
    anno.label("source").legend(move |(x, y)| {
        plotters::prelude::PathElement::new(vec![(x, y), (x + 20, y)], src_style)
    });
    let anno = chart.draw_secondary_series(LineSeries::new(prj_plot, prj_style))?;
    anno.label("project").legend(move |(x, y)| {
        plotters::prelude::PathElement::new(vec![(x, y), (x + 20, y)], prj_style)
    });

    chart
        .configure_series_labels()
        .position(SeriesLabelPosition::UpperLeft)
        .background_style(WHITE)
        .border_style(BLACK)
        .draw()?;

    chart.plotting_area().present()?;

    Ok(())
}

async fn build(db: &mut Db, opt: Option<OptCheck>) -> Result<()> {
    let update_db = opt.is_none();

    let dir = PathBuf::from(BUILD_DIR);

    if !dir.exists() {
        fs::create_dir(BUILD_DIR)?;
    }
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();

        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }

    let veryl = if let Some(opt) = &opt {
        if let Some(path) = &opt.path {
            path.canonicalize()?
        } else {
            which::which("veryl")?
        }
    } else {
        let binary = reqwest::get(VERYL_BINARY).await?.bytes().await?;
        zip_extract::extract(Cursor::new(binary), &dir, true)?;
        let mut veryl = dir.clone();
        veryl.push("veryl");
        veryl.canonicalize()?
    };

    let version = Command::new(&veryl).arg("--version").output()?;
    let version = String::from_utf8(version.stdout)?;
    let version = version.replace("veryl ", "").trim().to_string();

    let mut build_logs = vec![];
    for (id, prj) in &db.projects {
        if !update_db {
            let latest_log = prj.build_logs.last();
            if let Some(latest_log) = latest_log {
                if !latest_log.result && !opt.as_ref().unwrap().all {
                    continue;
                }
            }
        }

        let path = prj.url.path().strip_prefix('/').unwrap();
        let path = PathBuf::from(path);
        println!("Checkout: {}", prj.url);

        let _ = Command::new("git")
            .arg("clone")
            .arg("--depth=1")
            .arg(prj.url.as_str())
            .arg(&path)
            .current_dir(&dir)
            .output()?;

        let mut prj_dir = dir.clone();
        prj_dir.push(&path);

        let rev = Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(&prj_dir)
            .output()?;
        let rev = String::from_utf8(rev.stdout)?.trim().to_string();

        if update_db {
            let latest_log = prj.build_logs.last();
            if let Some(latest_log) = latest_log {
                if latest_log.rev == rev && latest_log.veryl_version == version {
                    continue;
                }
            }
        }

        let mut veryl_root = None;
        for entry in WalkDir::new(&prj_dir) {
            let entry = entry?;
            if entry.file_name() == "Veryl.toml" {
                veryl_root = Some(entry.path().parent().unwrap().to_path_buf());
            }
        }

        let result = if let Some(veryl_root) = veryl_root {
            let build = Command::new(&veryl)
                .arg("build")
                .current_dir(&veryl_root)
                .output()?;
            build.status.success()
        } else {
            false
        };

        let build_log = BuildLog {
            rev,
            veryl_version: version.clone(),
            result,
        };

        build_logs.push((*id, build_log));

        if result {
            println!("Build Success");
        } else {
            println!("Build Failure");
        }
    }

    for (id, build_log) in build_logs {
        db.projects
            .entry(id)
            .and_modify(|x| x.build_logs.push(build_log));
    }

    if update_db {
        db.save(PathBuf::from(JSON_PATH))?;
    }

    Ok(())
}

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
            update(&mut db).await?;
            plot(&db)?;
            build(&mut db, None).await?;
        }
        Commands::Check(x) => {
            build(&mut db, Some(x)).await?;
        }
    }

    Ok(())
}
