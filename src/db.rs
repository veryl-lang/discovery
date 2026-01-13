use crate::utils::{veryl_build, VerylBuildInfo};
use crate::OptCheck;
use anstyle::{AnsiColor, Style};
use anyhow::{anyhow, Result};
use chrono::serde::ts_seconds;
use chrono::{DateTime, TimeZone, Utc};
use octocrab::models::Code;
use octocrab::Page;
use plotters::prelude::*;
use secrecy::SecretString;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Cursor;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tokio::time;
use url::Url;
use walkdir::WalkDir;
use zip::read::ZipArchive;

const VERYL_BINARY: &str =
    "https://github.com/veryl-lang/veryl/releases/latest/download/veryl-x86_64-linux.zip";
const VERYL_RELEASE_API: &str = "https://api.github.com/repos/veryl-lang/veryl/releases";
const VERYLUP_RELEASE_API: &str = "https://api.github.com/repos/veryl-lang/verylup/releases";

#[derive(Default, Serialize, Deserialize, Debug)]
pub struct Db {
    pub discovered: Vec<Discovered>,
    pub projects: HashMap<u64, Project>,
    #[serde(default)]
    pub veryl_downloads: HashMap<Version, Vec<Download>>,
    #[serde(default)]
    pub verylup_downloads: HashMap<Version, Vec<Download>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Project {
    pub url: Url,
    pub build_logs: Vec<BuildLog>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BuildLog {
    pub rev: String,
    pub veryl_version: Version,
    pub result: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReleaseKind {
    Veryl,
    Verylup,
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

    fn push_discovered(&mut self, discovered: Discovered) {
        self.discovered.push(discovered);
    }

    fn push_release(&mut self, releases: &[GithubRelease], kind: ReleaseKind) {
        let date = Utc::now();
        for release in releases {
            let version = release.name.strip_prefix("v").unwrap();
            let version = Version::parse(version).unwrap();

            let mut counts = HashMap::new();

            for asset in &release.assets {
                let name = asset.name.as_str();
                let platform = if name.ends_with("x86_64-linux.zip") {
                    Platform::X86_64Linux
                } else if name.ends_with("x86_64-mac.zip") {
                    Platform::X86_64Mac
                } else if name.ends_with("x86_64-windows.zip") {
                    Platform::X86_64Windows
                } else if name.ends_with("aarch64-linux.zip") {
                    Platform::Aarch64Linux
                } else if name.ends_with("aarch64-mac.zip") {
                    Platform::Aarch64Mac
                } else if name.ends_with("aarch64-windows.zip") {
                    Platform::Aarch64Windows
                } else {
                    unreachable!()
                };
                counts.insert(platform, asset.download_count);
            }

            let download = Download { date, counts };

            match kind {
                ReleaseKind::Veryl => {
                    self.veryl_downloads
                        .entry(version)
                        .and_modify(|x| {
                            if x.last().unwrap().counts != download.counts {
                                x.push(download.clone());
                            }
                        })
                        .or_insert(vec![download]);
                }
                ReleaseKind::Verylup => {
                    self.verylup_downloads
                        .entry(version)
                        .and_modify(|x| {
                            if x.last().unwrap().counts != download.counts {
                                x.push(download.clone());
                            }
                        })
                        .or_insert(vec![download]);
                }
            }
        }
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

    async fn search(query: &str, retry: u32) -> Result<Page<Code>> {
        let token = SecretString::from(std::env::var("GITHUB_TOKEN").unwrap());
        let octocrab = octocrab::Octocrab::builder()
            .personal_token(token)
            .build()?;

        let mut duration = 30;

        for _ in 0..retry {
            if let Ok(page) = octocrab.search().code(query).send().await {
                return Ok(page);
            } else {
                time::sleep(Duration::from_secs(duration)).await;
                duration *= 2;
            }
        }

        Err(anyhow!("retry over"))
    }

    pub async fn update(&mut self) -> Result<()> {
        let page = Self::search("extension:veryl", 5).await?;
        let sources = page.total_count.unwrap_or(0);

        let mut page = Self::search("filename:Veryl.toml", 5).await?;
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
                let id = self.insert_project(project);
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

        self.push_discovered(discovered);

        let client = reqwest::Client::builder()
            .user_agent("veryl-discovery/0.1.0")
            .build()?;
        let veryl_releases = client
            .get(VERYL_RELEASE_API)
            .send()
            .await?
            .json::<Vec<GithubRelease>>()
            .await?;
        let verylup_releases = client
            .get(VERYLUP_RELEASE_API)
            .send()
            .await?
            .json::<Vec<GithubRelease>>()
            .await?;

        self.push_release(&veryl_releases, ReleaseKind::Veryl);
        self.push_release(&verylup_releases, ReleaseKind::Verylup);

        Ok(())
    }

    pub fn plot<T: AsRef<Path>>(&self, path: T) -> Result<()> {
        let mut src_plot = Vec::new();
        let mut prj_plot = Vec::new();
        let mut x_min = Utc.timestamp_opt(i32::MAX as i64, 0).unwrap().date_naive();
        let mut x_max = Utc.timestamp_opt(0, 0).unwrap().date_naive();
        let mut src_max = 0;
        let mut prj_max = 0;

        for discovered in &self.discovered {
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

        let backend = SVGBackend::new(path.as_ref(), (1200, 800));
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

    pub async fn build<T: AsRef<Path>>(&mut self, path: T, opt: Option<OptCheck>) -> Result<()> {
        let update_db = opt.is_none();

        let dir = path.as_ref();

        if !dir.exists() {
            fs::create_dir(dir)?;
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
            let mut zip = ZipArchive::new(Cursor::new(binary))?;
            zip.extract_unwrapped_root_dir(&dir, zip::read::root_dir_common_filter)?;
            let mut veryl = dir.to_path_buf();
            veryl.push("veryl");
            veryl.canonicalize()?
        };

        let version = Command::new(&veryl).arg("--version").output()?;
        let version = String::from_utf8(version.stdout)?;
        let version = version.split_whitespace().nth(1).unwrap();
        let version = Version::parse(&version).unwrap();

        let mut projects: Vec<_> = self.projects.clone().into_iter().collect();
        projects.sort_by_key(|x| x.0);

        let mut build_logs = vec![];
        for (id, prj) in &projects {
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

            let _ = Command::new("git")
                .arg("clone")
                .arg("--depth=1")
                .arg(prj.url.as_str())
                .arg(&path)
                .current_dir(&dir)
                .output()?;

            let mut prj_dir = dir.to_path_buf();
            prj_dir.push(&path);

            let rev = Command::new("git")
                .arg("rev-parse")
                .arg("HEAD")
                .current_dir(&prj_dir)
                .output();

            let Ok(rev) = rev else {
                let build_log = BuildLog {
                    rev: "".to_string(),
                    veryl_version: version.clone(),
                    result: false,
                };
                build_logs.push((*id, build_log));

                let color = Style::new().fg_color(Some(AnsiColor::BrightRed.into()));
                println!("{color}Failure{color:#}: {}", prj.url);

                continue;
            };

            let rev = String::from_utf8(rev.stdout)?.trim().to_string();

            if update_db {
                let latest_log = prj.build_logs.last();
                if let Some(latest_log) = latest_log {
                    if latest_log.rev == rev && latest_log.veryl_version == version {
                        continue;
                    }
                }
            }

            let mut veryl_roots = vec![];
            for entry in WalkDir::new(&prj_dir) {
                let entry = entry?;
                if entry.file_name() == "Veryl.toml" {
                    veryl_roots.push(entry.path().parent().unwrap().to_path_buf());
                }
            }

            let mut migrated = false;
            let mut prj_result = true;
            let mut fail_paths = vec![];

            for veryl_root in veryl_roots {
                let version_arg =
                    if let Some(x) = opt.as_ref().map(|x| x.veryl_version.clone()).flatten() {
                        Some(format!("+{x}"))
                    } else {
                        None
                    };

                let mut build_info = VerylBuildInfo {
                    version: version.clone(),
                    veryl: veryl.clone(),
                    veryl_root: veryl_root.clone(),
                    version_arg: version_arg.clone(),
                    compare: false,
                };

                let check_result = veryl_build(&build_info, &mut migrated)?;

                let result = if let Some(ref_version) =
                    opt.as_ref().map(|x| x.ref_version.clone()).flatten()
                {
                    let ref_version = Some(format!("+{ref_version}"));
                    build_info.version_arg = ref_version;
                    let _ = veryl_build(&build_info, &mut migrated)?;

                    build_info.version_arg = version_arg;
                    build_info.compare = true;
                    veryl_build(&build_info, &mut migrated)?
                } else {
                    check_result
                };

                if !result {
                    prj_result = false;
                    let path = if veryl_root == prj_dir {
                        PathBuf::from(".")
                    } else {
                        veryl_root.strip_prefix(&prj_dir).unwrap().to_path_buf()
                    };
                    fail_paths.push(path);
                }
            }

            let build_log = BuildLog {
                rev,
                veryl_version: version.clone(),
                result: prj_result,
            };

            build_logs.push((*id, build_log));

            if prj_result {
                let color = Style::new().fg_color(Some(AnsiColor::BrightGreen.into()));
                if migrated {
                    println!("{color}Migrate{color:#}: {}", prj.url);
                } else {
                    println!("{color}Success{color:#}: {}", prj.url);
                }
            } else {
                let color = Style::new().fg_color(Some(AnsiColor::BrightRed.into()));
                let mut fails = String::new();
                for x in fail_paths {
                    fails.push_str(&format!(" {}", x.to_string_lossy()));
                }
                println!("{color}Failure{color:#}: {} ({})", prj.url, &fails[1..]);
            }
        }

        for (id, build_log) in build_logs {
            self.projects
                .entry(id)
                .and_modify(|x| x.build_logs.push(build_log));
        }

        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Discovered {
    #[serde(with = "ts_seconds")]
    pub date: DateTime<Utc>,
    pub sources: u64,
    pub projects: Vec<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Download {
    #[serde(with = "ts_seconds")]
    pub date: DateTime<Utc>,
    pub counts: HashMap<Platform, u64>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Hash, Clone)]
pub enum Platform {
    Aarch64Linux,
    Aarch64Mac,
    Aarch64Windows,
    X86_64Linux,
    X86_64Mac,
    X86_64Windows,
}

#[derive(Deserialize, Debug)]
pub struct GithubRelease {
    name: String,
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Deserialize, Debug)]
pub struct GithubReleaseAsset {
    name: String,
    download_count: u64,
}
