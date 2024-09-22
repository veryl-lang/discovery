use anyhow::Result;
use chrono::serde::ts_seconds;
use chrono::{DateTime, Utc};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use url::Url;

#[derive(Default, Serialize, Deserialize, Debug)]
pub struct Db {
    pub discovered: Vec<Discovered>,
    pub projects: HashMap<u64, Project>,
    #[serde(default)]
    pub downloads: HashMap<Version, Vec<Download>>,
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

    pub fn push_release(&mut self, releases: &[GithubRelease]) {
        let date = Utc::now();
        for release in releases {
            let version = release.name.strip_prefix("v").unwrap();
            let version = Version::parse(version).unwrap();

            let mut counts = HashMap::new();

            for asset in &release.assets {
                let platform = match asset.name.as_str() {
                    "veryl-x86_64-linux.zip" => Platform::X86_64Linux,
                    "veryl-x86_64-mac.zip" => Platform::X86_64Mac,
                    "veryl-x86_64-windows.zip" => Platform::X86_64Windows,
                    "veryl-aarch64-mac.zip" => Platform::Aarch64Mac,
                    _ => unreachable!(),
                };
                counts.insert(platform, asset.download_count);
            }

            let download = Download { date, counts };

            self.downloads
                .entry(version)
                .and_modify(|x| {
                    if x.last().unwrap().counts != download.counts {
                        x.push(download.clone());
                    }
                })
                .or_insert(vec![download]);
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Download {
    #[serde(with = "ts_seconds")]
    pub date: DateTime<Utc>,
    pub counts: HashMap<Platform, u64>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Hash, Clone)]
pub enum Platform {
    Aarch64Mac,
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
