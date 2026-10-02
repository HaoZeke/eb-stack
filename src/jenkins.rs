//! Read and write the build-list format a Jenkins easyconfigs pipeline consumes.
//!
//! One easyconfig filename per line, each optionally followed by `eb` flags
//! (`OpenMPI-5.0.8-GCC-14.3.0.eb --hooks=/sw/.../mpi_hook.py`). Lines starting
//! with `#` are comments; the pipeline reads the rest top to bottom, so the
//! order is the build order.
//!
//! A new generation's list keeps the site's per-package flags from the previous
//! one, the hooks, site easyblocks and licence acceptances. Flags that name a
//! specific recipe revision (`--from-commit`, `--from-pr`) or excuse one build
//! (`--ignore-test-failure`, `--rebuild`) describe the old version and are not
//! carried.

use crate::domain::LockPackage;

/// One non-comment line of a build list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildListLine {
    /// Easyconfig filename.
    pub file: String,
    /// The flags after it, each with its leading dashes, in file order.
    pub flags: Vec<String>,
}

/// Flags a package keeps from one generation's list to the next.
const CARRIED: &[&str] = &[
    "--hook",
    "--hooks",
    "--include-easyblocks",
    "--accept-eula-for",
];

/// Parse a build list. Comment and blank lines are skipped.
pub fn parse_build_list(text: &str) -> Vec<BuildListLine> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            let file = words.next()?;
            if !file.ends_with(".eb") {
                return None;
            }
            Some(BuildListLine {
                file: file.to_string(),
                flags: words.map(str::to_string).collect(),
            })
        })
        .collect()
}

/// Whether `file` is an easyconfig of package `name`: the filename starts with
/// `name-` and the version follows, so `hwloc` does not match `hwloc-CUDA-...`.
fn names_package(file: &str, name: &str) -> bool {
    file.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| c.is_ascii_digit() || c == 'v')
}

/// The flags `name` carries over from `previous`, taken from its last line.
pub fn carried_flags(previous: &[BuildListLine], name: &str) -> Vec<String> {
    previous
        .iter()
        .rev()
        .find(|l| names_package(&l.file, name))
        .map(|l| {
            l.flags
                .iter()
                .filter(|f| {
                    let key = f.split('=').next().unwrap_or(f);
                    CARRIED.contains(&key)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// The package name a build-list filename names: everything before the first
/// `-` that a version follows (`hwloc-CUDA-2.11.2-...` is `hwloc-CUDA`).
/// `None` when no version is found.
pub fn package_name(file: &str) -> Option<&str> {
    let stem = file.strip_suffix(".eb")?;
    stem.match_indices('-').find_map(|(i, _)| {
        let rest = &stem[i + 1..];
        let mut chars = rest.chars();
        let first = chars.next()?;
        let versioned = first.is_ascii_digit()
            || (first == 'v' && chars.next().is_some_and(|c| c.is_ascii_digit()));
        versioned.then_some(&stem[..i])
    })
}

/// Options for [`format_build_list`].
#[derive(Debug, Clone, Default)]
pub struct JenkinsOptions {
    /// Previous lists whose per-package flags carry over.
    pub previous: Vec<BuildListLine>,
    /// Systems a CUDA module is restricted to with `--include-systems`, when
    /// non-empty. A module counts as CUDA when its name or versionsuffix says so.
    pub gpu_systems: Vec<String>,
}

/// Render packages, already in build order, as a Jenkins build list.
pub fn format_build_list<'a>(
    packages: impl IntoIterator<Item = &'a LockPackage>,
    options: &JenkinsOptions,
) -> String {
    let mut out = String::new();
    for p in packages {
        let Some(file) = p
            .easyconfig_path
            .rsplit('/')
            .next()
            .filter(|f| f.ends_with(".eb"))
        else {
            continue;
        };
        out.push_str(file);
        let mut flags = carried_flags(&options.previous, &p.name);
        let cuda = p.name.contains("CUDA")
            || p.versionsuffix
                .as_deref()
                .is_some_and(|s| s.contains("CUDA"));
        if cuda
            && !options.gpu_systems.is_empty()
            && !flags.iter().any(|f| f.starts_with("--include-systems"))
        {
            flags.push(format!(
                "--include-systems={}",
                options.gpu_systems.join(",")
            ));
        }
        for f in flags {
            out.push(' ');
            out.push_str(&f);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Toolchain;

    fn pkg(name: &str, path: &str, suffix: Option<&str>) -> LockPackage {
        LockPackage {
            name: name.into(),
            version: "1".into(),
            toolchain: Toolchain {
                name: "foss".into(),
                version: "2026.1".into(),
            },
            versionsuffix: suffix.map(str::to_string),
            easyconfig_path: path.into(),
        }
    }

    const PREVIOUS: &str = "#### MPI ####\n\
OpenMPI-5.0.8-GCC-14.3.0.eb --hooks=/sw/hooks/mpi_hook.py --include-easyblocks=/sw/eb/openmpi.py --from-commit=953321cef7def8a07ac690c9c528ce859f3e9702\n\
hwloc-CUDA-2.11.2-GCCcore-14.2.0-CUDA-12.8.0.eb --skip-sanity-check\n\
hwloc-2.11.2-GCCcore-14.2.0.eb\n\
\n\
impi-2021.15.0-intel-compilers-2025.1.1.eb --accept-eula-for=Intel-oneAPI --ignore-test-failure\n";

    #[test]
    fn a_build_list_is_read_without_comments_or_blank_lines() {
        let lines = parse_build_list(PREVIOUS);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].file, "OpenMPI-5.0.8-GCC-14.3.0.eb");
        assert_eq!(lines[0].flags.len(), 3);
    }

    #[test]
    fn site_flags_carry_and_revision_flags_do_not() {
        let previous = parse_build_list(PREVIOUS);
        assert_eq!(
            carried_flags(&previous, "OpenMPI"),
            [
                "--hooks=/sw/hooks/mpi_hook.py",
                "--include-easyblocks=/sw/eb/openmpi.py"
            ]
        );
        assert_eq!(
            carried_flags(&previous, "impi"),
            ["--accept-eula-for=Intel-oneAPI"]
        );
        // hwloc is not hwloc-CUDA, and the CUDA build's sanity-check skip stays behind.
        assert!(carried_flags(&previous, "hwloc").is_empty());
        assert!(carried_flags(&previous, "hwloc-CUDA").is_empty());
    }

    #[test]
    fn cuda_modules_are_restricted_to_the_gpu_systems() {
        let options = JenkinsOptions {
            previous: parse_build_list(PREVIOUS),
            gpu_systems: vec!["gpu-a".into(), "gpu-b".into()],
        };
        let packages = [
            pkg("OpenMPI", "o/OpenMPI/OpenMPI-5.0.10-GCC-15.2.0.eb", None),
            pkg(
                "GROMACS",
                "g/GROMACS/GROMACS-2026.3-foss-2026.1-CUDA-13.0.2.eb",
                Some("-CUDA-13.0.2"),
            ),
            pkg("ghost", "", None),
        ];
        let text = format_build_list(&packages, &options);
        assert_eq!(
            text,
            "OpenMPI-5.0.10-GCC-15.2.0.eb --hooks=/sw/hooks/mpi_hook.py --include-easyblocks=/sw/eb/openmpi.py\n\
GROMACS-2026.3-foss-2026.1-CUDA-13.0.2.eb --include-systems=gpu-a,gpu-b\n"
        );
    }

    #[test]
    fn a_filename_names_its_package_up_to_the_version() {
        assert_eq!(
            package_name("hwloc-CUDA-2.11.2-GCCcore-14.2.0-CUDA-12.8.0.eb"),
            Some("hwloc-CUDA")
        );
        assert_eq!(package_name("GCCcore-14.2.0.eb"), Some("GCCcore"));
        assert_eq!(package_name("tbb-v2021.5.0-GCCcore-11.3.0.eb"), Some("tbb"));
        assert_eq!(package_name("notarecipe.eb"), None);
    }
}
