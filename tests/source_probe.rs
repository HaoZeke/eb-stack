//! Per-architecture source probe integration tests (`stack arch --probe-sources`).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/source_probe_fixtures")
        .join(name)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .output()
        .expect("run eb-stack")
}

struct FixtureServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl FixtureServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        listener
            .set_nonblocking(true)
            .expect("set fixture server nonblocking");
        let addr = listener.local_addr().expect("fixture server address");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_worker = Arc::clone(&stop);
        let worker = thread::spawn(move || loop {
            if stop_worker.load(Ordering::Relaxed) {
                break;
            }
            match listener.accept() {
                Ok((mut stream, _)) => handle_connection(&mut stream),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        });
        Self {
            addr,
            stop,
            worker: Some(worker),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.addr);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn handle_connection(stream: &mut TcpStream) {
    let mut buffer = [0u8; 4096];
    let Ok(read) = stream.read(&mut buffer) else {
        return;
    };
    if read == 0 {
        return;
    }
    let request = String::from_utf8_lossy(&buffer[..read]);
    let mut lines = request.lines();
    let Some(first) = lines.next() else {
        return;
    };
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    let status = match (method, path) {
        ("HEAD", "/kaleido-0.2.1-manylinux1-x86_64.whl") => "200 OK",
        ("GET", "/kaleido-0.2.1-manylinux1-x86_64.whl") => "206 Partial Content",
        _ => "404 Not Found",
    };
    let response = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let _ = stream.write_all(response.as_bytes());
}

fn write_real_pr_1670_fixture(base_url: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("tempdir");
    let tree = temp.path().join("easyconfigs/k/Kaleido");
    std::fs::create_dir_all(&tree).expect("fixture tree");
    let recipe = std::fs::read_to_string(fixture(
        "easyconfigs/k/Kaleido/Kaleido-0.2.1-GCCcore-15.2.0.eb",
    ))
    .expect("read fixture");
    std::fs::write(
        tree.join("Kaleido-0.2.1-GCCcore-15.2.0.eb"),
        recipe.replace("__BASE_URL__", base_url),
    )
    .expect("write recipe");
    std::fs::write(
        temp.path().join("kaleido.yml"),
        std::fs::read_to_string(fixture("easystacks/kaleido.yml")).expect("read easystack"),
    )
    .expect("write easystack");
    temp
}

#[test]
fn stack_arch_source_probe_is_opt_in_and_flags_arch_missing_sources() {
    let server = FixtureServer::start();
    let fixture = write_real_pr_1670_fixture(&server.base_url());
    let easystack = fixture.path().join("kaleido.yml");
    let easyconfigs = fixture.path().join("easyconfigs");

    let without_probe = run(&[
        "stack",
        "arch",
        "--arch",
        "x86_64",
        "--easystack",
        easystack.to_str().unwrap(),
        "--easyconfigs",
        easyconfigs.to_str().unwrap(),
    ]);
    assert!(
        without_probe.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        text(&without_probe.stdout),
        text(&without_probe.stderr)
    );

    let with_probe = run(&[
        "stack",
        "arch",
        "--arch",
        "x86_64",
        "--easystack",
        easystack.to_str().unwrap(),
        "--easyconfigs",
        easyconfigs.to_str().unwrap(),
        "--probe-sources",
    ]);
    let stdout = text(&with_probe.stdout);
    let stderr = text(&with_probe.stderr);
    assert_eq!(with_probe.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("Kaleido-0.2.1-GCCcore-15.2.0.eb source kaleido-0.2.1-manylinux2014-aarch64.whl is arch-missing on aarch64"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Kaleido-0.2.1-GCCcore-15.2.0.eb source kaleido-0.2.1-manylinux2014-riscv64.whl is arch-missing on riscv64"),
        "{stdout}"
    );
    assert!(
        stdout.contains("source_unreachable=0 source_arch_missing=2"),
        "{stdout}"
    );
    assert!(stderr.contains("is arch-missing on aarch64"), "{stderr}");
}

#[test]
fn stack_arch_reports_a_parse_failure_and_probes_the_other_architectures() {
    let server = FixtureServer::start();
    let fixture = write_real_pr_1670_fixture(&server.base_url());
    let recipe = fixture
        .path()
        .join("easyconfigs/k/Kaleido/Kaleido-0.2.1-GCCcore-15.2.0.eb");
    let source = std::fs::read_to_string(&recipe).expect("read recipe");
    std::fs::write(&recipe, source.replace(", 'riscv64': '2014'}", "}")).expect("drop riscv64");
    let easystack = fixture.path().join("kaleido.yml");
    let easyconfigs = fixture.path().join("easyconfigs");

    let out = run(&[
        "stack",
        "arch",
        "--easystack",
        easystack.to_str().unwrap(),
        "--easyconfigs",
        easyconfigs.to_str().unwrap(),
        "--probe-sources",
    ]);
    let stdout = text(&out.stdout);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("Kaleido-0.2.1-GCCcore-15.2.0.eb failed to parse on riscv64"),
        "{stdout}"
    );
    assert!(stdout.contains("is arch-missing on aarch64"), "{stdout}");
    assert!(!stdout.contains("is arch-missing on riscv64"), "{stdout}");
    assert!(
        stdout.contains(
            "parse_failed=1 unsupported=0 flagged=0 source_unreachable=0 source_arch_missing=1\n"
        ),
        "{stdout}"
    );
}
