//! Export the tables a browser needs to answer a widget without the solver.
//!
//! `eb-stack` compiled to WebAssembly answers every widget, and until that
//! exists a page can still answer the ones whose whole behaviour is a lookup
//! plus a few lines of mechanical application. `eb_easyblock` established the
//! shape: emit the table from here, apply it there, and keep a cross-check
//! that compares the two implementations over many inputs.
//!
//! Two tables live here.
//!
//! The template spec carries EasyBuild's `TEMPLATE_CONSTANTS` verbatim and a
//! list saying which derived key follows which rule. The rule vocabulary is
//! deliberately tiny and closed, so a consumer applies rules rather than
//! reimplementing `build_templates`. Adding a key in `eb_parse` without adding
//! it here is what the cross-check catches.
//!
//! The hierarchy table carries the generations `hierarchy::known_hierarchy`
//! answers from fixtures. That one is pure data: a consumer looks up a parent
//! and reads the members in order, with no rule to apply.

use serde_json::{json, Value};

use crate::eb_template_constants::EB_TEMPLATE_CONSTANTS;
use crate::hierarchy::known_hierarchy;
use crate::domain::Toolchain;

/// How a derived template value is produced from the recipe's own fields.
///
/// Serialised as a string so the consumer matches on a closed set. Anything
/// outside this vocabulary belongs in the engine proper, not in a page.
pub const TEMPLATE_RULES: &[(&str, &str)] = &[
    // Straight copies of a field.
    ("name", "field:name"),
    ("version", "field:version"),
    ("versionsuffix", "field:versionsuffix"),
    ("toolchain_name", "field:toolchain_name"),
    ("toolchain_version", "field:toolchain_version"),
    // Case and first-letter forms of the name.
    ("namelower", "lower:name"),
    ("nameletter", "first_char:name"),
    ("nameletterlower", "first_char_lower:name"),
    // Forge account defaults, which fall back to the lowercased name when the
    // recipe does not set them.
    ("github_account", "lower:name"),
    ("bitbucket_account", "lower:name"),
    // Dotted version pieces. `part:N` is one component; `join:A-B` is the
    // inclusive span rejoined with dots. Absent when the version has too few
    // components, which is why the consumer must skip rather than emit empty.
    ("version_major", "part:0"),
    ("version_minor", "part:1"),
    ("version_patch", "part:2"),
    ("version_major_minor", "join:0-1"),
    ("version_minor_patch", "join:1-2"),
    ("version_major_minor_patch", "join:0-2"),
    // The machine's architecture, not the recipe's. A page reports the
    // browser's platform here and says so.
    ("arch", "host_arch"),
    // Python defines the py* family from its own version, and that is correct
    // only for Python itself. Everything else takes them from the Python it
    // depends on, which a page has no universe to look up.
    ("pyver", "field:version;only_if_name:Python"),
    ("pymajver", "part:0;only_if_name:Python"),
    ("pyminver", "part:1;only_if_name:Python"),
    ("pyshortver", "join:0-1;only_if_name:Python"),
];

/// Generations `known_hierarchy` answers from a fixture.
///
/// Kept beside the fixture list it mirrors: a generation added there and not
/// here exports nothing, and the widget says the generation is unknown rather
/// than inventing a chain.
pub const EXPORTED_HIERARCHY_GENERATIONS: &[(&str, &str)] = &[
    ("foss", "2023b"),
    ("foss", "2024a"),
    ("foss", "2025a"),
    ("foss", "2025b"),
    ("foss", "2026.1"),
];

/// The template constants plus the derived-key rules, as JSON.
pub fn template_spec() -> Value {
    let constants: Vec<Value> = EB_TEMPLATE_CONSTANTS
        .iter()
        .map(|(name, value)| json!({ "name": name, "value": value }))
        .collect();
    let derived: Vec<Value> = TEMPLATE_RULES
        .iter()
        .map(|(key, rule)| json!({ "key": key, "rule": rule }))
        .collect();
    json!({
        "constants": constants,
        "derived": derived,
    })
}

/// The known toolchain hierarchies, parent first in each entry's `parent`.
///
/// `members` keeps the framework's order: most minimal subtoolchain first,
/// the named toolchain last. A consumer must not sort it.
pub fn hierarchy_table() -> Value {
    let mut entries = Vec::new();
    for (name, version) in EXPORTED_HIERARCHY_GENERATIONS {
        let parent = Toolchain {
            name: (*name).to_string(),
            version: (*version).to_string(),
        };
        if let Some(h) = known_hierarchy(&parent) {
            let members: Vec<Value> = h
                .members
                .iter()
                .map(|m| json!({ "name": m.name, "version": m.version }))
                .collect();
            entries.push(json!({
                "parent": { "name": h.parent.name, "version": h.parent.version },
                "members": members,
            }));
        }
    }
    // The GCC-family rule is synthetic rather than a fixture, so it is
    // described instead of enumerated: every version answers, and enumerating
    // would mean picking an arbitrary set of versions.
    json!({
        "generations": entries,
        "synthetic": [
            { "name": "GCCcore", "members": ["system", "GCCcore"] },
            { "name": "GCC", "members": ["system", "GCCcore", "GCC"] },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_constant_is_exported() {
        let spec = template_spec();
        let n = spec["constants"].as_array().unwrap().len();
        assert_eq!(n, EB_TEMPLATE_CONSTANTS.len());
        assert!(n >= 78, "expected the framework's constant table, got {n}");
    }

    #[test]
    fn rules_use_a_closed_vocabulary() {
        for (key, rule) in TEMPLATE_RULES {
            for clause in rule.split(';') {
                let head = clause.split(':').next().unwrap();
                assert!(
                    matches!(
                        head,
                        "field"
                            | "lower"
                            | "first_char"
                            | "first_char_lower"
                            | "part"
                            | "join"
                            | "host_arch"
                            | "only_if_name"
                    ),
                    "{key}: unknown rule head {head}"
                );
            }
        }
    }

    #[test]
    fn hierarchy_members_end_with_the_parent() {
        let table = hierarchy_table();
        let gens = table["generations"].as_array().unwrap();
        assert!(!gens.is_empty(), "no generations exported");
        for gen in gens {
            let parent = &gen["parent"];
            let members = gen["members"].as_array().unwrap();
            let last = members.last().unwrap();
            assert_eq!(
                last["name"], parent["name"],
                "members must end with the named toolchain"
            );
            assert_eq!(last["version"], parent["version"]);
            assert_eq!(
                members[0]["name"], "system",
                "members must start with the most minimal subtoolchain"
            );
        }
    }

    #[test]
    fn foss_hierarchy_has_the_expected_chain() {
        let table = hierarchy_table();
        let gens = table["generations"].as_array().unwrap();
        let foss = gens
            .iter()
            .find(|g| g["parent"]["version"] == "2025a")
            .expect("foss-2025a exported");
        let names: Vec<&str> = foss["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();
        for expected in ["GCCcore", "GCC", "gompi", "gfbf", "foss"] {
            assert!(names.contains(&expected), "missing {expected} in {names:?}");
        }
    }
}
