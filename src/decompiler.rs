use crate::colorize::InfoColors;
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
            DecompileArtifact::DecompiledClasses => 3,
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
    let gradle_executable = fetch_gradle(work_dir)?;

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
        let status = std::process::Command::new(&gradle_executable)
            .args(["--stop"])
            .current_dir(work_dir)
            .status()
            .change_context(MojError::Decompilation)
            .attach_printable("Failed to stop Gradle daemon")
            .attach_printable_lazy(|| format!("Gradle executable: {:?}", &gradle_executable))?;
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

    let status = std::process::Command::new(&gradle_executable)
        .args(args)
        .current_dir(work_dir)
        .status()
        .change_context(MojError::Decompilation)
        .attach_printable("Failed to execute decompilation")
        .attach_printable_lazy(|| format!("Gradle executable: {:?}", &gradle_executable))
        .attach_printable_lazy(|| format!("Version: {}", version.id))?;

    if status.success() {
        Ok(())
    } else {
        Err(Report::new(MojError::Decompilation)
            .attach_printable("Decompilation failed, see above output for details")
            .attach_printable(format!("Version: {}", version.id)))
    }
}

fn fetch_gradle(work_dir: &Path) -> MojResult<PathBuf> {
    const GRADLE_VERSION: &str = "8.12";
    const GRADLE_RELATIVE_PATH: &str = "gradle-install";
    let relative_dir = work_dir.join(GRADLE_RELATIVE_PATH).join(GRADLE_VERSION);
    let gradle_dir = std::path::absolute(&relative_dir)
        .change_context(MojError::Decompilation)
        .attach_printable("Failed to make absolute Gradle directory")
        .attach_printable_lazy(|| format!("Path: {:?}", &relative_dir))?;
    let gradle_executable = gradle_dir.join("bin/gradle");
    if gradle_executable.exists() {
        eprintln!(
            "Found Gradle executable at {}",
            gradle_executable.display().as_important_value()
        );
        return Ok(gradle_executable);
    }
    eprintln!(
        "Downloading Gradle {}...",
        GRADLE_VERSION.as_important_value()
    );
    std::fs::create_dir_all(&gradle_dir)
        .change_context(MojError::Decompilation)
        .attach_printable("Cannot create Gradle directory")
        .attach_printable_lazy(|| format!("Path: {:?}", gradle_dir))?;
    let url = format!("https://services.gradle.org/distributions/gradle-{GRADLE_VERSION}-bin.zip");
    let zip_file_req = ureq::get(&url)
        .call()
        .change_context(MojError::Decompilation)
        .attach_printable("Failed to start Gradle zip download")
        .attach_printable_lazy(|| format!("URL: {}", url))?;
    let mut temp_file = tempfile::tempfile()
        .change_context(MojError::Decompilation)
        .attach_printable("Failed to create temporary file for Gradle zip")?;
    std::io::copy(&mut zip_file_req.into_body().into_reader(), &mut temp_file)
        .change_context(MojError::Decompilation)
        .attach_printable("Failed to download Gradle zip")?;
    {
        let mut zip = zip::ZipArchive::new(temp_file)
            .change_context(MojError::Decompilation)
            .attach_printable("Failed to open Gradle zip")?;
        zip.extract(&gradle_dir)
            .change_context(MojError::Decompilation)
            .attach_printable("Failed to extract Gradle zip")
            .attach_printable_lazy(|| format!("To: {:?}", gradle_dir))?;
    }
    // See if it just got unpacked directly
    if gradle_executable.exists() {
        return Ok(gradle_executable);
    }
    // Currently Gradle distributes with a directory at the top level of the zip, so we need to
    // find it and move it to the right place.
    let gradle_dir_contents = std::fs::read_dir(&gradle_dir)
        .change_context(MojError::Decompilation)
        .attach_printable("Failed to start reading Gradle directory contents")
        .attach_printable_lazy(|| format!("Path: {:?}", gradle_dir))?;
    let gradle_dir_contents: Vec<_> = gradle_dir_contents
        .collect::<Result<_, _>>()
        .change_context(MojError::Decompilation)
        .attach_printable("Failed to read Gradle directory contents")
        .attach_printable_lazy(|| format!("In dir: {:?}", gradle_dir))?;
    if gradle_dir_contents.len() != 1 {
        return Err(Report::new(MojError::Decompilation)
            .attach_printable("Unexpected Gradle directory contents")
            .attach_printable(format!("Contents: {:?}", gradle_dir_contents))
            .attach_printable(format!("In dir: {:?}", gradle_dir)));
    }
    let file_type = gradle_dir_contents[0]
        .file_type()
        .change_context(MojError::Decompilation)
        .attach_printable("failed to read file type of Gradle directory entry")?;
    if !file_type.is_dir() {
        return Err(Report::new(MojError::Decompilation)
            .attach_printable("Unexpected Gradle directory entry")
            .attach_printable(format!("{:?} should be dir", file_type))
            .attach_printable(format!("{:?}", gradle_dir_contents[0].path())));
    }
    let subdir_contents = std::fs::read_dir(gradle_dir_contents[0].path())
        .change_context(MojError::Decompilation)
        .attach_printable("Failed to start reading Gradle subdirectory contents")
        .attach_printable_lazy(|| format!("Path: {:?}", gradle_dir_contents[0].path()))?;
    for entry in subdir_contents {
        let entry = entry
            .change_context(MojError::Decompilation)
            .attach_printable("Failed to read Gradle subdirectory entry")
            .attach_printable_lazy(|| format!("In dir: {:?}", gradle_dir_contents[0].path()))?;
        let source = entry.path();
        let dest = gradle_dir.join(entry.file_name());
        std::fs::rename(&source, &dest)
            .change_context(MojError::Decompilation)
            .attach_printable("Failed to move Gradle subdirectory entry")
            .attach_printable_lazy(|| format!("From: {:?}", source))
            .attach_printable_lazy(|| format!("To: {:?}", dest))?;
    }
    std::fs::remove_dir(gradle_dir_contents[0].path())
        .change_context(MojError::Decompilation)
        .attach_printable("Failed to remove Gradle subdirectory")
        .attach_printable_lazy(|| format!("Path: {:?}", gradle_dir_contents[0].path()))?;
    if !gradle_executable.exists() {
        return Err(Report::new(MojError::Decompilation)
            .attach_printable("Gradle executable not found after extraction")
            .attach_printable(format!("Path: {:?}", gradle_executable)));
    }
    Ok(gradle_executable)
}
