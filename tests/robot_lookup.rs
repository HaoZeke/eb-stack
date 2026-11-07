//! `robot_lookup` gives EasyBuild 5.4.2's robot answers.
//!
//! `tests/robot_lookup_fixtures/easybuild_5_4_2.json` holds scenario trees and
//! what easybuild-framework 5.4.2 itself answered for them:
//! `robot_find_easyconfig` for each lookup, `locate_files` for each named file
//! (under eight hash seeds), and for each `.eb` file the name and full version
//! EasyBuild parses from it and whether its robot finds that very file. Each
//! tree is rebuilt here and every answer is compared.

use eb_stack::robot_lookup::{
    candidate_full_version, ArchivedEasyconfigs, Resolution, RobotPath, DEFAULT_IGNORE_DIRS,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn scenarios() -> Vec<Value> {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/robot_lookup_fixtures/easybuild_5_4_2.json"
    ))
    .expect("recorded answers");
    let doc: Value = serde_json::from_str(&text).expect("json");
    assert_eq!(doc["easybuild"], "5.4.2");
    doc["scenarios"].as_array().expect("scenarios").clone()
}

struct Built {
    _dir: tempfile::TempDir,
    base: PathBuf,
    robot: RobotPath,
}

fn build(scenario: &Value) -> Built {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().to_path_buf();
    for (path, text) in scenario["files"].as_object().expect("files") {
        let full = base.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).expect("mkdir");
        std::fs::write(&full, text.as_str().unwrap()).expect("write");
    }
    let roots = scenario["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|root| base.join(root.as_str().unwrap()))
        .collect();
    let archived = if scenario["archived"] == true {
        ArchivedEasyconfigs::Considered
    } else {
        ArchivedEasyconfigs::Ignored
    };
    Built {
        _dir: dir,
        base,
        robot: RobotPath::new(roots, archived),
    }
}

/// The name and full version a file's content states, as eb-stack parses it.
fn identity(path: &Path) -> Result<(String, String), String> {
    let candidate = eb_stack::parse_easyconfig_file(path).map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let version = candidate_full_version(&candidate, &text)?;
    Ok((candidate.name, version))
}

fn rel(base: &Path, path: Option<PathBuf>) -> Value {
    match path {
        Some(path) => Value::from(
            path.strip_prefix(base)
                .expect("under the scenario")
                .display()
                .to_string(),
        ),
        None => Value::Null,
    }
}

#[test]
fn every_lookup_finds_the_file_easybuild_finds() {
    let mut checked = 0;
    for scenario in scenarios() {
        let built = build(&scenario);
        let answers = &scenario["easybuild"];
        let search: Vec<Value> = built
            .robot
            .search_roots()
            .into_iter()
            .map(|root| rel(&built.base, Some(root)))
            .collect();
        let robot_path: Vec<Value> = answers["robot_path"].as_array().unwrap().clone();
        // EasyBuild's robot_path holds the plain roots; archives follow them.
        assert_eq!(
            search[..robot_path.len()],
            robot_path[..],
            "{}",
            scenario["name"]
        );
        for lookup in answers["lookups"].as_array().unwrap() {
            let name = lookup["name"].as_str().unwrap();
            let version = lookup["version"].as_str().unwrap();
            let found = rel(&built.base, built.robot.find(name, version));
            assert_eq!(
                found, lookup["found"],
                "{}: {name} {version}",
                scenario["name"]
            );
            // Then the robot's file check, as verify_easyconfig_filename makes it.
            let resolution = built.robot.resolve(name, version, identity);
            match (&resolution, &lookup["accepted"]) {
                (Resolution::Missing, Value::Null) => {}
                (Resolution::Found(_), Value::Bool(true)) => {}
                (Resolution::Refused { .. }, Value::Bool(false)) => {}
                other => panic!("{}: {name} {version}: {other:?}", scenario["name"]),
            }
            checked += 1;
        }
    }
    assert!(checked >= 25, "{checked} lookups");
}

#[test]
fn a_named_file_is_located_like_locate_files_or_refused_when_easybuild_cannot_decide() {
    let mut ambiguous = 0;
    let mut checked = 0;
    for scenario in scenarios() {
        let built = build(&scenario);
        for named in scenario["easybuild"]["named"].as_array().unwrap() {
            let file = named["file"].as_str().unwrap();
            let picks = named["picks_over_hash_seeds"].as_array().unwrap();
            let got = built.robot.locate_named(file, DEFAULT_IGNORE_DIRS);
            if picks.len() > 1 {
                let error = got.expect_err(file);
                assert!(error.contains("not defined"), "{error}");
                for pick in picks {
                    assert!(
                        error.contains(pick.as_str().unwrap()),
                        "{error} lacks {pick}"
                    );
                }
                ambiguous += 1;
            } else {
                assert_eq!(
                    rel(&built.base, got.expect(file)),
                    named["found"],
                    "{}: {file}",
                    scenario["name"]
                );
            }
            checked += 1;
        }
    }
    assert_eq!(ambiguous, 3, "Q, S and Dep have two files of one name");
    assert_eq!(checked, 13, "{checked} named lookups");
}

#[test]
fn only_the_files_the_robot_reaches_are_effective_and_full_versions_match() {
    let mut files = 0;
    for scenario in scenarios() {
        let built = build(&scenario);
        let answers = scenario["easybuild"]["files"].as_object().unwrap();
        let mut candidates = Vec::new();
        for root in built.robot.roots() {
            let parsed = eb_stack::parse_easyconfig_tree(root).expect("parse");
            assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
            candidates.extend(parsed.candidates);
        }
        for candidate in &candidates {
            let path = Path::new(&candidate.easyconfig_path);
            let key = path
                .strip_prefix(&built.base)
                .unwrap()
                .display()
                .to_string();
            let text = std::fs::read_to_string(path).unwrap();
            assert_eq!(
                candidate_full_version(candidate, &text).unwrap(),
                answers[&key]["full_version"].as_str().unwrap(),
                "{key}"
            );
            files += 1;
        }
        let effective = built
            .robot
            .effective(candidates, |path| std::fs::read_to_string(path).ok());
        assert!(
            effective.undecidable.is_empty(),
            "{:?}",
            effective.undecidable
        );
        let kept: Vec<String> = effective
            .kept
            .iter()
            .map(|c| {
                Path::new(&c.easyconfig_path)
                    .strip_prefix(&built.base)
                    .unwrap()
                    .display()
                    .to_string()
            })
            .collect();
        let mut kept_sorted = kept.clone();
        kept_sorted.sort();
        let want: Vec<String> = answers
            .iter()
            .filter(|(_, answer)| answer["effective"] == true)
            .map(|(path, _)| path.clone())
            .collect();
        assert_eq!(kept_sorted, want, "{}", scenario["name"]);
        let refused: Vec<String> = effective
            .refused
            .iter()
            .map(|(path, _)| {
                Path::new(path)
                    .strip_prefix(&built.base)
                    .unwrap()
                    .display()
                    .to_string()
            })
            .collect();
        let want_refused: Vec<String> = answers
            .iter()
            .filter(|(_, answer)| answer["reached"] == true && answer["effective"] != true)
            .map(|(path, _)| path.clone())
            .collect();
        assert_eq!(refused, want_refused, "{}", scenario["name"]);
        let all = kept.len() + effective.unreachable.len() + effective.refused.len();
        assert_eq!(all, answers.len(), "{}", scenario["name"]);
        // Highest precedence first: an earlier search root before a later one.
        let order: Vec<usize> = kept
            .iter()
            .map(|path| {
                built
                    .robot
                    .search_roots()
                    .iter()
                    .enumerate()
                    .filter(|(_, root)| built.base.join(path).starts_with(root))
                    .map(|(index, _)| index)
                    .max()
                    .unwrap()
            })
            .collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted, "{}: {kept:?}", scenario["name"]);
    }
    assert!(files >= 30, "{files} files");
}

#[test]
fn what_cannot_be_told_exactly_is_reported_not_guessed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("r");
    std::fs::create_dir_all(root.join("v/Var")).unwrap();
    std::fs::write(
        root.join("v/Var/Var-1.0.eb"),
        "name = 'Var'\nversionprefix = local_p\nversion = '1.0'\ntoolchain = SYSTEM\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("t/Tpl")).unwrap();
    std::fs::write(
        root.join("t/Tpl/Tpl-1.0-%(nosuch)s.eb"),
        "name = 'Tpl'\nversion = '1.0'\nversionsuffix = '-%(nosuch)s'\ntoolchain = SYSTEM\n",
    )
    .unwrap();
    let prefix_error = eb_stack::parse_easyconfig_file(&root.join("v/Var/Var-1.0.eb"))
        .expect_err("an unresolved prefix must be rejected")
        .to_string();
    let robot = RobotPath::new(vec![root.clone()], ArchivedEasyconfigs::Ignored);
    let parsed = eb_stack::parse_easyconfig_tree(&root).expect("parse");
    let effective = robot.effective(parsed.candidates, |path| std::fs::read_to_string(path).ok());
    assert!(effective.kept.is_empty(), "{:?}", effective.kept);
    let reasons: Vec<&str> = effective
        .undecidable
        .iter()
        .map(|(_, why)| why.as_str())
        .chain(std::iter::once(prefix_error.as_str()))
        .collect();
    assert!(
        reasons.iter().any(|why| why.contains("versionprefix")),
        "{reasons:?}"
    );
    assert!(
        reasons.iter().any(|why| why.contains("template")),
        "{reasons:?}"
    );

    let missing = RobotPath::new(
        vec![root.clone(), dir.path().join("absent")],
        ArchivedEasyconfigs::Ignored,
    );
    let error = missing.require_directories().unwrap_err();
    assert!(error.contains("absent"), "{error}");
    assert!(robot.require_directories().is_ok());
}
