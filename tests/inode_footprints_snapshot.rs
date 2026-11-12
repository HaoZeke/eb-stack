//! Footprint provenance stays tied to the estimates actually consumed.

#[cfg(unix)]
#[test]
fn footprint_hash_and_estimates_use_one_input_snapshot() {
    use sha2::{Digest, Sha256};
    use std::ffi::CString;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let temporary = tempfile::tempdir().unwrap();
    let footprints = temporary.path().join("footprints.json");
    let replacement = temporary.path().join("replacement.json");
    let output = temporary.path().join("plan");
    let first = br#"{"default":11111,"footprints":{"PETSc":22222}}"#;
    let second = br#"{"default":33333,"footprints":{"PETSc":44444}}"#;
    std::fs::write(&replacement, second).unwrap();
    let fifo = CString::new(footprints.as_os_str().as_bytes()).unwrap();
    // SAFETY: the NUL-terminated path remains valid through mkfifo.
    let created = unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) };
    assert_eq!(created, 0, "{}", std::io::Error::last_os_error());

    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/stack_shard_fixtures");
    let mut child = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["stack", "shard", "--shards", "4", "--build-list"])
        .arg(fixtures.join("wave1.list"))
        .arg("--easyconfigs")
        .arg(fixtures.join("easyconfigs"))
        .arg("--installed")
        .arg(fixtures.join("installed.txt"))
        .arg("--installed-modules-dir")
        .arg(fixtures.join("modules/all"))
        .arg("--generated-at")
        .arg("2026-11-03T00:00:00Z")
        .arg("--inode-footprints")
        .arg(&footprints)
        .arg("--out-dir")
        .arg(&output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut writer = loop {
        match OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&footprints)
        {
            Ok(writer) => break writer,
            Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
                    let _ = child.kill();
                    let result = child.wait_with_output().unwrap();
                    panic!(
                        "footprint reader did not open the FIFO: {}",
                        String::from_utf8_lossy(&result.stderr)
                    );
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("open footprint FIFO: {error}"),
        }
    };
    writer.write_all(first).unwrap();
    // Replacing the pathname before EOF makes a second open consume other bytes.
    std::fs::rename(&replacement, &footprints).unwrap();
    drop(writer);
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let plan: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("plan.json")).unwrap()).unwrap();
    let expected: String = Sha256::digest(first)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(plan["inode_budget"]["footprints_sha256"], expected);
    assert_eq!(plan["inode_budget"]["default_footprint"], 11111);
    assert!(plan["shards"]
        .as_array()
        .unwrap()
        .iter()
        .any(|shard| shard["inode_footprint"] == 24222));
    assert_eq!(std::fs::read(footprints).unwrap(), second);
}
