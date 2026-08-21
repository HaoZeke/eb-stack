//! The solver must find a solution whenever one exists.
//!
//! Abate et al., "Dependency Solving Is Still Hard, but We Are Getting Better
//! at It" (2020), names incompleteness as the pitfall to guard against: a valid
//! assignment exists and the tool fails to find it. Fixture tests cannot catch
//! that, because a fixture only ever exercises the shapes someone thought of.
//!
//! So this generates small universes exhaustively, decides each one by brute
//! force, and holds the solver to that answer in both directions: it must solve
//! every solvable universe, and refuse every unsolvable one.

use eb_stack::domain::{Candidate, DepReq, Policy, Toolchain, Universe};
use eb_stack::select_stack;

const PACKAGES: [&str; 3] = ["Alpha", "Beta", "Gamma"];
const VERSIONS: [&str; 3] = ["1.0", "2.0", "3.0"];

fn toolchain() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2025a".into(),
    }
}

/// The requirement `Alpha` at index `i` places on the next package, as a
/// function of the case number. Every case is generated, so the table is a
/// complete statement of what a dependency can say here.
fn requirement(case: usize) -> &'static str {
    match case % 5 {
        0 => "",      // no constraint
        1 => ">=2.0", // rules out 1.0
        2 => "==3.0", // one version only
        3 => "<=2.0", // rules out 3.0
        _ => "==4.0", // no such candidate: the case that must be refused
    }
}

fn satisfies(version: &str, requirement: &str) -> bool {
    match requirement {
        "" => true,
        ">=2.0" => version != "1.0",
        "==3.0" => version == "3.0",
        "<=2.0" => version != "3.0",
        "==4.0" => false,
        other => panic!("unhandled requirement {other}"),
    }
}

/// A chain Alpha -> Beta -> Gamma, where each edge carries one requirement.
/// Every package is reachable from the root, so the solver's closure and the
/// brute force below range over the same set.
fn universe_for(case_ab: usize, case_bg: usize) -> Universe {
    let mut candidates = Vec::new();
    for (index, name) in PACKAGES.iter().enumerate() {
        for version in VERSIONS {
            let dependencies = match index {
                0 => vec![DepReq {
                    name: "Beta".into(),
                    version_req: requirement(case_ab).to_string(),
                    toolchain: None,
                    versionsuffix: None,
                }],
                1 => vec![DepReq {
                    name: "Gamma".into(),
                    version_req: requirement(case_bg).to_string(),
                    toolchain: None,
                    versionsuffix: None,
                }],
                _ => Vec::new(),
            };
            candidates.push(Candidate {
                name: (*name).to_string(),
                version: version.to_string(),
                toolchain: toolchain(),
                versionsuffix: None,
                dependencies,
                builddependencies: Vec::new(),
                easyconfig_path: format!("x/{name}/{name}-{version}-foss-2025a.eb"),
                exts_list: Vec::new(),
                moduleclass: None,
            });
        }
    }
    Universe {
        toolchain: toolchain(),
        generation_label: Some("foss-2025a".into()),
        candidates,
    }
}

/// Every assignment of one version per package, checked directly.
fn brute_force_has_solution(case_ab: usize, case_bg: usize) -> bool {
    for beta in VERSIONS {
        if !satisfies(beta, requirement(case_ab)) {
            continue;
        }
        for gamma in VERSIONS {
            if satisfies(gamma, requirement(case_bg)) {
                return true;
            }
        }
    }
    false
}

fn policy() -> Policy {
    Policy {
        toolchain: toolchain(),
        roots: vec!["Alpha".into()],
        root_priority: None,
        prefer_installed: false,
        pins: Vec::new(),
        forbid: Vec::new(),
        objective: "prefer_newer".into(),
        require_upgrade: Vec::new(),
    criteria: Vec::new(),
    }
}

#[test]
fn every_solvable_universe_is_solved_and_every_unsolvable_one_is_refused() {
    let mut solvable = 0;
    let mut unsolvable = 0;
    for case_ab in 0..5 {
        for case_bg in 0..5 {
            let universe = universe_for(case_ab, case_bg);
            let expected = brute_force_has_solution(case_ab, case_bg);
            let result = select_stack(&universe, &policy(), None);
            assert_eq!(
                result.is_ok(),
                expected,
                "ab={} bg={}: brute force says solvable={expected}, solver said {:?}",
                requirement(case_ab),
                requirement(case_bg),
                result.as_ref().err()
            );
            if let Ok(lock) = result {
                solvable += 1;
                // A solution the solver reports must actually hold, or the
                // agreement above would only mean both are wrong together.
                let beta = &lock.package("Beta").expect("Beta selected").version;
                let gamma = &lock.package("Gamma").expect("Gamma selected").version;
                assert!(satisfies(beta, requirement(case_ab)), "Beta {beta}");
                assert!(satisfies(gamma, requirement(case_bg)), "Gamma {gamma}");
            } else {
                unsolvable += 1;
            }
        }
    }
    // Both branches have to be exercised, or the test proves nothing about one
    // of the two directions it claims to check.
    assert!(solvable > 0, "no solvable case was generated");
    assert!(unsolvable > 0, "no unsolvable case was generated");
    assert_eq!(solvable + unsolvable, 25);
}
