//! Classify EasyBuild recipes the way EESSI decides the install prefix.
//!
//! EESSI's `pre_fetch_hook_check_installation_path` (software-layer-scripts
//! `eb_hooks.py`) calls `self.cfg.dependency_names()` with no arguments. In
//! EasyBuild 5.4.2 that method defaults to `build_only=False, runtime_only=False`,
//! which asks `dependencies()` for builddependencies plus dependencies and then
//! applies `--filter-deps`. A package is accelerator software when its name is
//! an accelerator runtime, when any remaining (build or runtime) dependency is
//! one, or when its toolchain is an accelerator toolchain. Everything else is
//! CPU-only and must not install into an `/accel/` prefix. Classification uses
//! those dependency and toolchain facts — not application package-name lists.

use crate::dep_filter::FilterDeps;
use crate::domain::{Candidate, DepReq};

/// Accelerator runtimes EESSI refuses to install outside `/accel/`.
pub const ACCELERATOR_DEP_NAMES: &[&str] = &["CUDA", "ROCm-LLVM"];

/// Toolchains EESSI treats as accelerator builds even without a direct CUDA
/// or ROCm-LLVM dependency entry.
pub const ACCELERATOR_TOOLCHAIN_NAMES: &[&str] = &["rocm-compilers", "rompi", "rfbf", "rfoss"];

/// Direct dependency names EasyBuild 5.4.2 `dependency_names()` would return
/// for this candidate after `--filter-deps`: builddependencies and
/// dependencies, with filtered names removed.
pub fn effective_dependency_names<'a>(
    candidate: &'a Candidate,
    filter: &FilterDeps,
) -> Vec<&'a str> {
    candidate
        .builddependencies
        .iter()
        .chain(candidate.dependencies.iter())
        .filter(|dep| !filter.dep_is_filtered(&dep.name, dep_version(dep)))
        .map(|dep| dep.name.as_str())
        .collect()
}

fn dep_version(dep: &DepReq) -> &str {
    // EasyBuild filters on the version field of the dependency tuple. A range
    // requirement is uncommon in filter matching; use the raw version_req.
    dep.version_req.as_str()
}

/// Whether this candidate is accelerator software under EESSI's install-path
/// check, after applying `filter` the way `dependency_names()` does.
pub fn is_accelerator_candidate_filtered(candidate: &Candidate, filter: &FilterDeps) -> bool {
    if ACCELERATOR_DEP_NAMES.contains(&candidate.name.as_str()) {
        return true;
    }
    if ACCELERATOR_TOOLCHAIN_NAMES.contains(&candidate.toolchain.name.as_str()) {
        return true;
    }
    effective_dependency_names(candidate, filter)
        .into_iter()
        .any(|name| ACCELERATOR_DEP_NAMES.contains(&name))
}

/// [`is_accelerator_candidate_filtered`] with an empty filter list.
pub fn is_accelerator_candidate(candidate: &Candidate) -> bool {
    is_accelerator_candidate_filtered(candidate, &FilterDeps::empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Toolchain;

    fn cand(name: &str, toolchain: &str, deps: &[&str], builddeps: &[&str]) -> Candidate {
        let to_deps = |names: &[&str]| {
            names
                .iter()
                .map(|d| DepReq {
                    name: (*d).into(),
                    version_req: "1.0".into(),
                    versionsuffix: None,
                    toolchain: None,
                })
                .collect()
        };
        Candidate {
            name: name.into(),
            version: "1.0".into(),
            toolchain: Toolchain {
                name: toolchain.into(),
                version: "2026.1".into(),
            },
            versionsuffix: None,
            versionprefix: None,
            easyconfig_path: format!("{name}-1.0.eb"),
            dependencies: to_deps(deps),
            builddependencies: to_deps(builddeps),
            exts_list: Vec::new(),
            easyblock: None,
            moduleclass: None,
        }
    }

    #[test]
    fn cuda_runtime_and_cuda_dependants_are_accelerator() {
        assert!(is_accelerator_candidate(&cand("CUDA", "system", &[], &[])));
        assert!(is_accelerator_candidate(&cand(
            "GROMACS",
            "foss",
            &["CUDA"],
            &[]
        )));
        assert!(is_accelerator_candidate(&cand(
            "UCX-CUDA",
            "gompi",
            &["CUDA"],
            &[]
        )));
    }

    #[test]
    fn build_only_cuda_is_accelerator_like_dependency_names() {
        // EasyBuild 5.4.2 dependency_names() defaults include builddependencies.
        // Public DualSPHysics lists CUDA only under builddependencies; Triton
        // is the same classifier path when CUDA is build-only.
        let triton = cand("Triton", "foss", &["Python"], &["CUDA"]);
        assert!(is_accelerator_candidate(&triton));
        let names = effective_dependency_names(&triton, &FilterDeps::empty());
        assert!(names.contains(&"CUDA"));
        assert!(names.contains(&"Python"));
    }

    #[test]
    fn filtered_cuda_is_no_longer_accelerator() {
        let triton = cand("Triton", "foss", &["Python"], &["CUDA"]);
        let filter = FilterDeps::parse(&["CUDA".into()]).unwrap();
        assert!(!is_accelerator_candidate_filtered(&triton, &filter));
        let names = effective_dependency_names(&triton, &filter);
        assert!(!names.contains(&"CUDA"));
        assert!(names.contains(&"Python"));
    }

    #[test]
    fn rocm_toolchain_is_accelerator_without_naming_the_app() {
        assert!(is_accelerator_candidate(&cand(
            "SomeApp",
            "rfoss",
            &[],
            &[]
        )));
        assert!(is_accelerator_candidate(&cand(
            "ROCm-LLVM",
            "system",
            &[],
            &[]
        )));
    }

    #[test]
    fn ordinary_foss_packages_without_cuda_are_cpu_only() {
        assert!(!is_accelerator_candidate(&cand(
            "HDF5",
            "foss",
            &["zlib"],
            &[]
        )));
        assert!(!is_accelerator_candidate(&cand(
            "GROMACS",
            "foss",
            &["FFTW"],
            &[]
        )));
    }
}
