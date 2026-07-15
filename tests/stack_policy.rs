use eb_stack::package::{
    CandidateExclusion, StackPin, StackPinMode, StackPolicy, STACK_POLICY_SCHEMA_VERSION,
};
use eb_stack::{solve_with_stack_policy, Candidate, DepReq, Policy, Toolchain};

fn toolchain() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    }
}

fn dep(name: &str, requirement: &str) -> DepReq {
    DepReq {
        name: name.into(),
        version_req: requirement.into(),
        versionsuffix: None,
        toolchain: None,
    }
}

fn candidate(name: &str, version: &str, dependencies: Vec<DepReq>) -> Candidate {
    Candidate {
        name: name.into(),
        version: version.into(),
        toolchain: toolchain(),
        versionsuffix: None,
        easyconfig_path: format!("{name}-{version}-foss-2026.1.eb"),
        dependencies,
        builddependencies: Vec::new(),
        exts_list: Vec::new(),
    }
}

fn universe(app_dependencies: Vec<DepReq>) -> Vec<Candidate> {
    vec![
        candidate("zlib", "1.2", Vec::new()),
        candidate("zlib", "1.3", Vec::new()),
        candidate("HDF5", "1.14.2", vec![dep("zlib", "==1.2")]),
        candidate("HDF5", "1.14.3", vec![dep("zlib", "==1.3")]),
        candidate("App", "1.0", app_dependencies),
    ]
}

fn policy() -> Policy {
    Policy {
        toolchain: toolchain(),
        roots: vec!["App".into()],
        root_priority: None,
        pins: Vec::new(),
        forbid: Vec::new(),
        objective: "prefer_newer".into(),
        require_upgrade: Vec::new(),
    }
}

fn stack_policy(mode: StackPinMode) -> StackPolicy {
    StackPolicy {
        schema_version: STACK_POLICY_SCHEMA_VERSION,
        name: "eessi-test".into(),
        toolchain: toolchain(),
        pins: vec![StackPin {
            name: "HDF5".into(),
            version_requirement: "==1.14.2".into(),
            toolchain: None,
            versionsuffix: None,
            mode,
            source: Some("eessi-test.cdx.json".into()),
        }],
        exclusions: Vec::new(),
    }
}

#[test]
fn favored_stack_pin_is_selected_when_satisfiable() {
    let result = solve_with_stack_policy(
        &universe(vec![dep("HDF5", ">=1.14")]),
        &policy(),
        None,
        &stack_policy(StackPinMode::Preferred),
    )
    .expect("favored stack pin solve");
    assert_eq!(
        result
            .selected
            .iter()
            .find(|candidate| candidate.name == "HDF5")
            .expect("HDF5")
            .version,
        "1.14.2"
    );
    let outcome = result
        .pin_outcomes
        .iter()
        .find(|outcome| outcome.name == "HDF5")
        .expect("pin outcome");
    assert!(!outcome.fallback);
    assert_eq!(outcome.selected_version.as_deref(), Some("1.14.2"));
}

#[test]
fn favored_stack_pin_falls_back_through_resolvo() {
    let result = solve_with_stack_policy(
        &universe(vec![dep("HDF5", ">=1.14"), dep("zlib", "==1.3")]),
        &policy(),
        None,
        &stack_policy(StackPinMode::Preferred),
    )
    .expect("compatible fallback solve");
    assert_eq!(
        result
            .selected
            .iter()
            .find(|candidate| candidate.name == "HDF5")
            .expect("HDF5")
            .version,
        "1.14.3"
    );
    let outcome = result
        .pin_outcomes
        .iter()
        .find(|outcome| outcome.name == "HDF5")
        .expect("pin outcome");
    assert!(outcome.fallback);
    assert_eq!(outcome.requested, "==1.14.2");
    assert_eq!(outcome.selected_version.as_deref(), Some("1.14.3"));
    assert!(outcome.fallback_reason.as_deref().is_some_and(|reason| {
        reason.contains("favored candidate") && reason.contains("Resolvo")
    }));
}

#[test]
fn locked_stack_pin_makes_incompatible_graph_unsatisfiable() {
    let error = solve_with_stack_policy(
        &universe(vec![dep("HDF5", ">=1.14"), dep("zlib", "==1.3")]),
        &policy(),
        None,
        &stack_policy(StackPinMode::Locked),
    )
    .expect_err("locked HDF5 must conflict with zlib 1.3");
    assert!(error.contains("unsatisfiable stack"), "{error}");
    assert!(error.contains("HDF5") || error.contains("zlib"), "{error}");
}

#[test]
fn locked_stack_pin_without_full_identity_constrains_resolvo_candidates() {
    let mut compatible = candidate("HDF5", "1.14.2", vec![dep("zlib", "==1.3")]);
    compatible.versionsuffix = Some("-mpi".into());
    compatible.easyconfig_path = "HDF5-1.14.2-foss-2026.1-mpi.eb".into();
    let result = solve_with_stack_policy(
        &[
            candidate("zlib", "1.2", Vec::new()),
            candidate("zlib", "1.3", Vec::new()),
            candidate("HDF5", "1.14.2", vec![dep("zlib", "==1.2")]),
            compatible,
            candidate(
                "App",
                "1.0",
                vec![dep("HDF5", "==1.14.2"), dep("zlib", "==1.3")],
            ),
        ],
        &policy(),
        None,
        &stack_policy(StackPinMode::Locked),
    )
    .expect("locked version requirement must remain a solver constraint");
    let selected = result
        .selected
        .iter()
        .find(|candidate| candidate.name == "HDF5")
        .expect("HDF5");
    assert_eq!(selected.version, "1.14.2");
    assert_eq!(selected.versionsuffix.as_deref(), Some("-mpi"));
}

#[test]
fn exclusions_are_solver_inputs_and_retain_reasons() {
    let mut stack = stack_policy(StackPinMode::Preferred);
    stack.pins.clear();
    stack.exclusions.push(CandidateExclusion {
        name: "HDF5".into(),
        version_requirement: "==1.14.3".into(),
        reason: "target ABI probe rejected this candidate".into(),
        scope: Some("cpu-default@builder".into()),
    });
    let result = solve_with_stack_policy(
        &universe(vec![dep("HDF5", ">=1.14")]),
        &policy(),
        None,
        &stack,
    )
    .expect("solve with exclusion");
    assert_eq!(
        result
            .selected
            .iter()
            .find(|candidate| candidate.name == "HDF5")
            .expect("HDF5")
            .version,
        "1.14.2"
    );
    assert_eq!(result.exclusions, stack.exclusions);
}

#[test]
fn public_stack_policy_example_parses() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/stacks/foss-2026.1.toml");
    let text = std::fs::read_to_string(path).expect("stack policy example");
    let policy: StackPolicy = toml::from_str(&text).expect("stack policy TOML");
    assert_eq!(policy.schema_version, STACK_POLICY_SCHEMA_VERSION);
    assert_eq!(policy.toolchain, toolchain());
    let expected = [
        ("PyTorch", "==2.9.1", "foss", "2024a"),
        ("xtb", "==6.7.1", "gfbf", "2024a"),
        ("Eigen", "==3.4.0", "GCCcore", "14.3.0"),
        ("Meson", "==1.8.2", "GCCcore", "13.3.0"),
    ];
    assert_eq!(policy.pins.len(), expected.len());
    for (name, requirement, toolchain_name, toolchain_version) in expected {
        let pin = policy
            .pins
            .iter()
            .find(|pin| pin.name == name)
            .unwrap_or_else(|| panic!("missing public stack pin {name}"));
        assert_eq!(pin.version_requirement, requirement);
        assert_eq!(pin.mode, StackPinMode::Preferred);
        assert_eq!(pin.versionsuffix.as_deref(), Some(""));
        let pin_toolchain = pin.toolchain.as_ref().expect("pin toolchain");
        assert_eq!(pin_toolchain.name, toolchain_name);
        assert_eq!(pin_toolchain.version, toolchain_version);
    }
}
