//! Decide how a patch set evolves across a version bump, from tree evidence.
//!
//! A version bump commonly changes the patch set: fixes land upstream and
//! their patches drop, and new releases need new patches. The strongest
//! evidence available without building is a *same-version sibling*: a recipe
//! for the new version that already exists in the robot tree under another
//! toolchain. Its patch list is what a maintainer already shipped for this
//! version, so the bump adopts it verbatim (the raw `patches = [...]` block,
//! preserving tuple entries and comments) and reports a per-patch decision
//! trail. Without a sibling, patches whose file name embeds the old version
//! are flagged undecided rather than silently carried.

use std::path::Path;

use crate::domain::{Candidate, Toolchain};
use crate::eb_emit::{find_list_assignment_span, EmitError};

/// What happens to one patch across the bump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchDecision {
    /// Kept: present before the bump and in the evidence recipe (or no
    /// evidence spoke against it).
    Carry,
    /// Removed: the same-version sibling ships without it.
    Drop,
    /// Added: the same-version sibling ships it and the source did not.
    Adopt,
    /// No evidence either way and the name embeds the old version, so
    /// carrying it forward is a guess.
    Undecided,
}

impl PatchDecision {
    /// Lowercase verb for residual summaries and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            PatchDecision::Carry => "carry",
            PatchDecision::Drop => "drop",
            PatchDecision::Adopt => "adopt",
            PatchDecision::Undecided => "undecided",
        }
    }
}

/// One patch, its decision, and the evidence the decision rests on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchCall {
    /// Patch file name as the recipe lists it.
    pub patch: String,
    /// What happens to it across the bump.
    pub decision: PatchDecision,
    /// The observation the decision rests on, naming the recipe consulted.
    pub evidence: String,
}

/// A same-version recipe already in the tree, resolved far enough to serve
/// as patch evidence. The caller owns the parse so this module stays pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiblingRecipe {
    /// Tree path of the sibling recipe, cited in every decision.
    pub easyconfig_path: String,
    /// Toolchain the sibling builds against; family match raises its rank.
    pub toolchain: Toolchain,
    /// Patch files the sibling ships, in its own order.
    pub patch_names: Vec<String>,
}

/// The full evolution plan for a bump's patch set.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PatchPlan {
    /// One decision per patch across old set and sibling set.
    pub calls: Vec<PatchCall>,
    /// Path of the same-version sibling recipe the plan adopts, when one
    /// exists in the tree.
    pub sibling: Option<String>,
}

impl PatchPlan {
    /// Patches that survive the bump, in evidence order.
    pub fn final_patches(&self) -> Vec<&str> {
        self.calls
            .iter()
            .filter(|c| {
                matches!(
                    c.decision,
                    PatchDecision::Carry | PatchDecision::Adopt | PatchDecision::Undecided
                )
            })
            .map(|c| c.patch.as_str())
            .collect()
    }

    /// Whether the plan changes the patch set at all.
    pub fn changed(&self) -> bool {
        self.calls
            .iter()
            .any(|c| matches!(c.decision, PatchDecision::Drop | PatchDecision::Adopt))
    }

    /// Patches whose fate could not be decided from evidence.
    pub fn undecided(&self) -> Vec<&str> {
        self.calls
            .iter()
            .filter(|c| c.decision == PatchDecision::Undecided)
            .map(|c| c.patch.as_str())
            .collect()
    }
}

/// Tree locations of same-version siblings worth resolving as evidence,
/// ranked: same toolchain family first, then the lexically newest toolchain
/// version, then path for determinism. The first resolvable one wins.
pub fn sibling_paths(
    name: &str,
    new_version: &str,
    candidates: &[Candidate],
    target_toolchain: &Toolchain,
) -> Vec<String> {
    let mut siblings: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| c.name == name && c.version == new_version && !c.easyconfig_path.is_empty())
        .collect();
    siblings.sort_by(|a, b| {
        let a_same = a.toolchain.name == target_toolchain.name;
        let b_same = b.toolchain.name == target_toolchain.name;
        b_same
            .cmp(&a_same)
            .then_with(|| b.toolchain.version.cmp(&a.toolchain.version))
            .then_with(|| a.easyconfig_path.cmp(&b.easyconfig_path))
    });
    siblings
        .into_iter()
        .map(|c| c.easyconfig_path.clone())
        .collect()
}

/// Decide the patch set for a bump to `new_version`.
///
/// `old_patches` come from the source recipe; `sibling` is the best
/// same-version recipe the caller resolved from the tree, if any.
pub fn plan_patch_evolution(
    new_version: &str,
    old_patches: &[String],
    sibling: Option<&SiblingRecipe>,
) -> PatchPlan {
    if let Some(sibling) = sibling {
        let sib_label = format!(
            "{} ({}-{})",
            sibling.easyconfig_path, sibling.toolchain.name, sibling.toolchain.version
        );
        let mut calls = Vec::new();
        // Sibling order wins wholesale: patch order is semantically load
        // bearing, and the sibling's order is what already ships.
        for patch in &sibling.patch_names {
            let decision = if old_patches.contains(patch) {
                PatchDecision::Carry
            } else {
                PatchDecision::Adopt
            };
            calls.push(PatchCall {
                patch: patch.clone(),
                decision,
                evidence: format!("same-version sibling {sib_label} ships it"),
            });
        }
        for patch in old_patches {
            if !sibling.patch_names.contains(patch) {
                calls.push(PatchCall {
                    patch: patch.clone(),
                    decision: PatchDecision::Drop,
                    evidence: format!(
                        "same-version sibling {sib_label} ships {new_version} without it"
                    ),
                });
            }
        }
        return PatchPlan {
            calls,
            sibling: Some(sibling.easyconfig_path.clone()),
        };
    }

    let calls = old_patches
        .iter()
        .map(|patch| {
            if pins_other_version(patch, new_version) {
                PatchCall {
                    patch: patch.clone(),
                    decision: PatchDecision::Undecided,
                    evidence: format!(
                        "no {new_version} sibling in the tree and the name pins another \
                         version; applicability to {new_version} is a guess"
                    ),
                }
            } else {
                PatchCall {
                    patch: patch.clone(),
                    decision: PatchDecision::Carry,
                    evidence: format!(
                        "no {new_version} sibling in the tree; name is version-neutral, \
                         carried forward unreviewed"
                    ),
                }
            }
        })
        .collect();
    PatchPlan {
        calls,
        sibling: None,
    }
}

/// Splice the sibling's literal `patches = [...]` assignment into `text`,
/// replacing the existing one (or inserting before `moduleclass` when the
/// source has none). Textual adoption keeps tuple entries, patch levels,
/// and comments exactly as the sibling ships them.
pub fn adopt_sibling_patch_block(text: &str, sibling_path: &str) -> Result<String, EmitError> {
    let sibling_text = std::fs::read_to_string(Path::new(sibling_path))
        .map_err(|e| EmitError::Rewrite(format!("read sibling {sibling_path}: {e}")))?;

    let ours = find_list_assignment_span(text, "patches")?;
    let theirs = find_list_assignment_span(&sibling_text, "patches")?;

    match (ours, theirs) {
        (Some((our_start, our_end)), Some((their_start, their_end))) => Ok(format!(
            "{}{}{}",
            &text[..our_start],
            &sibling_text[their_start..their_end],
            &text[our_end..]
        )),
        (Some((our_start, our_end)), None) => {
            // Sibling ships the new version with no patches at all: the list
            // goes away, along with a trailing newline so no blank hole stays.
            let mut end = our_end;
            if text[end..].starts_with('\n') {
                end += 1;
            }
            Ok(format!("{}{}", &text[..our_start], &text[end..]))
        }
        (None, Some((their_start, their_end))) => {
            let block = &sibling_text[their_start..their_end];
            if let Some(pos) = find_moduleclass_line(text) {
                Ok(format!("{}{}\n{}", &text[..pos], block, &text[pos..]))
            } else {
                let sep = if text.ends_with('\n') { "" } else { "\n" };
                Ok(format!("{text}{sep}{block}\n"))
            }
        }
        (None, None) => Ok(text.to_string()),
    }
}

/// Whether a patch file name embeds a version-like token other than the
/// version being bumped to. `OpenMPI-5.0.7_fix_gpfs.patch` pins 5.0.7 even
/// when the bump is 5.0.8 -> 5.0.10, so matching only the old version would
/// miss it; any foreign version pin makes applicability a guess.
fn pins_other_version(patch: &str, new_version: &str) -> bool {
    let bytes = patch.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let token = patch[start..i].trim_end_matches('.');
            if token.contains('.') && token != new_version {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

fn find_moduleclass_line(text: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("moduleclass") {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(name: &str, version: &str) -> Toolchain {
        Toolchain {
            name: name.into(),
            version: version.into(),
        }
    }

    fn sib(path: &str, toolchain: (&str, &str), patches: &[&str]) -> SiblingRecipe {
        SiblingRecipe {
            easyconfig_path: path.into(),
            toolchain: tc(toolchain.0, toolchain.1),
            patch_names: patches.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn candidate(name: &str, version: &str, toolchain: (&str, &str), path: &str) -> Candidate {
        Candidate {
            name: name.into(),
            version: version.into(),
            toolchain: tc(toolchain.0, toolchain.1),
            versionsuffix: None,
            easyconfig_path: path.into(),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
        }
    }

    #[test]
    fn sibling_drives_drop_carry_and_adopt() {
        let old = vec!["keep.patch".to_string(), "fixed-upstream.patch".to_string()];
        let sibling = sib(
            "tree/o.eb",
            ("GCC", "14.3.0"),
            &["keep.patch", "brand-new.patch"],
        );
        let plan = plan_patch_evolution("5.0.10", &old, Some(&sibling));
        assert_eq!(plan.sibling.as_deref(), Some("tree/o.eb"));
        let by_name = |p: &str| plan.calls.iter().find(|c| c.patch == p).unwrap().decision;
        assert_eq!(by_name("keep.patch"), PatchDecision::Carry);
        assert_eq!(by_name("brand-new.patch"), PatchDecision::Adopt);
        assert_eq!(by_name("fixed-upstream.patch"), PatchDecision::Drop);
        assert_eq!(plan.final_patches(), vec!["keep.patch", "brand-new.patch"]);
        assert!(plan.changed());
    }

    #[test]
    fn sibling_order_wins_over_source_order() {
        let old = vec!["b.patch".to_string(), "a.patch".to_string()];
        let sibling = sib("tree/x.eb", ("GCC", "14.3.0"), &["a.patch", "b.patch"]);
        let plan = plan_patch_evolution("2.0", &old, Some(&sibling));
        assert_eq!(plan.final_patches(), vec!["a.patch", "b.patch"]);
        assert!(!plan.changed());
    }

    #[test]
    fn sibling_ranking_prefers_family_then_newest_then_path() {
        let cands = vec![
            candidate("X", "2.0", ("intel", "2026a"), "tree/i.eb"),
            candidate("X", "2.0", ("GCC", "14.3.0"), "tree/g14.eb"),
            candidate("X", "2.0", ("GCC", "15.2.0"), "tree/g15.eb"),
            candidate("X", "1.0", ("GCC", "15.2.0"), "tree/old.eb"),
            candidate("Y", "2.0", ("GCC", "15.2.0"), "tree/other.eb"),
        ];
        let ranked = sibling_paths("X", "2.0", &cands, &tc("GCC", "16.1.0"));
        assert_eq!(ranked, vec!["tree/g15.eb", "tree/g14.eb", "tree/i.eb"]);
    }

    #[test]
    fn no_sibling_flags_version_pinned_names_undecided() {
        let old = vec![
            "X-1.0_fix-runpath.patch".to_string(),
            "portable-fix.patch".to_string(),
        ];
        let plan = plan_patch_evolution("2.0", &old, None);
        assert!(plan.sibling.is_none());
        assert_eq!(plan.undecided(), vec!["X-1.0_fix-runpath.patch"]);
        // Both stay in the emitted list; undecided is a review flag, not a drop.
        assert_eq!(
            plan.final_patches(),
            vec!["X-1.0_fix-runpath.patch", "portable-fix.patch"]
        );
        assert!(!plan.changed());
    }

    #[test]
    fn a_pin_on_any_foreign_version_is_undecided() {
        // The dropped OpenMPI patch pinned 5.0.7 while the bump was
        // 5.0.8 -> 5.0.10: matching only the source version misses it.
        let old = vec!["OpenMPI-5.0.7_fix_gpfs_compatibility.patch".to_string()];
        let plan = plan_patch_evolution("5.0.10", &old, None);
        assert_eq!(
            plan.undecided(),
            vec!["OpenMPI-5.0.7_fix_gpfs_compatibility.patch"]
        );
        // A pin on the bump target itself is not foreign.
        let new = vec!["X-2.0_fix.patch".to_string()];
        let plan = plan_patch_evolution("2.0", &new, None);
        assert!(plan.undecided().is_empty());
    }

    #[test]
    fn adoption_splices_the_sibling_block_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let sib_path = dir.path().join("sib.eb");
        std::fs::write(
            &sib_path,
            "name = 'X'\npatches = [\n    ('lvl.patch', 2),  # keep level\n    'plain.patch',\n]\nmoduleclass = 'lib'\n",
        )
        .unwrap();
        let ours = "name = 'X'\npatches = ['old.patch']\nmoduleclass = 'lib'\n";
        let out = adopt_sibling_patch_block(ours, sib_path.to_str().unwrap()).unwrap();
        assert!(out.contains("('lvl.patch', 2),  # keep level"), "{out}");
        assert!(!out.contains("old.patch"), "{out}");
    }

    #[test]
    fn sibling_without_patches_removes_our_list() {
        let dir = tempfile::tempdir().unwrap();
        let sib_path = dir.path().join("sib.eb");
        std::fs::write(&sib_path, "name = 'X'\nmoduleclass = 'lib'\n").unwrap();
        let ours = "name = 'X'\npatches = ['old.patch']\nmoduleclass = 'lib'\n";
        let out = adopt_sibling_patch_block(ours, sib_path.to_str().unwrap()).unwrap();
        assert!(!out.contains("patches"), "{out}");
        assert!(out.contains("moduleclass"), "{out}");
    }

    #[test]
    fn source_without_patches_gains_the_sibling_block_before_moduleclass() {
        let dir = tempfile::tempdir().unwrap();
        let sib_path = dir.path().join("sib.eb");
        std::fs::write(&sib_path, "patches = ['new.patch']\nmoduleclass = 'lib'\n").unwrap();
        let ours = "name = 'X'\nmoduleclass = 'lib'\n";
        let out = adopt_sibling_patch_block(ours, sib_path.to_str().unwrap()).unwrap();
        let p = out.find("patches").unwrap();
        let m = out.find("moduleclass").unwrap();
        assert!(p < m, "{out}");
    }
}
