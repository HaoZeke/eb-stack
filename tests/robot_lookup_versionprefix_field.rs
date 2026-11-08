//! First-class [`Candidate::versionprefix`]: field-based ModuleKey / full
//! version (no fold, no file re-read), EB5.4.2 system/dummy spelling,
//! install-stem dependency matching, and JSON round-trip. Additive — does not
//! change shipped assertions.
//!
//! Evidence of the earlier incorrect refusal for `local_p` is retained under
//! `docs/evidence/` in the final artifact; this file asserts the resolvable
//! behaviour.

use eb_stack::build_order::{build_order, module_name, Choice, ModuleKey};
use eb_stack::domain::{Candidate, DepReq, Toolchain};
use eb_stack::robot_lookup::{
    candidate_full_version, full_version, is_module_system_toolchain, versionprefix_of,
    ArchivedEasyconfigs, RobotPath, DEFAULT_IGNORE_DIRS,
};
use eb_stack::{
    candidate_matches_dep, easyconfig_basename_with_prefix, merge_candidates_with_precedence,
    resolve_easyconfig_str, ResolvedDep,
};
use std::path::PathBuf;

fn tc(name: &str, version: &str) -> Toolchain {
    Toolchain {
        name: name.into(),
        version: version.into(),
    }
}

fn cand(
    name: &str,
    version: &str,
    toolchain: Toolchain,
    versionprefix: Option<&str>,
    path: &str,
) -> Candidate {
    Candidate {
        name: name.into(),
        version: version.into(),
        toolchain,
        versionsuffix: None,
        versionprefix: versionprefix.map(str::to_string),
        easyconfig_path: path.into(),
        dependencies: vec![],
        builddependencies: vec![],
        exts_list: vec![],
        easyblock: None,
        moduleclass: None,
    }
}

fn dep(name: &str, version: &str) -> DepReq {
    DepReq {
        name: name.into(),
        // EasyBuild bare dep versions become exact pins, including install stems
        // such as `mpp-1.0` that do not start with a digit.
        version_req: eb_stack::version_field_to_req(version),
        versionsuffix: None,
        toolchain: None,
    }
}

#[test]
fn module_key_and_filenames_come_from_fields_not_disk() {
    let label = "/no/such/repo@deadbeef:easybuild/easyconfigs/m/Mol/Mol-mpp-1.0-GCCcore-15.2.0.eb";
    let c = cand("Mol", "1.0", tc("GCCcore", "15.2.0"), Some("mpp-"), label);
    assert_eq!(c.version, "1.0", "bare version stays upstream");
    assert_eq!(c.install_version_stem(), "mpp-1.0");
    assert_eq!(ModuleKey::of(&c).version, "mpp-1.0");
    assert_eq!(
        module_name(&ModuleKey::of(&c)),
        "Mol/mpp-1.0-GCCcore-15.2.0"
    );
    assert_eq!(
        easyconfig_basename_with_prefix(
            "Mol",
            c.versionprefix.as_deref().unwrap_or(""),
            &c.version,
            &c.toolchain,
            None,
        ),
        "Mol-mpp-1.0-GCCcore-15.2.0.eb"
    );
    assert!(!PathBuf::from(label).exists());
    assert_eq!(
        candidate_full_version(&c, "ignored").unwrap(),
        "mpp-1.0-GCCcore-15.2.0"
    );
}

#[test]
fn coincident_prefix_and_version_literals_prepend_once_from_fields() {
    let text = "name = 'Mol'\nversionprefix = 'mpp-'\nversion = 'mpp-1.0'\ntoolchain = SYSTEM\n";
    assert_eq!(versionprefix_of(text).unwrap(), "mpp-");
    let c = cand("Mol", "mpp-1.0", tc("system", "system"), Some("mpp-"), "");
    assert_eq!(c.version, "mpp-1.0");
    assert_eq!(candidate_full_version(&c, text).unwrap(), "mpp-mpp-1.0");
    assert_eq!(module_name(&ModuleKey::of(&c)), "Mol/mpp-mpp-1.0");
}

#[test]
fn mutating_file_after_parse_does_not_change_module_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m/Mol/Mol-mpp-1.0.eb");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "name = 'Mol'\nversionprefix = 'mpp-'\nversion = '1.0'\ntoolchain = SYSTEM\n";
    std::fs::write(&path, original).unwrap();
    let parsed = resolve_easyconfig_str(original).unwrap();
    let c = parsed.to_candidate();
    assert_eq!(c.versionprefix.as_deref(), Some("mpp-"));
    assert_eq!(ModuleKey::of(&c).version, "mpp-1.0");
    std::fs::write(
        &path,
        "name = 'Mol'\nversionprefix = 'CHANGED-'\nversion = '1.0'\ntoolchain = SYSTEM\n",
    )
    .unwrap();
    assert_eq!(ModuleKey::of(&c).version, "mpp-1.0");
    assert_eq!(module_name(&ModuleKey::of(&c)), "Mol/mpp-1.0");
}

#[test]
fn local_name_versionprefix_resolves_like_versionsuffix() {
    let src =
        "name = 'Var'\nlocal_p = 'v'\nversionprefix = local_p\nversion = '1.0'\ntoolchain = SYSTEM\n";
    let parsed = resolve_easyconfig_str(src).unwrap();
    assert_eq!(parsed.versionprefix.as_deref(), Some("v"));
    assert_eq!(parsed.version, "1.0");
    let c = parsed.to_candidate();
    assert_eq!(c.install_version_stem(), "v1.0");
    assert_eq!(candidate_full_version(&c, "").unwrap(), "v1.0");
}

#[test]
fn unresolved_versionprefix_template_is_refused_at_parse() {
    let src = "name = 'Bad'\nversionprefix = '%(nosuch)s'\nversion = '1.0'\ntoolchain = SYSTEM\n";
    let err = resolve_easyconfig_str(src).unwrap_err().to_string();
    assert!(err.contains("versionprefix"), "{err}");
    assert!(err.contains("template") || err.contains("%("), "{err}");
}

#[test]
fn nonempty_prefix_json_round_trips_and_old_json_defaults() {
    let c = cand("Mol", "1.0", tc("system", "system"), Some("mpp-"), "Mol.eb");
    let json = serde_json::to_string(&c).unwrap();
    assert!(json.contains("\"versionprefix\":\"mpp-\""), "{json}");
    assert!(json.contains("\"version\":\"1.0\""), "{json}");
    let back: Candidate = serde_json::from_str(&json).unwrap();
    assert_eq!(back.versionprefix.as_deref(), Some("mpp-"));
    assert_eq!(back.version, "1.0");

    let old = r#"{"name":"Mol","version":"1.0","toolchain":{"name":"system","version":"system"},"easyconfig_path":"Mol.eb"}"#;
    let bare: Candidate = serde_json::from_str(old).unwrap();
    assert_eq!(bare.versionprefix, None);
    assert_eq!(bare.install_version_stem(), "1.0");
}

#[test]
fn merge_keeps_prefixed_and_plain_as_distinct_identities() {
    let plain = cand("Mol", "1.0", tc("GCCcore", "15.2.0"), None, "b/Mol.eb");
    let prefixed = cand(
        "Mol",
        "1.0",
        tc("GCCcore", "15.2.0"),
        Some("mpp-"),
        "a/Mol.eb",
    );
    let merged = merge_candidates_with_precedence(&[vec![prefixed], vec![plain.clone()]]);
    assert_eq!(merged.len(), 2, "{merged:?}");
    assert!(merged.iter().any(|c| c.versionprefix.is_none()));
    assert!(merged
        .iter()
        .any(|c| c.versionprefix.as_deref() == Some("mpp-")));
    assert!(merged.iter().all(|c| c.version == "1.0"));
}

#[test]
fn install_stem_dep_matching_selects_prefixed_or_plain() {
    let plain = cand("Mol", "1.0", tc("GCCcore", "15.2.0"), None, "plain.eb");
    let prefixed = cand(
        "Mol",
        "1.0",
        tc("GCCcore", "15.2.0"),
        Some("mpp-"),
        "pref.eb",
    );
    let want_plain = ResolvedDep {
        name: "Mol".into(),
        version: "1.0".into(),
        versionsuffix: None,
        toolchain: None,
    };
    let want_pref = ResolvedDep {
        name: "Mol".into(),
        version: "mpp-1.0".into(),
        versionsuffix: None,
        toolchain: None,
    };
    assert!(candidate_matches_dep(&plain, &want_plain));
    assert!(!candidate_matches_dep(&prefixed, &want_plain));
    assert!(candidate_matches_dep(&prefixed, &want_pref));
    assert!(!candidate_matches_dep(&plain, &want_pref));

    let root_plain = Candidate {
        name: "RootPlain".into(),
        dependencies: vec![dep("Mol", "1.0")],
        ..cand("RootPlain", "1.0", tc("GCCcore", "15.2.0"), None, "root.eb")
    };
    let root_pref = Candidate {
        name: "RootPref".into(),
        dependencies: vec![dep("Mol", "mpp-1.0")],
        ..cand("RootPref", "1.0", tc("GCCcore", "15.2.0"), None, "rootp.eb")
    };
    let universe = vec![plain.clone(), prefixed.clone(), root_plain, root_pref];
    let order_plain = build_order(&universe, &["RootPlain".into()], Choice::Newest).unwrap();
    let mol_plain: Vec<_> = order_plain
        .iter()
        .filter(|c| c.name == "Mol")
        .map(|c| c.versionprefix.clone())
        .collect();
    assert_eq!(mol_plain, vec![None], "{order_plain:?}");

    let order_pref = build_order(&universe, &["RootPref".into()], Choice::Newest).unwrap();
    let mol_pref: Vec<_> = order_pref
        .iter()
        .filter(|c| c.name == "Mol")
        .map(|c| c.versionprefix.clone())
        .collect();
    assert_eq!(mol_pref, vec![Some("mpp-".into())], "{order_pref:?}");
}

#[test]
fn dummy_and_system_filename_identity_match_eb5() {
    assert!(!is_module_system_toolchain(&tc("dummy", "dummy")));
    assert!(!is_module_system_toolchain(&tc("System", "system")));
    assert!(is_module_system_toolchain(&tc("system", "system")));
    let dummy = cand("Daisy", "1", tc("dummy", "dummy"), None, "");
    assert_eq!(module_name(&ModuleKey::of(&dummy)), "Daisy/1-dummy-dummy");
    assert_eq!(
        easyconfig_basename_with_prefix("Daisy", "", "1", &tc("dummy", "dummy"), None),
        "Daisy-1-dummy-dummy.eb"
    );
    assert_eq!(
        easyconfig_basename_with_prefix("Daisy", "", "1", &tc("System", "system"), None),
        "Daisy-1-System-system.eb"
    );
    assert_eq!(
        easyconfig_basename_with_prefix("Daisy", "", "1", &tc("system", "system"), None),
        "Daisy-1.eb"
    );
    assert_eq!(
        full_version("1", &tc("dummy", "dummy"), "", ""),
        "1-dummy-dummy"
    );
    let sys = cand("Daisy", "1", tc("system", "system"), None, "");
    assert_eq!(module_name(&ModuleKey::of(&sys)), "Daisy/1");
}

#[test]
fn exact_create_paths_dir_fallback_ignores_nested_only_copies() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("r");
    std::fs::create_dir_all(root.join("d/Dep")).unwrap();
    std::fs::write(
        root.join("d/Dep/Dep-1.0.eb"),
        "name = 'Dep'\nversion = '1.0'\ntoolchain = SYSTEM\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("build/x")).unwrap();
    std::fs::write(
        root.join("build/x/Nested-1.0.eb"),
        "name = 'Nested'\nversion = '1.0'\ntoolchain = SYSTEM\n",
    )
    .unwrap();
    let robot = RobotPath::new(vec![root.clone()], ArchivedEasyconfigs::Ignored);
    assert_eq!(
        robot.find("Dep", "1.0"),
        Some(root.join("d/Dep/Dep-1.0.eb"))
    );
    assert_eq!(robot.find("Nested", "1.0"), None);
    let named = robot
        .locate_named("Nested-1.0.eb", DEFAULT_IGNORE_DIRS)
        .unwrap();
    assert_eq!(named, Some(root.join("build/x/Nested-1.0.eb")));
}

#[test]
fn git_virtual_path_uses_fields_not_disk() {
    let label = "/repo@abc:easybuild/easyconfigs/m/Mol/Mol-mpp-1.0.eb";
    let text = "name = 'Mol'\nversionprefix = 'mpp-'\nversion = '1.0'\ntoolchain = SYSTEM\n";
    let parsed = resolve_easyconfig_str(text).unwrap();
    let mut c = parsed.to_candidate();
    c.easyconfig_path = label.into();
    assert!(!PathBuf::from(label).exists());
    assert_eq!(module_name(&ModuleKey::of(&c)), "Mol/mpp-1.0");
    assert_eq!(candidate_full_version(&c, text).unwrap(), "mpp-1.0");
}
