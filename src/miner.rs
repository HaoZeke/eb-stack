//! Ring-corpus miner helpers for the reproduction grind (surf-notes
//! `orchestration/2026-08-03-ebstack-reproduction-grind.md`, tracker issues
//! `ebstack-l8e9`/`ebstack-y5yq`).
//!
//! A mechanical miner that turns a merged upstream PR into a `bump`
//! reproduction fixture pair has to reject two real shapes found while
//! hand-mining ~10 merged easybuilders/easybuild-easyconfigs bumps:
//! batch "toolchain refresh" PRs that mix a toolchain-generation
//! definition with application recipes in one diff, and non-linear
//! version history where the PR's target version is not actually the
//! newest at that toolchain generation (a backfill, not a clean
//! next-version bump). Both checks operate on data the miner already has
//! once it has parsed a candidate file and listed the merge-base tree, so
//! neither needs new I/O.

use crate::eb_parse::ResolvedEasyconfig;
use crate::version::cmp_version;
use std::cmp::Ordering;

/// True when `recipe` defines a toolchain generation itself (for example
/// `gompi-2026.1.eb`, `easyblock = 'Toolchain'`, `moduleclass = 'toolchain'`)
/// rather than an application built with one.
///
/// Toolchain-definition recipes are not `bump` targets: bumping an
/// application recipe assumes a toolchain generation to move it to
/// already exists, and a toolchain-definition recipe *is* that
/// generation, not a consumer of it. A miner that does not filter these
/// out will try to "reproduce" a recipe that never had a
/// previous-generation counterpart to bump from in the first place.
///
/// Checks `easyblock` first since it is the more direct signal (the
/// literal easyblock class EasyBuild uses to build a toolchain
/// definition); `moduleclass` is a secondary check for recipes where the
/// parser did not resolve an `easyblock` value.
pub fn is_toolchain_meta_recipe(recipe: &ResolvedEasyconfig) -> bool {
    matches!(recipe.easyblock.as_deref(), Some("Toolchain"))
        || matches!(recipe.moduleclass.as_deref(), Some("toolchain"))
}

/// True when `candidate_version` is not the highest version already
/// present in `existing_versions` for the same package and toolchain
/// generation, at the PR's merge-base commit.
///
/// A miner mining "PR bumps package X to version Y" candidates has to
/// distinguish a clean next-version bump (Y is newer than anything
/// already in the tree at that generation) from a backfill (an older
/// release added alongside a newer one that already merged separately).
/// Scoring a backfill as a bump-reproduction target compares the wrong
/// two files. Version ordering uses [`cmp_version`], not a string sort,
/// so `"4.2.1"` correctly outranks `"3.31.11"` (a plain string compare
/// would rank them the other way around, since `'4' > '3'` only holds
/// for the leading character and a string sort does not parse dotted
/// version segments numerically).
pub fn is_backfill(candidate_version: &str, existing_versions: &[String]) -> bool {
    existing_versions
        .iter()
        .any(|v| cmp_version(v, candidate_version) == Ordering::Greater)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eb_parse::resolve_easyconfig_str;

    /// gompi-2026.1.eb, easybuilders/easybuild-easyconfigs PR #25613,
    /// merge commit f929f1f2ef4c15feb19692f33da59e9e1e072572. Real
    /// toolchain-definition recipe from the same PR whose sibling file
    /// (OpenMPI-5.0.10-GCC-15.2.0.eb) is a genuine bump target.
    const GOMPI_2026_1: &str = r#"
easyblock = 'Toolchain'

name = 'gompi'
version = '2026.1'

homepage = '(none)'
description = """GNU Compiler Collection (GCC) based compiler toolchain,
 including OpenMPI for MPI support."""

toolchain = SYSTEM

local_gccver = '15.2.0'

dependencies = [
    ('GCC', local_gccver),
    ('OpenMPI', '5.0.10', '', ('GCC', local_gccver)),
]

moduleclass = 'toolchain'
"#;

    /// OpenMPI-5.0.10-GCC-15.2.0.eb, same PR #25613: a genuine application
    /// recipe, not a toolchain definition. Only the header fields matter
    /// for this classifier, so the body is trimmed to those.
    const OPENMPI_5_0_10: &str = r#"
easyblock = 'ConfigureMake'

name = 'OpenMPI'
version = '5.0.10'

homepage = 'https://www.open-mpi.org/'
description = "OpenMPI"

toolchain = {'name': 'GCC', 'version': '15.2.0'}

source_urls = ['https://download.open-mpi.org/release/open-mpi/v5.0']
sources = ['%(namelower)s-%(version)s.tar.bz2']
checksums = ['0000000000000000000000000000000000000000000000000000000000000000']

moduleclass = 'mpi'
"#;

    #[test]
    fn toolchain_definition_recipe_is_filtered() {
        let recipe = resolve_easyconfig_str(GOMPI_2026_1).expect("parse gompi-2026.1.eb");
        assert!(
            is_toolchain_meta_recipe(&recipe),
            "gompi-2026.1.eb (easyblock=Toolchain, moduleclass=toolchain) must be classified as a toolchain meta-recipe"
        );
    }

    #[test]
    fn application_recipe_in_the_same_pr_is_not_filtered() {
        let recipe =
            resolve_easyconfig_str(OPENMPI_5_0_10).expect("parse OpenMPI-5.0.10-GCC-15.2.0.eb");
        assert!(
            !is_toolchain_meta_recipe(&recipe),
            "OpenMPI-5.0.10-GCC-15.2.0.eb is a real application recipe and must not be filtered as a toolchain meta-recipe"
        );
    }

    #[test]
    fn backfill_below_an_existing_newer_version_is_detected() {
        // easybuilders/easybuild-easyconfigs PR #25231: "CMake v3.31.11" at
        // GCCcore-15.2.0 merges into a tree that already has
        // CMake-4.2.1-GCCcore-15.2.0.eb from an earlier PR. A plain string
        // compare of "3.31.11" vs "4.2.1" would (wrongly) rank 4.2.1 lower;
        // cmp_version must not make that mistake.
        let existing = vec!["4.2.1".to_string()];
        assert!(
            is_backfill("3.31.11", &existing),
            "3.31.11 merging after 4.2.1 already exists at the same generation must be flagged as a backfill"
        );
    }

    #[test]
    fn clean_next_version_bump_is_not_a_backfill() {
        // easybuilders/easybuild-easyconfigs PR #26010: Boost 1.90.0 at
        // GCC-15.2.0, with only 1.88.0 present at the prior generation
        // (GCC-14.3.0) in the merge-base tree, nothing newer already at
        // 15.2.0.
        let existing: Vec<String> = vec![];
        assert!(
            !is_backfill("1.90.0", &existing),
            "a version with nothing newer already in the tree at that generation is a clean bump, not a backfill"
        );
    }
}
