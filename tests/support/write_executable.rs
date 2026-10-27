/// Write `contents` to `path` and mark that file executable.
///
/// The bytes are written by a short-lived `sh`, not by this process. The
/// test binary is multi-threaded, so another thread can fork while a write
/// fd is open here and the child inherits it. `exec` of `path` then returns
/// ETXTBSY until that child execs and drops the fd. Closing the fd in this
/// process before the spawn does not cover that window. `sh` exits before
/// the caller runs `path`, and a sibling fork never sees the write fd.
pub(crate) fn write_executable(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("sh")
        .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "write-executable"])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("script writer has no stdin"))?
        .write_all(contents.as_bytes())?;
    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "write {}: {} {}",
            path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}
