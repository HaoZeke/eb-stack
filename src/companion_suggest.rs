//! Copy-paste companion argv for a failed generation bump.

use crate::package_config::PackageConfigLayer;
use crate::target::shell_quote;
use crate::version::{cmp_version, parse_requirement, RequirementOp};
use std::path::{Path, PathBuf};

/// If `--out-dir/easyconfigs` exists, search it first as a robot root.
pub fn with_outdir_overlay(mut roots: Vec<PathBuf>, out_dir: &Path) -> Vec<PathBuf> {
    let overlay = out_dir.join("easyconfigs");
    if overlay.is_dir() && !roots.iter().any(|root| root == &overlay) {
        roots.insert(0, overlay);
    }
    roots
}

/// Newest `{name}-*.eb` under `letter/name/`.
pub fn find_named_easyconfig(roots: &[PathBuf], name: &str) -> Option<PathBuf> {
    let letter = name.chars().next()?.to_ascii_lowercase();
    for root in roots {
        let dir = root.join(letter.to_string()).join(name);
        if let Some(found) = newest_named_eb(&dir, name) {
            return Some(found);
        }
    }
    None
}

fn newest_named_eb(dir: &Path, name: &str) -> Option<PathBuf> {
    let prefix = format!("{name}-");
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "eb")
                && path
                    .file_name()
                    .and_then(|file| file.to_str())
                    .is_some_and(|file| file.starts_with(&prefix))
        })
        .max_by(|left, right| {
            cmp_version(
                &easyconfig_version_key(left, &prefix),
                &easyconfig_version_key(right, &prefix),
            )
        })
}

fn easyconfig_version_key(path: &Path, prefix: &str) -> String {
    path.file_name()
        .and_then(|file| file.to_str())
        .and_then(|file| file.strip_prefix(prefix))
        .map(|rest| rest.trim_end_matches(".eb").to_string())
        .unwrap_or_default()
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
    if let Some(source) = find_named_easyconfig(roots, name) {
        let mut line = format!(
            "eb-stack package bump --source {} --toolchain-name {} --toolchain-version {}",
            shell_quote(&source.display().to_string()),
            shell_quote(toolchain_name),
            shell_quote(toolchain_version)
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
        let mut line = format!(
            "eb-stack package plan --package-config {}",
            shell_quote(&config.display().to_string())
        );
        if let Some(foreign) = find_foreign_package_py(roots, name) {
            line.push_str(&format!(
                " --source {}",
                shell_quote(&foreign.display().to_string())
            ));
        }
        let (plan_tc, policy) =
            plan_toolchain_and_policy(&config, toolchain_name, toolchain_version);
        line.push_str(&format!(
            " --toolchain-name {} --toolchain-version {}",
            shell_quote(&plan_tc),
            shell_quote(toolchain_version)
        ));
        if let Some(policy) = policy {
            line.push_str(&format!(
                " --stack-policy {}",
                shell_quote(&policy.display().to_string())
            ));
        }
        line.push_str(&format!(
            " --easyconfigs {} --out-dir {}",
            shell_quote(robot),
            shell_quote(&out_dir.display().to_string())
        ));
        return line;
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

fn companion_version_arg(pin: Option<&str>) -> Option<String> {
    let pin = pin.map(str::trim).filter(|value| !value.is_empty())?;
    if let Ok(requirement) = parse_requirement(pin) {
        return version_from_requirement(&requirement);
    }
    let stripped = pin.trim_start_matches(['=', 'v', 'V']);
    stripped
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit())
        .then(|| stripped.to_string())
}

fn version_from_requirement(requirement: &crate::version::Requirement) -> Option<String> {
    let mut lower = None;
    for clause in &requirement.clauses {
        match clause.op {
            RequirementOp::Exact
            | RequirementOp::AtLeast
            | RequirementOp::Above
            | RequirementOp::Compatible
            | RequirementOp::Caret
            | RequirementOp::Tilde => {
                if clause.version == "0" || clause.version == "0.0.0" {
                    continue;
                }
                lower = Some(clause.version.clone());
                if matches!(clause.op, RequirementOp::Exact) {
                    break;
                }
            }
            RequirementOp::NotEqual | RequirementOp::AtMost | RequirementOp::Below => {}
        }
    }
    lower
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
        fs::write(
            pkg.join("demo.toml"),
            "schema_version = 1\n\n[build]\neasyblock = \"PythonBundle\"\n",
        )
        .expect("bundle toml");
        fs::write(stacks.join("foss-2026.1.toml"), "schema_version = 1\n").expect("foss");
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
            "PythonBundle must retarget to gfbf even without gfbf-2026.1.toml, got {argv}"
        );
        assert!(
            !argv.contains("--stack-policy "),
            "missing gfbf policy is omitted, not a foss fallback: {argv}"
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
    }
}
