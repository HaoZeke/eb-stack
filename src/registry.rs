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
        let mut body = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut body)
            .map_err(|error| RegistryError::Fetch(format!("{url}: {error}")))?;
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
    !source.exists()
        && source
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        && source.components().count() == 1
}

/// Warehouse JSON URL for `name`.
pub fn pypi_json_url(base: &str, name: &str) -> String {
    format!("{}/pypi/{name}/json", base.trim_end_matches('/'))
}

/// Write Warehouse JSON (and sdist, when listed) under `ingest_root/pypi/`.
pub fn materialize_pypi(
    name: &str,
    client: &dyn RegistryClient,
    warehouse_base: &str,
    ingest_root: &Path,
) -> Result<MaterializedIngest, RegistryError> {
    let url = pypi_json_url(warehouse_base, name);
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
    let dump = dir.join(format!("{pkg}-{version}.json"));
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
    let Some(sdist) = sdist.or_else(|| urls.first()) else {
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
    let filename = sdist
        .get("filename")
        .and_then(|value| value.as_str())
        .unwrap_or("sdist.tar.gz");
    let archive = dir.join(filename);
    std::fs::write(&archive, &bytes).map_err(|error| RegistryError::Io(archive.clone(), error))?;
    let tree = dir.join(format!("{pkg}-{version}"));
    if unpack_sdist(&bytes, &tree).is_ok() {
        return Ok(Some(tree));
    }
    Ok(None)
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
    let url = format!("{}/{name}", crandb_base.trim_end_matches('/'));
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
    let dump = dir.join(format!("{pkg}-{version}.json"));
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
    let url = format!("{}/api/v1/crates/{name}", crates_base.trim_end_matches('/'));
    let bytes = client.get(&url)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| RegistryError::Parse(format!("crates.io json: {error}")))?;
    let pkg = value
        .pointer("/crate/name")
        .and_then(|value| value.as_str())
        .unwrap_or(name);
    let version = value
        .pointer("/crate/max_stable_version")
        .or_else(|| value.pointer("/crate/max_version"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| RegistryError::Parse("crates.io json missing version".into()))?;
    let dir = ingest_root.join("cargo");
    std::fs::create_dir_all(&dir).map_err(|error| RegistryError::Io(dir.clone(), error))?;
    let dump = dir.join(format!("{pkg}-{version}.json"));
    std::fs::write(&dump, &bytes).map_err(|error| RegistryError::Io(dump.clone(), error))?;
    Ok(MaterializedIngest {
        dump,
        source_tree: None,
    })
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
}
