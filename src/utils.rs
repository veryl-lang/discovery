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

        let build = Command::new(&info.veryl)
            .args(&build_args)
            .current_dir(&info.veryl_root)
            .output()?;
        Ok(build.status.success())
    }
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
