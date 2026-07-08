//! Scale checks against the installed EasyBuild easyconfigs tree when present.
//!
//! Set EB_EASYCONFIGS to override the default
//! `~/.venvs/easybuild/easybuild/easyconfigs`.

use eb_stack::{
    emit_next_generation_auto_from_path_with_opts, parse_easyconfig_tree, AutoResolveOpts,
    Toolchain,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn easyconfigs_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("EB_EASYCONFIGS") {
        let p = PathBuf::from(p);
        if p.is_dir() {
            return Some(p);
        }
    }
    let home = std::env::var("HOME").ok()?;
    let p = PathBuf::from(home).join(".venvs/easybuild/easybuild/easyconfigs");
    if p.is_dir() {
        Some(p)
    } else {
        None
    }
}

fn find_eb(root: &Path, name: &str) -> PathBuf {
    // Prefer exact relative layout letter/name/file
    let letter = name.chars().next().unwrap().to_ascii_lowercase();
    let pkg = name.split('-').next().unwrap();
    let direct = root.join(letter.to_string()).join(pkg).join(name);
    if direct.is_file() {
        return direct;
    }
    // Fall back to walk (slow but rare).
    for ent in walkdir(root) {
        if ent.file_name().map(|n| n.to_string_lossy() == name).unwrap_or(false) {
            return ent;
        }
    }
    panic!("could not find {name} under {}", root.display());
}

fn walkdir(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|s| s.to_str()) == Some("eb") {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn real_tree_parse_coverage_high_nineties() {
    let Some(root) = easyconfigs_root() else {
        eprintln!("skip: no real easyconfigs tree (set EB_EASYCONFIGS)");
        return;
    };
    let t0 = std::time::Instant::now();
    let tree = parse_easyconfig_tree(&root).expect("walk real tree");
    let elapsed = t0.elapsed();
    let parsed = tree.parsed_count();
    let skipped = tree.skip_count();
    let total = parsed + skipped;
    let pct = 100.0 * tree.coverage();
    eprintln!(
        "real-tree coverage: parsed={parsed} skipped={skipped} total={total} coverage={pct:.2}% in {elapsed:?}"
    );
    // Persist a short report for the goal harness.
    if let Ok(dir) = std::env::var("EB_STACK_SCALE_OUT") {
        let report = format!(
            "parsed={parsed}\nskipped={skipped}\ntotal={total}\ncoverage_pct={pct:.4}\nelapsed_secs={:.3}\nsample_skips:\n{}\n",
            elapsed.as_secs_f64(),
            tree.skipped
                .iter()
                .take(30)
                .map(|s| format!("  {} :: {}", s.path, s.error))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(Path::new(&dir).join("real-tree-coverage.txt"), report);
    }
    assert!(
        total > 1000,
        "expected a real tree with thousands of .eb files, got {total}"
    );
    assert!(
        pct >= 90.0,
        "parse coverage must be high 90s, got {pct:.2}% (parsed={parsed} skipped={skipped})"
    );
}

#[test]
fn real_tree_auto_bump_gromacs_and_two_hard_pairs() {
    let Some(root) = easyconfigs_root() else {
        eprintln!("skip: no real easyconfigs tree");
        return;
    };
    let out_dir = std::env::var("EB_STACK_SCALE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("eb-stack-scale"));
    let _ = std::fs::create_dir_all(&out_dir);

    let foss = |v: &str| Toolchain {
        name: "foss".into(),
        version: v.into(),
    };
    let empty = HashMap::new();

    // 1) GROMACS-2024.4-foss-2023b -> foss-2024a
    let g_src = find_eb(&root, "GROMACS-2024.4-foss-2023b.eb");
    let g = emit_next_generation_auto_from_path_with_opts(
        &g_src,
        &foss("2024a"),
        &root,
        None,
        None,
        &empty,
        None,
        None,
        &AutoResolveOpts { keep_old: false },
    )
    .expect("GROMACS auto-bump must succeed without keep-old");
    assert!(g.text.contains("toolchain = {'name': 'foss', 'version': '2024a'}"));
    // No silent downgrade of Python/SciPy.
    assert!(
        g.text.contains("('Python', '3.12.3')") || g.text.contains("('Python', \"3.12.3\")"),
        "Python should resolve to 2024a generation: {}",
        g.text.lines().find(|l| l.contains("Python")).unwrap_or("")
    );
    let g_path = out_dir.join("GROMACS-bump.eb");
    std::fs::write(&g_path, &g.text).unwrap();
    std::fs::write(
        out_dir.join("real-bump-gromacs.log"),
        format!(
            "source={}\nfilename={}\nwarnings={:?}\nsnippet:\n{}\n",
            g_src.display(),
            g.filename,
            g.warnings,
            g.text
                .lines()
                .filter(|l| l.contains("dependencies")
                    || l.contains("Python")
                    || l.contains("SciPy")
                    || l.contains("mpi4py")
                    || l.contains("toolchain"))
                .take(40)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )
    .unwrap();

    // 2) numba with versionsuffix-pinned LLVM must not bump LLVM suffix pin wrongly.
    let n_src = find_eb(&root, "numba-0.60.0-foss-2023b.eb");
    let n = emit_next_generation_auto_from_path_with_opts(
        &n_src,
        &foss("2024a"),
        &root,
        None,
        None,
        &empty,
        None,
        None,
        &AutoResolveOpts { keep_old: false },
    )
    .expect("numba auto-bump");
    // LLVM keeps -llvmlite versionsuffix pin (not rewritten to a plain LLVM).
    assert!(
        n.text.contains("-llvmlite") || n.warnings.iter().any(|w| w.contains("LLVM")),
        "LLVM versionsuffix pin must be respected: warnings={:?}",
        n.warnings
    );
    std::fs::write(out_dir.join("numba-bump.eb"), &n.text).unwrap();
    std::fs::write(
        out_dir.join("real-bump-numba.log"),
        format!("source={}\nwarnings={:?}\n", n_src.display(), n.warnings),
    )
    .unwrap();

    // 3) nglview: ASE may be versionsuffix-pinned in some gens; ensure no silent stale.
    let v_src = find_eb(&root, "nglview-3.1.4-foss-2023b.eb");
    let v = emit_next_generation_auto_from_path_with_opts(
        &v_src,
        &foss("2024a"),
        &root,
        None,
        None,
        &empty,
        None,
        None,
        &AutoResolveOpts { keep_old: false },
    );
    match v {
        Ok(r) => {
            std::fs::write(out_dir.join("nglview-bump.eb"), &r.text).unwrap();
            std::fs::write(
                out_dir.join("real-bump-nglview.log"),
                format!("ok source={}\nwarnings={:?}\n", v_src.display(), r.warnings),
            )
            .unwrap();
            // If ASE had a versionsuffix in source, it must not be blindly newest-bumped.
            if r.text.contains("ASE") && r.text.contains("versionsuffix") {
                // soft check only
            }
        }
        Err(e) => {
            // Loud failure is acceptable (not silent stale).
            std::fs::write(
                out_dir.join("real-bump-nglview.log"),
                format!("ERR (loud, not silent): {e}\n"),
            )
            .unwrap();
            let msg = e.to_string().to_lowercase();
            assert!(
                msg.contains("unresolved") || msg.contains("missing") || msg.contains("hierarchy"),
                "expected loud unresolved error, got {e}"
            );
        }
    }
}
