//! The emitted SBOM has to satisfy the schema it claims to be.
//!
//! `Bom::validate()` in cyclonedx-bom walks metadata, components, services,
//! vulnerabilities, dependencies and compositions, and stops. It never reaches
//! `formulation`, which is where the build order and the per-module input
//! hashes now live, so the richest part of the document is the part nothing in
//! the crate checks. This validates the bytes actually written against the
//! official CycloneDX 1.5 schema, vendored under fixtures/cyclonedx.

use eb_stack::domain::*;
use eb_stack::{lock_to_cyclonedx_with_facts, SbomFacts};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// Resolve the schema's siblings from disk instead of over the network.
///
/// bom-1.5.schema.json refers to spdx and jsf by absolute URL. A test that
/// reached for those would depend on the network and on cyclonedx.org being
/// up, so the two files are vendored beside it and served from there.
struct VendoredSchemas(PathBuf);

impl jsonschema::Retrieve for VendoredSchemas {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<&str>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let name = uri
            .path()
            .as_str()
            .rsplit('/')
            .next()
            .ok_or("a schema reference with no filename")?;
        let text = std::fs::read_to_string(self.0.join(name))?;
        Ok(serde_json::from_str(&text)?)
    }
}

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/cyclonedx")
}

/// The 1.5 schema, with its two siblings resolvable by name.
///
/// The document refers to `spdx.schema.json` and `jsf-0.82.schema.json`, which
/// sit beside it, so the base URI is the directory rather than the file.
fn schema() -> serde_json::Value {
    let text = std::fs::read_to_string(schema_dir().join("bom-1.5.schema.json"))
        .expect("vendored CycloneDX 1.5 schema");
    serde_json::from_str(&text).expect("the schema is JSON")
}

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
                versionsuffix: Some("-CUDA-12.8.0".into()),
                easyconfig_path: "a/App/App-1.0-foss-2026.1-CUDA-12.8.0.eb".into(),
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
            engine_version: "0.3.0".into(),
            timestamp: "2026-08-14T00:00:00Z".into(),
        },
    }
}

/// A document with every section this crate can fill: hashes, external
/// references, a dependency graph, compositions, and formulation.
fn full_document() -> serde_json::Value {
    let runtime = HashMap::from([("App".to_string(), vec!["Lib".to_string()])]);
    let build = HashMap::from([("Lib".to_string(), vec![])]);
    let artifacts = HashMap::from([(
        "App".to_string(),
        eb_stack::ArtifactFacts {
            checksums: vec!["a".repeat(64), "d41d8cd98f00b204e9800998ecf8427e".into()],
            source_urls: vec!["https://example.invalid/app-1.0.tar.gz".into()],
            patches: vec!["App-1.0_fix.patch".into()],
        },
    )]);
    let hashes = HashMap::from([
        ("App".to_string(), "a".repeat(64)),
        ("Lib".to_string(), "b".repeat(64)),
    ]);
    let unresolved = ["libmissing >= 3".to_string()];
    let environment = BTreeMap::from([
        (
            "easybuild:optarch".to_string(),
            "GCC:-O3 -march=znver4".to_string(),
        ),
        (
            "easybuild:cuda_compute_capabilities".to_string(),
            "9.0".to_string(),
        ),
    ]);
    lock_to_cyclonedx_with_facts(
        &lock(),
        SbomFacts {
            runtime_dep_map: Some(&runtime),
            build_dep_map: Some(&build),
            artifacts: Some(&artifacts),
            unresolved: Some(&unresolved),
            input_hashes: Some(&hashes),
            build_environment: Some(&environment),
        },
    )
}

#[test]
fn the_emitted_document_satisfies_the_cyclonedx_schema() {
    let schema = schema();
    let validator = jsonschema::options()
        .with_retriever(VendoredSchemas(schema_dir()))
        .build(&schema)
        .expect("the schema compiles");
    let document = full_document();
    let errors: Vec<String> = validator
        .iter_errors(&document)
        .map(|e| format!("{} at {}", e, e.instance_path))
        .collect();
    assert!(
        errors.is_empty(),
        "the document this crate writes is not valid CycloneDX 1.5:\n{}",
        errors.join("\n")
    );
}

/// The section the crate's own validator never reaches, checked for content
/// rather than only for shape: a build order and an identity per build.
#[test]
fn formulation_carries_the_build_order_and_the_identities() {
    let document = full_document();
    let workflow = &document["formulation"][0]["workflows"][0];
    assert_eq!(workflow["tasks"].as_array().map(Vec::len), Some(2));
    let edges = workflow["taskDependencies"].as_array().expect("edges");
    assert!(!edges.is_empty(), "{workflow}");
    let uids: Vec<&str> = workflow["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["uid"].as_str())
        .collect();
    assert!(uids.iter().all(|u| u.len() == 64), "{uids:?}");
}

/// A validation test that cannot fail proves nothing, so this breaks the
/// document on purpose and insists the validator notices.
///
/// The chosen break is the trap that matters here: `bom-ref` values have a
/// minimum length, and an empty one compiles in Rust and passes the crate's
/// own validate(), because that walk never reaches formulation.
#[test]
fn the_validator_rejects_a_document_it_should_reject() {
    let schema = schema();
    let validator = jsonschema::options()
        .with_retriever(VendoredSchemas(schema_dir()))
        .build(&schema)
        .expect("the schema compiles");

    let mut broken = full_document();
    broken["formulation"][0]["workflows"][0]["bom-ref"] = serde_json::Value::String(String::new());
    assert!(
        !validator.is_valid(&broken),
        "an empty bom-ref in a workflow must not pass: {}",
        broken["formulation"][0]["workflows"][0]
    );

    // A second break, in a section the crate does check, so the test says
    // something about the whole document rather than one corner of it.
    // `bomFormat` is enum-constrained; `specVersion` deliberately is not, since
    // the 1.5 schema declares it a plain string and offers "1.5" only as an
    // example, so a document claiming version 9.9 validates cleanly here. That
    // is worth knowing before trusting schema validation to catch everything.
    let mut wrong_format = full_document();
    wrong_format["bomFormat"] = serde_json::Value::String("SPDX".into());
    assert!(
        !validator.is_valid(&wrong_format),
        "bomFormat is an enum and must be checked"
    );
}
