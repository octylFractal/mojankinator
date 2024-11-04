use crate::{MojError, MojResult, SavedInfo, Version};
use error_stack::{Report, ResultExt};
use git2::{Index, IndexEntry, IndexTime, Oid, Repository, Signature, Time};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub struct MojRepository {
    git_repo: Repository,
}

impl MojRepository {
    pub fn init(repo_path: &Path) -> MojResult<Self> {
        let git_repo = Repository::init(repo_path).change_context(MojError::OpenGitRepo)?;
        Ok(Self { git_repo })
    }

    pub fn open(repo_path: &Path) -> MojResult<Self> {
        let git_repo = Repository::open(repo_path).change_context(MojError::OpenGitRepo)?;
        Ok(Self { git_repo })
    }

    fn version_reference(version_id: &str) -> String {
        format!("refs/tags/{}", version_id)
    }

    /// Get the info of the commit tagged with the version id, if it exists.
    pub fn find_version_tree_and_info(&self, version_id: &str) -> Option<(Oid, SavedInfo)> {
        let commit = self
            .git_repo
            .find_reference(&Self::version_reference(version_id))
            .ok()?
            .peel_to_commit()
            .expect("Tag should point to a commit");
        let oid = commit.tree().expect("Commit should have a tree").id();
        let message = commit.message().expect("Commit should have a message");
        let saved_info = match message.split_once("\n\n") {
            Some((_, info)) => toml::from_str(info).expect("Info should be deserializable"),
            None => SavedInfo::default(),
        };
        Some((oid, saved_info))
    }

    pub fn clear_branch(&self) -> MojResult<()> {
        let head_ref = match self.git_repo.head() {
            Ok(head) => head,
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
                // This branch is already clear
                return Ok(());
            }
            Err(e) => return Err(e).change_context(MojError::Reset),
        };
        let current_branch = head_ref
            .shorthand()
            .expect("HEAD should be a branch")
            .to_string();
        self.git_repo
            .set_head_detached(head_ref.resolve().unwrap().target().unwrap())
            .change_context(MojError::Reset)
            .attach_printable("Cannot detach HEAD")?;
        self.git_repo
            .find_branch(&current_branch, git2::BranchType::Local)
            .change_context(MojError::Reset)
            .attach_printable("HEAD should be a branch")?
            .delete()
            .change_context(MojError::Reset)
            .attach_printable("Cannot delete branch")?;
        self.git_repo
            .set_head(&format!("refs/heads/{}", current_branch))
            .change_context(MojError::Reset)
            .attach_printable("Cannot set HEAD to branch")?;
        Ok(())
    }

    pub fn create_tree(
        &self,
        base: Option<TreeBase>,
        source_files: &[SourcePath],
    ) -> MojResult<Oid> {
        let mut index = self.git_repo.index().change_context(MojError::Commit)?;

        index
            .clear()
            .change_context(MojError::Commit)
            .attach_printable("Cannot clear index")?;

        if let Some(base) = base {
            let base_tree = self
                .git_repo
                .find_tree(base.tree)
                .change_context(MojError::Commit)
                .attach_printable("Cannot find base tree")?;
            index
                .read_tree(&base_tree)
                .change_context(MojError::Commit)?;
            let mut pathspecs = Vec::with_capacity(1 + base.paths_to_include.len());
            // Don't match the paths to include
            for path in &base.paths_to_include {
                pathspecs.push(format!("!{}", path));
            }
            // Include all other paths
            pathspecs.push("*".to_string());
            eprintln!("Pathspecs: {:?}", pathspecs);
            index
                .remove_all(pathspecs, None)
                .change_context(MojError::Commit)
                .attach_printable("Cannot remove paths from index")?;
        }

        for SourcePath { root, repo_root } in source_files {
            if root.is_file() {
                add_file_to_index(&mut index, root.parent().unwrap(), repo_root.as_str(), root)?;
            } else {
                for entry in walkdir::WalkDir::new(root) {
                    let entry = entry.change_context(MojError::Commit)?;
                    if entry.file_type().is_file() {
                        add_file_to_index(&mut index, root, repo_root.as_str(), entry.path())?;
                    } else if entry.file_type().is_dir() {
                        // Skip directories
                    } else {
                        return Err(Report::new(MojError::Commit)
                            .attach_printable("Unknown file type, cannot copy")
                            .attach_printable(format!("File type: {:?}", entry.file_type()))
                            .attach_printable(format!("Path: {:?}", entry.path())));
                    }
                }
            }
        }

        index.write_tree().change_context(MojError::Commit)
    }

    pub fn commit_and_tag(
        &self,
        version: &Version,
        saved_info: &SavedInfo,
        tree: &Oid,
    ) -> MojResult<()> {
        let author = &self
            .git_repo
            .signature()
            .change_context(MojError::Commit)
            .attach_printable("Cannot find user to commit with")?;
        // Correct the signature with the release time
        let author = Signature::new(
            author.name().unwrap(),
            author.email().unwrap(),
            &Time::new(version.release_time.timestamp(), 0),
        )
        .unwrap();
        let parent = match self.git_repo.head() {
            Ok(head) => Some(head.peel_to_commit().unwrap()),
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => None,
            Err(e) => return Err(e).change_context(MojError::Commit),
        };

        let commit = self
            .git_repo
            .commit(
                Some("HEAD"),
                &author,
                &author,
                &format!(
                    "Version {}\n\n{}",
                    version.id,
                    toml::to_string(saved_info)
                        .change_context(MojError::Commit)
                        .attach_printable("Failed to serialize commit info")?
                ),
                &self.git_repo.find_tree(*tree).unwrap(),
                parent.as_ref().as_slice(),
            )
            .change_context(MojError::Commit)?;

        self.git_repo
            .tag(
                &version.id,
                &self.git_repo.find_commit(commit).unwrap().into_object(),
                &author,
                &version.id,
                true,
            )
            .change_context(MojError::Tag)?;

        Ok(())
    }

    pub fn reset(&self) -> MojResult<()> {
        self.git_repo
            .reset(
                &self
                    .git_repo
                    .head()
                    .change_context(MojError::Reset)
                    .attach_printable("Cannot find HEAD")?
                    .peel_to_commit()
                    .change_context(MojError::Reset)
                    .attach_printable("HEAD should point to a commit")?
                    .into_object(),
                git2::ResetType::Hard,
                None,
            )
            .change_context(MojError::Reset)?;
        Ok(())
    }
}

fn add_file_to_index(
    index: &mut Index,
    root: &Path,
    repo_root: &str,
    file: &Path,
) -> MojResult<()> {
    let stat = file
        .metadata()
        .change_context(MojError::Commit)
        .attach_printable_lazy(|| format!("Path: {:?}", file))?;
    assert!(stat.is_file(), "Only files can be added to the index");
    let index_entry = IndexEntry {
        ctime: IndexTime::new(
            stat.ctime().try_into().unwrap(),
            stat.ctime_nsec().try_into().unwrap(),
        ),
        mtime: IndexTime::new(
            stat.mtime().try_into().unwrap(),
            stat.mtime_nsec().try_into().unwrap(),
        ),
        dev: stat.dev().try_into().unwrap(),
        ino: stat.ino().try_into().unwrap(),
        mode: stat.mode(),
        uid: stat.uid(),
        gid: stat.gid(),
        file_size: stat.size().try_into().unwrap(),
        id: Oid::hash_file(git2::ObjectType::Blob, file)
            .change_context(MojError::Commit)
            .attach_printable_lazy(|| format!("Path: {:?}", file))?,
        flags: 0,
        flags_extended: 0,
        path: Path::new(repo_root)
            .join(file.strip_prefix(root).unwrap())
            .as_os_str()
            .as_bytes()
            .to_vec(),
    };
    let file_contents = std::fs::read(file)
        .change_context(MojError::Commit)
        .attach_printable_lazy(|| format!("Path: {:?}", file))?;
    index
        .add_frombuffer(&index_entry, &file_contents)
        .change_context(MojError::Commit)
        .attach_printable_lazy(|| format!("Path: {:?}", file))
}

#[derive(Debug)]
pub struct TreeBase {
    /// The tree to base the new tree on
    pub tree: Oid,
    /// Paths to include in the new tree
    pub paths_to_include: Vec<String>,
}

#[derive(Debug)]
pub struct SourcePath {
    pub root: PathBuf,
    pub repo_root: String,
}
