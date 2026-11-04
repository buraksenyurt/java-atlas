//! Java sources → [`SourceFileFacts`] and [`JpmsFacts`], in parallel.
//!
//! Every file is parsed independently of every other file, which makes this
//! the textbook case for data parallelism: collect the work list, hand it to
//! `rayon`, gather the results in the original order.

mod java;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use java_atlas_core::facts::{
    BuildSystem, JpmsFacts, ModuleFacts, ProjectFacts, ScanWarning, SourceFileFacts,
};
use rayon::prelude::*;

use crate::paths::relative_slash;
use crate::{ScanError, java_parser};

/// Reported after each parsed file; the UI turns it into a progress bar (phase 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

/// One file to parse.
pub(crate) struct Job {
    pub module: usize,
    pub path: PathBuf,
    pub is_test: bool,
}

/// What parsing one file produced.
pub(crate) struct Outcome {
    pub module: usize,
    pub file: Option<SourceFileFacts>,
    pub jpms: Option<(JpmsFacts, bool)>,
    pub warnings: Vec<ScanWarning>,
}

/// Standard Maven/Gradle layout, used when the build file declares nothing else.
/// Maven `sourceDirectory` is honoured; Gradle `sourceSets` are not read.
const SOURCE_ROOTS: [(&str, bool); 2] = [("src/main/java", false), ("src/test/java", true)];

/// The source roots of a module, relative to its folder, with their test flag,
/// in the order they are scanned. The order matters: it is the order of the
/// module's files, which incremental updates (phase 8) must reproduce.
pub(crate) fn module_roots(module: &ModuleFacts) -> Vec<(String, bool)> {
    if module.source_roots.is_empty() {
        SOURCE_ROOTS
            .iter()
            .map(|(p, t)| ((*p).to_owned(), *t))
            .collect()
    } else {
        module
            .source_roots
            .iter()
            .map(|r| (r.path.clone(), r.is_test))
            .collect()
    }
}

/// Plain projects have no source roots: a folder named `test` or `tests`
/// anywhere in the path marks test code.
pub(crate) fn plain_is_test(rel: &str) -> bool {
    rel.split('/').any(|seg| seg == "test" || seg == "tests")
}

/// All `.java` files below `dir`, sorted, honouring `.gitignore`.
pub(crate) fn java_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = ignore::WalkBuilder::new(dir)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(ignore::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|ext| ext == "java"))
        .collect();
    // The walk order depends on the file system; sorting makes results stable.
    files.sort();
    files
}

/// Builds the work list and warns about modules without a source root.
fn jobs(root: &Path, facts: &mut ProjectFacts) -> Vec<Job> {
    let mut jobs = Vec::new();

    if facts.build_system == BuildSystem::Plain {
        for path in java_files(root) {
            let is_test = plain_is_test(&relative_slash(root, &path));
            jobs.push(Job {
                module: 0,
                path,
                is_test,
            });
        }
        return jobs;
    }

    for (index, module) in facts.modules.iter().enumerate() {
        if module.aggregator {
            continue;
        }
        let dir = root.join(&module.path);
        let mut found_root = false;
        for (source_root, is_test) in module_roots(module) {
            let source_dir = dir.join(&source_root);
            if source_dir.is_dir() {
                found_root = true;
                jobs.extend(java_files(&source_dir).into_iter().map(|path| Job {
                    module: index,
                    path,
                    is_test,
                }));
            }
        }
        if !found_root {
            facts.warnings.push(ScanWarning {
                file: module.build_file.clone(),
                message: format!("{}: no source folder found", module.name),
            });
        }
    }
    jobs
}

/// Reads a file as UTF-8, falling back to a lossy decode (Windows-1254 files
/// still turn up in older Turkish code bases).
fn read_source(path: &Path, rel: &str, warnings: &mut Vec<ScanWarning>) -> Option<String> {
    match fs::read(path) {
        Ok(bytes) => Some(match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => {
                warnings.push(ScanWarning {
                    file: rel.to_owned(),
                    message: "not valid UTF-8; decoded lossily".into(),
                });
                String::from_utf8_lossy(e.as_bytes()).into_owned()
            }
        }),
        Err(e) => {
            warnings.push(ScanWarning {
                file: rel.to_owned(),
                message: format!("cannot read: {e}"),
            });
            None
        }
    }
}

pub(crate) fn parse_one(parser: &mut tree_sitter::Parser, root: &Path, job: &Job) -> Outcome {
    let rel = relative_slash(root, &job.path);
    let mut out = Outcome {
        module: job.module,
        file: None,
        jpms: None,
        warnings: Vec::new(),
    };

    let Some(src) = read_source(&job.path, &rel, &mut out.warnings) else {
        return out;
    };
    let Some(tree) = java::parse(parser, &src) else {
        out.warnings.push(ScanWarning {
            file: rel,
            message: "parser returned no tree".into(),
        });
        return out;
    };

    if job
        .path
        .file_name()
        .is_some_and(|n| n == "module-info.java")
    {
        match java::module_info(&tree, &src, rel.clone()) {
            Some(jpms) => out.jpms = Some((jpms, job.is_test)),
            None => out.warnings.push(ScanWarning {
                file: rel,
                message: "module-info.java without a module declaration".into(),
            }),
        }
        return out;
    }

    let facts = java::file_facts(&tree, &src, rel.clone(), job.is_test);
    if facts.has_errors {
        out.warnings.push(ScanWarning {
            file: rel,
            message: "syntax errors; facts may be incomplete".into(),
        });
    }
    out.file = Some(facts);
    out
}

/// Parses every source file of the modules in `facts` and stores the results
/// in them. `on_progress` is called from worker threads, hence `Sync`.
pub fn read_sources(
    root: &Path,
    facts: &mut ProjectFacts,
    on_progress: &(dyn Fn(Progress) + Sync),
) -> Result<(), ScanError> {
    // Fail early (and once) if the grammar cannot be loaded; inside the
    // parallel section the parser creation can then not fail.
    java_parser()?;

    let jobs = jobs(root, facts);
    let outcomes = parse_jobs(root, &jobs, on_progress);

    for outcome in outcomes {
        facts.warnings.extend(outcome.warnings);
        let module = &mut facts.modules[outcome.module];
        if let Some(file) = outcome.file {
            module.files.push(file);
        }
        if let Some((jpms, is_test)) = outcome.jpms {
            // A test source set may carry its own module-info; the main one wins.
            if !is_test || module.jpms.is_none() {
                module.jpms = Some(jpms);
            }
        }
    }
    Ok(())
}

/// Parses `jobs` in parallel; the outcomes keep the order of `jobs`.
/// The grammar must already be known to load (see [`java_parser`]).
pub(crate) fn parse_jobs(
    root: &Path,
    jobs: &[Job],
    on_progress: &(dyn Fn(Progress) + Sync),
) -> Vec<Outcome> {
    let total = jobs.len();
    let done = AtomicUsize::new(0);
    on_progress(Progress { done: 0, total });

    jobs.par_iter()
        // A `Parser` is not shareable between threads, and creating one per
        // file is wasteful. `map_init` gives each worker its own, reused.
        .map_init(
            || java_parser().expect("grammar was loaded above"),
            |parser, job| {
                let outcome = parse_one(parser, root, job);
                let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                on_progress(Progress {
                    done: finished,
                    total,
                });
                outcome
            },
        )
        .collect() // `collect` keeps the order of `jobs`.
}
