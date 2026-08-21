//! What a solve optimises for, written down.
//!
//! Selection is an optimisation problem: many assignments satisfy the
//! constraints and a stated preference picks one. Before this the preference
//! was two booleans with no stated precedence, and the precedence that actually
//! ran lived in a comparator nobody reading a lock could see.
//!
//! The criteria use the CUDF spelling the package-solving literature settled on
//! (OPIUM doi:10.1109/icse.2007.59, apt-pbo doi:10.1145/1858996.1859087), so
//! the ordering is familiar rather than invented.

use eb_stack::domain::{Criterion, Policy, Toolchain};

fn policy(prefer_installed: bool, criteria: Vec<&str>) -> Policy {
    Policy {
        toolchain: Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        },
        roots: vec!["FlexiBLAS".into()],
        root_priority: None,
        prefer_installed,
        pins: vec![],
        forbid: vec![],
        objective: "prefer_newer".into(),
        criteria: criteria.into_iter().map(str::to_string).collect(),
        require_upgrade: vec![],
    }
}

/// The default is the old behaviour written down, not a new one. Without
/// prefer_installed the only preference the solver ever expressed was newest.
#[test]
fn the_derived_default_is_what_the_solver_already_did() {
    assert_eq!(
        policy(false, vec![]).criteria(),
        vec![Criterion::MinimiseNotUptodate]
    );
}

/// With prefer_installed, staying put outranks moving to newest: a favoured
/// candidate is tried first and the solver takes the first that works. That
/// precedence was real before and unstated; here it is stated.
#[test]
fn prefer_installed_outranks_newest_and_says_so() {
    assert_eq!(
        policy(true, vec![]).criteria(),
        vec![Criterion::MinimiseChanged, Criterion::MinimiseNotUptodate]
    );
}

/// An explicit list wins over the derivation, including reversing it, which is
/// the point of having the field at all.
#[test]
fn an_explicit_list_overrides_the_derivation() {
    let p = policy(true, vec!["-notuptodate", "-changed"]);
    assert_eq!(
        p.criteria(),
        vec![Criterion::MinimiseNotUptodate, Criterion::MinimiseChanged],
        "explicit criteria must not be reordered by prefer_installed"
    );
}

/// A typo has to fail rather than quietly change what the solve optimises for.
#[test]
fn an_unknown_criterion_is_refused_with_the_known_ones_named() {
    let err = policy(false, vec!["-changed", "-newest"])
        .validate_criteria()
        .expect_err("an unknown criterion must not pass validation");
    assert!(err.contains("-newest"), "{err}");
    assert!(err.contains("-changed"), "the message should name what is known: {err}");
    assert!(policy(false, vec!["-changed"]).validate_criteria().is_ok());
}

#[test]
fn the_cudf_spelling_round_trips() {
    for c in [Criterion::MinimiseChanged, Criterion::MinimiseNotUptodate] {
        assert_eq!(Criterion::parse(c.as_str()).unwrap(), c);
        assert_eq!(c.to_string(), c.as_str());
    }
}

/// The half that makes it an explanation rather than a field: a reader of the
/// lock can see what the solve optimised for without reading the solver.
#[test]
fn the_lock_records_the_criteria_that_ran() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, body: &str| {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    };
    write(
        "z/Zlib/Zlib-1.3.1-GCCcore-15.2.0.eb",
        "easyblock = 'ConfigureMake'\nname = 'Zlib'\nversion = '1.3.1'\n\
         homepage = 'https://example.invalid/'\n\
         description = \"fixture: one package, so the solve is about criteria only\"\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         moduleclass = 'lib'\n",
    );
    let policy_path = dir.path().join("policy.json");
    fs::write(
        &policy_path,
        r#"{"toolchain": {"name": "GCCcore", "version": "15.2.0"},
            "roots": ["Zlib"], "objective": "prefer_newer"}"#,
    )
    .unwrap();
    let lock_out = dir.path().join("stack.lock.json");
    let lock = eb_stack::solve_from_easyconfigs_with_extras(
        &[dir.path()],
        &policy_path,
        None,
        &lock_out,
        None,
        eb_stack::SolveExtraOut {
            build_list_out: None,
            stack_diff_out: None,
        },
    )
    .expect("a one-package stack solves");

    assert_eq!(
        lock.solver.criteria,
        vec!["-notuptodate".to_string()],
        "the lock has to say what it optimised for"
    );

    // And it survives the round trip, since the point is a reader of the file.
    let on_disk: eb_stack::domain::StackLock =
        serde_json::from_str(&fs::read_to_string(&lock_out).unwrap()).unwrap();
    assert_eq!(on_disk.solver.criteria, lock.solver.criteria);
}
