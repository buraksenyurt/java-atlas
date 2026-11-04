//! Phase 8: an incremental update must leave the facts exactly as a full
//! scan of the changed project would.

use std::fs;
use std::path::{Path, PathBuf};

use java_atlas_core::facts::ProjectFacts;
use java_atlas_scan::incremental::{self, SourceChange};
use java_atlas_scan::{ScanOptions, read_project};

/// A private copy of a sample project, removed when dropped.
struct Sandbox(PathBuf);

impl Sandbox {
    fn copy_of(sample: &str, tag: &str) -> Self {
        let from = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../samples")
            .join(sample);
        let to = std::env::temp_dir().join(format!("java-atlas-inc-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&to);
        copy_dir(&from, &to);
        Sandbox(to)
    }
    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

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

fn scan(root: &Path) -> ProjectFacts {
    read_project(root, &ScanOptions::default(), &|_| {}).unwrap()
}

/// Warnings are appended as files are re-parsed; their order may differ.
fn same(mut a: ProjectFacts, mut b: ProjectFacts) {
    a.warnings
        .sort_by(|x, y| (&x.file, &x.message).cmp(&(&y.file, &y.message)));
    b.warnings
        .sort_by(|x, y| (&x.file, &x.message).cmp(&(&y.file, &y.message)));
    assert_eq!(a, b);
}

const SERVICE: &str = "bank-service/src/main/java/com/atlasbank/service";

#[test]
fn edits_creations_and_deletions_match_a_full_scan() {
    let sb = Sandbox::copy_of("atlas-bank", "files");
    let mut facts = scan(&sb.0);

    // Modify: AuditLog now also uses java.time.
    let audit = sb.path(&format!("{SERVICE}/audit/AuditLog.java"));
    let src = fs::read_to_string(&audit).unwrap();
    fs::write(
        &audit,
        src.replace(
            "package com.atlasbank.service.audit;",
            "package com.atlasbank.service.audit;\n\nimport java.time.Instant;",
        ),
    )
    .unwrap();
    // Create: a new type in a new package, and a test.
    fs::create_dir_all(sb.path(&format!("{SERVICE}/report"))).unwrap();
    let report = sb.path(&format!("{SERVICE}/report/Report.java"));
    fs::write(&report, "package com.atlasbank.service.report;\nimport com.atlasbank.service.account.AccountService;\npublic class Report { AccountService s; }\n").unwrap();
    // Delete: TransferService.
    let transfer = sb.path(&format!("{SERVICE}/account/TransferService.java"));
    fs::remove_file(&transfer).unwrap();

    let plan = incremental::plan(&sb.0, &facts, &[audit, report, transfer]);
    assert!(plan.full_scan.is_none());
    assert_eq!(plan.sources.len(), 3);
    let update = incremental::apply(&sb.0, &mut facts, &plan.sources).unwrap();
    assert_eq!((update.parsed, update.removed), (2, 1));
    assert_eq!(update.files.len(), 3);

    same(facts, scan(&sb.0));
}

#[test]
fn a_deleted_or_renamed_package_folder_is_handled_as_a_tree() {
    let sb = Sandbox::copy_of("atlas-bank", "tree");
    let mut facts = scan(&sb.0);

    // Renaming a package folder: the watcher may only report the two folders.
    let old = sb.path(&format!("{SERVICE}/audit"));
    let new = sb.path(&format!("{SERVICE}/journal"));
    fs::rename(&old, &new).unwrap();

    let plan = incremental::plan(&sb.0, &facts, &[old, new]);
    assert!(
        plan.sources
            .iter()
            .all(|c| matches!(c, SourceChange::Tree { .. }))
    );
    let update = incremental::apply(&sb.0, &mut facts, &plan.sources).unwrap();
    assert_eq!((update.parsed, update.removed), (2, 2), "{update:?}");

    same(facts, scan(&sb.0));
}

#[test]
fn build_files_ask_for_a_full_scan_and_noise_is_ignored() {
    let sb = Sandbox::copy_of("atlas-bank", "plan");
    let facts = scan(&sb.0);

    let noise = [
        sb.path("bank-api/target/classes/com/atlasbank/api/AccountController.class"),
        sb.path("bank-api/target/classes/META-INF/maven/com.atlasbank/bank-api/pom.xml"),
        sb.path("README.md"),
        sb.path(&format!("{SERVICE}/audit/AuditLog.java~")),
    ];
    assert!(incremental::plan(&sb.0, &facts, &noise).is_empty());

    let pom = sb.path("bank-api/pom.xml");
    let plan = incremental::plan(
        &sb.0,
        &facts,
        &[
            sb.path(&format!("{SERVICE}/audit/AuditLog.java")),
            pom.clone(),
        ],
    );
    assert_eq!(plan.full_scan, Some(pom));

    let module_info = sb.path("bank-domain/src/main/java/module-info.java");
    assert!(
        incremental::plan(&sb.0, &facts, &[module_info])
            .full_scan
            .is_some()
    );
}

#[test]
fn test_files_keep_their_test_flag() {
    let sb = Sandbox::copy_of("atlas-bank", "test");
    let mut facts = scan(&sb.0);
    let test = sb.path("bank-service/src/test/java/com/atlasbank/service/account/AuditTest.java");
    fs::write(
        &test,
        "package com.atlasbank.service.account;\nclass AuditTest {}\n",
    )
    .unwrap();

    let plan = incremental::plan(&sb.0, &facts, &[test]);
    incremental::apply(&sb.0, &mut facts, &plan.sources).unwrap();
    let file = facts
        .modules
        .iter()
        .flat_map(|m| &m.files)
        .find(|f| f.path.ends_with("AuditTest.java"))
        .unwrap();
    assert!(file.is_test);
    same(facts, scan(&sb.0));
}
