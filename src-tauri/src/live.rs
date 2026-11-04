//! From a batch of changed paths to a new atlas (phase 8).
//!
//! No Tauri in here, so the whole decision ("patch or rescan?") is tested
//! with plain unit tests; `commands` only wires it to the watcher and to
//! the `atlas-changed` event.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Instant;

use java_atlas_core::facts::ProjectFacts;
use java_atlas_scan::incremental;

use crate::dto::LiveEvent;
use crate::error::AppError;
use crate::project::{self, OpenProject};
use crate::watch::Batch;

/// At most this many file names travel to the UI; the count is exact.
const MAX_FILES: usize = 20;

/// Applies `batch` to `current`. `Ok(None)` means nothing relevant changed
/// (a class file, an editor backup, a README).
pub fn refresh(
    current: &OpenProject,
    batch: &Batch,
) -> Result<Option<(OpenProject, LiveEvent)>, AppError> {
    let started = Instant::now();
    let root = &current.root;
    let paths: Vec<PathBuf> = batch.paths.iter().cloned().collect();
    let plan = incremental::plan(root, &current.facts, &paths);

    if batch.rescan || plan.full_scan.is_some() {
        let project = project::scan(root, &|_| {}, &current.hint())?;
        let trigger = plan.full_scan.map(|p| {
            p.strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/")
        });
        let event = LiveEvent::Updated {
            summary: Box::new(project.summary.clone()),
            full: true,
            trigger,
            files: Vec::new(),
            file_count: 0,
            changed_keys: Vec::new(),
            ms: started.elapsed().as_millis() as u32,
        };
        return Ok(Some((project, event)));
    }
    if plan.sources.is_empty() {
        return Ok(None);
    }

    // The old project stays untouched (commands may still be reading it):
    // the facts are copied, patched, and analysed into a new project.
    let mut facts = current.facts.clone();
    let update = incremental::apply(root, &mut facts, &plan.sources)?;
    if update.files.is_empty() {
        // A folder event with no Java file inside, say.
        return Ok(None);
    }
    let scan_ms = started.elapsed().as_millis() as u32;

    let mut keys = BTreeSet::new();
    touched_keys(&current.facts, &update.files, &mut keys);
    touched_keys(&facts, &update.files, &mut keys);

    let project = OpenProject::from_facts(root, facts, scan_ms, &current.hint());
    // Keys of deleted nodes are dropped: there is nothing left to highlight.
    let changed_keys = keys
        .into_iter()
        .filter(|k| project.node_id(k).is_ok())
        .collect();
    let event = LiveEvent::Updated {
        summary: Box::new(project.summary.clone()),
        full: false,
        trigger: None,
        file_count: update.files.len() as u32,
        files: update.files.into_iter().take(MAX_FILES).collect(),
        changed_keys,
        ms: started.elapsed().as_millis() as u32,
    };
    Ok(Some((project, event)))
}

/// Module, package and type keys of the given files, as `Atlas::key` builds them.
fn touched_keys(facts: &ProjectFacts, files: &[String], out: &mut BTreeSet<String>) {
    for module in &facts.modules {
        for file in module.files.iter().filter(|f| files.contains(&f.path)) {
            out.insert(format!("m:{}", module.id));
            out.insert(format!("p:{}|{}", module.id, file.package));
            for t in &file.types {
                let fqn = if file.package.is_empty() {
                    t.name.clone()
                } else {
                    format!("{}.{}", file.package, t.name)
                };
                out.insert(format!("t:{fqn}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn sandbox(tag: &str) -> PathBuf {
        let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../samples/atlas-bank");
        let to = std::env::temp_dir().join(format!("java-atlas-live-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&to);
        copy_dir(&from, &to);
        to
    }

    fn batch(paths: &[PathBuf]) -> Batch {
        Batch {
            paths: paths.iter().cloned().collect(),
            rescan: false,
        }
    }

    #[test]
    fn breaking_a_cycle_shows_up_after_an_incremental_update() {
        let root = sandbox("cycle");
        let before = project::scan(&root, &|_| {}, &Default::default()).unwrap();
        assert_eq!(before.summary.cycles.len(), 2);

        // AuditLog stops using the account package: the account ⇄ audit cycle is gone.
        let audit =
            root.join("bank-service/src/main/java/com/atlasbank/service/audit/AuditLog.java");
        fs::write(
            &audit,
            "package com.atlasbank.service.audit;\npublic class AuditLog {}\n",
        )
        .unwrap();
        let (after, event) = refresh(&before, &batch(&[audit])).unwrap().unwrap();
        fs::remove_dir_all(&root).ok();

        assert_eq!(after.summary.cycles.len(), 1);
        let LiveEvent::Updated {
            full,
            files,
            changed_keys,
            ..
        } = event
        else {
            panic!("expected an update");
        };
        assert!(!full);
        assert_eq!(
            files,
            ["bank-service/src/main/java/com/atlasbank/service/audit/AuditLog.java"]
        );
        assert!(
            changed_keys
                .contains(&"p:com.atlasbank:bank-service|com.atlasbank.service.audit".to_owned())
        );
        assert!(changed_keys.contains(&"t:com.atlasbank.service.audit.AuditLog".to_owned()));
    }

    #[test]
    fn noise_changes_nothing_and_a_pom_rescans() {
        let root = sandbox("pom");
        let before = project::scan(&root, &|_| {}, &Default::default()).unwrap();

        let noise = root.join("bank-api/target/classes/X.class");
        assert!(refresh(&before, &batch(&[noise])).unwrap().is_none());

        let pom = root.join("bank-api/pom.xml");
        let (_, event) = refresh(&before, &batch(std::slice::from_ref(&pom)))
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            LiveEvent::Updated { full: true, trigger: Some(ref t), .. } if t == "bank-api/pom.xml"
        ));

        // A half-written POM is an error, not a crash; the caller keeps the old atlas.
        fs::write(&pom, "<project><artifactId>bank-api</groupId></project>").unwrap();
        let result = refresh(&before, &batch(&[pom]));
        fs::remove_dir_all(&root).ok();
        assert!(result.is_err());
    }
}
