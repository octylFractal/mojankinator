mod colorize;
mod decompiler;
mod repository;

use crate::colorize::InfoColors;
use crate::decompiler::{decompile_version, DecompileArtifact};
use crate::repository::{MojRepository, SourcePath, TreeBase};
use chrono::{DateTime, Datelike, Utc};
use error_stack::{Report, ResultExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
enum MojError {
    #[error("User error")]
    UserError,
    #[error("Failed to read config file")]
    ReadConfig,
    #[error("Failed to parse config file")]
    ParseConfig,
    #[error("Failed to fetch version manifest")]
    FetchVersionManifest,
    #[error("Failed to open git repository")]
    OpenGitRepo,
    #[error("Failed to decompile version")]
    Decompilation,
    #[error("Failed to add files and commit new version")]
    Commit,
    #[error("Failed to tag new version")]
    Tag,
    #[error("Failed to reset repository")]
    Reset,
}

type MojResult<T> = error_stack::Result<T, MojError>;

fn main() -> MojResult<()> {
    let config = Config::load()?;
    let spinner = indicatif::ProgressBar::new_spinner();
    spinner.suspend(|| {
        eprintln!(
            "Minimum version: {}",
            config.min_version.as_important_value()
        );
        eprintln!(
            "Maximum version: {}",
            config.max_version.as_important_value()
        );
        eprintln!(
            "Include snapshots: {}",
            config.include_snapshots.as_important_value()
        );
    });
    spinner.set_message("Fetching version manifest...");
    let mut all_versions =
        ureq::get("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json")
            .call()
            .change_context(MojError::FetchVersionManifest)?
            .into_body()
            .read_json::<VersionManifest>()
            .change_context(MojError::FetchVersionManifest)?
            .versions;

    let extracted_release_times = all_versions
        .iter()
        .fold((None, None), |(min, max), version| {
            if version.id == config.min_version {
                (Some(version.release_time), max)
            } else if version.id == config.max_version {
                (min, Some(version.release_time))
            } else {
                (min, max)
            }
        });
    let (min_release_time, max_release_time) =
        verify_release_times(&config, extracted_release_times)?;

    spinner.set_message("Sorting versions...");
    all_versions.sort_by_key(|version| version.release_time);

    let mut versions = all_versions.clone();

    spinner.set_message("Filtering versions...");
    versions.retain(|version| {
        let is_snapshot = version.type_ == "snapshot";
        let is_within_range =
            version.release_time >= min_release_time && version.release_time <= max_release_time;
        !is_april_fools(version) && is_within_range && (config.include_snapshots || !is_snapshot)
    });

    spinner.finish_and_clear();
    eprintln!("Found {} versions", versions.len().as_important_value());

    let repo_path = Path::new("./repository");
    let repo = if repo_path.exists() {
        eprintln!("Opening repository...");
        MojRepository::open(repo_path)?
    } else {
        eprintln!("Creating repository...");
        std::fs::create_dir(repo_path).change_context(MojError::OpenGitRepo)?;
        MojRepository::init(repo_path)?
    };

    let parchment_versions = decompiler::index_parchment_mc_versions(&all_versions);

    let versions_to_tree: HashMap<_, _> = versions
        .iter()
        .filter_map(|version| {
            Some((
                version.id.clone(),
                repo.find_version_tree_and_info(&version.id)?,
            ))
        })
        .collect();

    // Now that we have all the trees, rewind the branch to initial state.
    eprintln!("Clearing branch to rebuild...");
    repo.clear_branch()?;

    let progress_bar = indicatif::ProgressBar::new(versions.len() as u64)
        .with_style(indicatif::ProgressStyle::default_bar().template(
            "Version progress: {bar:40.white/blue} {pos:.cyan}/{len:.cyan} (running {elapsed_precise}, ETA {eta})",
        ).unwrap());

    for version in &versions {
        progress_bar.tick();
        eprintln!(); // Force the progress bar to be printed to console permanently.
        progress_bar.suspend(|| -> MojResult<()> {
            eprintln!("Checking version {}...", version.id.as_important_value());
            let mut tree_base = None;
            let mut existing_info = SavedInfo::default();
            if let Some((tree, info)) = versions_to_tree.get(&version.id) {
                if info.is_current() {
                    eprintln!(
                        "Version {} already processed.",
                        version.id.as_important_value()
                    );
                    repo.commit_and_tag(version, info, tree)?;
                    return Ok(());
                } else {
                    tree_base = Some(TreeBase {
                        tree: *tree,
                        paths_to_include: Vec::new(),
                    });
                    existing_info = info.clone();
                }
            }

            let mut artifacts_needed = Vec::new();
            for artifact in DecompileArtifact::all().iter().copied() {
                if existing_info.get_artifact_version(artifact) < artifact.version() {
                    eprintln!(
                        "Requesting {} for version {}.",
                        artifact.description().as_important_value(),
                        version.id.as_important_value()
                    );
                    artifacts_needed.push(artifact);
                } else if let Some(base) = tree_base.as_mut() {
                    base.paths_to_include
                        .push(artifact.path_in_repository().to_string());
                }
            }

            let result =
                decompile_version(version, parchment_versions[&version.id], &artifacts_needed)?;
            eprintln!(
                "Decompiled version {}, adding to repository...",
                version.id.as_important_value()
            );
            let tree = repo.create_tree(
                tree_base,
                &result
                    .artifacts()
                    .iter()
                    .map(|(artifact, root)| SourcePath {
                        root: root.to_path_buf(),
                        repo_root: artifact.path_in_repository().to_string(),
                    })
                    .collect::<Vec<_>>(),
            )?;
            repo.commit_and_tag(version, &SavedInfo::current(), &tree)?;
            eprintln!("Committed and tagged {}", version.id.as_important_value());
            Ok(())
        })?;
        progress_bar.inc(1);
    }

    // Do a reset to ensure that the repository is clean
    repo.reset()?;

    eprintln!("All versions added");

    Ok(())
}

fn is_april_fools(version: &Version) -> bool {
    version.release_time.month() == chrono::Month::April.number_from_month()
        && version.release_time.day() == 1
}

fn verify_release_times(
    config: &Config,
    extracted_release_times: (Option<DateTime<Utc>>, Option<DateTime<Utc>>),
) -> MojResult<(DateTime<Utc>, DateTime<Utc>)> {
    match extracted_release_times {
        (Some(min), Some(max)) => Ok((min, max)),
        (None, Some(_)) => Err(Report::new(MojError::UserError).attach_printable(format!(
            "Minimum version {} not found in version manifest",
            config.min_version
        ))),
        (Some(_), None) => Err(Report::new(MojError::UserError).attach_printable(format!(
            "Maximum version {} not found in version manifest",
            config.max_version
        ))),
        (None, None) => Err(Report::new(MojError::UserError).attach_printable(format!(
            "Neither minimum version {} nor maximum version {} found in version manifest",
            config.min_version, config.max_version
        ))),
    }
}

#[derive(Deserialize, Debug)]
pub struct VersionManifest {
    pub versions: Vec<Version>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Version {
    pub id: String,
    #[serde(rename = "releaseTime")]
    pub release_time: DateTime<Utc>,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Debug, Deserialize)]
struct Config {
    min_version: String,
    max_version: String,
    #[serde(default)]
    include_snapshots: bool,
}

impl Config {
    fn load() -> MojResult<Self> {
        let config_path = Path::new("./config.toml");
        let config = std::fs::read_to_string(config_path)
            .change_context(MojError::ReadConfig)
            .attach_printable_lazy(|| format!("Path: {:?}", config_path))?;
        toml::from_str(&config)
            .change_context(MojError::ParseConfig)
            .attach_printable_lazy(|| format!("Path: {:?}", config_path))
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SavedInfo {
    #[serde(default)]
    #[serde(alias = "output_version")]
    decompiled_classes_version: u32,
    #[serde(default)]
    libraries_output_version: u32,
}

impl SavedInfo {
    pub fn current() -> Self {
        Self {
            decompiled_classes_version: DecompileArtifact::DecompiledClasses.version(),
            libraries_output_version: DecompileArtifact::LibrariesTxt.version(),
        }
    }

    pub fn get_artifact_version(&self, artifact: DecompileArtifact) -> u32 {
        match artifact {
            DecompileArtifact::DecompiledClasses => self.decompiled_classes_version,
            DecompileArtifact::LibrariesTxt => self.libraries_output_version,
        }
    }

    pub fn is_current(&self) -> bool {
        self.decompiled_classes_version >= DecompileArtifact::DecompiledClasses.version()
            && self.libraries_output_version >= DecompileArtifact::LibrariesTxt.version()
    }
}
