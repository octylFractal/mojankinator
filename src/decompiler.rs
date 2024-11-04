use crate::{MojError, MojResult, Version};
use error_stack::{Report, ResultExt};
use linked_hash_map::LinkedHashMap;
use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::LazyLock;

static PARCHMENT_VERSIONS: LazyLock<LinkedHashMap<&str, &str>> = LazyLock::new(|| {
    let mut map = LinkedHashMap::new();
    map.insert("1.16.5", "2022.03.06");
    map.insert("1.17.1", "2021.12.12");
    map.insert("1.18.2", "2022.11.06");
    map.insert("1.19.2", "2022.11.27");
    map.insert("1.19.3", "2023.06.25");
    map.insert("1.19.4", "2023.06.26");
    map.insert("1.20.1", "2023.09.03");
    map.insert("1.20.2", "2023.12.10");
    map.insert("1.20.3", "2023.12.31");
    map.insert("1.20.4", "2024.04.14");
    map.insert("1.20.6", "2024.06.16");
    map.insert("1.21", "2024.07.28");
    map
});

pub fn index_parchment_mc_versions(
    all_versions_sorted_by_date: &[Version],
) -> HashMap<String, Option<&'static str>> {
    let mut map = HashMap::new();
    let mut parchment_versions_iter = PARCHMENT_VERSIONS.deref().keys().copied();
    let mut next_parchment_version = parchment_versions_iter.next();
    let mut current_parchment_version = None;
    for version in all_versions_sorted_by_date {
        if Some(version.id.as_str()) == next_parchment_version {
            current_parchment_version = next_parchment_version;
            next_parchment_version = parchment_versions_iter.next();
        }
        map.insert(version.id.clone(), current_parchment_version);
    }
    if let Some(next) = next_parchment_version {
        panic!("Parchment MC version {} not found in version list", next);
    }
    map
}

#[derive(Debug)]
pub struct DecompileResult {
    artifacts: HashMap<DecompileArtifact, PathBuf>,
}

impl DecompileResult {
    pub fn artifacts(&self) -> &HashMap<DecompileArtifact, PathBuf> {
        &self.artifacts
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum DecompileArtifact {
    DecompiledClasses,
    LibrariesTxt,
}

impl DecompileArtifact {
    pub const fn all() -> &'static [DecompileArtifact] {
        &[
            DecompileArtifact::DecompiledClasses,
            DecompileArtifact::LibrariesTxt,
        ]
    }

    pub const fn description(&self) -> &'static str {
        match self {
            DecompileArtifact::DecompiledClasses => "decompiled classes",
            DecompileArtifact::LibrariesTxt => "libraries.txt",
        }
    }

    /// Bumped any time the artifact output changes in any way.
    pub const fn version(&self) -> u32 {
        match self {
            DecompileArtifact::DecompiledClasses => 2,
            DecompileArtifact::LibrariesTxt => 1,
        }
    }

    pub fn path_in_repository(&self) -> &str {
        match self {
            DecompileArtifact::DecompiledClasses => "src",
            DecompileArtifact::LibrariesTxt => "libraries.txt",
        }
    }
}

/// Decompiles the given version and returns the path to the decompiled source.
pub fn decompile_version(
    version: &Version,
    parchment_mc_version: Option<&str>,
    requested_artifacts: &[DecompileArtifact],
) -> MojResult<DecompileResult> {
    let work_dir = Path::new("./decompilationWorkArea/");

    std::fs::create_dir_all(work_dir)
        .change_context(MojError::Decompilation)
        .attach_printable("Cannot create decompilation work area")?;

    run_decompile_work(version, parchment_mc_version, requested_artifacts, work_dir)?;

    Ok(DecompileResult {
        artifacts: requested_artifacts
            .iter()
            .map(|&artifact| {
                (
                    artifact,
                    work_dir.join(match artifact {
                        DecompileArtifact::DecompiledClasses => "decompiledSources",
                        DecompileArtifact::LibrariesTxt => "build/libraries.txt",
                    }),
                )
            })
            .collect(),
    })
}

static HAS_STOPPED_DAEMON: AtomicBool = AtomicBool::new(false);

fn run_decompile_work(
    version: &Version,
    parchment_mc_version: Option<&str>,
    requested_artifacts: &[DecompileArtifact],
    work_dir: &Path,
) -> MojResult<()> {
    std::fs::write(
        work_dir.join("settings.gradle.kts"),
        include_bytes!("./settings.gradle.kts"),
    )
    .change_context(MojError::Decompilation)
    .attach_printable("Cannot write settings.gradle.kts")?;

    std::fs::write(
        work_dir.join("build.gradle.kts"),
        include_bytes!("./build.gradle.kts"),
    )
    .change_context(MojError::Decompilation)
    .attach_printable("Cannot write build.gradle.kts")?;

    std::fs::write(
        work_dir.join("gradle.properties"),
        format!(
            "
            minecraft_version={}
            parchment_mc_version={}
            parchment_version={}
            ",
            version.id,
            parchment_mc_version.unwrap_or(""),
            parchment_mc_version
                .map(|v| PARCHMENT_VERSIONS[v])
                .unwrap_or(""),
        )
        .as_bytes(),
    )
    .change_context(MojError::Decompilation)
    .attach_printable("Cannot write gradle.properties")?;

    if HAS_STOPPED_DAEMON
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_ok()
    {
        let status = std::process::Command::new("gradle")
            .args(["--stop"])
            .current_dir(work_dir)
            .status()
            .change_context(MojError::Decompilation)
            .attach_printable("Failed to stop Gradle daemon")?;
        if !status.success() {
            return Err(Report::new(MojError::Decompilation)
                .attach_printable("Failed to stop Gradle daemon"));
        }
    }

    let mut args = vec!["--stacktrace", "--parallel", "--configuration-cache"];
    for artifact in requested_artifacts {
        match artifact {
            DecompileArtifact::DecompiledClasses => {
                args.push("unpackSourcesIntoKnownDir");
            }
            DecompileArtifact::LibrariesTxt => {
                args.push("exportLibraries");
            }
        }
    }

    let status = std::process::Command::new("gradle")
        .args(args)
        .current_dir(work_dir)
        .status()
        .change_context(MojError::Decompilation)
        .attach_printable("Failed to execute decompilation")
        .attach_printable_lazy(|| format!("Version: {}", version.id))?;

    if status.success() {
        Ok(())
    } else {
        Err(Report::new(MojError::Decompilation)
            .attach_printable("Decompilation failed, see above output for details")
            .attach_printable(format!("Version: {}", version.id)))
    }
}
