use anyhow::Result;
use semver::Version;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct VerylBuildInfo {
    pub version: Version,
    pub veryl: PathBuf,
    pub veryl_root: PathBuf,
    pub version_arg: Option<String>,
    pub compare: bool,
    pub local: bool,
}

pub fn veryl_build(info: &VerylBuildInfo, migrated: &mut bool) -> Result<bool> {
    let mut build_args = if let Some(x) = &info.version_arg {
        vec![x.as_str(), "build"]
    } else {
        vec!["build"]
    };

    if info.compare {
        build_args.push("--check");
    }

    let build = Command::new(&info.veryl)
        .args(&build_args)
        .current_dir(&info.veryl_root)
        .output()?;
    let first_result = build.status.success();

    if first_result {
        Ok(first_result)
    } else {
        *migrated = true;

        migrate(&info.version, &info.veryl, &info.veryl_root)?;
        if info.local {
            // The released-version chain above does not cover the unreleased
            // breaking change in the local binary itself.
            migrate_local(&info.veryl, &info.veryl_root)?;
        }

        let build = Command::new(&info.veryl)
            .args(&build_args)
            .current_dir(&info.veryl_root)
            .output()?;
        Ok(build.status.success())
    }
}

fn migrate_local(veryl: &Path, veryl_root: &Path) -> Result<()> {
    let _ = Command::new(veryl)
        .arg("migrate")
        .current_dir(veryl_root)
        .output()?;

    // `veryl build` wipes a dependency cache whose working tree is dirty
    // (lockfile.rs `is_clean`), discarding migrations just applied. Commit
    // so the next build sees a clean tree.
    commit_dirty_dependency_caches()?;
    Ok(())
}

fn commit_dirty_dependency_caches() -> Result<()> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(dirs::cache_dir);
    let cache_root = match base {
        Some(dir) => dir.join("veryl").join("dependencies"),
        None => return Ok(()),
    };

    if !cache_root.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(&cache_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if !path.join(".git").exists() {
            continue;
        }

        let status = Command::new("git")
            .args(["status", "-s"])
            .current_dir(&path)
            .output()?;
        if !status.status.success() || status.stdout.is_empty() {
            continue;
        }

        let _ = Command::new("git")
            .args([
                "-c",
                "user.email=migrate@discovery",
                "-c",
                "user.name=discovery",
                "commit",
                "-am",
                "migrate",
            ])
            .current_dir(&path)
            .output()?;
    }

    Ok(())
}

fn migrate(version: &Version, veryl: &Path, veryl_root: &Path) -> Result<()> {
    if version.major == 0 {
        let mut minor = version.minor;

        let mut migrate_success = false;
        while minor > 0 {
            let version_string = format!("+0.{}", minor);
            let migrate_args = vec![&version_string, "migrate"];

            let migrate = Command::new(&veryl)
                .args(&migrate_args)
                .current_dir(&veryl_root)
                .output()?;
            if migrate.status.success() {
                migrate_success = true;
                break;
            }

            minor -= 1;
        }

        if migrate_success {
            while version.minor >= minor {
                let version_string = format!("+0.{}", minor);
                let migrate_args = vec![&version_string, "migrate"];

                let _ = Command::new(&veryl)
                    .args(&migrate_args)
                    .current_dir(&veryl_root)
                    .output()?;

                minor += 1;
            }
        }
    }

    Ok(())
}
