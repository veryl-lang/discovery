use anyhow::Result;
use chrono::serde::ts_seconds;
use chrono::{DateTime, TimeZone, Utc};
use plotters::prelude::*;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Cursor;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use url::Url;

#[derive(Default, Serialize, Deserialize, Debug)]
pub struct Db {
    pub discovered: Vec<Discovered>,
    pub projects: HashMap<u64, Project>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Project {
    pub url: Url,
    pub build_logs: Vec<BuildLog>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BuildLog {
    pub rev: String,
    pub veryl_version: String,
    pub result: bool,
}

impl Db {
    pub fn load<T: AsRef<Path>>(path: T) -> Result<Db> {
        let mut file = File::open(&path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        let db: Db = serde_json::from_str(&String::from_utf8(buf)?)?;
        Ok(db)
    }

    pub fn save<T: AsRef<Path>>(&self, path: T) -> Result<()> {
        let mut file = File::create(&path)?;
        let encoded: Vec<u8> = serde_json::to_string(&self)?.into_bytes();
        file.write_all(&encoded)?;
        file.flush()?;

        Ok(())
    }

    pub fn push_discovered(&mut self, discovered: Discovered) {
        self.discovered.push(discovered);
    }

    pub fn insert_project(&mut self, prj: Project) -> u64 {
        if let Some(id) = self.find_project(&prj.url) {
            id
        } else {
            let id = self.projects.len() as u64;
            self.projects.insert(id, prj);
            id
        }
    }

    pub fn find_project(&self, url: &Url) -> Option<u64> {
        for (id, prj) in &self.projects {
            if url == &prj.url {
                return Some(*id);
            }
        }
        None
    }

    pub fn get_project(&self, id: u64) -> Option<&Project> {
        self.projects.get(&id)
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Discovered {
    #[serde(with = "ts_seconds")]
    pub date: DateTime<Utc>,
    pub sources: u64,
    pub projects: Vec<u64>,
}

const DB_DIR: &str = "db";
const BUILD_DIR: &str = "build";
const JSON_PATH: &str = "db/db.json";
const SVG_PATH: &str = "db/plot.svg";
const VERYL_RELEASE: &str =
    "https://github.com/veryl-lang/veryl/releases/latest/download/veryl-x86_64-linux.zip";

async fn update(db: &mut Db) -> Result<()> {
    let token = SecretString::from_str(&std::env::var("GITHUB_TOKEN").unwrap())?;
    let octocrab = octocrab::Octocrab::builder()
        .personal_token(token)
        .build()?;

    let page = octocrab.search().code("extension:veryl").send().await?;
    let sources = page.total_count.unwrap_or(0);

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
    db.save(&PathBuf::from(JSON_PATH))?;

    Ok(())
}

fn plot(db: &Db) -> Result<()> {
    let mut src_plot = Vec::new();
    let mut prj_plot = Vec::new();
    let mut x_min = Utc
        .timestamp_opt(std::i32::MAX as i64, 0)
        .unwrap()
        .date_naive();
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

    let anno = chart.draw_series(LineSeries::new(src_plot, src_style.clone()))?;
    anno.label("source").legend(move |(x, y)| {
        plotters::prelude::PathElement::new(vec![(x, y), (x + 20, y)], src_style.clone())
    });
    let anno = chart.draw_secondary_series(LineSeries::new(prj_plot, prj_style.clone()))?;
    anno.label("project").legend(move |(x, y)| {
        plotters::prelude::PathElement::new(vec![(x, y), (x + 20, y)], prj_style.clone())
    });

    chart
        .configure_series_labels()
        .position(SeriesLabelPosition::UpperLeft)
        .background_style(&WHITE)
        .border_style(&BLACK)
        .draw()?;

    chart.plotting_area().present()?;

    Ok(())
}

async fn build(db: &mut Db) -> Result<()> {
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

    let binary = reqwest::get(VERYL_RELEASE).await?.bytes().await?;
    zip_extract::extract(Cursor::new(binary), &dir, true)?;
    let mut veryl = dir.clone();
    veryl.push("veryl");
    let veryl = veryl.canonicalize()?;

    let version = Command::new(&veryl).arg("--version").output()?;
    let version = String::from_utf8(version.stdout)?;
    let version = version.replace("veryl ", "").trim().to_string();

    let mut build_logs = vec![];
    for (id, prj) in &db.projects {
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

        let latest_log = prj.build_logs.last();
        if let Some(latest_log) = latest_log {
            if latest_log.rev == rev && latest_log.veryl_version == version {
                continue;
            }
        }

        let build = Command::new(&veryl)
            .arg("build")
            .current_dir(&prj_dir)
            .output()?;
        let result = build.status.success();

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

    db.save(&PathBuf::from(JSON_PATH))?;

    Ok(())
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

    let _ = update(&mut db).await?;
    plot(&db)?;
    let _ = build(&mut db).await?;

    Ok(())
}
