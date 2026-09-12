//! Copy-paste companion argv for a failed generation bump.

use std::path::{Path, PathBuf};

/// If `--out-dir/easyconfigs` exists, search it as a robot root.
pub fn with_outdir_overlay(mut roots: Vec<PathBuf>, out_dir: &Path) -> Vec<PathBuf> {
    let overlay = out_dir.join("easyconfigs");
    if overlay.is_dir() && !roots.iter().any(|root| root == &overlay) {
        roots.push(overlay);
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
    let mut hits: Vec<PathBuf> = std::fs::read_dir(dir)
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
        .collect();
    hits.sort();
    hits.pop()
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

/// `package.py` beside a robot copy (`../spack/py_{name}/package.py`).
pub fn find_foreign_package_py(roots: &[PathBuf], name: &str) -> Option<PathBuf> {
    let slug = name.to_ascii_lowercase();
    let rels = [
        format!("py_{slug}/package.py"),
        format!("{slug}/package.py"),
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
    if let Some(source) = find_named_easyconfig(roots, name) {
        let mut line = format!(
            "eb-stack package bump --source {} --toolchain-name {toolchain_name} --toolchain-version {toolchain_version}",
            source.display()
        );
        if let Some(pin) = version_pin.filter(|pin| {
            !pin.is_empty() && pin.chars().next().is_some_and(|c| c.is_ascii_digit())
        }) {
            line.push_str(&format!(" --version {pin}"));
        }
        if let Some(config) = find_sibling_package_config(package_configs, name) {
            line.push_str(&format!(" --package-config {}", config.display()));
        }
        line.push_str(&format!(
            " --easyconfigs {robot} --out-dir {}",
            out_dir.display()
        ));
        return line;
    }
    if let Some(config) = find_sibling_package_config(package_configs, name) {
        let mut line = format!("eb-stack package plan --package-config {}", config.display());
        if let Some(foreign) = find_foreign_package_py(roots, name) {
            line.push_str(&format!(" --source {}", foreign.display()));
        }
        let (plan_tc, policy) =
            plan_toolchain_and_policy(&config, toolchain_name, toolchain_version);
        line.push_str(&format!(
            " --toolchain-name {plan_tc} --toolchain-version {toolchain_version}"
        ));
        if let Some(policy) = policy {
            line.push_str(&format!(" --stack-policy {}", policy.display()));
        }
        line.push_str(&format!(
            " --easyconfigs {robot} --out-dir {}",
            out_dir.display()
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
    if package_config_names_python_package(package_config) {
        if let Some(dir) = &dir {
            let gfbf = dir.join(format!("gfbf-{toolchain_version}.toml"));
            if gfbf.is_file() {
                return ("gfbf".into(), Some(gfbf));
            }
        }
    }
    let policy = dir.and_then(|dir| {
        let policy = dir.join(format!("{toolchain_name}-{toolchain_version}.toml"));
        policy.is_file().then_some(policy)
    });
    (toolchain_name.to_string(), policy)
}

fn package_config_names_python_package(config: &Path) -> bool {
    std::fs::read_to_string(config)
        .ok()
        .is_some_and(|text| text.contains("PythonPackage"))
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
            robot.join("a").join("ASAGI").join("ASAGI-1.0-foss-2023a.eb"),
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
            temp.path().join("spack").join("py_pspamm").join("package.py"),
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
}
