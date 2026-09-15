//! Copy-paste companion argv for a failed generation bump.

use crate::domain::Toolchain;
use crate::package_config::PackageConfigLayer;
use crate::package_sources::map_source_toolchain_to_target;
use crate::target::shell_quote;
use crate::version::{cmp_version, parse_requirement};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// Which same-version easyconfig to source when version keys tie.
#[derive(Clone, Copy, Default)]
struct SourcePick<'a> {
    parent_family: Option<&'a str>,
    want_cuda: bool,
}

/// If `--out-dir/easyconfigs` exists, search it first as a robot root.
pub fn with_outdir_overlay(mut roots: Vec<PathBuf>, out_dir: &Path) -> Vec<PathBuf> {
    let overlay = out_dir.join("easyconfigs");
    if overlay.is_dir() && !roots.iter().any(|root| root == &overlay) {
        roots.insert(0, overlay);
    }
    roots
}

/// Newest `{name}-*.eb` under `letter/name/`, then a flat robot root.
///
/// EasyBuild `--robot` accepts either the letter/name tree or a directory of
/// `.eb` files. Companion argv must find both.
pub fn find_named_easyconfig(roots: &[PathBuf], name: &str) -> Option<PathBuf> {
    find_named_easyconfig_preferring(roots, name, SourcePick::default())
}

fn find_named_easyconfig_preferring(
    roots: &[PathBuf],
    name: &str,
    pick: SourcePick<'_>,
) -> Option<PathBuf> {
    let letter = name.chars().next()?.to_ascii_lowercase();
    for root in roots {
        if let Some(dir) = named_package_dir(&root.join(letter.to_string()), name) {
            if let Some(found) = newest_named_eb_preferring(&dir, name, pick) {
                return Some(found);
            }
        }
        if let Some(found) = newest_named_eb_preferring(root, name, pick) {
            return Some(found);
        }
    }
    None
}

fn named_package_dir(letter_dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = letter_dir.join(name);
    if exact.is_dir() {
        return Some(exact);
    }
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(letter_dir).ok()? {
        let path = entry.ok()?.path();
        if path.is_dir()
            && path
                .file_name()
                .and_then(|file| file.to_str())
                .is_some_and(|file| file.eq_ignore_ascii_case(name))
        {
            matches.push(path);
        }
    }
    (matches.len() == 1).then(|| matches.pop().expect("one match"))
}

fn filename_matches_package(file: &str, name: &str) -> bool {
    let Some(prefix) = file.get(..name.len() + 1) else {
        return false;
    };
    if !prefix[..name.len()].eq_ignore_ascii_case(name) || !prefix.ends_with('-') {
        return false;
    }
    // The next token is the package version. `UCX-CUDA-1.18.0-…` is a
    // different package, not UCX with a CUDA toolchain.
    file.get(name.len() + 1..)
        .and_then(|rest| rest.split('-').next())
        .is_some_and(token_looks_like_version)
}

fn newest_named_eb(dir: &Path, name: &str) -> Option<PathBuf> {
    newest_named_eb_preferring(dir, name, SourcePick::default())
}

fn newest_named_eb_preferring(dir: &Path, name: &str, pick: SourcePick<'_>) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "eb")
                && path
                    .file_name()
                    .and_then(|file| file.to_str())
                    .is_some_and(|file| filename_matches_package(file, name))
        })
        .max_by(|left, right| compare_named_sources(left, right, name, pick))
}

fn compare_named_sources(left: &Path, right: &Path, name: &str, pick: SourcePick<'_>) -> Ordering {
    cmp_version(
        &easyconfig_version_key(left, name),
        &easyconfig_version_key(right, name),
    )
    .then_with(|| source_pick_score(left, name, pick).cmp(&source_pick_score(right, name, pick)))
    .then_with(|| left.file_name().cmp(&right.file_name()))
}

/// Parent family first, then unsuffixed unless the caller asked for CUDA.
fn source_pick_score(path: &Path, name: &str, pick: SourcePick<'_>) -> (u8, u8) {
    let family = toolchain_from_easyconfig_path(path, name).map(|toolchain| toolchain.name);
    let family_match = pick
        .parent_family
        .zip(family.as_deref())
        .is_some_and(|(want, have)| want.eq_ignore_ascii_case(have));
    let has_cuda = filename_has_cuda_token(path, name);
    let suffix_score = if pick.want_cuda {
        u8::from(has_cuda)
    } else {
        u8::from(!has_cuda)
    };
    (u8::from(family_match), suffix_score)
}

fn filename_has_cuda_token(path: &Path, name: &str) -> bool {
    easyconfig_rest(path, name).is_some_and(|rest| {
        rest.split('-')
            .any(|part| part.eq_ignore_ascii_case("CUDA"))
    })
}

fn easyconfig_rest<'a>(path: &'a Path, name: &str) -> Option<&'a str> {
    path.file_name()
        .and_then(|file| file.to_str())
        .and_then(|file| file.strip_suffix(".eb"))
        .and_then(|file| {
            file.get(name.len()..)
                .filter(|_| {
                    file.get(..name.len())
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(name))
                })
                .and_then(|rest| rest.strip_prefix('-'))
        })
}

fn toolchain_from_easyconfig_path(path: &Path, name: &str) -> Option<Toolchain> {
    let rest = easyconfig_rest(path, name)?;
    let parts: Vec<&str> = rest.split('-').collect();
    first_toolchain_span(&parts).map(|(name_at, version_at)| Toolchain {
        name: parts[name_at..version_at].join("-"),
        version: parts[version_at].to_string(),
    })
}

fn easyconfig_version_key(path: &Path, name: &str) -> String {
    let Some(rest) = easyconfig_rest(path, name) else {
        return String::new();
    };
    let parts: Vec<&str> = rest.split('-').collect();
    match first_toolchain_span(&parts) {
        Some((0, _)) => String::new(),
        Some((name_at, _)) => parts[..name_at].join("-"),
        // No-span rest still stops at a versionsuffix token so
        // `2.18.3-CUDA-12.8.0` keys as `2.18.3`, not a pre-release.
        None => match parts
            .iter()
            .position(|part| is_filename_versionsuffix_token(part))
        {
            Some(0) => String::new(),
            Some(at) => parts[..at].join("-"),
            None => rest.to_string(),
        },
    }
}

/// First toolchain in `parts`: all-alpha run plus the digit-leading version.
///
/// `intel-compilers-2023.2.0` is one name, not `compilers-2023.2.0`.
fn first_toolchain_span(parts: &[&str]) -> Option<(usize, usize)> {
    for index in 0..parts.len().saturating_sub(1) {
        if !parts[index].is_empty()
            && parts[index]
                .chars()
                .all(|character| character.is_ascii_alphabetic())
            && !is_filename_versionsuffix_token(parts[index])
            && parts[index + 1]
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
        {
            let mut name_at = index;
            while name_at > 0
                && !parts[name_at - 1].is_empty()
                && parts[name_at - 1]
                    .chars()
                    .all(|character| character.is_ascii_alphabetic())
            {
                name_at -= 1;
            }
            return Some((name_at, index + 1));
        }
    }
    None
}

/// `{name}.toml` next to a parent `--package-config`.
pub fn find_sibling_package_config(configs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let slug = name.to_ascii_lowercase();
    for config in configs {
        let dir = config.parent()?;
        let candidate = dir.join(format!("{slug}.toml"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// `package.py` beside a robot copy. Named slug first, then Spack
/// `py-{slug}` / `packages/` layout, then the underscore fallback.
pub fn find_foreign_package_py(roots: &[PathBuf], name: &str) -> Option<PathBuf> {
    let slug = name.to_ascii_lowercase();
    let rels = [
        format!("{slug}/package.py"),
        format!("packages/{slug}/package.py"),
        format!("packages/py-{slug}/package.py"),
        format!("py-{slug}/package.py"),
        format!("spack/packages/{slug}/package.py"),
        format!("spack/packages/py-{slug}/package.py"),
        format!("py_{slug}/package.py"),
        format!("spack/py_{slug}/package.py"),
        format!("spack/{slug}/package.py"),
    ];
    for root in roots {
        for rel in &rels {
            let direct = root.join(rel);
            if direct.is_file() {
                return Some(direct);
            }
            if let Some(parent) = root.parent() {
                let beside = parent.join(rel);
                if beside.is_file() {
                    return Some(beside);
                }
            }
        }
    }
    None
}

/// Full `eb-stack package …` argv. Text after `companion=` is `eval`-able.
pub fn companion_argv(
    name: &str,
    version_pin: Option<&str>,
    roots: &[PathBuf],
    package_configs: &[PathBuf],
    toolchain_name: &str,
    toolchain_version: &str,
    robot: &str,
    out_dir: &Path,
) -> String {
    let roots = with_outdir_overlay(roots.to_vec(), out_dir);
    let roots = roots.as_slice();
    let pick = SourcePick {
        parent_family: Some(toolchain_name),
        want_cuda: version_pin.is_some_and(pin_asks_for_cuda),
    };
    if let Some(source) = find_named_easyconfig_preferring(roots, name, pick) {
        let parent = Toolchain {
            name: toolchain_name.to_string(),
            version: toolchain_version.to_string(),
        };
        let parsed = toolchain_from_easyconfig_path(&source, name).unwrap_or_else(|| Toolchain {
            name: "system".into(),
            version: "system".into(),
        });
        let mapped = map_source_toolchain_to_target(Some(&parsed), &parent, None);
        // Mapper empties SYSTEM version; bump `--toolchain-version` needs `system`.
        let emitted_version = if mapped.is_system() && mapped.version.is_empty() {
            "system"
        } else {
            mapped.version.as_str()
        };
        let mut line = format!(
            "eb-stack package bump --source {} --toolchain-name {} --toolchain-version {}",
            shell_quote(&source.display().to_string()),
            shell_quote(&mapped.name),
            shell_quote(emitted_version)
        );
        if let Some(pin) = companion_version_arg(version_pin) {
            line.push_str(&format!(" --version {}", shell_quote(&pin)));
        }
        if let Some(config) = find_sibling_package_config(package_configs, name) {
            line.push_str(&format!(
                " --package-config {}",
                shell_quote(&config.display().to_string())
            ));
        }
        line.push_str(&format!(
            " --easyconfigs {} --out-dir {}",
            shell_quote(robot),
            shell_quote(&out_dir.display().to_string())
        ));
        return line;
    }
    if let Some(config) = find_sibling_package_config(package_configs, name) {
        if let (Some(foreign), (plan_tc, Some(policy))) = (
            find_foreign_package_py(roots, name),
            plan_toolchain_and_policy(&config, toolchain_name, toolchain_version),
        ) {
            return format!(
                "eb-stack package plan --package-config {} --source {} --toolchain-name {} --toolchain-version {} --stack-policy {} --easyconfigs {} --out-dir {}",
                shell_quote(&config.display().to_string()),
                shell_quote(&foreign.display().to_string()),
                shell_quote(&plan_tc),
                shell_quote(toolchain_version),
                shell_quote(&policy.display().to_string()),
                shell_quote(robot),
                shell_quote(&out_dir.display().to_string())
            );
        }
    }
    format!("eb-stack package bump # {name}: no source or package-config")
}

fn plan_toolchain_and_policy(
    package_config: &Path,
    toolchain_name: &str,
    toolchain_version: &str,
) -> (String, Option<PathBuf>) {
    let dir = package_config
        .parent()
        .map(|parent| parent.join("..").join("stacks"));
    if package_config_is_python_family(package_config) {
        let policy = dir.as_ref().and_then(|dir| {
            let gfbf = dir.join(format!("gfbf-{toolchain_version}.toml"));
            gfbf.is_file().then_some(gfbf)
        });
        return ("gfbf".into(), policy);
    }
    let policy = dir.and_then(|dir| {
        let policy = dir.join(format!("{toolchain_name}-{toolchain_version}.toml"));
        policy.is_file().then_some(policy)
    });
    (toolchain_name.to_string(), policy)
}

fn package_config_is_python_family(config: &Path) -> bool {
    PackageConfigLayer::from_path(config)
        .ok()
        .and_then(|layer| layer.build)
        .and_then(|build| build.easyblock)
        .is_some_and(|easyblock| {
            matches!(
                easyblock.as_str(),
                "PythonPackage" | "PythonBundle" | "PythonPackageBundle"
            )
        })
}

fn token_looks_like_version(token: &str) -> bool {
    let mut characters = token.chars();
    match characters.next() {
        Some(character) if character.is_ascii_digit() => true,
        Some('v' | 'V') => characters.next().is_some_and(|next| next.is_ascii_digit()),
        _ => false,
    }
}

fn is_filename_versionsuffix_token(name: &str) -> bool {
    name.eq_ignore_ascii_case("CUDA")
        || name.eq_ignore_ascii_case("Java")
        || name.eq_ignore_ascii_case("Python")
}

fn pin_asks_for_cuda(pin: &str) -> bool {
    pin.split(['-', '=', ',', ' '])
        .any(|token| token.eq_ignore_ascii_case("CUDA"))
}

fn companion_version_arg(pin: Option<&str>) -> Option<String> {
    let pin = pin.map(str::trim).filter(|value| !value.is_empty())?;
    if let Ok(requirement) = parse_requirement(pin) {
        return version_from_requirement(&requirement)
            .map(|version| peel_application_version(&version));
    }
    let stripped = pin.trim_start_matches('=');
    token_looks_like_version(stripped).then(|| peel_application_version(stripped))
}

/// Residual word 2 can be a module identity; bump `--version` is the app version.
fn peel_application_version(version: &str) -> String {
    let parts: Vec<&str> = version.split('-').collect();
    let cut = match first_toolchain_span(&parts) {
        Some((name_at, _)) if name_at > 0 => Some(name_at),
        Some(_) => None,
        None => parts
            .iter()
            .position(|part| is_filename_versionsuffix_token(part))
            .filter(|&at| at > 0),
    };
    match cut {
        Some(at) => parts[..at].join("-"),
        None => version.to_string(),
    }
}

fn version_from_requirement(requirement: &crate::version::Requirement) -> Option<String> {
    requirement
        .lower_bound()
        .filter(|version| *version != "0" && *version != "0.0.0")
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn companion_argv_is_evalable_bump_when_a_recipe_exists() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path().join("robot");
        let pkg = temp.path().join("packages");
        let out = temp.path().join("out");
        fs::create_dir_all(robot.join("a").join("ASAGI")).expect("asagi dir");
        fs::create_dir_all(&pkg).expect("pkg dir");
        fs::write(
            robot
                .join("a")
                .join("ASAGI")
                .join("ASAGI-1.0-foss-2023a.eb"),
            "name = 'ASAGI'\n",
        )
        .expect("asagi recipe");
        fs::write(pkg.join("asagi.toml"), "schema_version = 1\n").expect("asagi toml");
        let argv = companion_argv(
            "ASAGI",
            Some("1.0"),
            &[robot.clone()],
            &[pkg.join("asagi.toml")],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &out,
        );
        assert!(
            argv.starts_with("eb-stack package bump --source "),
            "{argv}"
        );
        assert!(!argv.contains("action="), "{argv}");
        assert!(argv.contains("--version 1.0"), "{argv}");
        assert!(argv.contains("--package-config "), "{argv}");
    }

    #[test]
    fn companion_argv_is_evalable_plan_when_only_a_foreign_recipe_exists() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path().join("robot");
        let pkg = temp.path().join("packages");
        let stacks = temp.path().join("stacks");
        let out = temp.path().join("out");
        fs::create_dir_all(&robot).expect("robot");
        fs::create_dir_all(&pkg).expect("pkg");
        fs::create_dir_all(&stacks).expect("stacks");
        fs::create_dir_all(temp.path().join("spack").join("py_pspamm")).expect("spack");
        fs::write(
            temp.path()
                .join("spack")
                .join("py_pspamm")
                .join("package.py"),
            "class PyPspamm:\n    pass\n",
        )
        .expect("package.py");
        fs::write(
            pkg.join("pspamm.toml"),
            "schema_version = 1\n\n[build]\neasyblock = \"PythonPackage\"\n",
        )
        .expect("pspamm toml");
        fs::write(stacks.join("foss-2025a.toml"), "schema_version = 1\n").expect("foss stack");
        fs::write(stacks.join("gfbf-2025a.toml"), "schema_version = 1\n").expect("gfbf stack");
        let argv = companion_argv(
            "PSpaMM",
            Some("0.3.1"),
            &[robot.clone()],
            &[pkg.join("pspamm.toml")],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &out,
        );
        assert!(
            argv.starts_with("eb-stack package plan --package-config "),
            "{argv}"
        );
        assert!(!argv.contains("action="), "{argv}");
        assert!(argv.contains("--source "), "{argv}");
        assert!(argv.contains("--stack-policy "), "{argv}");
        assert!(
            argv.contains("--toolchain-name gfbf"),
            "PythonPackage companion must retarget to gfbf, got {argv}"
        );
        assert!(
            argv.contains("gfbf-2025a.toml"),
            "PythonPackage companion must pick the gfbf sibling stack, got {argv}"
        );
    }

    #[test]
    fn newest_named_eb_picks_version_newest_not_lex_last() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("p").join("Python");
        fs::create_dir_all(&dir).expect("python dir");
        fs::write(
            dir.join("Python-3.9.16-GCCcore-12.3.0.eb"),
            "name = 'Python'\n",
        )
        .expect("3.9");
        fs::write(
            dir.join("Python-3.10.13-GCCcore-12.3.0.eb"),
            "name = 'Python'\n",
        )
        .expect("3.10");
        let found = newest_named_eb(&dir, "Python").expect("hit");
        assert!(
            found
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("Python-3.10.13-")),
            "3.10 must beat 3.9, got {}",
            found.display()
        );
    }

    #[test]
    fn newest_named_eb_prefers_a_release_over_the_matching_rc() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("p").join("Python");
        fs::create_dir_all(&dir).expect("python dir");
        fs::write(
            dir.join("Python-3.12.0rc1-GCCcore-13.3.0.eb"),
            "name = 'Python'\n",
        )
        .expect("rc");
        fs::write(
            dir.join("Python-3.12.0-GCCcore-13.3.0.eb"),
            "name = 'Python'\n",
        )
        .expect("release");
        let found = newest_named_eb(&dir, "Python").expect("hit");
        assert!(
            found
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("Python-3.12.0-")),
            "release must beat rc, got {}",
            found.display()
        );
    }

    #[test]
    fn find_named_easyconfig_finds_a_flat_robot_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path().join("robot");
        fs::create_dir_all(&robot).expect("robot");
        let path = robot.join("HDF5-1.14.6-foss-2025a.eb");
        fs::write(&path, "name = 'HDF5'\n").expect("flat hdf5");
        let found = find_named_easyconfig(&[robot], "HDF5").expect("flat robot hit");
        assert_eq!(found, path);
    }

    #[test]
    fn find_named_easyconfig_matches_module_case() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path().join("robot");
        let dir = robot.join("h").join("HDF5");
        fs::create_dir_all(&dir).expect("hdf5 dir");
        let path = dir.join("HDF5-1.14.3-foss-2023a.eb");
        fs::write(&path, "name = 'HDF5'\n").expect("hdf5 recipe");
        let found = find_named_easyconfig(&[robot], "hdf5").expect("case-insensitive hit");
        assert_eq!(found, path);
    }

    #[test]
    fn companion_argv_quotes_paths_with_spaces() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path().join("robot dir");
        let pkg = temp.path().join("packages");
        let out = temp.path().join("out dir");
        fs::create_dir_all(robot.join("a").join("ASAGI")).expect("asagi dir");
        fs::create_dir_all(&pkg).expect("pkg dir");
        let source = robot
            .join("a")
            .join("ASAGI")
            .join("ASAGI-1.0-foss-2023a.eb");
        fs::write(&source, "name = 'ASAGI'\n").expect("asagi recipe");
        fs::write(pkg.join("asagi.toml"), "schema_version = 1\n").expect("asagi toml");
        let argv = companion_argv(
            "ASAGI",
            Some("1.0"),
            &[robot.clone()],
            &[pkg.join("asagi.toml")],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &out,
        );
        assert!(
            argv.contains(&format!("--source '{}'", source.display())),
            "{argv}"
        );
        assert!(
            argv.contains(&format!("--out-dir '{}'", out.display())),
            "{argv}"
        );
        assert!(
            argv.contains(&format!("--easyconfigs '{}'", robot.display())),
            "{argv}"
        );
    }

    #[test]
    fn companion_argv_does_not_treat_cuda_suffix_as_the_toolchain() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path().join("robot");
        let dir = robot.join("o").join("OpenMPI");
        fs::create_dir_all(&dir).expect("openmpi dir");
        fs::write(
            dir.join("OpenMPI-5.0.7-NVHPC-25.11-CUDA-12.8.0.eb"),
            "name = 'OpenMPI'\n",
        )
        .expect("openmpi recipe");
        let argv = companion_argv(
            "OpenMPI",
            Some("5.0.7"),
            &[robot.clone()],
            &[],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &temp.path().join("out"),
        );
        assert!(
            !argv.contains("--toolchain-name CUDA"),
            "CUDA versionsuffix must not become the toolchain: {argv}"
        );
        assert!(
            argv.contains("--toolchain-name NVHPC") || argv.contains("--toolchain-name foss"),
            "expected NVHPC family or mapped parent, got {argv}"
        );
    }

    #[test]
    fn a_hyphenated_sibling_is_not_the_short_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path();
        fs::write(
            robot.join("UCX-CUDA-1.18.0-foss-2023a.eb"),
            "name = 'UCX-CUDA'\n",
        )
        .expect("ucx-cuda");
        let argv = companion_argv(
            "UCX",
            Some("1.18.0"),
            &[robot.to_path_buf()],
            &[],
            "foss",
            "2023a",
            robot.to_str().expect("utf8"),
            &temp.path().join("out"),
        );
        assert!(
            !argv.contains("UCX-CUDA-1.18.0"),
            "UCX-CUDA must not be sourced as UCX: {argv}"
        );
        assert!(
            !argv.contains("--toolchain-name CUDA"),
            "stolen prefix must not become CUDA: {argv}"
        );
    }

    #[test]
    fn a_filename_without_a_toolchain_pair_stays_system() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path();
        fs::write(robot.join("Java-11.0.2.eb"), "name = 'Java'\n").expect("java");
        let argv = companion_argv(
            "Java",
            Some("11.0.2"),
            &[robot.to_path_buf()],
            &[],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &temp.path().join("out"),
        );
        assert!(
            argv.contains("--toolchain-name system"),
            "Name-version.eb is SYSTEM, not the parent: {argv}"
        );
        assert!(
            argv.contains("--toolchain-version system"),
            "SYSTEM must emit version system, not empty quotes: {argv}"
        );
        assert!(
            !argv.contains("--toolchain-version ''"),
            "empty SYSTEM version would look like a generation move: {argv}"
        );
        assert!(
            !argv.contains("--toolchain-name foss"),
            "must not retarget SYSTEM onto foss: {argv}"
        );
    }

    #[test]
    fn a_system_cuda_suffix_is_not_the_toolchain() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path();
        fs::write(robot.join("NCCL-2.18.3-CUDA-12.8.0.eb"), "name = 'NCCL'\n").expect("nccl");
        let argv = companion_argv(
            "NCCL",
            Some("2.18.3"),
            &[robot.to_path_buf()],
            &[],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &temp.path().join("out"),
        );
        assert!(
            argv.contains("--toolchain-name system"),
            "SYSTEM+CUDA filename is SYSTEM: {argv}"
        );
        assert!(
            !argv.contains("--toolchain-name CUDA"),
            "CUDA versionsuffix must not become the toolchain: {argv}"
        );
    }

    #[test]
    fn a_v_prefixed_version_is_still_a_source() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path();
        let version = format!("v{}", 2406);
        let file = format!("Demo-{version}-foss-2023a.eb");
        fs::write(robot.join(&file), "name = 'Demo'\n").expect("demo");
        let argv = companion_argv(
            "Demo",
            Some(&version),
            &[robot.to_path_buf()],
            &[],
            "foss",
            "2023a",
            robot.to_str().expect("utf8"),
            &temp.path().join("out"),
        );
        assert!(
            argv.contains(&file),
            "v-prefixed version must be a source: {argv}"
        );
        assert!(
            argv.contains("--toolchain-name foss") || argv.contains("--toolchain-name GCC"),
            "expected foss family, got {argv}"
        );
        assert!(
            argv.contains(&format!("--version {version}")),
            "v-prefixed pin must keep the v: {argv}"
        );
    }

    #[test]
    fn companion_version_arg_keeps_a_v_prefixed_pin() {
        let pin = format!("v{}", 2406);
        assert_eq!(
            companion_version_arg(Some(&pin)).as_deref(),
            Some(pin.as_str())
        );
        assert_eq!(
            companion_version_arg(Some(&format!("=={pin}"))).as_deref(),
            Some(pin.as_str())
        );
        assert_eq!(
            companion_version_arg(Some(">=1.2.3")).as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            companion_version_arg(Some("1.2.3")).as_deref(),
            Some("1.2.3")
        );
        assert_eq!(companion_version_arg(Some(">=0")), None);
    }

    #[test]
    fn companion_version_arg_peels_a_module_identity() {
        assert_eq!(
            companion_version_arg(Some("5.0.3-GCC-13.3.0")).as_deref(),
            Some("5.0.3")
        );
        assert_eq!(
            companion_version_arg(Some("==5.0.3-GCC-13.3.0-CUDA-12.6.0")).as_deref(),
            Some("5.0.3")
        );
    }

    #[test]
    fn a_hyphenated_toolchain_name_stays_one_pair() {
        let path = Path::new("MKL-2023.2.0-intel-compilers-2023.2.0.eb");
        let toolchain = toolchain_from_easyconfig_path(path, "MKL").expect("toolchain");
        assert_eq!(toolchain.name, "intel-compilers");
        assert_eq!(toolchain.version, "2023.2.0");
        assert_eq!(easyconfig_version_key(path, "MKL"), "2023.2.0");
    }

    #[test]
    fn a_cuda_suffix_is_not_part_of_the_version_key() {
        let path = Path::new("NCCL-2.18.3-CUDA-12.8.0.eb");
        assert_eq!(easyconfig_version_key(path, "NCCL"), "2.18.3");
        assert!(toolchain_from_easyconfig_path(path, "NCCL").is_none());
    }

    #[test]
    fn newest_named_eb_prefers_unsuffixed_over_same_version_cuda() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path();
        fs::write(robot.join("NCCL-2.18.3.eb"), "name = 'NCCL'\n").expect("plain");
        fs::write(robot.join("NCCL-2.18.3-CUDA-12.8.0.eb"), "name = 'NCCL'\n").expect("cuda");
        let found = newest_named_eb(robot, "NCCL").expect("hit");
        assert_eq!(
            found.file_name().and_then(|name| name.to_str()),
            Some("NCCL-2.18.3.eb")
        );
        let argv = companion_argv(
            "NCCL",
            Some("2.18.3"),
            &[robot.to_path_buf()],
            &[],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &temp.path().join("out"),
        );
        assert!(
            argv.contains("NCCL-2.18.3.eb"),
            "unsuffixed source unless CUDA was requested: {argv}"
        );
        assert!(
            !argv.contains("NCCL-2.18.3-CUDA-12.8.0.eb"),
            "CUDA file must not win a plain pin: {argv}"
        );
        assert!(
            !argv.contains("--toolchain-name CUDA"),
            "CUDA must not become the toolchain: {argv}"
        );
    }

    #[test]
    fn newest_named_eb_prefers_parent_family_over_nvhpc() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path();
        fs::write(
            robot.join("OpenMPI-5.0.7-foss-2023a.eb"),
            "name = 'OpenMPI'\n",
        )
        .expect("foss");
        fs::write(
            robot.join("OpenMPI-5.0.7-NVHPC-25.11-CUDA-12.8.0.eb"),
            "name = 'OpenMPI'\n",
        )
        .expect("nvhpc");
        let argv = companion_argv(
            "OpenMPI",
            Some("5.0.7"),
            &[robot.to_path_buf()],
            &[],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &temp.path().join("out"),
        );
        assert!(
            argv.contains("OpenMPI-5.0.7-foss-2023a.eb"),
            "parent foss must source the foss file: {argv}"
        );
        assert!(
            !argv.contains("OpenMPI-5.0.7-NVHPC-25.11-CUDA-12.8.0.eb"),
            "NVHPC must not win when parent is foss: {argv}"
        );
        assert!(
            !argv.contains("--toolchain-name CUDA"),
            "CUDA versionsuffix must not become the toolchain: {argv}"
        );
        assert!(
            argv.contains("--toolchain-name foss"),
            "foss source must stay foss: {argv}"
        );
    }

    #[test]
    fn find_foreign_package_py_finds_hyphenated_spack_layout() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pkg = temp.path().join("spack").join("packages").join("py-pspamm");
        fs::create_dir_all(&pkg).expect("spack pkg");
        let package_py = pkg.join("package.py");
        fs::write(&package_py, "class PyPspamm:\n    pass\n").expect("package.py");
        let found = find_foreign_package_py(&[temp.path().to_path_buf()], "PSpaMM")
            .expect("hyphenated Spack dir");
        assert_eq!(found, package_py);
    }

    #[test]
    fn find_foreign_package_py_prefers_the_named_slug_over_py_slug() {
        let temp = tempfile::tempdir().expect("tempdir");
        let named = temp.path().join("asagi");
        let py = temp.path().join("py_asagi");
        fs::create_dir_all(&named).expect("named");
        fs::create_dir_all(&py).expect("py");
        fs::write(named.join("package.py"), "class Asagi:\n    pass\n").expect("named py");
        fs::write(py.join("package.py"), "class PyAsagi:\n    pass\n").expect("py slug");
        let found =
            find_foreign_package_py(&[temp.path().to_path_buf()], "ASAGI").expect("named slug");
        assert_eq!(found, named.join("package.py"));
    }

    #[test]
    fn companion_argv_searches_outdir_overlay_first() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path().join("robot");
        let out = temp.path().join("out");
        let overlay = out.join("easyconfigs").join("a").join("ASAGI");
        fs::create_dir_all(robot.join("a").join("ASAGI")).expect("robot asagi");
        fs::create_dir_all(&overlay).expect("overlay asagi");
        fs::write(
            robot
                .join("a")
                .join("ASAGI")
                .join("ASAGI-1.0-foss-2023a.eb"),
            "name = 'ASAGI'\n",
        )
        .expect("robot recipe");
        let overlay_source = overlay.join("ASAGI-1.1-foss-2025a.eb");
        fs::write(&overlay_source, "name = 'ASAGI'\n").expect("overlay recipe");
        let argv = companion_argv(
            "ASAGI",
            Some("1.1"),
            &[robot],
            &[],
            "foss",
            "2025a",
            "robot",
            &out,
        );
        assert!(
            argv.contains(&overlay_source.display().to_string()),
            "overlay recipe must win, got {argv}"
        );
    }

    #[test]
    fn plan_companion_retargets_python_bundle_to_gfbf_without_a_policy_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path().join("robot");
        let pkg = temp.path().join("packages");
        let stacks = temp.path().join("stacks");
        let out = temp.path().join("out");
        fs::create_dir_all(&robot).expect("robot");
        fs::create_dir_all(&pkg).expect("pkg");
        fs::create_dir_all(&stacks).expect("stacks");
        fs::create_dir_all(robot.join("demo")).expect("demo src");
        fs::write(
            robot.join("demo").join("package.py"),
            "class Demo:\n    pass\n",
        )
        .expect("package.py");
        fs::write(
            pkg.join("demo.toml"),
            "schema_version = 1\n\n[build]\neasyblock = \"PythonBundle\"\n",
        )
        .expect("bundle toml");
        fs::write(stacks.join("foss-2026.1.toml"), "schema_version = 1\n").expect("foss");
        fs::write(stacks.join("gfbf-2026.1.toml"), "schema_version = 1\n").expect("gfbf");
        let argv = companion_argv(
            "Demo",
            Some(">=1.2.3"),
            &[robot.clone()],
            &[pkg.join("demo.toml")],
            "foss",
            "2026.1",
            robot.to_str().expect("utf8"),
            &out,
        );
        assert!(
            argv.contains("--toolchain-name gfbf"),
            "PythonBundle must retarget to gfbf, got {argv}"
        );
        assert!(
            argv.contains("--stack-policy "),
            "plan companion must carry the required stack policy: {argv}"
        );
        assert!(
            argv.contains("--source "),
            "plan companion must carry the required source: {argv}"
        );
    }

    #[test]
    fn companion_argv_emits_the_lower_bound_of_a_requirement_pin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path().join("robot");
        let out = temp.path().join("out");
        fs::create_dir_all(robot.join("a").join("ASAGI")).expect("asagi dir");
        fs::write(
            robot
                .join("a")
                .join("ASAGI")
                .join("ASAGI-1.0-foss-2023a.eb"),
            "name = 'ASAGI'\n",
        )
        .expect("recipe");
        let argv = companion_argv(
            "ASAGI",
            Some(">=1.2.3"),
            &[robot.clone()],
            &[],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &out,
        );
        assert!(argv.contains("--version 1.2.3"), "{argv}");
        let unconstrained = companion_argv(
            "ASAGI",
            Some(">=0"),
            &[robot.clone()],
            &[],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &out,
        );
        assert!(
            !unconstrained.contains("--version "),
            ">=0 stays unpinned: {unconstrained}"
        );
        let compound = companion_argv(
            "ASAGI",
            Some(">=2.0,>=3.0"),
            &[robot.clone()],
            &[],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &out,
        );
        assert!(
            compound.contains("--version 3.0"),
            "matching floor: {compound}"
        );
        let above = companion_argv(
            "ASAGI",
            Some(">1.2"),
            &[robot.clone()],
            &[],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &out,
        );
        assert!(
            !above.contains("--version 1.2"),
            ">1.2 is not a floor of 1.2: {above}"
        );
    }

    #[test]
    fn plan_companion_without_source_is_a_stub() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path().join("robot");
        let pkg = temp.path().join("packages");
        let stacks = temp.path().join("stacks");
        let out = temp.path().join("out");
        fs::create_dir_all(&robot).expect("robot");
        fs::create_dir_all(&pkg).expect("pkg");
        fs::create_dir_all(&stacks).expect("stacks");
        fs::write(pkg.join("demo.toml"), "schema_version = 1\n").expect("toml");
        fs::write(stacks.join("foss-2025a.toml"), "schema_version = 1\n").expect("foss");
        let argv = companion_argv(
            "Demo",
            None,
            &[robot.clone()],
            &[pkg.join("demo.toml")],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &out,
        );
        assert!(
            argv.starts_with("eb-stack package bump #"),
            "toml-only plan is not eval-able without --source: {argv}"
        );
    }

    #[test]
    fn bump_companion_maps_a_gcccore_recipe_to_the_generation_member() {
        let temp = tempfile::tempdir().expect("tempdir");
        let robot = temp.path().join("robot");
        let out = temp.path().join("out");
        fs::create_dir_all(robot.join("c").join("CMake")).expect("cmake dir");
        fs::write(
            robot
                .join("c")
                .join("CMake")
                .join("CMake-3.26.3-GCCcore-12.3.0.eb"),
            "name = 'CMake'\n",
        )
        .expect("recipe");
        let argv = companion_argv(
            "CMake",
            Some("3.26.3"),
            &[robot.clone()],
            &[],
            "foss",
            "2025a",
            robot.to_str().expect("utf8"),
            &out,
        );
        assert!(
            argv.contains("--toolchain-name GCCcore"),
            "CMake companion must bump the GCCcore member, got {argv}"
        );
        assert!(
            !argv.contains("--toolchain-name foss"),
            "parent foss must not be stamped on a GCCcore recipe: {argv}"
        );
    }
}
