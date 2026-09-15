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
        moduleclass: None,
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
        prefer_installed: false,
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
    let mut mpi = candidate("HDF5", "1.14.2", vec![dep("zlib", "==1.3")]);
    mpi.versionsuffix = Some("-mpi".into());
    mpi.easyconfig_path = "HDF5-1.14.2-foss-2026.1-mpi.eb".into();
    let result = solve_with_stack_policy(
        &[
            candidate("zlib", "1.2", Vec::new()),
            candidate("zlib", "1.3", Vec::new()),
            candidate("HDF5", "1.14.2", vec![dep("zlib", "==1.3")]),
            mpi,
            candidate("HDF5", "1.14.3", vec![dep("zlib", "==1.3")]),
            candidate(
                "App",
                "1.0",
                vec![dep("HDF5", ">=1.14"), dep("zlib", "==1.3")],
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
    assert!(
        selected.versionsuffix.is_none() || selected.versionsuffix.as_deref() == Some(""),
        "a 2-tuple is the unsuffixed module, not -mpi: {:?}",
        selected.versionsuffix
    );
}

#[test]
fn locked_stack_pin_constrains_every_interned_key() {
    let foss = toolchain();
    let gcccore = Toolchain {
        name: "GCCcore".into(),
        version: "15.2.0".into(),
    };
    let system = Toolchain {
        name: "system".into(),
        version: "system".into(),
    };
    let perl = |version: &str, toolchain: &Toolchain| Candidate {
        name: "Perl".into(),
        version: version.into(),
        toolchain: toolchain.clone(),
        versionsuffix: None,
        easyconfig_path: format!("Perl-{version}-{}.eb", toolchain.identity_label()),
        dependencies: Vec::new(),
        builddependencies: Vec::new(),
        exts_list: Vec::new(),
        moduleclass: None,
    };
    let result = solve_with_stack_policy(
        &[
            perl("5.38.0", &system),
            perl("5.42.0", &system),
            perl("5.38.0", &gcccore),
            perl("5.42.0", &gcccore),
            Candidate {
                name: "App".into(),
                version: "1.0".into(),
                toolchain: foss.clone(),
                versionsuffix: None,
                easyconfig_path: "App-1.0-foss-2026.1.eb".into(),
                dependencies: vec![dep("Perl", "")],
                builddependencies: Vec::new(),
                exts_list: Vec::new(),
                moduleclass: None,
            },
        ],
        &policy(),
        None,
        &StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "eessi-test".into(),
            toolchain: foss,
            pins: vec![StackPin {
                name: "Perl".into(),
                version_requirement: "==5.38.0".into(),
                toolchain: None,
                versionsuffix: None,
                mode: StackPinMode::Locked,
                source: Some("eessi-test.cdx.json".into()),
            }],
            exclusions: Vec::new(),
        },
    )
    .expect("locked Perl 5.38.0");
    let perls: Vec<&Candidate> = result
        .selected
        .iter()
        .filter(|candidate| candidate.name == "Perl")
        .collect();
    assert!(
        !perls.is_empty(),
        "Perl must be selected: {:#?}",
        result.selected
    );
    for selected in perls {
        assert_eq!(
            selected.version, "5.38.0",
            "every interned Perl key must honor the lock: {selected:?}"
        );
    }
}

#[test]
fn gcccore_stack_pin_reports_the_gcccore_perl_not_system() {
    let foss = toolchain();
    let gcccore = Toolchain {
        name: "GCCcore".into(),
        version: "15.2.0".into(),
    };
    let system = Toolchain {
        name: "system".into(),
        version: "system".into(),
    };
    let perl = |version: &str, toolchain: &Toolchain| Candidate {
        name: "Perl".into(),
        version: version.into(),
        toolchain: toolchain.clone(),
        versionsuffix: None,
        easyconfig_path: format!("Perl-{version}-{}.eb", toolchain.identity_label()),
        dependencies: Vec::new(),
        builddependencies: Vec::new(),
        exts_list: Vec::new(),
        moduleclass: None,
    };
    let result = solve_with_stack_policy(
        &[
            perl("5.38", &system),
            perl("5.38", &gcccore),
            Candidate {
                name: "App".into(),
                version: "1.0".into(),
                toolchain: foss.clone(),
                versionsuffix: None,
                easyconfig_path: "App-1.0-foss-2026.1.eb".into(),
                dependencies: vec![
                    DepReq {
                        name: "Perl".into(),
                        version_req: "==5.38".into(),
                        versionsuffix: None,
                        toolchain: Some(system.clone()),
                    },
                    DepReq {
                        name: "Perl".into(),
                        version_req: "==5.38".into(),
                        versionsuffix: None,
                        toolchain: Some(gcccore.clone()),
                    },
                ],
                builddependencies: Vec::new(),
                exts_list: Vec::new(),
                moduleclass: None,
            },
        ],
        &policy(),
        None,
        &StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "eessi-test".into(),
            toolchain: foss,
            pins: vec![StackPin {
                name: "Perl".into(),
                version_requirement: "==5.38".into(),
                toolchain: Some(gcccore.clone()),
                versionsuffix: None,
                mode: StackPinMode::Preferred,
                source: Some("eessi-test.cdx.json".into()),
            }],
            exclusions: Vec::new(),
        },
    )
    .expect("GCCcore-scoped Perl pin");
    let perls: Vec<&Candidate> = result
        .selected
        .iter()
        .filter(|candidate| candidate.name == "Perl")
        .collect();
    assert!(
        perls.iter().any(|candidate| candidate.version == "5.38"
            && candidate.toolchain.name.eq_ignore_ascii_case("system")),
        "selected must keep Perl 5.38@system: {:#?}",
        result.selected
    );
    assert!(
        perls.iter().any(|candidate| {
            candidate.version == "5.38" && candidate.toolchain.name == "GCCcore"
        }),
        "selected must keep Perl 5.38@GCCcore: {:#?}",
        result.selected
    );
    let outcome = result
        .pin_outcomes
        .iter()
        .find(|outcome| outcome.name == "Perl")
        .expect("pin outcome");
    let selected_toolchain = outcome
        .selected_toolchain
        .as_ref()
        .expect("selected toolchain");
    assert_eq!(selected_toolchain.name, "GCCcore");
    assert!(!outcome.fallback);
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

fn parse_public_stack_policy(name: &str) -> StackPolicy {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/stacks")
        .join(name);
    let text = std::fs::read_to_string(path).expect("stack policy example");
    toml::from_str(&text).expect("stack policy TOML")
}

#[test]
fn public_stack_policy_example_parses() {
    let policy = parse_public_stack_policy("foss-2026.1.toml");
    assert_eq!(policy.schema_version, STACK_POLICY_SCHEMA_VERSION);
    assert_eq!(policy.toolchain, toolchain());
    assert!(policy.pins.is_empty());
}

#[test]
fn public_eon_stack_policy_pins_only_the_patch_generation() {
    // Core + rgpot product: the only stack identity is the Eigen 5 pin
    // backing the safemath core-guard patch. Cross-generation PyTorch/xtb
    // pins died with the fat product.
    let policy = parse_public_stack_policy("eon-foss-2026.1.toml");
    assert_eq!(policy.schema_version, STACK_POLICY_SCHEMA_VERSION);
    assert_eq!(policy.toolchain, toolchain());
    assert_eq!(policy.pins.len(), 1);
    let pin = &policy.pins[0];
    assert_eq!(pin.name, "Eigen");
    assert_eq!(pin.version_requirement, "==5.0.0");
    assert_eq!(pin.mode, StackPinMode::Preferred);
    assert_eq!(pin.versionsuffix.as_deref(), Some(""));
    let pin_toolchain = pin.toolchain.as_ref().expect("pin toolchain");
    assert_eq!(pin_toolchain.name, "GCCcore");
    assert_eq!(pin_toolchain.version, "15.2.0");
}
