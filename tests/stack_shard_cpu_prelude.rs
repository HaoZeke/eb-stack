//! Opt-in CPU-prerequisite list for accelerator shards.
//!
//! Synthetic tree covers: two CUDA roots sharing a GPU dependency plus an
//! unshared CPU dependency; build-only CUDA (public DualSPHysics / Triton
//! builddep pattern); filtered CUDA; an installed CPU provider; from-commit
//! materialization of a lifted CPU recipe; and two CPU roots sharing a CPU
//! dependency beside an accelerator root. Also runs the committed
//! `tests/stack_shard_fixtures/accel*.list` howto fixtures. Default
//! `stack shard` keeps legacy prelude semantics.

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;

fn write_ec(dir: &Path, file: &str, body: impl AsRef<[u8]>) {
    let path = dir.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn base_tree(root: &Path) {
    let ec = root.join("easyconfigs");
    write_ec(
        &ec,
        "c/CUDA/CUDA-13.3.0.eb",
        "easyblock = 'Binary'\n\
         name = 'CUDA'\n\
         version = '13.3.0'\n\
         homepage = 'https://example.org/cuda'\n\
         description = 'synthetic CUDA'\n\
         toolchain = SYSTEM\n\
         sources = ['cuda.sh']\n\
         moduleclass = 'lang'\n",
    );
    write_ec(
        &ec,
        "c/CpuLib/CpuLib-1.0-GCCcore-15.2.0.eb",
        "easyblock = 'ConfigureMake'\n\
         name = 'CpuLib'\n\
         version = '1.0'\n\
         homepage = 'https://example.org/cpulib'\n\
         description = 'synthetic CPU-only dependency'\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         source_urls = ['https://example.org/']\n\
         sources = [SOURCE_TAR_GZ]\n\
         moduleclass = 'lib'\n",
    );
    write_ec(
        &ec,
        "g/GpuLib/GpuLib-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb",
        "easyblock = 'ConfigureMake'\n\
         name = 'GpuLib'\n\
         version = '1.0'\n\
         versionsuffix = '-CUDA-13.3.0'\n\
         homepage = 'https://example.org/gpulib'\n\
         description = 'synthetic accelerator dependency'\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         source_urls = ['https://example.org/']\n\
         sources = [SOURCE_TAR_GZ]\n\
         dependencies = [('CUDA', '13.3.0', '', SYSTEM)]\n\
         moduleclass = 'lib'\n",
    );
    write_ec(
        &ec,
        "a/AccelApp/AccelApp-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb",
        "easyblock = 'ConfigureMake'\n\
         name = 'AccelApp'\n\
         version = '1.0'\n\
         versionsuffix = '-CUDA-13.3.0'\n\
         homepage = 'https://example.org/accelapp'\n\
         description = 'synthetic CUDA root with unshared CPU dep'\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         source_urls = ['https://example.org/']\n\
         sources = [SOURCE_TAR_GZ]\n\
         dependencies = [\n\
             ('CpuLib', '1.0'),\n\
             ('GpuLib', '1.0', '-CUDA-13.3.0'),\n\
             ('CUDA', '13.3.0', '', SYSTEM),\n\
         ]\n\
         moduleclass = 'tools'\n",
    );
    write_ec(
        &ec,
        "a/AccelPeer/AccelPeer-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb",
        "easyblock = 'ConfigureMake'\n\
         name = 'AccelPeer'\n\
         version = '1.0'\n\
         versionsuffix = '-CUDA-13.3.0'\n\
         homepage = 'https://example.org/accelpeer'\n\
         description = 'synthetic CUDA root sharing GpuLib only'\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         source_urls = ['https://example.org/']\n\
         sources = [SOURCE_TAR_GZ]\n\
         dependencies = [\n\
             ('GpuLib', '1.0', '-CUDA-13.3.0'),\n\
             ('CUDA', '13.3.0', '', SYSTEM),\n\
         ]\n\
         moduleclass = 'tools'\n",
    );
    write_ec(
        &ec,
        "t/Triton/Triton-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb",
        "easyblock = 'PythonPackage'\n\
         name = 'Triton'\n\
         version = '1.0'\n\
         versionsuffix = '-CUDA-13.3.0'\n\
         homepage = 'https://example.org/triton'\n\
         description = 'synthetic build-only CUDA (public Triton pattern)'\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         source_urls = ['https://example.org/']\n\
         sources = [SOURCE_TAR_GZ]\n\
         builddependencies = [('CUDA', '13.3.0', '', SYSTEM)]\n\
         dependencies = [('Python', '3.14.2')]\n\
         moduleclass = 'lang'\n",
    );
    write_ec(
        &ec,
        "p/Python/Python-3.14.2-GCCcore-15.2.0.eb",
        "easyblock = 'ConfigureMake'\n\
         name = 'Python'\n\
         version = '3.14.2'\n\
         homepage = 'https://example.org/python'\n\
         description = 'synthetic Python'\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         source_urls = ['https://example.org/']\n\
         sources = [SOURCE_TAR_GZ]\n\
         moduleclass = 'lang'\n",
    );
    write_ec(
        &ec,
        "g/GCCcore/GCCcore-15.2.0.eb",
        "easyblock = 'ConfigureMake'\n\
         name = 'GCCcore'\n\
         version = '15.2.0'\n\
         homepage = 'https://example.org/gcc'\n\
         description = 'synthetic GCCcore'\n\
         toolchain = SYSTEM\n\
         sources = [SOURCE_TAR_GZ]\n\
         moduleclass = 'compiler'\n",
    );
    fs::write(root.join("installed.txt"), "CUDA/13.3.0\nGCCcore/15.2.0\n").unwrap();
}

fn run(root: &Path, out: &Path, list: &str, extra: &[&str]) -> std::process::Output {
    fs::write(root.join("build.list"), list).unwrap();
    let mut args: Vec<String> = [
        "stack",
        "shard",
        "--build-list",
        root.join("build.list").to_str().unwrap(),
        "--easyconfigs",
        root.join("easyconfigs").to_str().unwrap(),
        "--installed",
        root.join("installed.txt").to_str().unwrap(),
        "--shards",
        "4",
        "--generated-at",
        "2026-11-07T00:00:00Z",
        "--out-dir",
        out.to_str().unwrap(),
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    args.extend(extra.iter().map(|s| (*s).to_string()));
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(&args)
        .output()
        .expect("run eb-stack")
}

fn plan_json(out: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(out.join("plan.json")).unwrap()).unwrap()
}

fn modules(plan: &Value, key: &str) -> Vec<String> {
    let empty = Vec::new();
    let arr = match key {
        "prelude" => plan["prelude"]["modules"].as_array().unwrap(),
        "cpu" => plan["cpu_prerequisites"]["modules"]
            .as_array()
            .unwrap_or(&empty),
        _ => plan["shards"][0]["modules"].as_array().unwrap(),
    };
    let mut names: Vec<String> = arr
        .iter()
        .map(|m| {
            m.get("module")
                .and_then(|v| v.as_str())
                .or_else(|| m.as_str())
                .unwrap()
                .to_string()
        })
        .collect();
    names.sort();
    names
}

/// Baseline: without the flag, shared GpuLib is in prelude and unshared
/// CpuLib stays inside a CUDA shard (legacy defect for EESSI accel jobs).
#[test]
fn baseline_keeps_unshared_cpu_in_shard_and_shared_gpu_in_prelude() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("tree");
    base_tree(&root);
    let out = tmp.path().join("out-default");
    let run = run(
        &root,
        &out,
        "AccelApp-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb\n\
         AccelPeer-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb\n",
        &[],
    );
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(!out.join("cpu-prerequisites.list").exists());
    let plan = plan_json(&out);
    assert_eq!(plan["lift_cpu_prerequisites"], false);
    assert!(plan.get("cpu_prerequisites").is_none());
    let prelude = modules(&plan, "prelude");
    assert!(
        prelude.iter().any(|m| m.starts_with("GpuLib/")),
        "shared accelerator dep in ordinary prelude: {prelude:?}"
    );
    let all_shard: Vec<String> = plan["shards"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["modules"].as_array().unwrap())
        .map(|m| m.as_str().unwrap().to_string())
        .collect();
    assert!(
        all_shard.iter().any(|m| m.starts_with("CpuLib/")),
        "defect: unshared CpuLib remains in a CUDA shard: {all_shard:?}"
    );
}

/// Opt-in: CpuLib → cpu-prerequisites; GpuLib stays in accelerator prelude.
#[test]
fn lift_splits_cpu_list_from_shared_accel_prelude() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("tree");
    base_tree(&root);
    let out = tmp.path().join("out-lift");
    let run = run(
        &root,
        &out,
        "AccelApp-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb\n\
         AccelPeer-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb\n",
        &["--lift-cpu-prerequisites"],
    );
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("cpu_prerequisites="),
        "summary names the CPU list when the flag is on: {stdout}"
    );
    let plan = plan_json(&out);
    assert_eq!(plan["lift_cpu_prerequisites"], true);
    let prelude = modules(&plan, "prelude");
    let cpu = modules(&plan, "cpu");
    assert!(
        prelude.iter().any(|m| m.starts_with("GpuLib/")),
        "shared GpuLib stays in accelerator prelude: {prelude:?}"
    );
    assert!(
        prelude.iter().all(|m| !m.starts_with("CpuLib/")),
        "CpuLib must not share the accelerator prelude: {prelude:?}"
    );
    assert!(
        cpu.iter().any(|m| m.starts_with("CpuLib/")),
        "CpuLib goes to cpu-prerequisites: {cpu:?}"
    );
    let cpu_list = fs::read_to_string(out.join("cpu-prerequisites.list")).unwrap();
    assert!(cpu_list.contains("CpuLib-1.0-GCCcore-15.2.0.eb"));
    assert!(!cpu_list.contains("GpuLib"));
    let prelude_list = fs::read_to_string(out.join("prelude.list")).unwrap();
    assert!(prelude_list.contains("GpuLib"));
    assert!(!prelude_list.contains("CpuLib"));
}

/// Build-only CUDA (Triton pattern) classifies as accelerator; filtering CUDA
/// makes it CPU-only and emits it under CPU settings when it is a root.
#[test]
fn build_only_cuda_and_filtered_cuda() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("tree");
    base_tree(&root);
    // Extend installed so Python is present when Triton is a CPU root after filter.
    fs::write(
        root.join("installed.txt"),
        "CUDA/13.3.0\nGCCcore/15.2.0\nPython/3.14.2-GCCcore-15.2.0\n",
    )
    .unwrap();

    let out_accel = tmp.path().join("out-triton-accel");
    let accel = run(
        &root,
        &out_accel,
        "Triton-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb\n",
        &["--lift-cpu-prerequisites"],
    );
    assert!(
        accel.status.success(),
        "build-only CUDA is accelerator: {}",
        String::from_utf8_lossy(&accel.stderr)
    );
    let plan = plan_json(&out_accel);
    let shards = plan["shards"].as_array().unwrap();
    assert!(
        !shards.is_empty(),
        "Triton with build-only CUDA stays on an accelerator shard"
    );
    assert!(
        plan.get("cpu_prerequisites")
            .and_then(|c| c["roots"].as_array())
            .map(|r| r.is_empty())
            .unwrap_or(true),
        "Triton is not a CPU root when CUDA builddep counts"
    );

    let out_cpu = tmp.path().join("out-triton-filtered");
    let filtered = run(
        &root,
        &out_cpu,
        "Triton-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb\n",
        &["--lift-cpu-prerequisites", "--filter-deps", "CUDA"],
    );
    assert!(
        filtered.status.success(),
        "filtered CUDA → CPU root: {}",
        String::from_utf8_lossy(&filtered.stderr)
    );
    let plan = plan_json(&out_cpu);
    let roots = plan["cpu_prerequisites"]["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 1);
    assert!(roots[0]["easyconfig"]
        .as_str()
        .unwrap()
        .starts_with("Triton-"));
    assert!(
        plan["shards"].as_array().unwrap().is_empty(),
        "filtered Triton leaves no accelerator shard"
    );
    let cpu_list = fs::read_to_string(out_cpu.join("cpu-prerequisites.list")).unwrap();
    assert!(cpu_list.contains("Triton-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb"));
}

/// An already-installed CPU provider is not listed in cpu-prerequisites.
#[test]
fn installed_cpu_provider_is_not_lifted() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("tree");
    base_tree(&root);
    fs::write(
        root.join("installed.txt"),
        "CUDA/13.3.0\nGCCcore/15.2.0\nCpuLib/1.0-GCCcore-15.2.0\n",
    )
    .unwrap();
    let out = tmp.path().join("out-installed");
    let run = run(
        &root,
        &out,
        "AccelApp-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb\n\
         AccelPeer-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb\n",
        &["--lift-cpu-prerequisites"],
    );
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let plan = plan_json(&out);
    let cpu = modules(&plan, "cpu");
    assert!(
        cpu.iter().all(|m| !m.starts_with("CpuLib/")),
        "installed CpuLib must not appear in cpu-prerequisites: {cpu:?}"
    );
    assert!(
        !out.join("cpu-prerequisites.list").exists()
            || !fs::read_to_string(out.join("cpu-prerequisites.list"))
                .unwrap()
                .contains("CpuLib"),
        "cpu-prerequisites.list omits installed CpuLib"
    );
}

/// From-commit CPU recipes keep source options on the separate CPU list and
/// in materialization digests.
#[test]
fn lifted_from_commit_cpu_recipe_keeps_source_options_on_cpu_list() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("tree");
    base_tree(&root);
    fs::remove_file(root.join("easyconfigs/c/CpuLib/CpuLib-1.0-GCCcore-15.2.0.eb")).unwrap();
    let commit = "abcdef1";
    let commit_dir = root.join("commit").join(commit);
    for rel in [
        "c/CpuLib/CpuLib-1.0-GCCcore-15.2.0.eb",
        "g/GCCcore/GCCcore-15.2.0.eb",
        "a/AccelApp/AccelApp-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb",
        "a/AccelPeer/AccelPeer-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb",
        "g/GpuLib/GpuLib-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb",
        "c/CUDA/CUDA-13.3.0.eb",
    ] {
        let src = if rel.starts_with("c/CpuLib/") {
            "easyblock = 'ConfigureMake'\n\
             name = 'CpuLib'\n\
             version = '1.0'\n\
             homepage = 'https://example.org/cpulib'\n\
             description = 'synthetic CPU-only dependency from commit'\n\
             toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
             source_urls = ['https://example.org/']\n\
             sources = [SOURCE_TAR_GZ]\n\
             checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
             moduleclass = 'lib'\n"
                .to_string()
        } else {
            fs::read_to_string(root.join("easyconfigs").join(rel)).unwrap()
        };
        write_ec(&commit_dir, rel, src);
    }
    fs::write(
        root.join("build.list"),
        format!(
            "AccelApp-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb --from-commit={commit}\n\
             AccelPeer-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb --from-commit={commit}\n"
        ),
    )
    .unwrap();
    let out = tmp.path().join("out-commit");
    let run = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "stack",
            "shard",
            "--build-list",
            root.join("build.list").to_str().unwrap(),
            "--easyconfigs",
            root.join("easyconfigs").to_str().unwrap(),
            "--commit-tree",
            &format!("{commit}={}", commit_dir.display()),
            "--installed",
            root.join("installed.txt").to_str().unwrap(),
            "--shards",
            "4",
            "--lift-cpu-prerequisites",
            "--materialize",
            "--build-robot-path",
            root.join("easyconfigs").to_str().unwrap(),
            "--generated-at",
            "2026-11-07T00:00:00Z",
            "--out-dir",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let cpu_list = fs::read_to_string(out.join("cpu-prerequisites.list")).unwrap();
    assert!(
        cpu_list.contains("CpuLib-1.0-GCCcore-15.2.0.eb"),
        "{cpu_list}"
    );
    assert!(
        cpu_list.contains(&format!("--from-commit={commit}")),
        "CPU list keeps the commit pin: {cpu_list}"
    );
    assert!(out.join("cpu-prerequisites.local.list").exists());
    let plan = plan_json(&out);
    let digest_key = format!("CpuLib-1.0-GCCcore-15.2.0.eb --from-commit={commit}");
    assert!(
        plan["inputs"]["easyconfigs"]
            .as_object()
            .unwrap()
            .contains_key(&digest_key),
        "inputs digest keys the pinned CPU recipe"
    );
    assert!(
        plan["materialized"]["cpu_prerequisites"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m.as_str().unwrap().starts_with("CpuLib/")),
        "materialized metadata lists rewritten CPU modules"
    );
    let prelude = fs::read_to_string(out.join("prelude.list")).unwrap();
    assert!(
        prelude.contains("GpuLib"),
        "shared accelerator dep stays in ordinary prelude: {prelude}"
    );
}

/// Two CPU roots sharing CpuLib plus one accelerator root: the shared CPU
/// dependency and both roots belong to the CPU phase; the accelerator prelude
/// / shard contain only accelerator packages.
#[test]
fn two_cpu_roots_sharing_cpu_dep_with_accel_root() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out-cpu-roots");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "stack",
            "shard",
            "--build-list",
            "tests/stack_shard_fixtures/accel-with-cpu-roots.list",
            "--easyconfigs",
            "tests/stack_shard_fixtures/accel-easyconfigs",
            "--installed",
            "tests/stack_shard_fixtures/accel-installed.txt",
            "--shards",
            "4",
            "--lift-cpu-prerequisites",
            "--generated-at",
            "2026-11-07T00:00:00Z",
            "--out-dir",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let plan = plan_json(&out);
    let cpu = modules(&plan, "cpu");
    assert!(
        cpu.iter().any(|m| m.starts_with("CpuLib/")),
        "shared CpuLib is owned by the CPU phase: {cpu:?}"
    );
    let roots = plan["cpu_prerequisites"]["roots"].as_array().unwrap();
    let root_names: Vec<&str> = roots
        .iter()
        .map(|r| r["module"].as_str().unwrap())
        .collect();
    assert!(
        root_names.iter().any(|m| m.starts_with("CpuApp/")),
        "{root_names:?}"
    );
    assert!(
        root_names.iter().any(|m| m.starts_with("CpuPeer/")),
        "{root_names:?}"
    );
    let cpu_list = fs::read_to_string(out.join("cpu-prerequisites.list")).unwrap();
    assert!(cpu_list.contains("CpuLib-1.0-GCCcore-15.2.0.eb"));
    assert!(cpu_list.contains("CpuApp-1.0-GCCcore-15.2.0.eb"));
    assert!(cpu_list.contains("CpuPeer-1.0-GCCcore-15.2.0.eb"));

    let prelude = modules(&plan, "prelude");
    assert!(
        prelude
            .iter()
            .all(|m| m.contains("CUDA") || m.starts_with("GpuLib/") || m.starts_with("ROCm")),
        "accelerator prelude must not hold CPU packages: {prelude:?}"
    );
    assert!(
        prelude.iter().all(|m| !m.starts_with("CpuLib/")
            && !m.starts_with("CpuApp/")
            && !m.starts_with("CpuPeer/")),
        "CpuLib must not remain in ordinary prelude: {prelude:?}"
    );
    for shard in plan["shards"].as_array().unwrap() {
        for m in shard["modules"].as_array().unwrap() {
            let name = m.as_str().unwrap();
            assert!(
                name.contains("CUDA") || name.starts_with("GpuLib/") || name.starts_with("Accel"),
                "accelerator shard holds non-accelerator module {name}"
            );
        }
    }
}

/// The howto fixture (`accel.list`) is runnable and splits CpuLib from GpuLib.
#[test]
fn howto_accel_fixture_lifts_cpu_lib_from_shared_gpu_prelude() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out-howto");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "stack",
            "shard",
            "--build-list",
            "tests/stack_shard_fixtures/accel.list",
            "--easyconfigs",
            "tests/stack_shard_fixtures/accel-easyconfigs",
            "--installed",
            "tests/stack_shard_fixtures/accel-installed.txt",
            "--shards",
            "4",
            "--lift-cpu-prerequisites",
            "--generated-at",
            "2026-11-07T00:00:00Z",
            "--out-dir",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains("cpu_prerequisites=1"),
        "howto summary: {stdout}"
    );
    assert_eq!(
        fs::read_to_string(out.join("cpu-prerequisites.list"))
            .unwrap()
            .trim(),
        "CpuLib-1.0-GCCcore-15.2.0.eb"
    );
    assert_eq!(
        fs::read_to_string(out.join("prelude.list")).unwrap().trim(),
        "GpuLib-1.0-GCCcore-15.2.0-CUDA-13.3.0.eb"
    );
}
