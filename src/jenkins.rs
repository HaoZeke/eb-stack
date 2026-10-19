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
    /// When set, only these package names are written. A pipeline that runs
    /// `eb --robot` on each line builds their dependencies anyway, so a list
    /// of roots keeps the site's view at one line per application.
    pub only: Option<Vec<String>>,
}

/// Render packages, already in build order, as a Jenkins build list.
pub fn format_build_list<'a>(
    packages: impl IntoIterator<Item = &'a LockPackage>,
    options: &JenkinsOptions,
) -> String {
    let mut out = String::new();
    for p in packages {
        if options
            .only
            .as_ref()
            .is_some_and(|names| !names.iter().any(|n| n == &p.name))
        {
            continue;
        }
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

/// A titled group of packages in a build list, as a `#### Title ####` header
/// and the entries below it up to the next header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The header text without its `#` marks.
    pub title: String,
    /// Package names under the header, commented-out entries included: a
    /// line a site switched off still says where that package belongs.
    pub packages: Vec<String>,
}

/// The easyconfig filename an entry line names, commented out or not, with
/// the line's text after the comment marks.
fn entry(line: &str) -> Option<(&str, &str)> {
    let body = line.trim_start_matches('#').trim();
    let file = body.split_whitespace().next()?;
    file.ends_with(".eb").then_some((file, body))
}

/// The title of a section header: three or more `#`, then text that is not an
/// entry. `## Todo: ...` and `# a remark` are comments, not headers.
fn header(line: &str) -> Option<&str> {
    let line = line.trim();
    if !line.starts_with("###") || entry(line).is_some() {
        return None;
    }
    let title = line.trim_matches('#').trim();
    (!title.is_empty()).then_some(title)
}

/// Read the sections of a build list, in file order.
pub fn parse_sections(text: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    for line in text.lines() {
        if let Some(title) = header(line) {
            sections.push(Section {
                title: title.to_string(),
                packages: Vec::new(),
            });
        } else if let (Some((file, _)), Some(section)) = (entry(line), sections.last_mut()) {
            if let Some(name) = package_name(file) {
                if !section.packages.iter().any(|p| p == name) {
                    section.packages.push(name.to_string());
                }
            }
        }
    }
    sections
}

/// Regroup the entries of `list` under the sections a previous list files
/// their packages in. Each entry line is kept as written, commented out or
/// not and with its flags; within a section entries keep their order in
/// `list`. Entries whose package no section names go under `unfiled`, last.
/// The comment block before the first entry of `list` stays on top; other
/// comments and headers of `list` are dropped.
pub fn group_by_sections(list: &str, sections: &[Section], unfiled: &str) -> String {
    let mut preamble = Vec::new();
    let mut grouped: Vec<Vec<&str>> = vec![Vec::new(); sections.len() + 1];
    let mut seen_entry = false;
    for line in list.lines() {
        let Some((file, _)) = entry(line) else {
            if !seen_entry && header(line).is_none() {
                preamble.push(line.trim_end());
            }
            continue;
        };
        seen_entry = true;
        let slot = package_name(file)
            .and_then(|name| {
                sections
                    .iter()
                    .position(|s| s.packages.iter().any(|p| p == name))
            })
            .unwrap_or(sections.len());
        grouped[slot].push(line.trim_end());
    }
    while preamble.last().is_some_and(|l| l.is_empty()) {
        preamble.pop();
    }
    let mut out = String::new();
    for line in &preamble {
        out.push_str(line);
        out.push('\n');
    }
    let titles = sections.iter().map(|s| s.title.as_str()).chain([unfiled]);
    for (title, lines) in titles.zip(&grouped) {
        if lines.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("#### {title} ####\n"));
        for line in lines {
            out.push_str(line);
            out.push('\n');
        }
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
            only: None,
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

    #[test]
    fn only_the_named_roots_are_written() {
        let options = JenkinsOptions {
            only: Some(vec!["GROMACS".into()]),
            ..JenkinsOptions::default()
        };
        let packages = [
            pkg("OpenMPI", "o/OpenMPI/OpenMPI-5.0.10-GCC-15.2.0.eb", None),
            pkg("GROMACS", "g/GROMACS/GROMACS-2026.3-foss-2026.1.eb", None),
        ];
        assert_eq!(
            format_build_list(&packages, &options),
            "GROMACS-2026.3-foss-2026.1.eb\n"
        );
    }

    const SECTIONED: &str = "#### Basic Compilers ####\n\
GCC-14.2.0.eb\n\
## Todo: not a header\n\
### MPI Libraries ###\n\
# OpenMPI-5.0.7-GCC-14.2.0.eb --hooks=/sw/hooks/mpi_hook.py\n\
#### Chemistry ####\n\
GROMACS-2025.2-foss-2025a.eb\n\
CP2K-2025.1-foss-2025a.eb\n";

    #[test]
    fn sections_keep_commented_entries_and_skip_remarks() {
        let sections = parse_sections(SECTIONED);
        let titles: Vec<&str> = sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Basic Compilers", "MPI Libraries", "Chemistry"]);
        assert_eq!(sections[0].packages, ["GCC"]);
        assert_eq!(sections[1].packages, ["OpenMPI"]);
        assert_eq!(sections[2].packages, ["GROMACS", "CP2K"]);
    }

    #[test]
    fn a_list_regroups_under_the_previous_sections() {
        let sections = parse_sections(SECTIONED);
        let list = "# Snel-2026 header\n\
#\n\
#### 2. Closure ####\n\
# CP2K-2026.1-foss-2026.1.eb\n\
# GROMACS-2026.3-foss-2026.1.eb --hooks=/sw/hooks/x.py\n\
#### 3. Carried ####\n\
# OpenMPI-5.0.10-GCC-15.2.0.eb\n\
# SeisSol-1.3.1-foss-2026.1.eb\n";
        assert_eq!(
            group_by_sections(list, &sections, "New in this generation"),
            "# Snel-2026 header\n\
#\n\
\n\
#### MPI Libraries ####\n\
# OpenMPI-5.0.10-GCC-15.2.0.eb\n\
\n\
#### Chemistry ####\n\
# CP2K-2026.1-foss-2026.1.eb\n\
# GROMACS-2026.3-foss-2026.1.eb --hooks=/sw/hooks/x.py\n\
\n\
#### New in this generation ####\n\
# SeisSol-1.3.1-foss-2026.1.eb\n"
        );
    }
}
