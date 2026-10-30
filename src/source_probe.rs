//! Per-architecture source reachability probing.
//!
//! A software-layer plan can parse and close correctly while still failing on
//! one architecture because the archive or wheel URL exists only for another.
//! This module resolves `source_urls` / `sources` per target architecture and
//! probes each concrete URL with HEAD and a ranged GET fallback.

use crate::eb_parse::resolve_easyconfig_file_for_arch;
use crate::source_fetch::source_download_urls;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

/// Architectures probed by default for EESSI software-layer plans.
pub const DEFAULT_TARGET_ARCHES: [&str; 3] = ["x86_64", "aarch64", "riscv64"];

/// Probe timing and retry policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOptions {
    /// Extra attempts after the first one.
    pub retries: u32,
    /// Timeout applied to one HTTP request.
    pub timeout: Duration,
}

impl Default for ProbeOptions {
    fn default() -> Self {
        Self {
            retries: 2,
            timeout: Duration::from_secs(8),
        }
    }
}

/// One source-probe finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceProbeFinding {
    /// Easyconfig file name.
    pub easyconfig: String,
    /// Source file name from `sources`.
    pub source: String,
    /// Architecture the probe ran with.
    pub arch: String,
    /// `unreachable` or `arch-missing`.
    pub kind: String,
    /// Probe outcome detail.
    pub detail: String,
}

impl SourceProbeFinding {
    /// One line for command output and fatal errors.
    pub fn message(&self) -> String {
        format!(
            "{} source {} is {} on {}: {}",
            self.easyconfig, self.source, self.kind, self.arch, self.detail
        )
    }
}

#[derive(Debug, Clone)]
struct ProbeRow {
    easyconfig: String,
    source_slot: usize,
    source: String,
    arch: String,
    reachable: bool,
    detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProbeHttp {
    Ok(u16),
    Status(u16),
    Transport(String),
}

trait SourceProbeHttp {
    fn head(&self, url: &str, timeout: Duration) -> ProbeHttp;
    fn range0(&self, url: &str, timeout: Duration) -> ProbeHttp;
}

struct UreqSourceProbeHttp;

impl SourceProbeHttp for UreqSourceProbeHttp {
    fn head(&self, url: &str, timeout: Duration) -> ProbeHttp {
        ureq_outcome(
            ureq::AgentBuilder::new()
                .timeout_connect(timeout)
                .timeout_read(timeout)
                .timeout_write(timeout)
                .build()
                .request("HEAD", url)
                .call(),
        )
    }

    fn range0(&self, url: &str, timeout: Duration) -> ProbeHttp {
        ureq_outcome(
            ureq::AgentBuilder::new()
                .timeout_connect(timeout)
                .timeout_read(timeout)
                .timeout_write(timeout)
                .build()
                .get(url)
                .set("Range", "bytes=0-0")
                .call(),
        )
    }
}

fn ureq_outcome(result: Result<ureq::Response, ureq::Error>) -> ProbeHttp {
    match result {
        Ok(response) => ProbeHttp::Ok(response.status()),
        Err(ureq::Error::Status(status, _)) => ProbeHttp::Status(status),
        Err(ureq::Error::Transport(error)) => ProbeHttp::Transport(error.to_string()),
    }
}

fn retryable(outcome: &ProbeHttp) -> bool {
    matches!(
        outcome,
        ProbeHttp::Transport(_) | ProbeHttp::Status(408 | 425 | 429)
    ) || matches!(outcome, ProbeHttp::Status(code) if (500..=599).contains(code))
}

fn basename(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| path.display().to_string(), str::to_string)
}

fn probe_urls(
    urls: &[String],
    options: &ProbeOptions,
    http: &dyn SourceProbeHttp,
) -> (bool, String) {
    if urls.is_empty() {
        return (false, "no source_urls entry to download from".into());
    }
    let mut traces = Vec::new();
    for url in urls {
        let mut last = String::new();
        for attempt in 0..=options.retries {
            let head = http.head(url, options.timeout);
            if matches!(head, ProbeHttp::Ok(200..=399)) {
                return (true, format!("{url} via HEAD"));
            }
            let range = http.range0(url, options.timeout);
            if matches!(range, ProbeHttp::Ok(200..=399) | ProbeHttp::Status(206)) {
                return (true, format!("{url} via GET Range bytes=0-0"));
            }
            last = format!(
                "attempt {}/{} HEAD={} RANGE={}",
                attempt + 1,
                options.retries + 1,
                render_http(&head),
                render_http(&range)
            );
            if attempt < options.retries && (retryable(&head) || retryable(&range)) {
                continue;
            }
            break;
        }
        traces.push(format!("{url} ({last})"));
    }
    (false, traces.join("; "))
}

fn render_http(outcome: &ProbeHttp) -> String {
    match outcome {
        ProbeHttp::Ok(status) => format!("ok:{status}"),
        ProbeHttp::Status(status) => format!("status:{status}"),
        ProbeHttp::Transport(error) => format!("transport:{error}"),
    }
}

fn probe_rows_for_arch(
    easyconfig_path: &Path,
    arch: &str,
    options: &ProbeOptions,
    http: &dyn SourceProbeHttp,
) -> Result<Vec<ProbeRow>, String> {
    let recipe = resolve_easyconfig_file_for_arch(easyconfig_path, arch)
        .map_err(|error| format!("{}: {error}", easyconfig_path.display()))?;
    let mut rows = Vec::new();
    for (source_slot, source) in recipe.source_files.iter().enumerate() {
        if source.git {
            continue;
        }
        if source.filename.contains("%(") || source.download_filename.contains("%(") {
            rows.push(ProbeRow {
                easyconfig: basename(easyconfig_path),
                source_slot,
                source: source.filename.clone(),
                arch: arch.to_string(),
                reachable: false,
                detail: "source filename keeps an unresolved template".into(),
            });
            continue;
        }
        let urls = source_download_urls(&recipe, source);
        let (reachable, detail) = probe_urls(&urls, options, http);
        rows.push(ProbeRow {
            easyconfig: basename(easyconfig_path),
            source_slot,
            source: source.filename.clone(),
            arch: arch.to_string(),
            reachable,
            detail,
        });
    }
    Ok(rows)
}

fn classify_rows(rows: Vec<ProbeRow>) -> Vec<SourceProbeFinding> {
    let mut by_source: BTreeMap<(String, usize), Vec<ProbeRow>> = BTreeMap::new();
    for row in rows {
        by_source
            .entry((row.easyconfig.clone(), row.source_slot))
            .or_default()
            .push(row);
    }
    let mut findings = Vec::new();
    for ((easyconfig, _slot), group) in by_source {
        let has_reachable = group.iter().any(|row| row.reachable);
        for row in group {
            if row.reachable {
                continue;
            }
            findings.push(SourceProbeFinding {
                easyconfig: easyconfig.clone(),
                source: row.source.clone(),
                arch: row.arch,
                kind: if has_reachable {
                    "arch-missing".into()
                } else {
                    "unreachable".into()
                },
                detail: row.detail,
            });
        }
    }
    findings.sort_by(|a, b| {
        (&a.easyconfig, &a.source, &a.arch, &a.kind).cmp(&(
            &b.easyconfig,
            &b.source,
            &b.arch,
            &b.kind,
        ))
    });
    findings
}

/// Probe every source artifact of every easyconfig for every architecture.
pub fn probe_easyconfig_sources(
    easyconfig_paths: &[std::path::PathBuf],
    arches: &[String],
    options: &ProbeOptions,
) -> Result<Vec<SourceProbeFinding>, String> {
    probe_easyconfig_sources_with(easyconfig_paths, arches, options, &UreqSourceProbeHttp)
}

fn probe_easyconfig_sources_with(
    easyconfig_paths: &[std::path::PathBuf],
    arches: &[String],
    options: &ProbeOptions,
    http: &dyn SourceProbeHttp,
) -> Result<Vec<SourceProbeFinding>, String> {
    let mut rows = Vec::new();
    for path in easyconfig_paths {
        for arch in arches {
            rows.extend(probe_rows_for_arch(path, arch, options, http)?);
        }
    }
    Ok(classify_rows(rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::thread;

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
            ("HEAD", "/range-only.tar.gz") => "405 Method Not Allowed",
            ("GET", "/range-only.tar.gz") => "206 Partial Content",
            _ => "404 Not Found",
        };
        let body = if status.starts_with("206") { "x" } else { "" };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
    }

    #[test]
    fn marks_missing_arch_sources_from_the_kaleido_failure_shape() {
        let server = FixtureServer::start();
        let temp = tempfile::tempdir().expect("tempdir");
        let recipe = temp.path().join("Kaleido-0.2.1-GCCcore-15.2.0.eb");
        std::fs::write(
            &recipe,
            format!(
                "name = 'Kaleido'\n\
                 version = '0.2.1'\n\
                 toolchain = SYSTEM\n\
                 manylinux = {{'x86_64': '1', 'aarch64': '2014', 'riscv64': '2014'}}\n\
                 source_urls = ['{}/']\n\
                 sources = ['kaleido-%(version)s-manylinux%s-%(arch)s.whl' % manylinux[ARCH]]\n",
                server.base_url()
            ),
        )
        .expect("write recipe");
        let findings = probe_easyconfig_sources(
            &[recipe],
            &DEFAULT_TARGET_ARCHES
                .iter()
                .map(|arch| arch.to_string())
                .collect::<Vec<_>>(),
            &ProbeOptions::default(),
        )
        .expect("probe");
        let messages: Vec<String> = findings.iter().map(SourceProbeFinding::message).collect();
        assert!(
            messages
                .iter()
                .any(|line| line.contains("is arch-missing on aarch64")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|line| line.contains("is arch-missing on riscv64")),
            "{messages:?}"
        );
        assert!(
            messages.iter().all(|line| !line.contains("on x86_64")),
            "{messages:?}"
        );
    }

    #[test]
    fn falls_back_to_range_when_head_is_not_allowed() {
        let server = FixtureServer::start();
        let temp = tempfile::tempdir().expect("tempdir");
        let recipe = temp.path().join("RangeOnly-1.0-GCCcore-15.2.0.eb");
        std::fs::write(
            &recipe,
            format!(
                "name = 'RangeOnly'\n\
                 version = '1.0'\n\
                 toolchain = SYSTEM\n\
                 source_urls = ['{}/']\n\
                 sources = ['range-only.tar.gz']\n",
                server.base_url()
            ),
        )
        .expect("write recipe");
        let findings = probe_easyconfig_sources(
            &[recipe],
            &[String::from("x86_64")],
            &ProbeOptions::default(),
        )
        .expect("probe");
        assert!(findings.is_empty(), "{findings:?}");
    }
}
