//! Fetch writes a dump. SAT and tests only read dumps.
//!
//! A registry name hits Warehouse / CRAN / crates.io once and materializes
//! the JSON (and sdist, when present) under `ingest/<format>/`. Replaying
//! `--source` on that dump is offline.

use crate::foreign::ForeignFormat;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// HTTP GET used by name-mode ingest. Tests inject a map; the CLI uses ureq.
pub trait RegistryClient {
    /// Fetch `url` as bytes.
    fn get(&self, url: &str) -> Result<Vec<u8>, RegistryError>;
}

/// In-memory client. `cargo test` never leaves this map.
#[derive(Debug, Default)]
pub struct MapClient {
    /// Exact URL to body.
    pub pages: BTreeMap<String, Vec<u8>>,
}

impl RegistryClient for MapClient {
    fn get(&self, url: &str) -> Result<Vec<u8>, RegistryError> {
        self.pages
            .get(url)
            .cloned()
            .ok_or_else(|| RegistryError::Missing(url.to_string()))
    }
}

/// Live HTTPS client. Not used by the default test gate.
#[derive(Debug, Default)]
pub struct UreqClient;

impl RegistryClient for UreqClient {
    fn get(&self, url: &str) -> Result<Vec<u8>, RegistryError> {
        let response = ureq::get(url)
            .set("User-Agent", "eb-stack/0.3.0 (registry-ingest)")
            .call()
            .map_err(|error| RegistryError::Fetch(format!("{url}: {error}")))?;
        const MAX_BODY: u64 = 32 * 1024 * 1024;
        let mut body = Vec::new();
        response
            .into_reader()
            .take(MAX_BODY + 1)
            .read_to_end(&mut body)
            .map_err(|error| RegistryError::Fetch(format!("{url}: {error}")))?;
        if body.len() as u64 > MAX_BODY {
            return Err(RegistryError::Fetch(format!(
                "{url}: body exceeds {MAX_BODY} bytes"
            )));
        }
        Ok(body)
    }
}

/// Paths written by a name-mode fetch.
#[derive(Debug, Clone)]
pub struct MaterializedIngest {
    /// Frozen registry document SAT will parse.
    pub dump: PathBuf,
    /// Extracted sdist/crate tree, when a source archive was fetched.
    pub source_tree: Option<PathBuf>,
}

/// Why a name-mode fetch failed.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// The test map (or the live host) has no body for this URL.
    #[error("registry missing {0}")]
    Missing(String),
    /// The live GET failed.
    #[error("registry fetch: {0}")]
    Fetch(String),
    /// The dump could not be parsed enough to name the file.
    #[error("registry parse: {0}")]
    Parse(String),
    /// Writing the dump or sdist failed.
    #[error("registry io {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
}

/// True when `--source` is a registry name, not a dump file.
pub fn is_registry_name(source: &Path) -> bool {
    // `tqdm==4.67.1` is a name with a version, not a path.
    !source.exists()
        && source
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        && source.components().count() == 1
}

/// A registry name, and the version it pins when it pins one.
///
/// `tqdm==4.67.1` asks for the release a site actually carries rather than
/// whatever is newest today, which is what regenerating an existing recipe
/// needs, and what makes the same command produce the same recipe tomorrow.
pub fn split_name_and_version(source: &str) -> (&str, Option<&str>) {
    for separator in ["==", "@"] {
        if let Some((name, version)) = source.split_once(separator) {
            let version = version.trim();
            if !name.is_empty() && !version.is_empty() {
                return (name, Some(version));
            }
        }
    }
    (source, None)
}

/// Warehouse JSON URL for `name`, at `version` when one is pinned.
pub fn pypi_json_url(base: &str, name: &str) -> String {
    format!("{}/pypi/{name}/json", base.trim_end_matches('/'))
}

/// Warehouse JSON URL for one release.
pub fn pypi_release_json_url(base: &str, name: &str, version: &str) -> String {
    format!("{}/pypi/{name}/{version}/json", base.trim_end_matches('/'))
}

/// Write Warehouse JSON (and sdist, when listed) under `ingest_root/pypi/`.
pub fn materialize_pypi(
    name: &str,
    client: &dyn RegistryClient,
    warehouse_base: &str,
    ingest_root: &Path,
) -> Result<MaterializedIngest, RegistryError> {
    let (name, pinned) = split_name_and_version(name);
    let url = match pinned {
        Some(version) => pypi_release_json_url(warehouse_base, name, version),
        None => pypi_json_url(warehouse_base, name),
    };
    let bytes = client.get(&url)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| RegistryError::Parse(format!("warehouse json: {error}")))?;
    let pkg = value
        .pointer("/info/name")
        .and_then(|value| value.as_str())
        .unwrap_or(name);
    let version = value
        .pointer("/info/version")
        .and_then(|value| value.as_str())
        .ok_or_else(|| RegistryError::Parse("warehouse json missing info.version".into()))?;
    let dir = ingest_root.join("pypi");
    std::fs::create_dir_all(&dir).map_err(|error| RegistryError::Io(dir.clone(), error))?;
    let dump = dir.join(format!("{}.json", sanitize_ingest_name(pkg, version)));
    std::fs::write(&dump, &bytes).map_err(|error| RegistryError::Io(dump.clone(), error))?;
    let source_tree = materialize_pypi_sdist(&value, client, &dir, pkg, version)?;
    Ok(MaterializedIngest { dump, source_tree })
}

fn materialize_pypi_sdist(
    value: &serde_json::Value,
    client: &dyn RegistryClient,
    dir: &Path,
    pkg: &str,
    version: &str,
) -> Result<Option<PathBuf>, RegistryError> {
    let Some(urls) = value.get("urls").and_then(|value| value.as_array()) else {
        return Ok(None);
    };
    let sdist = urls.iter().find(|url| {
        url.get("packagetype")
            .and_then(|value| value.as_str())
            .is_some_and(|kind| kind.eq_ignore_ascii_case("sdist"))
    });
    let Some(sdist) = sdist else {
        return Ok(None);
    };
    let Some(url) = sdist.get("url").and_then(|value| value.as_str()) else {
        return Ok(None);
    };
    if url.starts_with("https://example.invalid/") {
        return Ok(None);
    }
    let bytes = match client.get(url) {
        Ok(bytes) => bytes,
        Err(RegistryError::Missing(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    if let Some(expected) = sdist
        .pointer("/digests/sha256")
        .and_then(|value| value.as_str())
    {
        let got = sha256_hex(&bytes);
        if !got.eq_ignore_ascii_case(expected) {
            return Err(RegistryError::Parse(format!(
                "sdist sha256 {got} != warehouse {expected}"
            )));
        }
    }
    let filename = sdist
        .get("filename")
        .and_then(|value| value.as_str())
        .unwrap_or("sdist.tar.gz");
    let archive = dir.join(filename);
    std::fs::write(&archive, &bytes).map_err(|error| RegistryError::Io(archive.clone(), error))?;
    let tree = dir.join(sanitize_ingest_name(pkg, version));
    unpack_sdist(&bytes, &tree)?;
    Ok(Some(tree))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sanitize_ingest_name(pkg: &str, version: &str) -> String {
    format!("{pkg}-{version}")
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' => '_',
            other => other,
        })
        .collect()
}

fn unpack_sdist(bytes: &[u8], dest: &Path) -> Result<(), RegistryError> {
    use flate2::read::GzDecoder;
    use tar::Archive;
    std::fs::create_dir_all(dest).map_err(|error| RegistryError::Io(dest.to_path_buf(), error))?;
    let decoder = GzDecoder::new(std::io::Cursor::new(bytes));
    let mut archive = Archive::new(decoder);
    archive
        .unpack(dest)
        .map_err(|error| RegistryError::Io(dest.to_path_buf(), error))?;
    Ok(())
}

/// Write a CRAN JSON dump under `ingest_root/cran/`.
pub fn materialize_cran(
    name: &str,
    client: &dyn RegistryClient,
    crandb_base: &str,
    ingest_root: &Path,
) -> Result<MaterializedIngest, RegistryError> {
    let (name, pinned) = split_name_and_version(name);
    let url = match pinned {
        Some(version) => format!("{}/{name}/{version}", crandb_base.trim_end_matches('/')),
        None => format!("{}/{name}", crandb_base.trim_end_matches('/')),
    };
    let bytes = client.get(&url)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| RegistryError::Parse(format!("cran json: {error}")))?;
    let pkg = value
        .get("Package")
        .or_else(|| value.get("package"))
        .and_then(|value| value.as_str())
        .unwrap_or(name);
    let version = value
        .get("Version")
        .or_else(|| value.get("version"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| RegistryError::Parse("cran json missing Version".into()))?;
    let dir = ingest_root.join("cran");
    std::fs::create_dir_all(&dir).map_err(|error| RegistryError::Io(dir.clone(), error))?;
    let dump = dir.join(format!("{}.json", sanitize_ingest_name(pkg, version)));
    std::fs::write(&dump, &bytes).map_err(|error| RegistryError::Io(dump.clone(), error))?;
    Ok(MaterializedIngest {
        dump,
        source_tree: None,
    })
}

/// Write a crates.io JSON dump under `ingest_root/cargo/`.
pub fn materialize_cargo(
    name: &str,
    client: &dyn RegistryClient,
    crates_base: &str,
    ingest_root: &Path,
) -> Result<MaterializedIngest, RegistryError> {
    let (name, pinned) = split_name_and_version(name);
    let url = match pinned {
        Some(version) => format!(
            "{}/api/v1/crates/{name}/{version}",
            crates_base.trim_end_matches('/')
        ),
        None => format!("{}/api/v1/crates/{name}", crates_base.trim_end_matches('/')),
    };
    let bytes = client.get(&url)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| RegistryError::Parse(format!("crates.io json: {error}")))?;
    let pkg = value
        .pointer("/crate/name")
        .and_then(|value| value.as_str())
        .unwrap_or(name);
    let version = pinned
        .or_else(|| {
            value
                .pointer("/crate/max_stable_version")
                .filter(|value| !value.is_null())
                .and_then(|value| value.as_str())
        })
        .or_else(|| {
            value
                .pointer("/crate/max_version")
                .filter(|value| !value.is_null())
                .and_then(|value| value.as_str())
        })
        .ok_or_else(|| RegistryError::Parse("crates.io json missing version".into()))?;
    let dir = ingest_root.join("cargo");
    std::fs::create_dir_all(&dir).map_err(|error| RegistryError::Io(dir.clone(), error))?;
    let dump = dir.join(format!("{}.json", sanitize_ingest_name(pkg, version)));
    std::fs::write(&dump, &bytes).map_err(|error| RegistryError::Io(dump.clone(), error))?;
    Ok(MaterializedIngest {
        dump,
        source_tree: None,
    })
}

/// Resolve a source path or a registry name into a dump on disk.
///
/// A file is used as-is. A bare PyPI/CRAN/cargo name is fetched under
/// `out_dir/ingest`, the same way the CLI inspect/plan path does.
pub fn resolve_ingest_source(
    source: &Path,
    format: Option<ForeignFormat>,
    out_dir: &Path,
) -> Result<PathBuf, String> {
    if source.is_file() {
        return Ok(source.to_path_buf());
    }
    if !is_registry_name(source) {
        return Err(format!(
            "source {} is not a file or a registry name",
            source.display()
        ));
    }
    let format = format.ok_or_else(|| {
        "format pypi|cran|cargo is required when source is a registry name".to_string()
    })?;
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid registry name {}", source.display()))?;
    let ingest = materialize_registry_name(name, format, &UreqClient, &out_dir.join("ingest"))
        .map_err(|error| format!("fetch {name} as {}: {error}", format.as_str()))?;
    Ok(ingest.dump)
}

/// Fetch a registry name into `ingest_root` and return the dump path.
pub fn materialize_registry_name(
    name: &str,
    format: ForeignFormat,
    client: &dyn RegistryClient,
    ingest_root: &Path,
) -> Result<MaterializedIngest, RegistryError> {
    match format {
        ForeignFormat::Pypi => materialize_pypi(name, client, "https://pypi.org", ingest_root),
        ForeignFormat::Cran => {
            materialize_cran(name, client, "https://crandb.r-pkg.org", ingest_root)
        }
        ForeignFormat::Cargo => materialize_cargo(name, client, "https://crates.io", ingest_root),
        other => Err(RegistryError::Parse(format!(
            "name-mode ingest is pypi/cran/cargo, not {}",
            other.as_str()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materialize_pypi_writes_dump_from_map_client() {
        let mut client = MapClient::default();
        client.pages.insert(
            "https://pypi.org/pypi/demo/json".into(),
            br#"{
              "info": {"name": "demo", "version": "1.0.0", "requires_dist": []},
              "urls": []
            }"#
            .to_vec(),
        );
        let root = tempfile::tempdir().expect("temp");
        let ingest = materialize_pypi("demo", &client, "https://pypi.org", root.path())
            .expect("materialize");
        assert!(ingest.dump.ends_with("pypi/demo-1.0.0.json"));
        assert!(ingest.dump.is_file());
        let replay = std::fs::read_to_string(&ingest.dump).expect("read");
        assert!(replay.contains("\"name\": \"demo\""));
    }

    #[test]
    fn registry_name_is_a_single_missing_component() {
        assert!(is_registry_name(Path::new("eon-akmc")));
        assert!(!is_registry_name(Path::new("fixtures/pypi.json")));
    }

    #[test]
    fn resolve_ingest_source_keeps_an_existing_file() {
        let temp = tempfile::tempdir().expect("temp");
        let dump = temp.path().join("dump.json");
        std::fs::write(&dump, "{}").expect("write");
        let resolved = resolve_ingest_source(&dump, None, temp.path()).expect("resolve");
        assert_eq!(resolved, dump);
    }

    #[test]
    fn resolve_ingest_source_requires_format_for_a_registry_name() {
        let temp = tempfile::tempdir().expect("temp");
        let err = resolve_ingest_source(Path::new("numpy"), None, temp.path()).unwrap_err();
        assert!(err.contains("format"), "{err}");
    }
}
