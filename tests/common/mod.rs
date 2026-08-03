//! The shared gate for tests that need a real EasyBuild easyconfigs tree.
//!
//! These tests assert against the robot tree, which is not in the repository.
//! Without one they have to skip, and a skip that says nothing is
//! indistinguishable from a pass: the suite reports green on any host lacking
//! the tree while the assertions never ran.
//!
//! So every skip prints a named `SKIPPED[...]` marker, and setting
//! `EB_REQUIRE_EASYCONFIGS=1` turns a skip into a failure, which is how a CI
//! job that is supposed to have the tree proves it used it.

// Each test binary includes this module and uses only part of it.
#![allow(dead_code)]

use std::path::PathBuf;

/// Environment variable naming the easyconfigs tree.
pub const TREE_ENV: &str = "EB_EASYCONFIGS";
/// Set this to turn a missing tree into a failure instead of a skip.
pub const REQUIRE_ENV: &str = "EB_REQUIRE_EASYCONFIGS";
/// Where the tree lives when the environment says nothing.
pub const DEFAULT_RELATIVE_PATH: &str = ".venvs/easybuild/easybuild/easyconfigs";

/// The easyconfigs tree: `EB_EASYCONFIGS` when it names a directory, otherwise
/// the conventional path under `$HOME`.
pub fn easyconfigs_root() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var(TREE_ENV) {
        let candidate = PathBuf::from(raw);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    let home = std::env::var("HOME").ok()?;
    let candidate = PathBuf::from(home).join(DEFAULT_RELATIVE_PATH);
    candidate.is_dir().then_some(candidate)
}

/// The tree, or `None` after announcing the skip under `test_name`.
///
/// Panics instead of returning `None` when `EB_REQUIRE_EASYCONFIGS` is set, so
/// a run that must exercise the robot tree cannot quietly decline to.
pub fn require_easyconfigs_tree(test_name: &str) -> Option<PathBuf> {
    if let Some(root) = easyconfigs_root() {
        return Some(root);
    }
    let looked_at = std::env::var(TREE_ENV).unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|h| format!("{h}/{DEFAULT_RELATIVE_PATH}"))
            .unwrap_or_else(|_| "<no HOME>".into())
    });
    let message = format!(
        "SKIPPED[{test_name}]: no easyconfigs tree at {looked_at:?}; \
         set {TREE_ENV} to a robot tree to run this assertion"
    );
    if require_tree() {
        panic!("{message} ({REQUIRE_ENV} is set, so a skip is a failure)");
    }
    eprintln!("{message}");
    None
}

fn require_tree() -> bool {
    matches!(
        std::env::var(REQUIRE_ENV).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}
