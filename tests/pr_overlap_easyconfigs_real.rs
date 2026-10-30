//! Two real easyconfigs groups added since v5.4.1 that share a new module.
//!
//! Ignored: it parses two full easyconfigs trees. Point
//! `EB_EASYCONFIGS` at develop `easybuild/easyconfigs` and
//! `EB_EASYCONFIGS_541` at the v5.4.1 checkout.

use eb_stack::build_order::{module_name, ModuleKey};
use eb_stack::parse_easyconfig_tree;
use eb_stack::pr_overlap::{
    easyconfig_record, easyconfig_record_seeing, overlap_index, FileChange, OverlapConfig, PrInput,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

fn easyconfigs_under(key: &str) -> String {
    let path = PathBuf::from(std::env::var(key).unwrap_or_else(|_| panic!("{key} is required")));
    let nested = path.join("easybuild/easyconfigs");
    if nested.is_dir() {
        nested.display().to_string()
    } else {
        path.display().to_string()
    }
}

fn config() -> OverlapConfig {
    OverlapConfig {
        eessi_version: String::new(),
        installed_source: String::new(),
        exclude_paths: Vec::new(),
        include_paths: Vec::new(),
        generated_at: "2026-10-22T00:00:00Z".into(),
        filter_deps: eb_stack::dep_filter::FilterDeps::empty(),
        easyblocks: None,
        build_scope: eb_stack::pr_overlap::BuildScope::Added,
    }
}

fn git_root(start: &Path) -> PathBuf {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return dir;
        }
        assert!(dir.pop(), "no git root above {}", start.display());
    }
}

fn added_easyconfigs(develop: &Path) -> Vec<String> {
    let repo = git_root(develop);
    let diff = Command::new("git")
        .args([
            "-C",
            &repo.display().to_string(),
            "diff",
            "--diff-filter=A",
            "--name-only",
            "easybuild-easyconfigs-v5.4.1",
            "develop",
            "--",
            "easybuild/easyconfigs",
        ])
        .output()
        .unwrap();
    assert!(
        diff.status.success(),
        "{}",
        String::from_utf8_lossy(&diff.stderr)
    );
    String::from_utf8(diff.stdout)
        .unwrap()
        .lines()
        .filter(|line| line.contains("GCCcore-15.2.0") || line.contains("2026.1"))
        .map(str::to_string)
        .collect()
}

fn change(develop: &Path, rel: &str) -> FileChange {
    let under = rel.strip_prefix("easybuild/easyconfigs/").unwrap_or(rel);
    FileChange {
        path: rel.to_string(),
        base: None,
        head: Some(std::fs::read_to_string(develop.join(under)).unwrap()),
        base_blob: None,
        head_blob: None,
    }
}

fn pr(number: u64, files: &[FileChange]) -> PrInput {
    PrInput {
        number,
        head_sha: number.to_string().repeat(40),
        title: Some(format!("group {number}")),
        easystacks: Vec::new(),
        files: Vec::new(),
        easyconfigs: files.to_vec(),
        file_list_truncated: false,
    }
}

/// `Name ==1.2.3` from an unresolved line `from needs Name ==1.2.3`.
fn needs(line: &str) -> Option<(&str, &str)> {
    let rest = line.split(" needs ").nth(1)?;
    let mut parts = rest.split_whitespace();
    let name = parts.next()?;
    let spec = parts.next()?;
    let version = spec.trim_start_matches(|c: char| !c.is_ascii_digit());
    if version.is_empty() {
        None
    } else {
        Some((name, version))
    }
}

fn provides(module: &str, name: &str, version: &str) -> bool {
    let prefix = format!("{name}/{version}");
    module == prefix
        || module
            .strip_prefix(&prefix)
            .is_some_and(|rest| rest.starts_with('-'))
}

fn closure(start: usize, deps: &[Vec<usize>]) -> BTreeSet<usize> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![start];
    while let Some(node) = stack.pop() {
        if !seen.insert(node) {
            continue;
        }
        stack.extend(&deps[node]);
    }
    seen
}

#[test]
#[ignore]
fn two_develop_groups_share_a_new_module() {
    let started = std::time::Instant::now();
    let develop = PathBuf::from(std::env::var("EB_EASYCONFIGS").expect("EB_EASYCONFIGS"));
    let v541 = PathBuf::from(easyconfigs_under("EB_EASYCONFIGS_541"));
    let paths = added_easyconfigs(&develop);
    assert!(paths.len() >= 2, "added {}", paths.len());
    let base = parse_easyconfig_tree(&v541).unwrap();
    let head = parse_easyconfig_tree(&develop).unwrap();
    let base_modules: BTreeSet<String> = base
        .candidates
        .iter()
        .map(|candidate| module_name(&ModuleKey::of(candidate)))
        .collect();
    let cfg = config();
    let files: Vec<FileChange> = paths.iter().map(|rel| change(&develop, rel)).collect();
    let mut own: Vec<String> = Vec::new();
    let mut deps: Vec<Vec<usize>> = Vec::new();
    for file in &files {
        let record = easyconfig_record(
            &pr(1, std::slice::from_ref(file)),
            &base.candidates,
            None,
            &cfg,
        )
        .unwrap();
        let module = record
            .changed_easyconfigs
            .iter()
            .find_map(|changed| changed.module.clone())
            .unwrap_or_default();
        own.push(module);
        let mut providers = Vec::new();
        for line in &record.unresolved {
            let Some((name, version)) = needs(line) else {
                continue;
            };
            for (index, module) in own.iter().enumerate() {
                if provides(module, name, version) {
                    providers.push(index);
                }
            }
            // A provider later in the list is filled on the next pass.
        }
        deps.push(providers);
    }
    // Second pass: unresolved lines can name a file we had not recorded yet.
    for (index, file) in files.iter().enumerate() {
        let record = easyconfig_record(
            &pr(1, std::slice::from_ref(file)),
            &base.candidates,
            None,
            &cfg,
        )
        .unwrap();
        for line in &record.unresolved {
            let Some((name, version)) = needs(line) else {
                continue;
            };
            for (provider, module) in own.iter().enumerate() {
                if provides(module, name, version) && !deps[index].contains(&provider) {
                    deps[index].push(provider);
                }
            }
        }
    }
    let mut consumers: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, providers) in deps.iter().enumerate() {
        for provider in providers {
            consumers.entry(*provider).or_default().push(index);
        }
    }
    let mut choice = None;
    for (provider, users) in &consumers {
        if users.len() < 2 {
            continue;
        }
        for left in users {
            for right in users {
                if left >= right {
                    continue;
                }
                let group1 = closure(*left, &deps);
                let mut group2: BTreeSet<usize> = closure(*right, &deps)
                    .difference(&group1)
                    .copied()
                    .collect();
                if group2.is_empty() && !group1.contains(right) {
                    group2.insert(*right);
                }
                if group1.is_empty() || group2.is_empty() || !group1.is_disjoint(&group2) {
                    continue;
                }
                if !group1.contains(provider) && !group2.contains(provider) {
                    continue;
                }
                choice = Some((*provider, group1, group2));
                break;
            }
            if choice.is_some() {
                break;
            }
        }
        if choice.is_some() {
            break;
        }
    }
    let (provider, group1, group2) =
        choice.expect("two disjoint groups that share a new dependency");
    let take = |group: &BTreeSet<usize>| -> Vec<FileChange> {
        group.iter().map(|index| files[*index].clone()).collect()
    };
    let names = |group: &BTreeSet<usize>| -> Vec<&str> {
        group
            .iter()
            .map(|index| paths[*index].rsplit('/').next().unwrap_or(&paths[*index]))
            .collect()
    };
    eprintln!(
        "choice shared={} group1={:?} group2={:?}",
        own[provider],
        names(&group1),
        names(&group2)
    );
    let left = pr(1, &take(&group1));
    let right = pr(2, &take(&group2));
    let left_rec =
        easyconfig_record_seeing(&left, &base.candidates, &right.easyconfigs, None, &cfg).unwrap();
    let right_rec =
        easyconfig_record_seeing(&right, &base.candidates, &left.easyconfigs, None, &cfg).unwrap();
    assert!(!left_rec.closure.is_empty());
    assert!(!right_rec.closure.is_empty());
    for module in left_rec.new_modules.iter().chain(&right_rec.new_modules) {
        assert!(
            !base_modules.contains(module),
            "{module} is already in v5.4.1"
        );
    }
    let index = overlap_index(&[left_rec.clone(), right_rec.clone()], &cfg);
    let shared_rows: Vec<_> = index
        .rows
        .iter()
        .filter(|row| row.kind == "shared-new-module")
        .collect();
    assert!(!shared_rows.is_empty(), "rows {:?}", index.rows);
    assert!(
        shared_rows.iter().any(|row| row.module == own[provider]),
        "shared {:?} wanted {}",
        shared_rows
            .iter()
            .map(|row| &row.module)
            .collect::<Vec<_>>(),
        own[provider]
    );
    for (label, input, record) in [("group1", &left, &left_rec), ("group2", &right, &right_rec)] {
        let on_develop = easyconfig_record(input, &head.candidates, None, &cfg).unwrap();
        let allowed: BTreeSet<_> = on_develop.unresolved.iter().cloned().collect();
        for line in &record.unresolved {
            assert!(
                allowed.contains(line),
                "{label} unresolved only on v5.4.1: {line}"
            );
        }
        eprintln!(
            "{label} closure={} new={} unresolved={} develop_unresolved={}",
            record.closure.len(),
            record.new_modules.len(),
            record.unresolved.len(),
            on_develop.unresolved.len()
        );
    }
    eprintln!(
        "added={} shared_new_rows={} elapsed={:.1}s",
        paths.len(),
        shared_rows.len(),
        started.elapsed().as_secs_f64()
    );
}
