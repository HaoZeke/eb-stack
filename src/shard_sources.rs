//! Sources of `--from-commit` lines for `stack shard`: a commit read in place
//! from a git object database, and the selected part of a commit written out
//! so a build list can run without `--from-commit`.
//!
//! EasyBuild 5.4 runs `eb X.eb --from-commit=SHA` like this
//! (`easybuild/tools/github.py`, `easybuild/tools/robot.py`,
//! `easybuild/framework/easyconfig/tools.py`). It downloads the commit,
//! symlinks every subdirectory of its `easybuild/easyconfigs` into a
//! `files_commit_SHA` directory, and copies `X.eb` to that directory's top.
//! `X.eb` on the command line becomes that copy. `files_commit_SHA` is
//! appended to the robot path after every configured path, and the robot
//! takes the first path that has a file, so a dependency comes from the
//! configured robot path when it is there and from the commit otherwise.
//!
//! The sources directory written here is `files_commit_SHA` restricted to
//! the files the planned closures read: the named easyconfigs at its top,
//! and each easyconfig, patch, and local source at its path in the commit.
//! Every file in it is the commit's file at the same path, so a lookup in it
//! finds what the same lookup in `files_commit_SHA` finds, or nothing. A
//! file is selected wherever EasyBuild's first match for it is in the commit.
//!
//! `--robot-paths` with an empty element splices in EasyBuild's built-in
//! default only, not a value set in `EASYBUILD_ROBOT_PATHS` or a config file,
//! so an executable line spells out the build host's robot paths and then
//! the sources directory.

use crate::domain::Candidate;
use crate::easyconfig_tree::{locate_in, Lookup, Tree};
use crate::eb_parse::{resolve_local_files, LocalFileKind, LocalFileOwner};
use std::collections::{BTreeMap, BTreeSet};

/// Schema of the sources directory layout and its manifest.
pub(crate) const SOURCES_SCHEMA_VERSION: u32 = 1;

/// Directory under the output directory that holds the sources of each
/// commit, `sources/v1/<commit>/`, and its manifest, `sources/v1/<commit>.json`.
pub(crate) const SOURCES_DIR: &str = "sources/v1";

/// Options that change where EasyBuild looks for easyconfigs, or take an
/// easyconfig from elsewhere. A line with one of these, or an abbreviation
/// EasyBuild would expand to one, runs unchanged.
const RESOLUTION_OPTIONS: &[&str] = &[
    "robot-paths",
    "from-pr",
    "from-commit",
    "try-",
    "copy-ec",
    "review-pr",
    "search",
    "filter-ecs",
    "consider-archived-easyconfigs",
    "ignore-dirs",
    "easystack",
    "include-easyblocks-from-pr",
];

/// Why a build-list line cannot be run from a sources directory, or `None`.
///
/// The line must name one easyconfig, give `--from-commit` exactly once,
/// and carry no other option that changes easyconfig resolution. An option
/// value written as a separate word is refused too, because it cannot be
/// told apart from a second easyconfig.
pub(crate) fn unsupported_option(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().skip(1).collect();
    let mut from_commit = 0;
    let mut j = 0;
    while j < words.len() {
        let word = words[j];
        let key = word.split('=').next().unwrap_or(word);
        if key == "--from-commit" {
            from_commit += 1;
            if !word.contains('=') {
                j += 1;
            }
            j += 1;
            continue;
        }
        if !word.starts_with('-') {
            return Some(format!(
                "{word} is not an option; EasyBuild would read it as another easyconfig"
            ));
        }
        if key == "--robot" || key == "-r" {
            if word.contains('=') {
                return Some(format!("{word} adds robot paths"));
            }
            if words.get(j + 1).is_some_and(|next| !next.starts_with('-')) {
                return Some(format!("{word} {} adds robot paths", words[j + 1]));
            }
            j += 1;
            continue;
        }
        if !key.starts_with("--") && key != "-f" {
            return Some(format!("{word} is a short option eb-stack does not model"));
        }
        // The full helper option takes precedence over PR option abbreviations.
        if key == "--include-easyblocks" && word.contains('=') {
            j += 1;
            continue;
        }
        let name = key.trim_start_matches('-');
        if RESOLUTION_OPTIONS
            .iter()
            .any(|option| name.starts_with(option) || (name.len() >= 3 && option.starts_with(name)))
        {
            return Some(format!("{word} changes how EasyBuild finds easyconfigs"));
        }
        j += 1;
    }
    match from_commit {
        1 => None,
        0 => Some("no --from-commit".into()),
        _ => Some("--from-commit is given more than once".into()),
    }
}

/// `text` as a line that runs from a sources directory: the easyconfig
/// named by `root` when the commit provides it, `--from-commit` dropped,
/// every other word kept, and `--robot-paths` set to `robot_paths`.
pub(crate) fn executable_line(text: &str, root: Option<&str>, robot_paths: &str) -> String {
    let mut words = text.split_whitespace();
    let first = words.next().unwrap_or_default();
    let mut out = vec![root.unwrap_or(first).to_string()];
    let rest: Vec<&str> = words.collect();
    let mut j = 0;
    while j < rest.len() {
        let word = rest[j];
        if word.starts_with("--from-commit=") {
            j += 1;
            continue;
        }
        if word == "--from-commit" {
            j += 2;
            continue;
        }
        out.push(word.to_string());
        j += 1;
    }
    out.push(format!("--robot-paths={robot_paths}"));
    out.join(" ")
}

/// Parse easyconfig text read from `label`.
pub(crate) fn parse_text(bytes: &[u8], label: &str) -> Result<Candidate, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| format!("{label}: {error}"))?;
    let mut resolved = crate::eb_parse::resolve_easyconfig_str(text)
        .map_err(|error| format!("{label}: {error}"))?;
    resolved.easyconfig_path = label.to_string();
    Ok(resolved.to_candidate())
}

/// The easyconfigs of a git `tree` that can decide a plan: those whose file
/// name the robot path lacks, and the easyconfigs lines name (`roots`) unless
/// the robot path's file of that name has the same content. A file the
/// robot path also has loses to it for a dependency. `robot` maps a file name
/// to the SHA-256 of the robot path's effective easyconfig of that name.
/// Their bytes, and those of a named easyconfig left out for having the
/// robot path's content, go into `contents` under [`Tree::label`]. Returns
/// the candidates and the files that did not parse.
pub(crate) fn git_overlay(
    tree: &Tree,
    robot: &BTreeMap<String, String>,
    roots: &BTreeSet<String>,
    contents: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(Vec<Candidate>, Vec<String>), String> {
    let rels: Vec<String> = tree
        .files_under("")
        .into_iter()
        .filter(|rel| rel.ends_with(".eb"))
        .filter(|rel| {
            let name = rel.rsplit('/').next().unwrap_or(rel);
            !robot.contains_key(name) || roots.contains(name)
        })
        .collect();
    let read = tree.read_all(&rels).map_err(|error| format!("{error:#}"))?;
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();
    for (rel, bytes) in read {
        let name = rel.rsplit('/').next().unwrap_or(&rel);
        let label = tree.label(&rel);
        if robot
            .get(name)
            .is_some_and(|digest| *digest == sha256_hex(&bytes))
        {
            contents.insert(label, bytes);
            continue;
        }
        match parse_text(&bytes, &label) {
            Ok(candidate) => {
                contents.insert(label, bytes);
                candidates.push(candidate);
            }
            Err(error) => skipped.push(error),
        }
    }
    Ok((candidates, skipped))
}

/// The commit files one recipe reads, by the lookups EasyBuild makes for
/// its patches and local sources: each file whose first match is in
/// `commit`. `beside` is the tree and directory the recipe is read from.
/// Fails when a file name keeps a template, or a patch is found nowhere.
pub(crate) fn recipe_needs(
    text: &str,
    beside: (&Tree, &str),
    commit: &Tree,
    robot: &[Tree],
) -> Result<BTreeSet<String>, String> {
    let (recipe, files) = resolve_local_files(text).map_err(|error| error.to_string())?;
    let mut roots: Vec<&Tree> = robot.iter().collect();
    roots.push(commit);
    let mut out = BTreeSet::new();
    for file in files {
        if file.filename.contains("%(") {
            return Err(format!(
                "{} keeps a template eb-stack does not resolve",
                file.filename
            ));
        }
        let location = file.alt_location.as_deref().unwrap_or(&recipe.name);
        let extension = matches!(file.owner, LocalFileOwner::Extension { .. });
        let mut lookups = vec![Lookup {
            location,
            beside: file.alt_location.is_none(),
            extension,
        }];
        if let LocalFileOwner::Component { name, .. } = &file.owner {
            let component = file.alt_location.as_deref().unwrap_or(name);
            for beside in [true, false] {
                lookups.push(Lookup {
                    location: component,
                    beside: beside && file.alt_location.is_none(),
                    extension: false,
                });
            }
        }
        let mut seen = false;
        for lookup in &lookups {
            if let Some((tree, rel)) = locate_in(&file.filename, lookup, beside.0, beside.1, &roots)
            {
                seen = true;
                if std::ptr::eq(tree, commit) {
                    out.insert(rel);
                }
            }
        }
        let patch_file = file.filename.ends_with(".patch") || file.filename.ends_with(".diff");
        let required =
            file.kind != LocalFileKind::Source && (patch_file || recipe.source_urls.is_empty());
        if !seen && required {
            return Err(format!(
                "{} {} is on neither the robot path nor in the commit",
                if file.kind == LocalFileKind::ChecksumPatch {
                    "checksummed patch"
                } else {
                    "patch"
                },
                file.filename
            ));
        }
    }
    Ok(out)
}

/// One file of a sources directory.
pub(crate) struct SourceFile {
    /// Path inside the sources directory.
    pub(crate) path: String,
    /// Path inside the commit's `easybuild/easyconfigs`.
    pub(crate) from: String,
}

/// Write the sources directory `dir` and its manifest `manifest`: each of
/// `files` copied from `commit`. Returns the manifest document.
pub(crate) fn write_sources(
    commit: &Tree,
    pin: &str,
    provider: &str,
    dir: &std::path::Path,
    manifest: &std::path::Path,
    files: &[SourceFile],
) -> anyhow::Result<serde_json::Value> {
    use anyhow::Context;
    let wanted: Vec<String> = files
        .iter()
        .map(|f| f.from.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let bytes = commit.read_all(&wanted)?;
    let mut entries = Vec::new();
    let mut digest_input = String::new();
    for file in files {
        let content = &bytes[&file.from];
        let target = dir.join(&file.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        std::fs::write(&target, content).with_context(|| format!("write {}", target.display()))?;
        let sha = sha256_hex(content);
        digest_input.push_str(&format!("{sha}  {}\n", file.path));
        entries.push(serde_json::json!({
            "path": file.path,
            "from": format!("easybuild/easyconfigs/{}", file.from),
            "sha256": sha,
            "bytes": content.len(),
        }));
    }
    let document = serde_json::json!({
        "schema_version": SOURCES_SCHEMA_VERSION,
        "commit": pin,
        "resolved_commit": commit.commit(),
        "provider": provider,
        "files": entries,
        "sha256": sha256_hex(digest_input.as_bytes()),
    });
    crate::write_json_pretty(manifest, &document)?;
    Ok(document)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn plain_source_options_are_supported() {
        for line in [
            format!("A-1.0.eb --from-commit={SHA}"),
            format!("A-1.0.eb --from-commit {SHA} --include-systems=snel-zen2"),
            format!("A-1.0.eb --robot --from-commit={SHA} --include-easyblocks-from-commit={SHA}"),
            format!("A-1.0.eb -r --rebuild --from-commit={SHA}"),
        ] {
            assert_eq!(unsupported_option(&line), None, "{line}");
        }
    }

    #[test]
    fn explicit_easyblock_paths_preserve_recipe_resolution() {
        for helpers in [
            "/helpers/generic.py",
            "/helpers/first.py,/helpers/second.py",
        ] {
            let line = format!("A-1.0.eb --from-commit={SHA} --include-easyblocks={helpers}");
            assert_eq!(unsupported_option(&line), None, "{line}");
            assert_eq!(
                executable_line(&line, Some("/selected/A-1.0.eb"), "/robot:/selected"),
                format!(
                    "/selected/A-1.0.eb --include-easyblocks={helpers} --robot-paths=/robot:/selected"
                )
            );
        }
    }

    #[test]
    fn easyblock_pull_request_options_remain_unsupported() {
        for option in [
            "--include-easyblocks-from-pr=42",
            "--include-easyblocks-from-p=42",
            "--include-easyblocks-from=42",
            "--include-easyblocks /helpers/generic.py",
        ] {
            let line = format!("A-1.0.eb --from-commit={SHA} {option}");
            assert!(unsupported_option(&line).is_some(), "{line}");
        }
    }

    #[test]
    fn resolution_options_and_extra_words_are_refused() {
        for line in [
            "A-1.0.eb".to_string(),
            format!("A-1.0.eb --from-commit={SHA} --from-commit={SHA}"),
            format!("A-1.0.eb B-1.0.eb --from-commit={SHA}"),
            format!("A-1.0.eb --from-commit={SHA} --robot-paths=/x"),
            format!("A-1.0.eb --from-commit={SHA} --robot=/x"),
            format!("A-1.0.eb --from-commit={SHA} --robot /x"),
            format!("A-1.0.eb --from-commit={SHA} --rob=/x"),
            format!("A-1.0.eb --from-c={SHA}"),
            format!("A-1.0.eb --from-commit={SHA} --try-toolchain=GCC,15.2.0"),
            format!("A-1.0.eb --from-commit={SHA} --include-systems snel"),
            format!("A-1.0.eb --from-commit={SHA} -D"),
            format!("A-1.0.eb --from-commit={SHA} --consider-archived-easyconfigs"),
        ] {
            assert!(unsupported_option(&line).is_some(), "{line}");
        }
    }

    #[test]
    fn an_executable_line_drops_from_commit_and_appends_the_robot_paths() {
        assert_eq!(
            executable_line(
                &format!("A-1.0.eb --from-commit={SHA} --include-systems=snel-zen2"),
                Some("/out/sources/v1/x/A-1.0.eb"),
                "/robot:/out/sources/v1/x"
            ),
            "/out/sources/v1/x/A-1.0.eb --include-systems=snel-zen2 --robot-paths=/robot:/out/sources/v1/x"
        );
        assert_eq!(
            executable_line(
                &format!("A-1.0.eb --from-commit {SHA} --robot"),
                None,
                "/r:/s"
            ),
            "A-1.0.eb --robot --robot-paths=/r:/s"
        );
    }

    #[test]
    fn a_patch_found_first_in_the_commit_is_selected_and_one_on_the_robot_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let robot = tmp.path().join("robot");
        let commit = tmp.path().join("commit");
        for (root, rel) in [
            (&robot, "a/A/both.patch"),
            (&commit, "a/A/both.patch"),
            (&commit, "a/A/commit-only.patch"),
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, rel).unwrap();
        }
        let text = "name = 'A'\nversion = '1.0'\ntoolchain = SYSTEM\n\
                    patches = ['both.patch', 'commit-only.patch']\n";
        let robot = vec![Tree::at(&robot)];
        let commit = Tree::at(&commit);
        // A dependency read from the robot path: its own directory first.
        let robot_dir = Tree::at(&tmp.path().join("robot/a/A"));
        let needs = recipe_needs(text, (&robot_dir, ""), &commit, &robot).unwrap();
        assert_eq!(needs, BTreeSet::from(["a/A/commit-only.patch".to_string()]));
        // The commit's own copy of A: beside it, the commit's patch wins.
        let needs = recipe_needs(text, (&commit, "a/A"), &commit, &robot).unwrap();
        assert_eq!(
            needs,
            BTreeSet::from([
                "a/A/both.patch".to_string(),
                "a/A/commit-only.patch".to_string()
            ])
        );
        let missing = "name = 'A'\nversion = '1.0'\ntoolchain = SYSTEM\npatches = ['gone.patch']\n";
        let error = recipe_needs(missing, (&commit, "a/A"), &commit, &robot).unwrap_err();
        assert!(error.contains("gone.patch"), "{error}");
        let templated =
            "name = 'A'\nversion = '1.0'\ntoolchain = SYSTEM\npatches = ['%(nosuchtemplate)s-x.patch']\n";
        assert!(recipe_needs(templated, (&commit, "a/A"), &commit, &robot).is_err());
    }
}
