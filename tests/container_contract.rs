use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn entrypoint() -> PathBuf {
    root().join("skills/new-package/container/rocky9/eb-entrypoint")
}

fn fake_easybuild(directory: &Path) -> PathBuf {
    let bin = directory.join("bin");
    fs::create_dir_all(&bin).expect("create fake bin");
    let eb = bin.join("eb");
    fs::write(&eb, "#!/usr/bin/env sh\nprintf 'eb:%s\\n' \"$*\"\n").expect("write fake eb");
    let mut permissions = fs::metadata(&eb).expect("fake eb metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&eb, permissions).expect("make fake eb executable");
    bin
}

#[test]
fn rocky_image_entrypoint_accepts_helper_and_routed_commands() {
    let entrypoint = entrypoint();
    assert!(entrypoint.is_file(), "missing {}", entrypoint.display());
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_bin = fake_easybuild(temp.path());
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let helper = Command::new(&entrypoint)
        .arg("--version")
        .env("PATH", &path)
        .output()
        .expect("run helper-style command");
    assert!(helper.status.success());
    assert_eq!(String::from_utf8_lossy(&helper.stdout), "eb:--version\n");

    let doctor = Command::new(&entrypoint)
        .args(["eb", "--version"])
        .env("PATH", &path)
        .output()
        .expect("run doctor-style command");
    assert!(doctor.status.success());
    assert_eq!(String::from_utf8_lossy(&doctor.stdout), "eb:--version\n");

    let routed = Command::new(&entrypoint)
        .args([
            "env",
            "ROUTED_VALUE=present",
            "sh",
            "-c",
            "printf '%s' \"$ROUTED_VALUE\"",
        ])
        .env("PATH", path)
        .output()
        .expect("run target-routed command");
    assert!(routed.status.success());
    assert_eq!(String::from_utf8_lossy(&routed.stdout), "present");
}

#[test]
fn rocky_containerfile_installs_the_compatible_entrypoint() {
    let containerfile =
        fs::read_to_string(root().join("skills/new-package/container/rocky9/Containerfile"))
            .expect("read Containerfile");
    assert!(containerfile.contains("COPY eb-entrypoint /usr/local/bin/eb-stack-entrypoint"));
    assert!(containerfile.contains("ENTRYPOINT [\"/usr/local/bin/eb-stack-entrypoint\"]"));
}
