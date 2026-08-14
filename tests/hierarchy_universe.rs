//! A stack is not built at one toolchain.
//!
//! A recipe at GCC takes its CMake from GCCcore and its licence bits from
//! SYSTEM, the way EasyBuild's minimal-toolchain search does. A universe
//! filtered to the policy toolchain alone cannot satisfy its own members'
//! dependencies, and the solver reports them as missing packages rather than
//! as an unsatisfiable constraint, which is a confusing way to learn that the
//! candidate was never loaded.

use std::fs;
use std::path::Path;

fn write(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// Tree shaped like the real case: an application at GCC-15.2.0 whose build
/// dependency exists only at GCCcore-15.2.0.
fn tree(root: &Path) {
    write(
        root,
        "f/FlexiBLAS/FlexiBLAS-3.5.0-GCC-15.2.0.eb",
        "easyblock = 'CMakeMake'\nname = 'FlexiBLAS'\nversion = '3.5.0'\n\
         homepage = 'https://example.invalid/'\n\
         description = \"fixture: an app whose build dependency lives one level down\"\n\
         toolchain = {'name': 'GCC', 'version': '15.2.0'}\n\
         builddependencies = [('CMake', '4.2.1')]\n\
         moduleclass = 'lib'\n",
    );
    write(
        root,
        "c/CMake/CMake-4.2.1-GCCcore-15.2.0.eb",
        "easyblock = 'ConfigureMake'\nname = 'CMake'\nversion = '4.2.1'\n\
         homepage = 'https://example.invalid/'\n\
         description = \"fixture: the build dependency, at the subtoolchain\"\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         moduleclass = 'devel'\n",
    );
}

#[test]
fn a_build_dependency_at_the_subtoolchain_resolves() {
    let dir = tempfile::tempdir().unwrap();
    tree(dir.path());
    let policy_path = dir.path().join("policy.json");
    fs::write(
        &policy_path,
        r#"{"toolchain": {"name": "GCC", "version": "15.2.0"},
            "roots": ["FlexiBLAS"], "objective": "prefer_newer"}"#,
    )
    .unwrap();
    let lock_out = dir.path().join("stack.lock.json");
    let build_list = dir.path().join("build-list.txt");
    let lock = eb_stack::solve_from_easyconfigs_with_extras(
        &[dir.path()],
        &policy_path,
        None,
        &lock_out,
        None,
        eb_stack::SolveExtraOut {
            build_list_out: Some(&build_list),
            stack_diff_out: None,
        },
    )
    .expect("a GCC recipe must be able to build against its own GCCcore layer");

    assert!(
        lock.package("FlexiBLAS").is_some(),
        "the root is missing: {lock:?}"
    );
    assert!(
        lock.package("CMake").is_some(),
        "the subtoolchain build dependency was not selected: {lock:?}"
    );

    // The whole point of solving it: the order to build them in.
    let listing = fs::read_to_string(&build_list).unwrap();
    let cmake_at = listing.find("CMake").expect("CMake in the build list");
    let flexiblas_at = listing
        .find("FlexiBLAS")
        .expect("FlexiBLAS in the build list");
    assert!(
        cmake_at < flexiblas_at,
        "a dependency has to be built first:\n{listing}"
    );
}

/// The call the solve path makes, in isolation: if this returns nothing the
/// universe filter silently degrades to the policy toolchain alone.
#[test]
fn the_solve_path_can_get_a_hierarchy_for_a_bare_gcc_toolchain() {
    let gcc = eb_stack::domain::Toolchain {
        name: "GCC".into(),
        version: "15.2.0".into(),
    };
    let got = eb_stack::hierarchy::hierarchy_for_with_tree(&gcc, None, &[]);
    let members = got.expect("GCC admits its own GCCcore and system").members;
    let names: Vec<String> = members.iter().map(|m| m.name.clone()).collect();
    assert!(names.iter().any(|n| n == "GCCcore"), "{names:?}");
}

/// Between parsing and solving, where does the subtoolchain candidate go?
#[test]
fn the_parsed_tree_carries_the_subtoolchain_candidate() {
    let dir = tempfile::tempdir().unwrap();
    tree(dir.path());
    let parsed = eb_stack::parse_easyconfig_tree(dir.path()).expect("parse");
    let seen: Vec<String> = parsed
        .candidates
        .iter()
        .map(|c| format!("{}@{}-{}", c.name, c.toolchain.name, c.toolchain.version))
        .collect();
    assert!(
        seen.iter().any(|s| s.starts_with("CMake@GCCcore")),
        "parse dropped it: {seen:?}, skipped: {:?}",
        parsed.skipped
    );

    let gcc = eb_stack::domain::Toolchain {
        name: "GCC".into(),
        version: "15.2.0".into(),
    };
    let members = eb_stack::hierarchy::hierarchy_for_with_tree(&gcc, None, &parsed.candidates)
        .unwrap()
        .members;
    let kept = eb_stack::filter_toolchain_hierarchy(&parsed.candidates, &gcc, &members);
    let kept_names: Vec<String> = kept.iter().map(|c| c.name.clone()).collect();
    assert!(kept_names.iter().any(|n| n == "CMake"), "{kept_names:?}");
}

/// The same universe, but the subtoolchain package is what was asked for.
/// A root has to be findable wherever it legitimately lives in the hierarchy.
#[test]
fn a_root_that_lives_at_the_subtoolchain_is_found() {
    let dir = tempfile::tempdir().unwrap();
    tree(dir.path());
    let policy_path = dir.path().join("policy-cmake.json");
    fs::write(
        &policy_path,
        r#"{"toolchain": {"name": "GCC", "version": "15.2.0"},
            "roots": ["CMake"], "objective": "prefer_newer"}"#,
    )
    .unwrap();
    let lock_out = dir.path().join("cmake.lock.json");
    let lock = eb_stack::solve_from_easyconfigs(&[dir.path()], &policy_path, None, &lock_out, None)
        .expect("CMake at GCCcore is in the generation of a GCC policy");
    assert_eq!(lock.package("CMake").unwrap().version, "4.2.1");
}

/// The conflict a whole-generation solve hits: a generation carries the same
/// package at two levels, and both are wanted.
///
/// EasyBuild installs Perl at GCCcore and Perl at SYSTEM side by side as
/// different modules, and recipes pin whichever they were built against. A
/// solver that keys a package by name alone makes those one variable, so the
/// stack is unsatisfiable by construction rather than by any real conflict.
/// Co-installability is the property being modelled: Vouillon and Di Cosmo
/// state it for Debian in doi:10.1145/2522920.2522927, and it is why Spack
/// keys by whole spec rather than name, doi:10.1109/sc41404.2022.00040.
fn two_level_tree(root: &Path) {
    // The bootstrap Perl, at SYSTEM, which zlib pins explicitly.
    write(
        root,
        "p/Perl/Perl-5.38.0.eb",
        "easyblock = 'ConfigureMake'\nname = 'Perl'\nversion = '5.38.0'\n\
         homepage = 'https://example.invalid/'\n\
         description = \"fixture: the bootstrap Perl at SYSTEM\"\n\
         toolchain = SYSTEM\nmoduleclass = 'lang'\n",
    );
    // The generation's Perl, one level up.
    write(
        root,
        "p/Perl/Perl-5.42.0-GCCcore-15.2.0.eb",
        "easyblock = 'ConfigureMake'\nname = 'Perl'\nversion = '5.42.0'\n\
         homepage = 'https://example.invalid/'\n\
         description = \"fixture: the generation Perl at GCCcore\"\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\nmoduleclass = 'lang'\n",
    );
    write(
        root,
        "z/zlib/zlib-2.3.2-GCCcore-15.2.0.eb",
        "easyblock = 'ConfigureMake'\nname = 'zlib'\nversion = '2.3.2'\n\
         homepage = 'https://example.invalid/'\n\
         description = \"fixture: pins the bootstrap Perl by toolchain\"\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         builddependencies = [('Perl', '5.38.0', '', SYSTEM)]\nmoduleclass = 'lib'\n",
    );
    write(
        root,
        "o/OpenMPI/OpenMPI-5.0.10-GCC-15.2.0.eb",
        "easyblock = 'ConfigureMake'\nname = 'OpenMPI'\nversion = '5.0.10'\n\
         homepage = 'https://example.invalid/'\n\
         description = \"fixture: wants the generation Perl and zlib\"\n\
         toolchain = {'name': 'GCC', 'version': '15.2.0'}\n\
         builddependencies = [('Perl', '5.42.0')]\n\
         dependencies = [('zlib', '2.3.2')]\nmoduleclass = 'mpi'\n",
    );
}

#[test]
fn one_package_at_two_toolchain_levels_is_two_packages() {
    let dir = tempfile::tempdir().unwrap();
    two_level_tree(dir.path());
    let policy_path = dir.path().join("policy.json");
    fs::write(
        &policy_path,
        r#"{"toolchain": {"name": "GCC", "version": "15.2.0"},
            "roots": ["OpenMPI"], "objective": "prefer_newer"}"#,
    )
    .unwrap();
    let lock_out = dir.path().join("gen.lock.json");
    let lock = eb_stack::solve_from_easyconfigs(&[dir.path()], &policy_path, None, &lock_out, None)
        .expect("both Perls are installable side by side");

    // Both levels are selected, because both are required by something.
    let perls: Vec<String> = lock
        .packages
        .iter()
        .filter(|p| p.name == "Perl")
        .map(|p| format!("{}@{}", p.version, p.toolchain.name))
        .collect();
    assert!(perls.contains(&"5.38.0@system".to_string()), "{perls:?}");
    assert!(perls.contains(&"5.42.0@GCCcore".to_string()), "{perls:?}");
    assert!(lock.package("OpenMPI").is_some(), "{lock:?}");
}
