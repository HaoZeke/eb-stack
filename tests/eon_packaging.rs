//! eOn EasyBuild packaging: drive the real shipped parser/resolve path on the
//! recipes under fixtures/eon_packaging (mirrors eOn/easybuild drafts).
//!
//! Primary product is **foss-2024a feedstock-parity** (metatomic/xtb/serve/rgpot).
//! eOn 2.16.0 requires `meson_version: '>= 1.8.0'`; drafts supply Meson-1.8.2
//! plus metatensor / metatomic-torch companions.

use eb_stack::{
    check_recipe_deps, packaging_gate, parse_easyconfig_trees, resolve_easyconfig_file,
};
use std::path::PathBuf;

/// Minimum Meson version required by eOn 2.16.0 (from project meson_version).
const EON_MESON_FLOOR: (u64, u64, u64) = (1, 8, 0);

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/eon_packaging")
}

/// Parse `X.Y.Z` (or `X.Y`) into a comparable triple; non-numeric → None.
fn parse_semver_triple(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts
        .next()
        .map(|p| {
            p.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().ok())
        .unwrap_or(Some(0))?;
    Some((major, minor, patch))
}

fn version_meets_floor(version: &str, floor: (u64, u64, u64)) -> bool {
    parse_semver_triple(version).is_some_and(|v| v >= floor)
}

#[test]
fn resolve_eon_full_product_foss_2024a_feedstock_parity() {
    let p = root().join("easyconfigs/e/eOn/eOn-2.16.0-foss-2024a.eb");
    let r = resolve_easyconfig_file(&p).expect("resolve full eOn.eb");
    assert_eq!(r.name, "eOn");
    assert_eq!(r.version, "2.16.0");
    assert_eq!(r.toolchain.name, "foss");
    assert_eq!(r.toolchain.version, "2024a");
    assert_eq!(r.easyblock.as_deref(), Some("MesonNinja"));
    assert_eq!(r.moduleclass.as_deref(), Some("chem"));
    // Multi-source: eOn + rgpot + readcon-core (Siesta/feedstock style)
    assert!(
        r.checksums.len() >= 3,
        "expected >=3 multi-source checksums, got {}",
        r.checksums.len()
    );
    let opts = r.configopts.as_deref().unwrap_or("");
    for flag in [
        "-Dwith_metatomic=true",
        "-Dwith_xtb=true",
        "-Dwith_serve=true",
        "-Dwith_rgpot=true",
        "-Dwith_fortran=true",
        "-Dpip_metatomic=false",
        "-Dtorch_path=",
    ] {
        assert!(opts.contains(flag), "configopts missing {flag}: {opts}");
    }
    packaging_gate(
        &r,
        &[
            "-Dwith_metatomic=true",
            "-Dwith_xtb=true",
            "-Dwith_serve=true",
            "-Dwith_rgpot=true",
        ],
    )
    .expect("gate");
    let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
    for need in [
        "Python",
        "SciPy-bundle",
        "Eigen",
        "Highway",
        "inih",
        "quill",
        "xtb",
        "CapnProto",
        "PyTorch",
        "metatensor",
        "metatensor-torch",
        "metatomic-torch",
    ] {
        assert!(names.contains(&need), "missing dep {need} in {names:?}");
    }
    let meson = r
        .builddependencies
        .iter()
        .find(|d| d.name == "Meson")
        .expect("Meson builddep");
    assert_eq!(meson.version, "1.8.2");
    assert!(version_meets_floor(&meson.version, EON_MESON_FLOOR));
    let mta = r
        .dependencies
        .iter()
        .find(|d| d.name == "metatomic-torch")
        .unwrap();
    assert_eq!(mta.version, "0.1.15");
    // readcon-core v0.13.1 MSRV 1.88; cargo-c feedstock-style C-API install.
    let rust = r
        .builddependencies
        .iter()
        .find(|d| d.name == "Rust")
        .expect("Rust builddep");
    assert!(
        version_meets_floor(&rust.version, (1, 88, 0)),
        "Rust {} must be >= 1.88.0 for readcon-core",
        rust.version
    );
    assert!(
        r.builddependencies.iter().any(|d| d.name == "cargo-c"),
        "cargo-c required for feedstock-style readcon cinstall"
    );
    let eon_txt = std::fs::read_to_string(&p).unwrap();
    assert!(
        eon_txt.contains("cargo cinstall"),
        "preconfigopts must cargo-c install staged readcon-core"
    );
    // EasyBuild lifecycle: make_installdir wipes %(installdir)s after preconfig.
    // Stage for link in builddir; re-populate product prefix in postinstallcmds.
    assert!(
        eon_txt.contains("%(builddir)s/readcon-stage"),
        "preconfig must stage readcon under builddir for meson link"
    );
    assert!(
        eon_txt.contains("postinstallcmds"),
        "must re-stage readcon into installdir after make_installdir wipe"
    );
    assert!(
        eon_txt.contains("%(installdir)s/lib")
            && eon_txt.contains("libreadcon_core")
            && eon_txt.contains("readcon-core.h"),
        "postinstall/sanity must place libreadcon_core + header under installdir"
    );
    // preconfig-only cinstall into installdir is wiped — must not be the sole path
    let preconfig_block = eon_txt
        .lines()
        .filter(|l| l.contains("preconfigopts"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !preconfig_block.contains("--prefix %(installdir)s"),
        "preconfig must not cargo-c solely into installdir (wiped by make_installdir)"
    );
    assert!(
        !eon_txt.contains("('Rust', '1.83"),
        "must not pin Rust 1.83 (below readcon MSRV)"
    );
}

/// EasyBuild `convert_name`: `-` → `min` (e.g. SciPy-bundle → EBROOTSCIPYMINBUNDLE).
/// Hyphenated module roots must use EBROOTMETATENSORMINTORCH / EBROOTMETATOMICMINTORCH,
/// not the silent-empty EBROOTMETATENSORTORCH / EBROOTMETATOMICTORCH.
#[test]
fn eon_and_companions_use_ebroot_min_for_hyphenated_deps() {
    let drafts = root().join("easyconfigs");
    let eon = std::fs::read_to_string(drafts.join("e/eOn/eOn-2.16.0-foss-2024a.eb")).unwrap();
    assert!(
        eon.contains("$EBROOTMETATENSORMINTORCH"),
        "eOn preconfigopts must use convert_name EBROOTMETATENSORMINTORCH"
    );
    assert!(
        eon.contains("$EBROOTMETATOMICMINTORCH"),
        "eOn preconfigopts must use convert_name EBROOTMETATOMICMINTORCH"
    );
    // Wrong forms (missing 'MIN') must not appear as env refs.
    assert!(
        !eon.contains("$EBROOTMETATENSORTORCH") && !eon.contains("$EBROOTMETATOMICTORCH"),
        "eOn must not use wrong EBROOT* without MIN for hyphenated deps"
    );

    let mta = std::fs::read_to_string(
        drafts.join("m/metatomic-torch/metatomic-torch-0.1.15-foss-2024a.eb"),
    )
    .unwrap();
    assert!(
        mta.contains("$EBROOTMETATENSORMINTORCH"),
        "metatomic-torch companion must use EBROOTMETATENSORMINTORCH"
    );
    assert!(
        !mta.contains("$EBROOTMETATENSORTORCH"),
        "metatomic-torch must not use wrong EBROOTMETATENSORTORCH"
    );
}

#[test]
fn resolve_quill_companion_easyconfig() {
    let p = root().join("easyconfigs/q/quill/quill-11.1.0-GCCcore-13.3.0.eb");
    let r = resolve_easyconfig_file(&p).expect("resolve quill.eb");
    assert_eq!(r.name, "quill");
    assert_eq!(r.version, "11.1.0");
    assert_eq!(r.toolchain.label(), "GCCcore-13.3.0");
    assert_eq!(r.easyblock.as_deref(), Some("CMakeMake"));
    assert_eq!(r.moduleclass.as_deref(), Some("lib"));
}

#[test]
fn resolve_meson_companion_meets_eon_floor() {
    let p = root().join("easyconfigs/m/Meson/Meson-1.8.2-GCCcore-13.3.0.eb");
    let r = resolve_easyconfig_file(&p).expect("resolve Meson companion .eb");
    assert_eq!(r.name, "Meson");
    assert_eq!(r.version, "1.8.2");
    assert_eq!(r.toolchain.label(), "GCCcore-13.3.0");
    assert_eq!(r.easyblock.as_deref(), Some("PythonPackage"));
    assert_eq!(r.moduleclass.as_deref(), Some("tools"));
    assert!(
        version_meets_floor(&r.version, EON_MESON_FLOOR),
        "companion Meson {} must be >= {:?}",
        r.version,
        EON_MESON_FLOOR
    );
    // 1.4.0 (what 2024a ships alone) must NOT satisfy the floor check used here.
    assert!(!version_meets_floor("1.4.0", EON_MESON_FLOOR));
    assert!(version_meets_floor("1.8.0", EON_MESON_FLOOR));
}

#[test]
fn eon_full_recipe_deps_found_in_drafts_plus_real_robot() {
    let drafts = root().join("easyconfigs");
    let recipe = resolve_easyconfig_file(&drafts.join("e/eOn/eOn-2.16.0-foss-2024a.eb")).unwrap();
    // Drafts alone: companions present; Python/PyTorch/xtb still missing.
    let draft_only = parse_easyconfig_trees(&[&drafts]).unwrap();
    let incomplete = check_recipe_deps(&recipe, &draft_only.candidates);
    assert!(
        !incomplete.ok(),
        "drafts alone cannot supply Python/PyTorch/…"
    );
    assert!(
        incomplete.missing.iter().any(|m| m.name == "Python"),
        "expected Python missing: {:?}",
        incomplete.missing
    );
    for companion in [
        "quill",
        "metatensor",
        "metatensor-torch",
        "metatomic-torch",
        "Meson",
    ] {
        assert!(
            incomplete.found.iter().any(|f| f.contains(companion)),
            "drafts must supply {companion}: {:?}",
            incomplete.found
        );
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let real = PathBuf::from(&home).join(".venvs/easybuild/easybuild/easyconfigs");
    if !real.is_dir() {
        eprintln!("skip full robot check: {real:?} missing");
        return;
    }
    let real_only = parse_easyconfig_trees(&[real.as_path()]).expect("real robot");
    let without_drafts = check_recipe_deps(&recipe, &real_only.candidates);
    // Upstream robot lacks companions (quill, metatomic stack, Meson 1.8.2).
    assert!(
        without_drafts
            .missing
            .iter()
            .any(|m| m.name == "quill" || m.name == "metatomic-torch"),
        "upstream robot must lack companions: missing={:?}",
        without_drafts.missing
    );

    let merged = parse_easyconfig_trees(&[real.as_path(), drafts.as_path()]).expect("overlay");
    let check = check_recipe_deps(&recipe, &merged.candidates);
    eprintln!(
        "eOn full robot check: found={} missing={:?} coverage={:.2}%",
        check.found.len(),
        check.missing.iter().map(|m| &m.name).collect::<Vec<_>>(),
        100.0 * merged.coverage()
    );
    assert!(
        check.ok(),
        "full feedstock-parity recipe must resolve with drafts overlay: missing={:?}",
        check.missing
    );
    assert!(check.found.iter().any(|f| f.contains("metatomic-torch")));
    assert!(check
        .found
        .iter()
        .any(|f| f.contains("xtb") || f.contains("XTB") || f.contains("xtb")));
    assert!(check
        .found
        .iter()
        .any(|f| f.contains("PyTorch") || f.contains("pytorch") || f.contains("PyTorch")));
}
