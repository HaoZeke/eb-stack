include!("../tests/support/write_executable.rs");

#[cfg(unix)]
#[test]
fn a_script_written_outside_this_process_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("eb");
    write_executable(&path, "#!/bin/sh\nprintf 'ok\\n'\n").expect("write");
    let output = std::process::Command::new(&path).output().expect("run");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"ok\n");
}
