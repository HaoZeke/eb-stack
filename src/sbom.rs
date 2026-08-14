//! Planned CycloneDX 1.5 SBOM from a stack lock (pre-install inventory).
//!
//! Built with the official [`cyclonedx_bom`] crate (same models as
//! `cargo-cyclonedx` / CycloneDX Rust Cargo). Documents are serialized as
//! JSON 1.5 with serial numbers, tool metadata, lifecycle phase, and
//! declared dependency edges — not a post-build compliance scan.

use crate::domain::{StackLock, Universe};
use cyclonedx_bom::models::bom::BomReference;
use cyclonedx_bom::models::component::{Classification, Component, Components};
use cyclonedx_bom::models::composition::{AggregateType, Composition, Compositions};
use cyclonedx_bom::models::dependency::{Dependencies, Dependency};
use cyclonedx_bom::models::external_reference::{
    ExternalReference, ExternalReferenceType, ExternalReferences, Uri as ExternalUri,
};
use cyclonedx_bom::models::formulation::workflow::input::{Input, RequiredInputField};
use cyclonedx_bom::models::formulation::workflow::output::{
    Output, RequiredOutputField, Type as OutputType,
};
use cyclonedx_bom::models::formulation::workflow::resource_reference::ResourceReference;
use cyclonedx_bom::models::formulation::workflow::step::{Command, Step};
use cyclonedx_bom::models::formulation::workflow::{Task, TaskType, Workflow};
use cyclonedx_bom::models::formulation::Formula;
use cyclonedx_bom::models::hash::{Hash, HashAlgorithm, HashValue, Hashes};
use cyclonedx_bom::models::lifecycle::{Lifecycle, Lifecycles, Phase};
use cyclonedx_bom::models::metadata::Metadata;
use cyclonedx_bom::models::property::{Properties, Property};
use cyclonedx_bom::models::tool::{Tool, Tools};
use cyclonedx_bom::prelude::*;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::convert::TryFrom;
use std::str::FromStr;

fn bom_ref(name: &str, version: &str, toolchain_label: &str) -> String {
    format!("pkg:generic/{name}@{version}?toolchain={toolchain_label}")
}

/// What an easyconfig states about the artifact one component builds from.
///
/// A lock records a selection, not an artifact. These are the fields that make
/// the difference between an inventory and a document someone can verify:
/// which bytes were expected, and where they were fetched from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactFacts {
    /// Checksums the easyconfig states, in the order it states them. Only
    /// 64-character hex values are emitted as SHA-256; anything else is
    /// carried as a property rather than asserted as a hash of the wrong kind.
    pub checksums: Vec<String>,
    /// Source URLs the easyconfig downloads from.
    pub source_urls: Vec<String>,
    /// Patch filenames applied on top of the source.
    pub patches: Vec<String>,
}

/// Everything a caller can tell the SBOM builder beyond the lock itself.
///
/// Grouped into one struct so the builder keeps a single entry point as more
/// of the spec is filled in, rather than growing another positional argument
/// per field.
#[derive(Debug, Default, Clone, Copy)]
pub struct SbomFacts<'a> {
    /// Runtime edges, name to names, which become the `dependencies` graph.
    pub runtime_dep_map: Option<&'a HashMap<String, Vec<String>>>,
    /// Build edges, kept as a property because CycloneDX `dependencies` does
    /// not distinguish build from runtime in 1.5.
    pub build_dep_map: Option<&'a HashMap<String, Vec<String>>>,
    /// Per-package artifact facts, keyed by package name.
    pub artifacts: Option<&'a HashMap<String, ArtifactFacts>>,
    /// Requirements the plan could not resolve. Their presence is what makes
    /// the document's `compositions` say `incomplete` rather than `complete`.
    pub unresolved: Option<&'a [String]>,
    /// Input hash per package name, from [`crate::input_hash`]. Used as the
    /// `uid` of the task that builds it, which is what CycloneDX means by a
    /// unique identifier for a resource instance: two plans producing the same
    /// hash describe the same build.
    pub input_hashes: Option<&'a HashMap<String, String>>,
    /// The build environment the plan targets: optimisation flags, compute
    /// capabilities, the EasyBuild version. A build is reproducible only
    /// against a stated environment, since the same recipe compiled with
    /// different flags is a different binary, and an SBOM that omits it
    /// describes something nobody can reproduce (doi:10.1109/ms.2021.3073045).
    pub build_environment: Option<&'a BTreeMap<String, String>>,
}

/// A SHA-256 as the spec wants it, or nothing.
///
/// EasyBuild checksums are usually SHA-256 but the parameter also carries
/// `('md5', ...)` tuples, dict-per-source forms and, historically, bare MD5.
/// Guessing the algorithm from a string of the wrong length would put a false
/// assertion in a document whose only purpose is to be trusted.
fn sha256_hash(value: &str) -> Option<Hash> {
    let candidate = value.trim();
    if candidate.len() != 64 || !candidate.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(Hash {
        alg: HashAlgorithm::SHA_256,
        content: HashValue(candidate.to_ascii_lowercase()),
    })
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
    lock_to_bom_with_facts(
        lock,
        SbomFacts {
            runtime_dep_map,
            build_dep_map,
            ..SbomFacts::default()
        },
    )
}

/// Build the BOM from a lock plus whatever else the caller knows.
///
/// Everything beyond the lock is optional, and what is absent is left absent
/// rather than guessed: a component with no stated checksum carries no
/// `hashes`, and a plan with nothing unresolved is the only one that claims
/// `complete`.
pub fn lock_to_bom_with_facts(lock: &StackLock, facts: SbomFacts<'_>) -> Bom {
    let SbomFacts {
        runtime_dep_map,
        build_dep_map,
        artifacts,
        unresolved,
        input_hashes,
        build_environment,
    } = facts;
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
        let mut component = Component::new(Classification::Library, &p.name, &p.version, Some(r));
        component.purl = Purl::from_str(&purl_str).ok();

        if let Some(facts) = artifacts.and_then(|m| m.get(&p.name)) {
            let hashes: Vec<Hash> = facts
                .checksums
                .iter()
                .filter_map(|c| sha256_hash(c))
                .collect();
            if !hashes.is_empty() {
                component.hashes = Some(Hashes(hashes));
            }
            // A checksum the spec cannot express as a hash is still evidence,
            // and dropping it silently would hide that the recipe states one.
            for stated in &facts.checksums {
                if sha256_hash(stated).is_none() && !stated.trim().is_empty() {
                    props.push(Property::new("easybuild:checksum_unmapped", stated));
                }
            }
            let refs: Vec<ExternalReference> = facts
                .source_urls
                .iter()
                .filter_map(|url| {
                    Uri::try_from(url.clone())
                        .ok()
                        .map(|uri| ExternalReference {
                            external_reference_type: ExternalReferenceType::Distribution,
                            url: ExternalUri::Url(uri),
                            comment: Some("source_urls entry of the easyconfig".to_string()),
                            hashes: None,
                        })
                })
                .collect();
            if !refs.is_empty() {
                component.external_references = Some(ExternalReferences(refs));
            }
            // Patches are named, not carried: CycloneDX pedigree wants the diff
            // itself, and an easyconfig states only a filename in its own tree.
            for patch in &facts.patches {
                props.push(Property::new("easybuild:patch", patch));
            }
        }

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
    let stack_ref = format!("pkg:generic/{stack_name}@{stack_ver}");
    let mut meta_component = Component::new(
        Classification::Application,
        &stack_name,
        &stack_ver,
        Some(stack_ref.clone()),
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
        formulation: Some(vec![build_formula(
            &stack_ref,
            lock,
            &package_refs,
            runtime_dep_map,
            build_dep_map,
            input_hashes,
            build_environment,
        )]),
        compositions: Some(Compositions(vec![compositions_statement(
            &stack_ref, unresolved,
        )])),
        properties: None,
        vulnerabilities: None,
        signature: None,
        annotations: None,
        spec_version: SpecVersion::V1_5,
    }
}

/// How this stack is built, as CycloneDX formulation.
///
/// The document already says what a stack contains. This says how it is made,
/// and it is the only place two things eb-stack knows can live: the build
/// order, because the top-level `dependencies` array is a runtime relation and
/// validated against component references, and the identity of each build,
/// which is the input hash.
///
/// One workflow for the stack, one task per module, and `taskDependencies` for
/// the edges. Nothing is timed and no trigger is claimed: the lifecycle is
/// pre-build, nothing has run, and a fabricated timestamp would be the one
/// untrue thing in the document.
fn build_formula(
    stack_ref: &str,
    lock: &StackLock,
    package_refs: &HashMap<String, String>,
    runtime_dep_map: Option<&HashMap<String, Vec<String>>>,
    build_dep_map: Option<&HashMap<String, Vec<String>>>,
    input_hashes: Option<&HashMap<String, String>>,
    build_environment: Option<&BTreeMap<String, String>>,
) -> Formula {
    let task_ref = |package_ref: &str| format!("{package_ref}#task");
    let mut tasks: Vec<Task> = Vec::new();
    let mut task_edges: Vec<Dependency> = Vec::new();

    for package in &lock.packages {
        let Some(package_ref) = package_refs.get(&package.name) else {
            continue;
        };
        let uid = input_hashes
            .and_then(|m| m.get(&package.name).cloned())
            .unwrap_or_else(|| package_ref.clone());

        let mut inputs: Vec<Input> = Vec::new();
        let mut edges: Vec<String> = Vec::new();
        for map in [runtime_dep_map, build_dep_map].into_iter().flatten() {
            for dep_name in map.get(&package.name).into_iter().flatten() {
                if let Some(dep_ref) = package_refs.get(dep_name) {
                    // A task consumes the component its dependency produced,
                    // which is what ties the how back to the what.
                    inputs.push(Input {
                        required: RequiredInputField::Resource(ResourceReference::Ref(
                            dep_ref.clone(),
                        )),
                        source: None,
                        target: None,
                        properties: None,
                    });
                    edges.push(task_ref(dep_ref));
                }
            }
        }
        edges.sort();
        edges.dedup();

        tasks.push(Task {
            bom_ref: BomReference::new(task_ref(package_ref)),
            uid,
            name: Some(format!("{}/{}", package.name, package.version)),
            description: Some(format!(
                "EasyBuild install from {}",
                package.easyconfig_path
            )),
            resource_references: None,
            task_types: vec![TaskType::Build],
            trigger: None,
            // One invocation. EasyBuild runs configure, build, install and the
            // sanity check inside it, and splitting them here would claim a
            // decomposition the plan does not control.
            steps: Some(vec![Step {
                name: Some("easybuild".into()),
                description: None,
                commands: Some(vec![Command {
                    // Not a Property: those normalize whitespace, and a command
                    // is worth keeping verbatim.
                    executed: Some(format!("eb {} --robot", package.easyconfig_path)),
                    properties: None,
                }]),
                properties: None,
            }]),
            inputs: if inputs.is_empty() {
                None
            } else {
                Some(inputs)
            },
            outputs: Some(vec![Output {
                required: RequiredOutputField::Resource(ResourceReference::Ref(
                    package_ref.clone(),
                )),
                r#type: Some(OutputType::Artifact),
                source: None,
                target: None,
                properties: None,
            }]),
            time_start: None,
            time_end: None,
            workspaces: None,
            runtime_topology: None,
            properties: Some(Properties(vec![Property::new(
                "easybuild:easyconfig_path",
                &package.easyconfig_path,
            )])),
        });

        task_edges.push(Dependency {
            dependency_ref: task_ref(package_ref),
            dependencies: edges,
        });
    }

    Formula {
        bom_ref: Some(BomReference::new(format!("{stack_ref}#formula"))),
        components: None,
        services: None,
        workflows: Some(vec![Workflow {
            bom_ref: BomReference::new(format!("{stack_ref}#workflow")),
            uid: lock
                .generation_label
                .clone()
                .unwrap_or_else(|| lock.toolchain.label()),
            name: Some(format!("{stack_ref} build")),
            description: Some("EasyBuild module builds in dependency order".into()),
            resource_references: None,
            tasks: Some(tasks),
            // The build order. Deliberately not the top-level dependencies
            // array, which describes a runtime relation between components.
            task_dependencies: Some(task_edges),
            task_types: vec![TaskType::Build],
            trigger: None,
            steps: None,
            inputs: None,
            outputs: None,
            time_start: None,
            time_end: None,
            workspaces: None,
            runtime_topology: None,
            properties: None,
        }]),
        // The environment every task in this workflow is built against, stated
        // once rather than repeated per task.
        properties: build_environment.map(|env| {
            Properties(
                env.iter()
                    .map(|(name, value)| Property::new(name, value))
                    .collect(),
            )
        }),
    }
}

/// State whether this document accounts for everything the plan needed.
///
/// A planned SBOM is worth as much as its completeness claim, and CycloneDX
/// gives that claim a field rather than leaving it to prose. A plan that left
/// requirements unresolved describes an `incomplete` composition, and each
/// unresolved requirement is named as a property so the gap is readable
/// without going back to the residuals report.
fn compositions_statement(stack_ref: &str, unresolved: Option<&[String]>) -> Composition {
    let missing = unresolved.unwrap_or(&[]);
    Composition {
        bom_ref: None,
        aggregate: if missing.is_empty() {
            AggregateType::Complete
        } else {
            AggregateType::Incomplete
        },
        assemblies: Some(vec![BomReference::new(stack_ref)]),
        dependencies: None,
        vulnerabilities: None,
        signature: None,
    }
}

/// Read the easyconfigs a lock names, for the facts only they carry.
///
/// This is the one place the module touches the filesystem, and it earns it: a
/// lock records which easyconfig was selected, and the checksums and source
/// URLs that make the document verifiable live in that file rather than in the
/// lock. A path that cannot be read or parsed contributes nothing rather than
/// failing the document, since an SBOM missing one component's hashes is worth
/// more than no SBOM, and the count of what was read is reported to the caller.
pub fn artifact_facts_for_lock(lock: &StackLock) -> HashMap<String, ArtifactFacts> {
    let mut out = HashMap::new();
    for package in &lock.packages {
        if package.easyconfig_path.is_empty() {
            continue;
        }
        let Ok(resolved) = crate::eb_parse::resolve_easyconfig_file(std::path::Path::new(
            &package.easyconfig_path,
        )) else {
            continue;
        };
        if resolved.checksums.is_empty() && resolved.source_urls.is_empty() {
            continue;
        }
        out.insert(
            package.name.clone(),
            ArtifactFacts {
                checksums: resolved.checksums.clone(),
                source_urls: resolved.source_urls.clone(),
                patches: Vec::new(),
            },
        );
    }
    out
}

/// JSON document from a lock plus caller-supplied facts.
pub fn lock_to_cyclonedx_with_facts(lock: &StackLock, facts: SbomFacts<'_>) -> Value {
    bom_to_json_value(lock_to_bom_with_facts(lock, facts))
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
pub fn dep_map_from_universe(
    lock: &StackLock,
    universe: &Universe,
) -> HashMap<String, Vec<String>> {
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
                assert_ne!(on.len(), others, "dependsOn must not be all other packages");
            }
        }
        // Typed BOM validates under cyclonedx-bom.
        let bom = lock_to_bom(&lock, Some(&map), None);
        let vr = bom.validate();
        assert!(vr.passed(), "cyclonedx-bom Validate failed: {vr:?}");
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
            prefer_installed: false,
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
        let app_c = universe
            .candidates
            .iter()
            .find(|c| c.name == "App")
            .unwrap();
        let json = serde_json::to_value(app_c).unwrap();
        assert_eq!(json["dependencies"][0]["name"], "Lib");
        assert_eq!(json["builddependencies"][0]["name"], "Tool");

        // Build edges land on property, not runtime dependsOn.
        let sbom = lock_to_cyclonedx_with_runtime_and_build(&lock, Some(&runtime), Some(&build));
        let comps = sbom["components"].as_array().unwrap();
        let app_c = comps
            .iter()
            .find(|c| c["name"].as_str() == Some("App"))
            .unwrap();
        let props = app_c["properties"].as_array().unwrap();
        assert!(
            props.iter().any(|p| {
                p["name"].as_str() == Some("eb_stack:buildDependsOn")
                    && p["value"].as_str().unwrap_or("").contains("Tool")
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

#[cfg(test)]
mod artifact_facts_tests {
    use super::*;
    use crate::domain::*;

    fn one_package_lock() -> StackLock {
        StackLock {
            schema_version: 1,
            toolchain: Toolchain {
                name: "foss".into(),
                version: "2025a".into(),
            },
            generation_label: Some("2025a".into()),
            packages: vec![LockPackage {
                name: "Example".into(),
                version: "1.2.3".into(),
                toolchain: Toolchain {
                    name: "foss".into(),
                    version: "2025a".into(),
                },
                versionsuffix: None,
                easyconfig_path: "e/Example/Example-1.2.3-foss-2025a.eb".into(),
            }],
            solver: SolverMeta {
                engine: "resolvo".into(),
                engine_version: "0.0.0".into(),
                timestamp: "2026-08-12T00:00:00Z".into(),
            },
        }
    }

    fn facts_map(facts: ArtifactFacts) -> HashMap<String, ArtifactFacts> {
        HashMap::from([("Example".to_string(), facts)])
    }

    fn component(sbom: &Value) -> Value {
        sbom.get("components").unwrap().as_array().unwrap()[0].clone()
    }

    #[test]
    fn a_stated_sha256_becomes_a_hash_the_spec_can_verify() {
        let sha = "e".repeat(64);
        let map = facts_map(ArtifactFacts {
            checksums: vec![sha.clone()],
            ..Default::default()
        });
        let bom = lock_to_bom_with_facts(
            &one_package_lock(),
            SbomFacts {
                artifacts: Some(&map),
                ..SbomFacts::default()
            },
        );
        let c = component(&bom_to_json_value(bom));
        let hashes = c.get("hashes").unwrap().as_array().unwrap();
        assert_eq!(hashes[0]["alg"], "SHA-256");
        assert_eq!(hashes[0]["content"], sha);
    }

    /// EasyBuild also carries md5 values and dict-per-source forms. Calling one
    /// of those SHA-256 would put a false assertion in the document.
    #[test]
    fn a_checksum_of_another_kind_is_recorded_but_not_asserted_as_sha256() {
        let map = facts_map(ArtifactFacts {
            checksums: vec!["d41d8cd98f00b204e9800998ecf8427e".into()],
            ..Default::default()
        });
        let bom = lock_to_bom_with_facts(
            &one_package_lock(),
            SbomFacts {
                artifacts: Some(&map),
                ..SbomFacts::default()
            },
        );
        let c = component(&bom_to_json_value(bom));
        assert!(c.get("hashes").is_none(), "{c}");
        let props = c.get("properties").unwrap().as_array().unwrap().clone();
        assert!(
            props
                .iter()
                .any(|p| p["name"] == "easybuild:checksum_unmapped"),
            "{props:?}"
        );
    }

    #[test]
    fn source_urls_become_distribution_references_and_patches_are_named() {
        let map = facts_map(ArtifactFacts {
            checksums: vec![],
            source_urls: vec!["https://example.org/src/Example-1.2.3.tar.gz".into()],
            patches: vec!["Example-1.2.3_fix-build.patch".into()],
        });
        let bom = lock_to_bom_with_facts(
            &one_package_lock(),
            SbomFacts {
                artifacts: Some(&map),
                ..SbomFacts::default()
            },
        );
        let c = component(&bom_to_json_value(bom));
        let refs = c.get("externalReferences").unwrap().as_array().unwrap();
        assert_eq!(refs[0]["type"], "distribution");
        assert_eq!(
            refs[0]["url"],
            "https://example.org/src/Example-1.2.3.tar.gz"
        );
        let props = c.get("properties").unwrap().as_array().unwrap();
        assert!(
            props
                .iter()
                .any(|p| p["name"] == "easybuild:patch"
                    && p["value"] == "Example-1.2.3_fix-build.patch"),
            "{props:?}"
        );
    }

    #[test]
    fn a_plan_that_resolved_everything_says_complete() {
        let bom = lock_to_bom_with_facts(&one_package_lock(), SbomFacts::default());
        let json = bom_to_json_value(bom);
        let comps = json.get("compositions").unwrap().as_array().unwrap();
        assert_eq!(comps[0]["aggregate"], "complete");
    }

    /// The claim that matters: a plan with residuals must not describe itself
    /// as a complete inventory.
    #[test]
    fn a_plan_with_residuals_says_incomplete() {
        let missing = ["libfoo >= 2".to_string()];
        let bom = lock_to_bom_with_facts(
            &one_package_lock(),
            SbomFacts {
                unresolved: Some(&missing),
                ..SbomFacts::default()
            },
        );
        let json = bom_to_json_value(bom);
        let comps = json.get("compositions").unwrap().as_array().unwrap();
        assert_eq!(comps[0]["aggregate"], "incomplete");
        let assemblies = comps[0]["assemblies"].as_array().unwrap();
        assert!(
            assemblies[0].as_str().unwrap().contains("easybuild-stack"),
            "{assemblies:?}"
        );
    }

    #[test]
    fn a_lock_with_no_facts_carries_no_hashes_or_references() {
        let bom = lock_to_bom_with_facts(&one_package_lock(), SbomFacts::default());
        let c = component(&bom_to_json_value(bom));
        assert!(c.get("hashes").is_none(), "{c}");
        assert!(c.get("externalReferences").is_none(), "{c}");
    }
}

#[cfg(test)]
mod formulation_tests {
    use super::*;
    use crate::domain::*;

    fn lock() -> StackLock {
        let toolchain = Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        };
        StackLock {
            schema_version: 1,
            toolchain: toolchain.clone(),
            generation_label: Some("2026.1".into()),
            packages: vec![
                LockPackage {
                    name: "App".into(),
                    version: "1.0".into(),
                    toolchain: toolchain.clone(),
                    versionsuffix: None,
                    easyconfig_path: "a/App/App-1.0-foss-2026.1.eb".into(),
                },
                LockPackage {
                    name: "Lib".into(),
                    version: "2.0".into(),
                    toolchain,
                    versionsuffix: None,
                    easyconfig_path: "l/Lib/Lib-2.0-foss-2026.1.eb".into(),
                },
            ],
            solver: SolverMeta {
                engine: "resolvo".into(),
                engine_version: "0".into(),
                timestamp: "2026-08-14T00:00:00Z".into(),
            },
        }
    }

    fn document() -> Value {
        let deps = HashMap::from([("App".to_string(), vec!["Lib".to_string()])]);
        let hashes = HashMap::from([
            ("App".to_string(), "aaaa".to_string()),
            ("Lib".to_string(), "bbbb".to_string()),
        ]);
        lock_to_cyclonedx_with_facts(
            &lock(),
            SbomFacts {
                runtime_dep_map: Some(&deps),
                input_hashes: Some(&hashes),
                ..SbomFacts::default()
            },
        )
    }

    #[test]
    fn the_build_order_has_a_home_in_formulation() {
        let json = document();
        let workflow = &json["formulation"][0]["workflows"][0];
        let tasks = workflow["tasks"].as_array().expect("tasks");
        assert_eq!(tasks.len(), 2, "{workflow}");
        let edges = workflow["taskDependencies"].as_array().expect("edges");
        // The task that builds App waits on the task that builds Lib.
        let app_edge = edges
            .iter()
            .find(|e| e["ref"].as_str().unwrap_or("").contains("App"))
            .expect("an edge for App");
        assert!(
            app_edge["dependsOn"][0]
                .as_str()
                .unwrap_or("")
                .contains("Lib"),
            "{app_edge}"
        );
    }

    /// The identity of a build, which a version number cannot express.
    #[test]
    fn a_task_is_identified_by_its_input_hash() {
        let json = document();
        let tasks = json["formulation"][0]["workflows"][0]["tasks"]
            .as_array()
            .unwrap()
            .clone();
        let app = tasks
            .iter()
            .find(|t| t["name"].as_str() == Some("App/1.0"))
            .expect("the App task");
        assert_eq!(app["uid"], "aaaa");
        assert_eq!(app["taskTypes"][0], "build");
        assert!(
            app["steps"][0]["commands"][0]["executed"]
                .as_str()
                .unwrap_or("")
                .starts_with("eb a/App/App-1.0"),
            "{app}"
        );
    }

    /// Task references belong to the workflow alone. The top-level graph is a
    /// runtime relation between components and is validated against component
    /// references, so a task reference there fails an external validator even
    /// though the crate's own validate() would not notice.
    #[test]
    fn task_references_stay_out_of_the_component_graph() {
        let json = document();
        let component_graph = serde_json::to_string(&json["dependencies"]).unwrap();
        assert!(!component_graph.contains("#task"), "{component_graph}");
        let compositions = serde_json::to_string(&json["compositions"]).unwrap();
        assert!(!compositions.contains("#task"), "{compositions}");
    }

    #[test]
    fn nothing_is_timed_because_nothing_has_run() {
        let json = document();
        let workflow = &json["formulation"][0]["workflows"][0];
        assert!(workflow.get("timeStart").is_none(), "{workflow}");
        assert!(workflow.get("timeEnd").is_none(), "{workflow}");
        assert_eq!(json["metadata"]["lifecycles"][0]["phase"], "pre-build");
    }
}

#[cfg(test)]
mod build_environment_tests {
    use super::*;
    use crate::domain::*;

    /// The same recipe compiled with different flags is a different binary, so
    /// an SBOM that omits the environment describes something nobody can
    /// reproduce.
    #[test]
    fn the_environment_a_stack_is_built_against_is_recorded_once() {
        let toolchain = Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        };
        let lock = StackLock {
            schema_version: 1,
            toolchain: toolchain.clone(),
            generation_label: Some("2026.1".into()),
            packages: vec![LockPackage {
                name: "App".into(),
                version: "1.0".into(),
                toolchain,
                versionsuffix: None,
                easyconfig_path: "a/App/App-1.0-foss-2026.1.eb".into(),
            }],
            solver: SolverMeta {
                engine: "resolvo".into(),
                engine_version: "0".into(),
                timestamp: "2026-08-14T00:00:00Z".into(),
            },
        };
        let env = BTreeMap::from([
            (
                "easybuild:optarch".to_string(),
                "GCC:-O3 -march=znver4".to_string(),
            ),
            (
                "easybuild:cuda_compute_capabilities".to_string(),
                "9.0".to_string(),
            ),
        ]);
        let json = lock_to_cyclonedx_with_facts(
            &lock,
            SbomFacts {
                build_environment: Some(&env),
                ..SbomFacts::default()
            },
        );
        let props = json["formulation"][0]["properties"]
            .as_array()
            .expect("props");
        let optarch = props
            .iter()
            .find(|p| p["name"] == "easybuild:optarch")
            .expect("the optimisation flags");
        assert_eq!(optarch["value"], "GCC:-O3 -march=znver4");
        assert!(
            props
                .iter()
                .any(|p| p["name"] == "easybuild:cuda_compute_capabilities"),
            "{props:?}"
        );
    }

    #[test]
    fn an_unstated_environment_is_left_absent_rather_than_invented() {
        let toolchain = Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        };
        let lock = StackLock {
            schema_version: 1,
            toolchain: toolchain.clone(),
            generation_label: None,
            packages: vec![LockPackage {
                name: "App".into(),
                version: "1.0".into(),
                toolchain,
                versionsuffix: None,
                easyconfig_path: "a/App/App-1.0.eb".into(),
            }],
            solver: SolverMeta {
                engine: "resolvo".into(),
                engine_version: "0".into(),
                timestamp: "2026-08-14T00:00:00Z".into(),
            },
        };
        let json = lock_to_cyclonedx_with_facts(&lock, SbomFacts::default());
        assert!(
            json["formulation"][0].get("properties").is_none(),
            "{}",
            json["formulation"][0]
        );
    }
}
