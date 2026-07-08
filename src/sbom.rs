//! Planned CycloneDX 1.5-ish JSON SBOM from a stack lock (pre-install inventory).

use crate::domain::{StackLock, Universe};
use serde_json::{json, Value};
use std::collections::HashMap;

fn bom_ref(name: &str, version: &str, toolchain_label: &str) -> String {
    format!("pkg:generic/{name}@{version}?toolchain={toolchain_label}")
}

/// Build a CycloneDX JSON document from a lock only (co-stack edges: each package
/// lists every other co-selected package as dependsOn — weak co-install inventory).
pub fn lock_to_cyclonedx(lock: &StackLock) -> Value {
    lock_to_cyclonedx_with_deps(lock, None)
}

/// Preferred: when the selected candidates (or full universe selection map) are known,
/// emit dependsOn from each package's *declared* EasyBuild-style dependency list
/// intersected with co-selected lock members. Falls back to co-stack edges if no map.
pub fn lock_to_cyclonedx_with_deps(
    lock: &StackLock,
    selected_dep_map: Option<&HashMap<String, Vec<String>>>,
) -> Value {
    let toolchain_label = lock.toolchain.label();
    let mut components = Vec::new();
    let mut package_refs: HashMap<String, String> = HashMap::new();

    for p in &lock.packages {
        let r = bom_ref(&p.name, &p.version, &toolchain_label);
        package_refs.insert(p.name.clone(), r.clone());
        components.push(json!({
            "type": "library",
            "bom-ref": r,
            "name": p.name,
            "version": p.version,
            "purl": r,
            "properties": [
                { "name": "easybuild:toolchain", "value": toolchain_label },
                { "name": "easybuild:easyconfig_path", "value": p.easyconfig_path },
                { "name": "eb_stack:lifecycle", "value": "pre-install-plan" }
            ]
        }));
    }

    let mut deps = Vec::new();
    for p in &lock.packages {
        let r = package_refs.get(&p.name).cloned().unwrap();
        let depends_on: Vec<String> = if let Some(map) = selected_dep_map {
            map.get(&p.name)
                .into_iter()
                .flatten()
                .filter_map(|dep_name| package_refs.get(dep_name).cloned())
                .collect()
        } else {
            // co-selected stack inventory: every package depends on all *other* co-selected
            package_refs
                .iter()
                .filter(|(n, _)| *n != &p.name)
                .map(|(_, br)| br.clone())
                .collect()
        };
        deps.push(json!({
            "ref": r,
            "dependsOn": depends_on
        }));
    }

    json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "timestamp": lock.solver.timestamp,
            "tools": [{
                "vendor": "SURF",
                "name": "eb-stack",
                "version": lock.solver.engine_version
            }],
            "component": {
                "type": "application",
                "name": format!("easybuild-stack-{}", lock.toolchain.label()),
                "version": lock.generation_label.clone().unwrap_or_else(|| lock.toolchain.label()),
                "description": "Planned EasyBuild stack inventory from eb-stack lock (pre-install; not a post-build compliance scan)"
            },
            "properties": [
                { "name": "eb_stack:document_kind", "value": "planned-sbom-from-lock" },
                { "name": "eb_stack:solver_engine", "value": lock.solver.engine },
                { "name": "eb_stack:toolchain", "value": lock.toolchain.label() }
            ]
        },
        "components": components,
        "dependencies": deps
    })
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

    #[test]
    fn sbom_uses_declared_deps_not_gromacs_only_hardcode() {
        let baseline: StackLock = load_json("baseline.lock.json");
        let universe: Universe = load_json("universe_next.json");
        let policy: Policy = load_json("policy_prefer_newer.json");
        let lock = select_stack(&universe, &policy, Some(&baseline)).unwrap();
        let map = dep_map_from_universe(&lock, &universe);
        let sbom = lock_to_cyclonedx_with_deps(&lock, Some(&map));
        let deps = sbom["dependencies"].as_array().unwrap();
        // FFTW should depend on OpenBLAS and OpenMPI per universe, not empty
        let fftw_ref = deps
            .iter()
            .find(|d| d["ref"].as_str().unwrap().contains("FFTW@"))
            .unwrap();
        let fftw_on = fftw_ref["dependsOn"].as_array().unwrap();
        assert!(
            fftw_on.iter().any(|x| x.as_str().unwrap().contains("OpenBLAS")),
            "FFTW must list OpenBLAS dep: {fftw_on:?}"
        );
        // GROMACS lists co-deps from its candidate
        let g_ref = deps
            .iter()
            .find(|d| d["ref"].as_str().unwrap().contains("GROMACS@"))
            .unwrap();
        let g_on = g_ref["dependsOn"].as_array().unwrap();
        assert!(g_on.len() >= 2, "GROMACS dependsOn co-deps: {g_on:?}");
        // co-stack fallback also non-empty for leaf without hardcoding only GROMACS
        let co = lock_to_cyclonedx(&lock);
        for d in co["dependencies"].as_array().unwrap() {
            let on = d["dependsOn"].as_array().unwrap();
            assert_eq!(on.len(), lock.packages.len() - 1);
        }
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
            }],
            builddependencies: vec![DepReq {
                name: "Tool".into(),
                version_req: "==1.0".into(),
            }],
        };
        let lib = Candidate {
            name: "Lib".into(),
            version: "1.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "Lib.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
        };
        let tool = Candidate {
            name: "Tool".into(),
            version: "1.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "Tool.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
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
            require_upgrade: None,
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
