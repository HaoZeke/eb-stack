//! Planned CycloneDX 1.5 SBOM from a stack lock (pre-install inventory).
//!
//! Built with the official [`cyclonedx_bom`] crate (same models as
//! `cargo-cyclonedx` / CycloneDX Rust Cargo). Documents are serialized as
//! JSON 1.5 with serial numbers, tool metadata, lifecycle phase, and
//! declared dependency edges — not a post-build compliance scan.

use crate::domain::{StackLock, Universe};
use cyclonedx_bom::models::component::{Classification, Component, Components};
use cyclonedx_bom::models::dependency::{Dependencies, Dependency};
use cyclonedx_bom::models::lifecycle::{Lifecycle, Lifecycles, Phase};
use cyclonedx_bom::models::metadata::Metadata;
use cyclonedx_bom::models::property::{Properties, Property};
use cyclonedx_bom::models::tool::{Tool, Tools};
use cyclonedx_bom::prelude::*;
use serde_json::Value;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::str::FromStr;

fn bom_ref(name: &str, version: &str, toolchain_label: &str) -> String {
    format!("pkg:generic/{name}@{version}?toolchain={toolchain_label}")
}

/// Build a CycloneDX JSON document from a lock only (no dependency map).
///
/// Without declared edges each component gets an empty `dependsOn` list —
/// never all-to-all co-stack edges (those create invalid cyclic BOMs).
pub fn lock_to_cyclonedx(lock: &StackLock) -> Value {
    lock_to_cyclonedx_with_deps(lock, None)
}

/// Preferred: when the selected candidates (or full universe selection map) are known,
/// emit dependsOn from each package's *declared* EasyBuild-style dependency list
/// intersected with co-selected lock members. When `selected_dep_map` is `None`,
/// each package's `dependsOn` is empty (unknown), not all-to-all.
pub fn lock_to_cyclonedx_with_deps(
    lock: &StackLock,
    selected_dep_map: Option<&HashMap<String, Vec<String>>>,
) -> Value {
    lock_to_cyclonedx_with_runtime_and_build(lock, selected_dep_map, None)
}

/// Like [`lock_to_cyclonedx_with_deps`], also records build-time edges as a
/// component property (`eb_stack:buildDependsOn`) while runtime edges fill
/// the CycloneDX `dependencies` graph.
pub fn lock_to_cyclonedx_with_runtime_and_build(
    lock: &StackLock,
    runtime_dep_map: Option<&HashMap<String, Vec<String>>>,
    build_dep_map: Option<&HashMap<String, Vec<String>>>,
) -> Value {
    let bom = lock_to_bom(lock, runtime_dep_map, build_dep_map);
    bom_to_json_value(bom)
}

/// Typed CycloneDX BOM (1.5 models) — preferred when callers want validation.
pub fn lock_to_bom(
    lock: &StackLock,
    runtime_dep_map: Option<&HashMap<String, Vec<String>>>,
    build_dep_map: Option<&HashMap<String, Vec<String>>>,
) -> Bom {
    let toolchain_label = lock.toolchain.label();
    let mut package_refs: HashMap<String, String> = HashMap::new();
    let mut components: Vec<Component> = Vec::new();

    for p in &lock.packages {
        let r = bom_ref(&p.name, &p.version, &toolchain_label);
        package_refs.insert(p.name.clone(), r.clone());

        let mut props = vec![
            Property::new("easybuild:toolchain", &toolchain_label),
            Property::new("easybuild:easyconfig_path", &p.easyconfig_path),
            Property::new("eb_stack:lifecycle", "pre-install-plan"),
        ];
        if let Some(vs) = p.versionsuffix.as_deref() {
            if !vs.is_empty() {
                props.push(Property::new("easybuild:versionsuffix", vs));
            }
        }
        if let Some(bmap) = build_dep_map {
            if let Some(bdeps) = bmap.get(&p.name) {
                if !bdeps.is_empty() {
                    let joined = bdeps
                        .iter()
                        .filter_map(|n| package_refs.get(n).cloned().or_else(|| Some(n.clone())))
                        .collect::<Vec<_>>()
                        .join(",");
                    props.push(Property::new("eb_stack:buildDependsOn", &joined));
                }
            }
        }

        let purl_str = r.clone();
        let mut component = Component::new(
            Classification::Library,
            &p.name,
            &p.version,
            Some(r),
        );
        component.purl = Purl::from_str(&purl_str).ok();
        component.properties = Some(Properties(props));
        components.push(component);
    }

    let mut deps: Vec<Dependency> = Vec::new();
    for p in &lock.packages {
        let r = package_refs.get(&p.name).cloned().unwrap();
        let depends_on: Vec<String> = if let Some(map) = runtime_dep_map {
            map.get(&p.name)
                .into_iter()
                .flatten()
                .filter_map(|dep_name| package_refs.get(dep_name).cloned())
                .collect()
        } else {
            Vec::new()
        };
        deps.push(Dependency {
            dependency_ref: r,
            dependencies: depends_on,
        });
    }

    let stack_name = format!("easybuild-stack-{}", toolchain_label);
    let stack_ver = lock
        .generation_label
        .clone()
        .unwrap_or_else(|| toolchain_label.clone());
    let mut meta_component = Component::new(
        Classification::Application,
        &stack_name,
        &stack_ver,
        Some(format!("pkg:generic/{stack_name}@{stack_ver}")),
    );
    meta_component.description = Some(NormalizedString::new(
        "Planned EasyBuild stack inventory from eb-stack lock (pre-install; not a post-build compliance scan)",
    ));

    let mut metadata = Metadata::new().unwrap_or_default();
    // Prefer lock solver timestamp when parseable as ISO-8601.
    if let Ok(dt) = DateTime::try_from(lock.solver.timestamp.clone()) {
        metadata.timestamp = Some(dt);
    }
    metadata.tools = Some(Tools::List(vec![Tool::new(
        "SURF",
        "eb-stack",
        &lock.solver.engine_version,
    )]));
    metadata.component = Some(meta_component);
    metadata.properties = Some(Properties(vec![
        Property::new("eb_stack:document_kind", "planned-sbom-from-lock"),
        Property::new("eb_stack:solver_engine", &lock.solver.engine),
        Property::new("eb_stack:toolchain", &toolchain_label),
    ]));
    metadata.lifecycles = Some(Lifecycles(vec![Lifecycle::Phase(Phase::PreBuild)]));

    Bom {
        version: 1,
        serial_number: Some(UrnUuid::generate()),
        metadata: Some(metadata),
        components: Some(Components(components)),
        services: None,
        external_references: None,
        dependencies: Some(Dependencies(deps)),
        compositions: None,
        properties: None,
        vulnerabilities: None,
        signature: None,
        annotations: None,
        formulation: None,
        spec_version: SpecVersion::V1_5,
    }
}

fn bom_to_json_value(bom: Bom) -> Value {
    let mut buf = Vec::new();
    bom.output_as_json_v1_5(&mut buf)
        .expect("cyclonedx-bom JSON 1.5 serialize");
    serde_json::from_slice(&buf).expect("cyclonedx JSON is valid serde_json::Value")
}

/// Build dep map name -> **runtime** dependency names from universe candidates
/// matching the lock. Build-time deps are intentionally omitted here so SBOM
/// `dependsOn` edges stay role-specific; use [`build_dep_map_from_universe`] for
/// the build-time list (same shape, separate map).
pub fn dep_map_from_universe(lock: &StackLock, universe: &Universe) -> HashMap<String, Vec<String>> {
    dep_names_map_from_universe(lock, universe, false)
}

/// Build dep map name -> **build-time** dependency names (`builddependencies`)
/// from universe candidates matching the lock.
pub fn build_dep_map_from_universe(
    lock: &StackLock,
    universe: &Universe,
) -> HashMap<String, Vec<String>> {
    dep_names_map_from_universe(lock, universe, true)
}

fn dep_names_map_from_universe(
    lock: &StackLock,
    universe: &Universe,
    build_time: bool,
) -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    for p in &lock.packages {
        if let Some(c) = universe.candidates.iter().find(|c| {
            c.name == p.name
                && c.version == p.version
                && c.toolchain.name == p.toolchain.name
                && c.toolchain.version == p.toolchain.version
        }) {
            let names: Vec<String> = if build_time {
                c.builddependencies.iter().map(|d| d.name.clone()).collect()
            } else {
                c.dependencies.iter().map(|d| d.name.clone()).collect()
            };
            map.insert(p.name.clone(), names);
        } else {
            map.insert(p.name.clone(), Vec::new());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::*;
    use crate::select::select_stack;
    use std::path::PathBuf;

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/gromacs_2025_to_next")
    }

    fn load_json<T: serde::de::DeserializeOwned>(name: &str) -> T {
        let p = fixture_dir().join(name);
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    /// cyclonedx-bom skips serializing empty `dependsOn` arrays.
    fn depends_on_list(dep: &Value) -> Vec<String> {
        dep.get("dependsOn")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn sbom_uses_declared_deps_not_gromacs_only_hardcode() {
        let baseline: StackLock = load_json("baseline.lock.json");
        let universe: Universe = load_json("universe_next.json");
        let policy: Policy = load_json("policy_prefer_newer.json");
        let lock = select_stack(&universe, &policy, Some(&baseline)).unwrap();
        let map = dep_map_from_universe(&lock, &universe);
        let sbom = lock_to_cyclonedx_with_deps(&lock, Some(&map));
        assert_eq!(sbom["bomFormat"], "CycloneDX");
        assert_eq!(sbom["specVersion"], "1.5");
        assert!(
            sbom["serialNumber"]
                .as_str()
                .unwrap_or("")
                .starts_with("urn:uuid:"),
            "serialNumber: {:?}",
            sbom["serialNumber"]
        );
        let deps = sbom["dependencies"].as_array().expect("dependencies array");
        // GROMACS declares real co-deps (Python + stack libs), not empty/hardcoded.
        let g_ref = deps
            .iter()
            .find(|d| {
                d.get("ref")
                    .and_then(|r| r.as_str())
                    .is_some_and(|s| s.contains("GROMACS"))
            })
            .unwrap_or_else(|| panic!("GROMACS dep entry missing in {deps:?}"));
        let g_on = depends_on_list(g_ref);
        assert!(
            g_on.iter().any(|x| x.contains("OpenBLAS")),
            "GROMACS must list OpenBLAS dep: {g_on:?}"
        );
        assert!(
            g_on.iter().any(|x| x.contains("Python")),
            "GROMACS must list Python dep: {g_on:?}"
        );
        assert!(g_on.len() >= 3, "GROMACS dependsOn co-deps: {g_on:?}");
        // Leaf FFTW has no runtime deps in the realistic fixture.
        let fftw_ref = deps
            .iter()
            .find(|d| {
                d.get("ref")
                    .and_then(|r| r.as_str())
                    .is_some_and(|s| s.contains("FFTW"))
            })
            .expect("FFTW dep entry");
        let fftw_on = depends_on_list(fftw_ref);
        assert!(
            fftw_on.is_empty(),
            "FFTW leaf should have empty dependsOn: {fftw_on:?}"
        );
        // Lock-only path: no all-to-all co-stack edges (empty when map unknown).
        // cyclonedx-bom omits empty dependsOn (skip_serializing_if empty).
        let co = lock_to_cyclonedx(&lock);
        for d in co["dependencies"].as_array().unwrap() {
            let on = depends_on_list(d);
            assert!(
                on.is_empty(),
                "lock-only SBOM must not invent all-to-all dependsOn: {on:?}"
            );
        }
        // And never every-other-package.
        let others = lock.packages.len().saturating_sub(1);
        if others > 0 {
            for d in co["dependencies"].as_array().unwrap() {
                let on = depends_on_list(d);
                assert_ne!(
                    on.len(),
                    others,
                    "dependsOn must not be all other packages"
                );
            }
        }
        // Typed BOM validates under cyclonedx-bom.
        let bom = lock_to_bom(&lock, Some(&map), None);
        let vr = bom.validate();
        assert!(
            vr.passed(),
            "cyclonedx-bom Validate failed: {vr:?}"
        );
    }

    #[test]
    fn lock_only_sbom_depends_on_is_empty_not_all_to_all() {
        let lock: StackLock = load_json("expected_prefer_newer.lock.json");
        assert!(
            lock.packages.len() >= 3,
            "fixture must have several packages so all-to-all would be visible"
        );
        let sbom = lock_to_cyclonedx(&lock);
        let deps = sbom["dependencies"].as_array().expect("dependencies array");
        assert_eq!(deps.len(), lock.packages.len());
        for d in deps {
            let on = depends_on_list(d);
            assert!(
                on.is_empty(),
                "without a dep map dependsOn must be empty, got {on:?} for {:?}",
                d.get("ref")
            );
        }
        // Real declared-map path still has non-empty edges for GROMACS.
        let universe: Universe = load_json("universe_next.json");
        let map = dep_map_from_universe(&lock, &universe);
        let with_map = lock_to_cyclonedx_with_deps(&lock, Some(&map));
        let g = with_map["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| {
                d.get("ref")
                    .and_then(|r| r.as_str())
                    .is_some_and(|s| s.contains("GROMACS"))
            })
            .expect("GROMACS entry");
        assert!(
            !depends_on_list(g).is_empty(),
            "declared map must still emit real edges"
        );
    }

    #[test]
    fn dep_maps_keep_build_and_runtime_distinct() {
        let tc = Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        };
        let app = Candidate {
            name: "App".into(),
            version: "1.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "App.eb".into(),
            dependencies: vec![DepReq {
                name: "Lib".into(),
                version_req: "==1.0".into(),
                versionsuffix: None,
                toolchain: None,
            }],
            builddependencies: vec![DepReq {
                name: "Tool".into(),
                version_req: "==1.0".into(),
                versionsuffix: None,
                toolchain: None,
            }],
            exts_list: vec![],
        };
        let lib = Candidate {
            name: "Lib".into(),
            version: "1.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "Lib.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
        };
        let tool = Candidate {
            name: "Tool".into(),
            version: "1.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "Tool.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
        };
        let universe = Universe {
            toolchain: tc.clone(),
            generation_label: None,
            candidates: vec![app, lib, tool],
        };
        let policy = Policy {
            toolchain: tc,
            roots: vec!["App".into()],
            root_priority: None,
            pins: vec![],
            forbid: vec![],
            objective: "prefer_newer".into(),
            require_upgrade: vec![],
        };
        let lock = select_stack(&universe, &policy, None).unwrap();
        let runtime = dep_map_from_universe(&lock, &universe);
        let build = build_dep_map_from_universe(&lock, &universe);
        assert_eq!(runtime.get("App").unwrap(), &vec!["Lib".to_string()]);
        assert_eq!(build.get("App").unwrap(), &vec!["Tool".to_string()]);
        assert!(
            !runtime.get("App").unwrap().contains(&"Tool".to_string()),
            "runtime map must not include build-only deps"
        );
        assert!(
            !build.get("App").unwrap().contains(&"Lib".to_string()),
            "build map must not include runtime-only deps"
        );
        // Serialized candidate still carries both roles separately.
        let app_c = universe.candidates.iter().find(|c| c.name == "App").unwrap();
        let json = serde_json::to_value(app_c).unwrap();
        assert_eq!(json["dependencies"][0]["name"], "Lib");
        assert_eq!(json["builddependencies"][0]["name"], "Tool");

        // Build edges land on property, not runtime dependsOn.
        let sbom = lock_to_cyclonedx_with_runtime_and_build(
            &lock,
            Some(&runtime),
            Some(&build),
        );
        let comps = sbom["components"].as_array().unwrap();
        let app_c = comps
            .iter()
            .find(|c| c["name"].as_str() == Some("App"))
            .unwrap();
        let props = app_c["properties"].as_array().unwrap();
        assert!(
            props.iter().any(|p| {
                p["name"].as_str() == Some("eb_stack:buildDependsOn")
                    && p["value"]
                        .as_str()
                        .unwrap_or("")
                        .contains("Tool")
            }),
            "buildDependsOn property missing: {props:?}"
        );
    }

    #[test]
    fn universe_json_without_builddependencies_deserializes() {
        let universe: Universe = load_json("universe_next.json");
        for c in &universe.candidates {
            assert!(
                c.builddependencies.is_empty(),
                "legacy universe JSON should default builddependencies to empty for {}",
                c.name
            );
        }
    }
}
