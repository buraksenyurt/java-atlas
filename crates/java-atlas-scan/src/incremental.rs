//! Keeping facts up to date without a full scan (phase 8).
//!
//! Facts are per file: a `SourceFileFacts` depends on its own file and on
//! nothing else. When a `.java` file changes, re-parsing that one file and
//! swapping its facts gives exactly what a full scan would. Build files are
//! different: a changed `pom.xml` can move source roots, add modules or
//! change versions, so it asks for a full scan.
//!
//! The work is split in two steps:
//!
//! 1. [`plan`] — pure: sorts changed paths into "re-parse these" and
//!    "scan everything again", touching the disk only to ask whether a path
//!    is a folder.
//! 2. [`apply`] — reads and parses the planned files and patches the facts.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use java_atlas_core::facts::{BuildSystem, ProjectFacts};

use crate::paths::{normalize, relative_slash};
use crate::sources::{Job, java_files, module_roots, parse_jobs, plain_is_test};
use crate::{ScanError, java_parser};

/// Files whose change can reshape the project: modules, source roots,
/// dependencies, JPMS borders. They always trigger a full scan.
const STRUCTURE_FILES: [&str; 7] = [
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "libs.versions.toml",
    "module-info.java",
];

/// Folders that hold tool output, never sources. Maven even copies `pom.xml`
/// into `target/classes/META-INF/maven/…`, which must not trigger a rescan.
const IGNORED_DIRS: [&str; 8] = [
    "target",
    "build",
    "out",
    ".gradle",
    ".git",
    ".idea",
    ".vscode",
    "node_modules",
];

/// What changed at one path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceChange {
    /// A `.java` file was created, modified or deleted.
    File { path: PathBuf, module: usize },
    /// A folder inside a source root appeared or disappeared (a package was
    /// renamed, copied in or deleted). Some platforms report only the folder,
    /// not every file in it, so the whole subtree is re-read.
    Tree { path: PathBuf, module: usize },
}

impl SourceChange {
    pub fn path(&self) -> &Path {
        match self {
            SourceChange::File { path, .. } | SourceChange::Tree { path, .. } => path,
        }
    }
}

/// The verdict on a batch of changed paths.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The first path that requires a full scan, if any. When set,
    /// `sources` is irrelevant.
    pub full_scan: Option<PathBuf>,
    /// Source changes, without duplicates, in a stable order.
    pub sources: Vec<SourceChange>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.full_scan.is_none() && self.sources.is_empty()
    }
}

/// Sorts `changed` (absolute paths, as a file watcher reports them) into a
/// plan. Paths outside every source root and outside the build files are
/// ignored: editor backups, compiled classes, notes, images.
pub fn plan(root: &Path, facts: &ProjectFacts, changed: &[PathBuf]) -> Plan {
    let root = normalize(root);
    let mut sources = BTreeSet::new();
    for path in changed {
        let path = normalize(path);
        let Ok(rel) = path.strip_prefix(&root) else {
            continue;
        };
        if is_structure_file(rel) {
            return Plan {
                full_scan: Some(path),
                sources: Vec::new(),
            };
        }
        if let Some(change) = source_change(&root, facts, &path) {
            sources.insert(change);
        }
    }
    Plan {
        full_scan: None,
        sources: sources.into_iter().collect(),
    }
}

fn in_ignored_dir(rel: &Path) -> bool {
    rel.components()
        .any(|c| IGNORED_DIRS.iter().any(|d| c.as_os_str() == *d))
}

fn is_structure_file(rel: &Path) -> bool {
    rel.file_name()
        .is_some_and(|n| STRUCTURE_FILES.iter().any(|f| n == *f))
        && !in_ignored_dir(rel)
}

/// The module and test flag of a path inside a source root, if it is one.
fn locate(root: &Path, facts: &ProjectFacts, path: &Path) -> Option<(usize, bool)> {
    if facts.build_system == BuildSystem::Plain {
        let rel = path.strip_prefix(root).ok()?;
        // Plain projects scan the whole tree: only tool folders are excluded.
        return (!in_ignored_dir(rel)).then(|| (0, plain_is_test(&relative_slash(root, path))));
    }
    // The longest matching root wins: `src/main/java` inside a module
    // whose own folder is inside another module's folder.
    facts
        .modules
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.aggregator)
        .flat_map(|(index, m)| {
            module_roots(m)
                .into_iter()
                .map(move |(source_root, is_test)| {
                    (
                        index,
                        is_test,
                        normalize(&root.join(&m.path).join(source_root)),
                    )
                })
        })
        .filter(|(_, _, dir)| path.starts_with(dir) && path != dir)
        .max_by_key(|(_, _, dir)| dir.components().count())
        .map(|(index, is_test, _)| (index, is_test))
}

fn source_change(root: &Path, facts: &ProjectFacts, path: &Path) -> Option<SourceChange> {
    let (module, _) = locate(root, facts, path)?;
    let path = path.to_path_buf();
    if path.extension().is_some_and(|e| e == "java") {
        Some(SourceChange::File { path, module })
    } else if path.is_dir() || (!path.exists() && path.extension().is_none()) {
        // A folder, or something gone that looked like one.
        Some(SourceChange::Tree { path, module })
    } else {
        None
    }
}

/// What [`apply`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SourceUpdate {
    /// Files parsed again (created or modified).
    pub parsed: usize,
    /// Files whose facts were dropped because the file is gone.
    pub removed: usize,
    /// Every relative path that was parsed or removed, sorted.
    pub files: Vec<String>,
}

/// Re-reads the planned source changes and patches `facts` in place.
/// Afterwards `facts` equals what a full scan would produce, except for
/// the order of warnings.
pub fn apply(
    root: &Path,
    facts: &mut ProjectFacts,
    changes: &[SourceChange],
) -> Result<SourceUpdate, ScanError> {
    java_parser()?;
    let mut before: HashSet<String> = HashSet::new();
    let mut jobs: Vec<Job> = Vec::new();
    let mut touched: BTreeSet<usize> = BTreeSet::new();

    for change in changes {
        let rel = relative_slash(root, change.path());
        match change {
            SourceChange::File { path, module } => {
                before.extend(remove(facts, |p| p == rel));
                if path.is_file() {
                    jobs.push(job(root, facts, *module, path.clone()));
                }
                touched.insert(*module);
            }
            SourceChange::Tree { path, module } => {
                let prefix = format!("{rel}/");
                before.extend(remove(facts, |p| p.starts_with(&prefix)));
                if path.is_dir() {
                    for file in java_files(path) {
                        jobs.push(job(root, facts, *module, file));
                    }
                }
                touched.insert(*module);
            }
        }
    }

    // A file reported twice (a file event and its folder's) is parsed once.
    let mut seen = HashSet::new();
    jobs.retain(|j| seen.insert(j.path.clone()));

    let mut after: HashSet<String> = HashSet::new();
    for outcome in parse_jobs(root, &jobs, &|_| {}) {
        facts.warnings.extend(outcome.warnings);
        let module = &mut facts.modules[outcome.module];
        if let Some(file) = outcome.file {
            after.insert(file.path.clone());
            module.files.push(file);
        }
        // module-info.java is a structure file: `plan` sends it to a full scan.
    }

    for &index in &touched {
        sort_like_a_full_scan(root, facts, index);
    }

    let mut files: Vec<String> = before.union(&after).cloned().collect();
    files.sort();
    Ok(SourceUpdate {
        parsed: after.len(),
        removed: before.difference(&after).count(),
        files,
    })
}

fn job(root: &Path, facts: &ProjectFacts, module: usize, path: PathBuf) -> Job {
    let is_test = locate(&normalize(root), facts, &normalize(&path)).is_some_and(|(_, t)| t);
    Job {
        module,
        path,
        is_test,
    }
}

/// Drops the facts of every file whose relative path matches, and the
/// warnings about them. Returns the dropped paths.
fn remove(facts: &mut ProjectFacts, matches: impl Fn(&str) -> bool) -> Vec<String> {
    let mut dropped = Vec::new();
    for module in &mut facts.modules {
        module.files.retain(|f| {
            let hit = matches(&f.path);
            if hit {
                dropped.push(f.path.clone());
            }
            !hit
        });
    }
    facts.warnings.retain(|w| !matches(&w.file));
    dropped
}

/// A full scan lists a module's files source root by source root, each
/// root's files sorted by path. New files are put in the same place.
fn sort_like_a_full_scan(root: &Path, facts: &mut ProjectFacts, index: usize) {
    let module = &facts.modules[index];
    let roots: Vec<String> = if facts.build_system == BuildSystem::Plain {
        vec![String::new()]
    } else {
        module_roots(module)
            .into_iter()
            .map(|(source_root, _)| {
                relative_slash(root, &root.join(&module.path).join(source_root))
            })
            .collect()
    };
    let rank = |path: &str| {
        roots
            .iter()
            .position(|r| r.is_empty() || path.starts_with(&format!("{r}/")))
            .unwrap_or(usize::MAX)
    };
    facts.modules[index].files.sort_by(|a, b| {
        (rank(&a.path), Path::new(&a.path)).cmp(&(rank(&b.path), Path::new(&b.path)))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_folders_are_not_structure() {
        assert!(is_structure_file(Path::new("bank-api/pom.xml")));
        assert!(!is_structure_file(Path::new(
            "bank-api/target/classes/META-INF/maven/g/a/pom.xml"
        )));
        assert!(is_structure_file(Path::new(
            "bank-api/src/main/java/module-info.java"
        )));
        assert!(!is_structure_file(Path::new("bank-api/README.md")));
    }
}
