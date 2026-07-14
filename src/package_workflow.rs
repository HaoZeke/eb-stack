//! Canonical foreign-package planning bundle: manifest, SBOM, locks, recipes.

use crate::domain::Toolchain;
use crate::eb_parse::{easyconfig_letter_dir, parse_easyconfig_trees};
use crate::foreign::{parse_foreign_path, ForeignFormat};
use crate::manifest::package_plan_from_foreign;
use crate::package::{package_plan_to_cyclonedx, PackagePlan, ProfileLock, StackPolicy};
use crate::package_config::{apply_profile_layers, ProfileConfigLayer};
use crate::package_emit::{emit_profile_easyconfigs, EmittedEasyconfig};
use crate::package_solve::solve_package_profile;
use serde_json::Value;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct NewPackageRequest {
    pub source: PathBuf,
    pub format: Option<ForeignFormat>,
    pub toolchain: Toolchain,
    pub profile_layers: Vec<ProfileConfigLayer>,
    pub easyconfig_roots: Vec<PathBuf>,
    pub stack_policy: StackPolicy,
}

#[derive(Debug, Clone)]
pub struct PackageBundle {
    pub plan: PackagePlan,
    pub sbom: Value,
    pub locks: Vec<ProfileLock>,
    pub easyconfigs: Vec<EmittedEasyconfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenPackageBundle {
    pub manifest: PathBuf,
    pub sbom: PathBuf,
    pub locks: Vec<PathBuf>,
    pub easyconfigs: Vec<PathBuf>,
}

pub fn inspect_new_package(
    source: &Path,
    format: Option<ForeignFormat>,
    toolchain: &Toolchain,
    profile_layers: &[ProfileConfigLayer],
) -> Result<(PackagePlan, Value), PackageWorkflowError> {
    let recipe = parse_foreign_path(source, format)
        .map_err(|error| PackageWorkflowError::Foreign(error.to_string()))?;
    let mut plan = package_plan_from_foreign(&recipe, toolchain);
    if !profile_layers.is_empty() {
        apply_profile_layers(&mut plan, profile_layers)
            .map_err(|error| PackageWorkflowError::Config(error.to_string()))?;
    }
    let sbom = package_plan_to_cyclonedx(&plan)
        .map_err(|error| PackageWorkflowError::Sbom(error.to_string()))?;
    Ok((plan, sbom))
}

pub fn plan_new_package(
    request: &NewPackageRequest,
) -> Result<PackageBundle, PackageWorkflowError> {
    if request.easyconfig_roots.is_empty() {
        return Err(PackageWorkflowError::NoEasyconfigRoots);
    }
    let (plan, sbom) = inspect_new_package(
        &request.source,
        request.format,
        &request.toolchain,
        &request.profile_layers,
    )?;
    let roots = request
        .easyconfig_roots
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let tree = parse_easyconfig_trees(&roots)
        .map_err(|error| PackageWorkflowError::Robot(error.to_string()))?;
    let mut locks = Vec::new();
    for output in &plan.outputs {
        locks.push(
            solve_package_profile(
                &plan,
                &output.profile,
                &Default::default(),
                &tree.candidates,
                &request.stack_policy,
            )
            .map_err(|error| PackageWorkflowError::Solve(error.to_string()))?,
        );
    }
    let easyconfigs = emit_profile_easyconfigs(&plan, &locks)
        .map_err(|error| PackageWorkflowError::Emit(error.to_string()))?;
    Ok(PackageBundle {
        plan,
        sbom,
        locks,
        easyconfigs,
    })
}

pub fn write_package_bundle(
    bundle: &PackageBundle,
    output_directory: &Path,
) -> Result<WrittenPackageBundle, PackageWorkflowError> {
    std::fs::create_dir_all(output_directory)
        .map_err(|error| PackageWorkflowError::Io(output_directory.to_path_buf(), error))?;
    let manifest = output_directory.join("package.plan.json");
    let sbom = output_directory.join("package.sbom.cdx.json");
    write_json(&manifest, &bundle.plan)?;
    write_json(&sbom, &bundle.sbom)?;

    let lock_directory = output_directory.join("locks");
    std::fs::create_dir_all(&lock_directory)
        .map_err(|error| PackageWorkflowError::Io(lock_directory.clone(), error))?;
    let mut locks = Vec::new();
    for lock in &bundle.locks {
        let path = lock_directory.join(format!("{}.lock.json", lock.profile));
        write_json(&path, lock)?;
        locks.push(path);
    }

    let recipe_directory = output_directory
        .join("easyconfigs")
        .join(easyconfig_letter_dir(&bundle.plan.package.name))
        .join(&bundle.plan.package.name);
    std::fs::create_dir_all(&recipe_directory)
        .map_err(|error| PackageWorkflowError::Io(recipe_directory.clone(), error))?;
    let mut easyconfigs = Vec::new();
    for recipe in &bundle.easyconfigs {
        let path = recipe_directory.join(&recipe.filename);
        std::fs::write(&path, &recipe.text)
            .map_err(|error| PackageWorkflowError::Io(path.clone(), error))?;
        easyconfigs.push(path);
    }

    Ok(WrittenPackageBundle {
        manifest,
        sbom,
        locks,
        easyconfigs,
    })
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), PackageWorkflowError> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| PackageWorkflowError::Json(path.to_path_buf(), error))?;
    text.push('\n');
    std::fs::write(path, text).map_err(|error| PackageWorkflowError::Io(path.to_path_buf(), error))
}

#[derive(Debug, Error)]
pub enum PackageWorkflowError {
    #[error("foreign package parse: {0}")]
    Foreign(String),
    #[error("package profile config: {0}")]
    Config(String),
    #[error("package SBOM: {0}")]
    Sbom(String),
    #[error("at least one EasyBuild robot root is required")]
    NoEasyconfigRoots,
    #[error("EasyBuild robot parse: {0}")]
    Robot(String),
    #[error("package profile solve: {0}")]
    Solve(String),
    #[error("EasyBuild recipe emission: {0}")]
    Emit(String),
    #[error("write {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("serialize {0}: {1}")]
    Json(PathBuf, serde_json::Error),
}
