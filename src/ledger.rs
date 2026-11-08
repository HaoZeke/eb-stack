//! Per-stack software ledger: one append-only JSON Lines file of install,
//! rebuild, retire, move, access, removal, finding and correction events,
//! hashed as an RFC 9162 Merkle log.
//!
//! Each line is the canonical JSON of one record: keys sorted by their UTF-8
//! bytes, no whitespace outside strings, integers only. The leaf of record
//! `N` is [`rfc9162::leaf_hash`] over the bytes of line `N` without its
//! newline. A head is the size and root over the first `size` leaves, signed
//! with Ed25519 under [`DOMAIN`]. An inclusion proof shows one record is in a
//! signed head; a consistency proof shows a later head extends an earlier
//! one, which a rewritten log cannot.
//!
//! The signing key is a path the writer passes on each call. A key inside
//! the log's directory is refused, so the key never travels with the log.
//!
//! Every line is also an entry of the tile log beside the file
//! ([`crate::ledger_tiles`]): `append` writes both, `head` signs both, and
//! `verify` checks that the tile log extends the signed head and holds the
//! same lines.

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use ed25519_dalek::pkcs8::{DecodePrivateKey, DecodePublicKey};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The name a head signature covers, so a head of this log is not a head of
/// any other log that signs RFC 9162 heads.
pub const DOMAIN: &str = "eb-stack-ledger-v1";

/// The events a record can carry.
pub const ACTIONS: &[&str] = &[
    "install",
    "rebuild-inplace",
    "rebuild-bump",
    "retire",
    "unretire",
    "move",
    "access",
    "remove",
    "finding",
    "correction",
];

/// Stages of an operation that runs in steps.
pub const STAGES: &[&str] = &["started", "finished", "aborted"];

/// Fields every record carries, each a non-empty string.
const REQUIRED: &[&str] = &[
    "ts", "actor", "stack", "target", "module", "location", "action",
];

/// Fields that name another record by its `id`.
const LINKS: &[&str] = &[
    "supersedes",
    "superseded_by",
    "moved_from",
    "moved_to",
    "corrects",
];

/// Fields with one value from a fixed set: `stage` from [`STAGES`] and
/// `route` from [`crate::ledger_tiles::ROUTES`].
const ENUMERATED: &[&str] = &["stage", "route"];

/// Fields with structured content: free text or an object of references.
/// `build` as an object may carry `inodes_peak`, the most inodes the build
/// tree held on scratch, which `stack footprints` reads; it must be a whole
/// number when present.
const STRUCTURED: &[&str] = &[
    "reason",
    "easyconfig",
    "sources",
    "sbom",
    "modulefile",
    "build",
];

/// Fields the ledger writes: `id` and `index`. `sig` is an optional record
/// signature, kept in the line and left out of `id`.
const DERIVED: &[&str] = &["id", "index", "sig"];

/// The canonical JSON of `value`: object keys sorted by their UTF-8 bytes,
/// no whitespace outside strings, strings escaped as `serde_json` writes
/// them. A number that is not an integer is refused, because two writers
/// can print the same float two ways.
pub fn canonical(value: &Value) -> Result<String> {
    let mut out = String::new();
    write_canonical(value, &mut out)?;
    Ok(out)
}

fn write_canonical(value: &Value, out: &mut String) -> Result<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => {
            if number.is_f64() {
                bail!("ledger records hold integers only, not {number}");
            }
            out.push_str(&number.to_string());
        }
        Value::String(text) => out.push_str(&serde_json::to_string(text)?),
        Value::Array(items) => {
            out.push('[');
            for (at, item) in items.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                write_canonical(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            out.push('{');
            for (at, key) in keys.into_iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key)?);
                out.push(':');
                write_canonical(&map[key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut text = String::with_capacity(64);
    for byte in digest {
        write!(&mut text, "{byte:02x}").expect("writing to a String cannot fail");
    }
    text
}

/// The record `id`: SHA-256 of the canonical JSON of the record without
/// `id` and `sig`. `index` is inside it, so a record moved to another
/// position has another id.
pub fn record_id(record: &Map<String, Value>) -> Result<String> {
    let mut body = record.clone();
    body.remove("id");
    body.remove("sig");
    Ok(sha256_hex(canonical(&Value::Object(body))?.as_bytes()))
}

fn event_id(record: &Map<String, Value>) -> Result<Option<&str>> {
    match record.get("build").and_then(|build| build.get("event_id")) {
        Some(Value::String(id)) if !id.trim().is_empty() => Ok(Some(id)),
        Some(_) => bail!("ledger build.event_id must be a non-empty string"),
        None => Ok(None),
    }
}

/// Check a record's fields against the ledger format. `id` and `index` are
/// not checked here.
pub fn check_fields(record: &Map<String, Value>) -> Result<()> {
    for key in record.keys() {
        let known = REQUIRED.contains(&key.as_str())
            || LINKS.contains(&key.as_str())
            || STRUCTURED.contains(&key.as_str())
            || DERIVED.contains(&key.as_str())
            || ENUMERATED.contains(&key.as_str());
        if !known {
            bail!("unknown ledger field {key:?}");
        }
    }
    for field in REQUIRED {
        match record.get(*field) {
            Some(Value::String(text)) if !text.is_empty() => {}
            Some(_) => bail!("ledger field {field} must be a non-empty string"),
            None => bail!("ledger record has no {field}"),
        }
    }
    let action = record["action"].as_str().unwrap_or_default();
    if !ACTIONS.contains(&action) {
        bail!(
            "ledger action {action:?} is not one of {}",
            ACTIONS.join(", ")
        );
    }
    let ts = record["ts"].as_str().unwrap_or_default();
    let parsed = DateTime::parse_from_rfc3339(ts)
        .with_context(|| format!("ledger ts {ts:?} is not RFC 3339"))?;
    if parsed.offset().local_minus_utc() != 0 {
        bail!("ledger ts {ts:?} is not UTC");
    }
    for field in LINKS
        .iter()
        .copied()
        .chain(ENUMERATED.iter().copied())
        .chain(["sig"])
    {
        if let Some(value) = record.get(field) {
            if value.as_str().is_none_or(|text| text.is_empty()) {
                bail!("ledger field {field} must be a non-empty string");
            }
        }
    }
    if let Some(stage) = record.get("stage").and_then(Value::as_str) {
        if !STAGES.contains(&stage) {
            bail!("ledger stage {stage:?} is not one of {}", STAGES.join(", "));
        }
    }
    if let Some(route) = record.get("route").and_then(Value::as_str) {
        if !crate::ledger_tiles::ROUTES.contains(&route) {
            bail!(
                "ledger route {route:?} is not one of {}",
                crate::ledger_tiles::ROUTES.join(", ")
            );
        }
    }
    if action == "correction" && !record.contains_key("corrects") {
        bail!("a correction record names the record it corrects in corrects");
    }
    if let Some(peak) = record
        .get("build")
        .and_then(Value::as_object)
        .and_then(|build| build.get("inodes_peak"))
    {
        if peak.as_u64().is_none() {
            bail!("ledger field build.inodes_peak must be a whole number of inodes, not {peak}");
        }
    }
    event_id(record)?;
    Ok(())
}

/// One line of the log.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Leaf number, which is also the record's `index`.
    pub index: usize,
    /// The record's `id`.
    pub id: String,
    /// The canonical line, without its newline. Its bytes are the leaf.
    pub line: String,
    /// The parsed record.
    pub record: Map<String, Value>,
}

impl Entry {
    /// The RFC 9162 leaf hash of this record.
    #[must_use]
    pub fn leaf(&self) -> [u8; 32] {
        rfc9162::leaf_hash(self.line.as_bytes())
    }

    /// The record's `module`.
    #[must_use]
    pub fn module(&self) -> &str {
        self.record["module"].as_str().unwrap_or_default()
    }
}

/// Parse log text. Every line is checked: it is canonical, its `index` is its
/// line number, its `id` recomputes, its fields are the ledger's, and its
/// `stack` is the stack of the first line.
pub fn parse_log(text: &str) -> Result<Vec<Entry>> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let Some(body) = text.strip_suffix('\n') else {
        bail!("the last ledger line has no newline; the log ends in a partial write");
    };
    let mut entries = Vec::new();
    for (index, line) in body.split('\n').enumerate() {
        let entry = parse_line(line, index).with_context(|| format!("ledger line {index}"))?;
        if let Some(first) = entries.first() {
            let first: &Entry = first;
            if first.record["stack"] != entry.record["stack"] {
                bail!(
                    "ledger line {index} is stack {} in a log of stack {}",
                    entry.record["stack"],
                    first.record["stack"]
                );
            }
        }
        entries.push(entry);
    }
    Ok(entries)
}

pub(crate) fn parse_line(line: &str, index: usize) -> Result<Entry> {
    let value: Value = serde_json::from_str(line).context("not JSON")?;
    let Value::Object(record) = value else {
        bail!("not a JSON object");
    };
    check_fields(&record)?;
    if record.get("index").and_then(Value::as_u64) != Some(index as u64) {
        bail!("index is {:?}, not {index}", record.get("index"));
    }
    let id = record_id(&record)?;
    if record.get("id").and_then(Value::as_str) != Some(id.as_str()) {
        bail!("id is {:?}; the record hashes to {id}", record.get("id"));
    }
    if canonical(&Value::Object(record.clone()))? != line {
        bail!("not in canonical form");
    }
    Ok(Entry {
        index,
        id,
        line: line.to_string(),
        record,
    })
}

/// Read and check a log file.
pub fn read_log(path: &Path) -> Result<Vec<Entry>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("read ledger {}", path.display()))?;
    parse_log(&text).with_context(|| format!("ledger {}", path.display()))
}

/// The leaves of `entries`.
#[must_use]
pub fn leaves(entries: &[Entry]) -> Vec<[u8; 32]> {
    entries.iter().map(Entry::leaf).collect()
}

/// Turn a record into the log line for position `index`. A record that
/// already carries `index` or `id` must carry the values the ledger would
/// write; `ts` is filled with the current UTC time when absent.
pub fn seal_record(record: &Value, index: usize, now: &str) -> Result<Entry> {
    let Value::Object(record) = record else {
        bail!("a ledger record is a JSON object");
    };
    let mut record = record.clone();
    let given_id = record.remove("id");
    if let Some(given) = record.get("index") {
        if given.as_u64() != Some(index as u64) {
            bail!("record says index {given}, but it would be line {index}");
        }
    }
    record.insert("index".to_string(), Value::from(index as u64));
    record
        .entry("ts".to_string())
        .or_insert_with(|| Value::from(now));
    check_fields(&record)?;
    let id = record_id(&record)?;
    if let Some(given) = given_id {
        if given.as_str() != Some(id.as_str()) {
            bail!("record says id {given}, but it hashes to {id}");
        }
    }
    record.insert("id".to_string(), Value::from(id.clone()));
    let line = canonical(&Value::Object(record.clone()))?;
    Ok(Entry {
        index,
        id,
        line,
        record,
    })
}

/// A signed tree head, as written to a head file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedHead {
    /// Always [`DOMAIN`].
    pub domain: String,
    /// Number of records the root covers.
    pub size: usize,
    /// Merkle root over the first `size` leaves, lower-case hex.
    pub root: String,
    /// UTC time the head was signed, RFC 3339.
    pub ts: String,
    /// Ed25519 public key of the signer, lower-case hex.
    pub public_key: String,
    /// Ed25519 signature over [`SignedHead::material`], lower-case hex.
    pub signature: String,
}

impl SignedHead {
    /// The bytes the signature covers: [`rfc9162::Head::material`] under
    /// [`DOMAIN`], a tab, and `ts`.
    #[must_use]
    pub fn material(size: usize, root: &str, ts: &str) -> Vec<u8> {
        let head = rfc9162::Head {
            size,
            root: root.to_string(),
        };
        let mut bytes = head.material(DOMAIN);
        bytes.push(b'\t');
        bytes.extend_from_slice(ts.as_bytes());
        bytes
    }

    /// Sign the head over `entries`.
    #[must_use]
    pub fn sign(entries: &[Entry], key: &SigningKey, ts: &str) -> Self {
        let head = rfc9162::Head::over(&leaves(entries));
        let signature = key.sign(&Self::material(head.size, &head.root, ts));
        Self {
            domain: DOMAIN.to_string(),
            size: head.size,
            root: head.root,
            ts: ts.to_string(),
            public_key: rfc9162::hex(key.verifying_key().as_bytes()),
            signature: rfc9162::hex(&signature.to_bytes()),
        }
    }

    /// Check the domain, that the signer is `trusted`, and the signature.
    pub fn check(&self, trusted: &VerifyingKey) -> Result<()> {
        if self.domain != DOMAIN {
            bail!("head domain is {:?}, not {DOMAIN}", self.domain);
        }
        let signer = rfc9162::hex(trusted.as_bytes());
        if self.public_key != signer {
            bail!(
                "head is signed by {}, not by the trusted key {signer}",
                self.public_key
            );
        }
        let raw = decode_signature(&self.signature)
            .ok_or_else(|| anyhow!("head signature is not 128 hex characters"))?;
        trusted
            .verify_strict(
                &Self::material(self.size, &self.root, &self.ts),
                &Signature::from_bytes(&raw),
            )
            .map_err(|_| anyhow!("head signature does not verify"))
    }

    /// Read a head file.
    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read head {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parse head {}", path.display()))
    }

    /// Write a head file: the head as JSON, one line. The file is replaced
    /// atomically, so a reader sees the previous head or this one and a
    /// failed write leaves the previous head in place. A symbolic link at
    /// `path` is refused rather than replaced or followed.
    pub fn write(&self, path: &Path) -> Result<()> {
        let mut text = serde_json::to_string(self)?;
        text.push('\n');
        replace_file(path, text.as_bytes())
            .with_context(|| format!("write head {}", path.display()))
    }
}

/// Write `bytes` to a new file beside `path`, flush it, rename it over
/// `path`, and flush the directory so the rename survives a crash.
fn replace_file(path: &Path, bytes: &[u8]) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("the path names no file"))?;
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    if let Ok(existing) = std::fs::symlink_metadata(path) {
        if existing.file_type().is_symlink() {
            bail!("the path is a symbolic link; refusing to replace it");
        }
        if existing.is_dir() {
            bail!("the path is a directory");
        }
    }
    loop {
        let mut temporary = OsString::from(".");
        temporary.push(name);
        temporary.push(format!(
            ".{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let temporary = directory.join(temporary);
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create a temporary file"),
        };
        let written = file
            .write_all(bytes)
            .and_then(|()| file.sync_all())
            .context("write the temporary file")
            .and_then(|()| std::fs::rename(&temporary, path).context("rename into place"));
        drop(file);
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written?;
        return sync_directory(directory);
    }
}

/// Flush a directory so entries created, renamed or removed in it survive a
/// crash.
fn sync_directory(directory: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(directory)
            .and_then(|handle| handle.sync_all())
            .with_context(|| format!("flush directory {}", directory.display()))?;
    }
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

/// A 64 byte signature from 128 lower-case hex characters.
fn decode_signature(text: &str) -> Option<[u8; 64]> {
    if text.len() != 128 || !text.is_ascii() {
        return None;
    }
    let mut raw = [0u8; 64];
    raw[..32].copy_from_slice(&rfc9162::from_hex(&text[..64])?);
    raw[32..].copy_from_slice(&rfc9162::from_hex(&text[64..])?);
    Some(raw)
}

/// Read an Ed25519 private key: PKCS#8 PEM, as `openssl genpkey -algorithm
/// ed25519` writes it.
pub fn read_signing_key(path: &Path) -> Result<SigningKey> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read signing key {}", path.display()))?;
    SigningKey::from_pkcs8_pem(&text).map_err(|error| {
        anyhow!(
            "signing key {} is not Ed25519 PKCS#8 PEM: {error}",
            path.display()
        )
    })
}

/// Read an Ed25519 public key: SubjectPublicKeyInfo PEM, as `openssl pkey
/// -pubout` writes it, or the 64 hex characters a head file carries.
pub fn read_public_key(path: &Path) -> Result<VerifyingKey> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read public key {}", path.display()))?;
    let trimmed = text.trim();
    if let Some(raw) = rfc9162::from_hex(trimmed) {
        return VerifyingKey::from_bytes(&raw)
            .map_err(|error| anyhow!("public key {} is not Ed25519: {error}", path.display()));
    }
    VerifyingKey::from_public_key_pem(trimmed).map_err(|error| {
        anyhow!(
            "public key {} is neither Ed25519 PEM nor 64 hex characters: {error}",
            path.display()
        )
    })
}

/// Refuse a signing key that sits inside the log's directory.
fn refuse_key_beside_log(key: &Path, log: &Path) -> Result<()> {
    let key = key
        .canonicalize()
        .with_context(|| format!("signing key {}", key.display()))?;
    let log = std::path::absolute(log).with_context(|| format!("ledger {}", log.display()))?;
    let directory = log.parent().unwrap_or(Path::new("/"));
    let directory = directory
        .canonicalize()
        .with_context(|| format!("ledger directory {}", directory.display()))?;
    if key.starts_with(&directory) {
        bail!(
            "signing key {} is inside the ledger directory {}; keep the key off the log's tree",
            key.display(),
            directory.display()
        );
    }
    Ok(())
}

/// A signing key read for one log, with the file it was read from.
struct KeyFile {
    key: SigningKey,
    /// The resolved path of the key file.
    path: PathBuf,
    /// Device and inode of the key file.
    inode: Option<(u64, u64)>,
}

/// Read the signing key for `log` from the file whose resolved path was
/// checked to lie outside the log's directory. A key with a second hard
/// link is refused: that name could sit in the log's tree, where the
/// directory check cannot see it.
fn open_signing_key(path: &Path, log: &Path) -> Result<KeyFile> {
    let mut file =
        File::open(path).with_context(|| format!("read signing key {}", path.display()))?;
    let opened = file
        .metadata()
        .with_context(|| format!("signing key {}", path.display()))?;
    if !opened.is_file() {
        bail!("signing key {} is not a regular file", path.display());
    }
    refuse_key_beside_log(path, log)?;
    let resolved = path
        .canonicalize()
        .with_context(|| format!("signing key {}", path.display()))?;
    let inode = file_identity(&opened);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.nlink() != 1 {
            bail!(
                "signing key {} has {} hard links; a key with another name could travel with the log",
                path.display(),
                opened.nlink()
            );
        }
        let current = std::fs::metadata(&resolved)
            .with_context(|| format!("signing key {}", path.display()))?;
        if file_identity(&current) != inode {
            bail!("signing key {} changed while it was read", path.display());
        }
    }
    let mut text = String::new();
    file.read_to_string(&mut text)
        .with_context(|| format!("read signing key {}", path.display()))?;
    let key = SigningKey::from_pkcs8_pem(&text).map_err(|error| {
        anyhow!(
            "signing key {} is not Ed25519 PKCS#8 PEM: {error}",
            path.display()
        )
    })?;
    Ok(KeyFile {
        key,
        path: resolved,
        inode,
    })
}

/// Device and inode, where the platform has them.
fn file_identity(metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

/// The files that belong to one log, by resolved path.
#[derive(Debug, Clone)]
struct LedgerPaths {
    log: PathBuf,
    directory: PathBuf,
    record_lock: PathBuf,
    /// File name prefix of tails moved out of the log.
    quarantine: OsString,
}

impl LedgerPaths {
    fn of(log: &Path) -> Result<Self> {
        let log = log
            .canonicalize()
            .with_context(|| format!("resolve ledger {}", log.display()))?;
        let directory = log.parent().unwrap_or(Path::new("/")).to_path_buf();
        let name = log.file_name().unwrap_or_default();
        let mut record_lock = name.to_os_string();
        record_lock.push(".posix-lock");
        let mut quarantine = name.to_os_string();
        quarantine.push(".quarantine.");
        Ok(Self {
            record_lock: directory.join(record_lock),
            log,
            directory,
            quarantine,
        })
    }
}

/// Refuse a head path that would replace or write through the log, its
/// record lock, a quarantined tail, or the signing key, by name, symbolic
/// link or hard link. A path whose directory does not exist names none of
/// them, and writing the head reports it.
fn check_head_target(out: &Path, paths: &LedgerPaths, key: Option<&KeyFile>) -> Result<()> {
    let absolute = std::path::absolute(out).with_context(|| format!("head {}", out.display()))?;
    let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) else {
        bail!("head path {} names no file", out.display());
    };
    let Ok(parent) = parent.canonicalize() else {
        return Ok(());
    };
    let resolved = parent.join(name);
    let refuse = |what: &str| -> Result<()> {
        bail!(
            "head path {} is {what}; a head never replaces it",
            out.display()
        )
    };
    if resolved == paths.log {
        return refuse("the ledger");
    }
    if resolved == paths.record_lock {
        return refuse("the ledger's record lock");
    }
    if key.is_some_and(|key| key.path == resolved) {
        return refuse("the signing key");
    }
    if parent == paths.directory
        && name
            .as_encoded_bytes()
            .starts_with(paths.quarantine.as_encoded_bytes())
    {
        return refuse("a quarantined ledger tail");
    }
    let Ok(existing) = std::fs::symlink_metadata(&resolved) else {
        return Ok(());
    };
    if existing.file_type().is_symlink() {
        bail!(
            "head path {} is a symbolic link; refusing to replace it",
            out.display()
        );
    }
    let Some(inode) = file_identity(&existing) else {
        return Ok(());
    };
    let identity = |path: &Path| {
        std::fs::symlink_metadata(path)
            .ok()
            .and_then(|metadata| file_identity(&metadata))
    };
    if identity(&paths.log) == Some(inode) {
        return refuse("a link to the ledger");
    }
    if identity(&paths.record_lock) == Some(inode) {
        return refuse("a link to the ledger's record lock");
    }
    if key.is_some_and(|key| key.inode == Some(inode)) {
        return refuse("a link to the signing key");
    }
    Ok(())
}

fn now_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// What `ledger append` needs.
#[derive(Debug, Clone)]
pub struct AppendRequest {
    /// The JSON Lines log. Created when absent.
    pub log: PathBuf,
    /// The record, a JSON object file.
    pub record: PathBuf,
    /// A head the log must still be at: a writer passes the head it last
    /// saw, so two writers cannot both append on the same head unnoticed.
    pub expect_head: Option<PathBuf>,
    /// Ed25519 PKCS#8 PEM key to sign the new head with.
    pub key: Option<PathBuf>,
    /// Where to write the new signed head. Needs `key`.
    pub head_out: Option<PathBuf>,
    /// The tile log, else the log's path with `.tiles`.
    pub tiles: Option<PathBuf>,
    /// The checkpoint origin, else derived from the log. Used with `key`.
    pub origin: Option<String>,
}

/// What `ledger append` wrote.
#[derive(Debug, Clone)]
pub struct AppendReport {
    /// The appended record's leaf number.
    pub index: usize,
    /// The appended record's id.
    pub id: String,
    /// The canonical line written.
    pub line: String,
    /// The head after the append.
    pub head: rfc9162::Head,
    /// The signed head, when a key was given.
    pub signed: Option<SignedHead>,
    /// Whether this acknowledges an identical event that is in the log.
    pub replayed: bool,
    /// The byte count and quarantine file of an interrupted tail that was
    /// cut from the log before this append.
    pub quarantined: Option<(usize, PathBuf)>,
    /// The tile log after the append.
    pub tiles: crate::ledger_tiles::SyncReport,
    /// The tile log's checkpoint, when a key was given.
    pub checkpoint: Option<crate::ledger_tiles::CheckpointReport>,
}

impl AppendReport {
    /// One line for stdout.
    #[must_use]
    pub fn summary(&self) -> String {
        let status = if self.replayed {
            "already recorded"
        } else {
            "appended"
        };
        let mut text = format!(
            "{status} record {} id {}; head size {} root {}",
            self.index, self.id, self.head.size, self.head.root
        );
        if let Some((bytes, path)) = &self.quarantined {
            let _ = write!(
                text,
                "; moved {bytes} bytes of an interrupted append to {}",
                path.display()
            );
        }
        text
    }
}

/// POSIX record locks belong to a process, not to a descriptor: a second
/// lock request from the same process converts the first instead of
/// waiting, and closing any descriptor for the inode drops every lock the
/// process holds on it. One slot per ledger inode per process keeps a second
/// writer or signer in this process from reaching the record-lock inode
/// while another holds it.
#[cfg(unix)]
mod process_slot {
    use std::sync::{Condvar, Mutex, PoisonError};

    static HELD: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
    static RELEASED: Condvar = Condvar::new();

    pub(super) struct Slot((u64, u64));

    impl Slot {
        pub(super) fn claim(inode: (u64, u64)) -> Self {
            let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
            while held.contains(&inode) {
                held = RELEASED.wait(held).unwrap_or_else(PoisonError::into_inner);
            }
            held.push(inode);
            Self(inode)
        }
    }

    impl Drop for Slot {
        fn drop(&mut self) {
            let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
            held.retain(|inode| *inode != self.0);
            RELEASED.notify_all();
        }
    }
}

/// Whether a guard excludes everyone or only writers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LockMode {
    /// A writer: excludes writers and signers.
    Write,
    /// A signer: excludes writers. Needs read access only.
    Read,
}

/// Resolve an output namespace through its existing ancestor without creating it.
fn resolve_namespace_path(path: &Path) -> Result<PathBuf> {
    let mut ancestor =
        std::path::absolute(path).with_context(|| format!("resolve path {}", path.display()))?;
    let mut missing = Vec::new();
    let mut resolved = loop {
        match ancestor.canonicalize() {
            Ok(resolved) => break resolved,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = ancestor
                    .file_name()
                    .ok_or_else(|| anyhow!("cannot resolve path {}", path.display()))?;
                missing.push(name.to_os_string());
                ancestor.pop();
            }
            Err(error) => {
                return Err(error).with_context(|| format!("resolve path {}", path.display()))
            }
        }
    };
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

/// The tile tree and its persistent writer gates own a separate namespace.
/// Reject conflicting canonical inputs and head outputs before an append.
fn check_tile_namespace(
    log: &Path,
    tiles: Option<&Path>,
    key: Option<&KeyFile>,
    head: Option<&Path>,
) -> Result<()> {
    let dir = crate::ledger_tiles::tiles_dir(log, tiles);
    let root = resolve_namespace_path(&dir)?;
    let mut gate = root.as_os_str().to_owned();
    gate.push(".writer-lock");
    let gate = PathBuf::from(gate);
    let mut record_gate = gate.as_os_str().to_owned();
    record_gate.push(".posix-lock");
    let record_gate = PathBuf::from(record_gate);
    let check = |path: &Path, kind: &str| -> Result<()> {
        let resolved = resolve_namespace_path(path)?;
        if resolved.starts_with(&root) || resolved == gate || resolved == record_gate {
            bail!(
                "{kind} {} conflicts with tile namespace {}; choose a separate path",
                path.display(),
                dir.display()
            );
        }
        Ok(())
    };
    check(log, "ledger")?;
    let mut record_lock = log.as_os_str().to_owned();
    record_lock.push(".posix-lock");
    check(Path::new(&record_lock), "ledger record lock")?;
    if let Some(key) = key {
        check(&key.path, "signing key")?;
    }
    if let Some(head) = head {
        check(head, "head path")?;
    }
    Ok(())
}

/// A writer's local gate keeps contenders from opening the process-scoped
/// record-lock inode. Closing any descriptor for that inode releases the
/// process's lock.
pub(crate) struct LedgerLock {
    gate: Option<File>,
    record: Option<File>,
    #[cfg(unix)]
    slot: Option<process_slot::Slot>,
    owner: u32,
    paths: LedgerPaths,
}

/// Exclude tile writers through a stable sibling of the replaceable tile tree.
/// The first append creates this gate, so signing a read-only canonical log
/// needs no new files in its directory. Its record lock also excludes writers
/// on hosts that do not share a local flock implementation.
pub(crate) fn tile_writer_lock(dir: &Path) -> Result<LedgerLock> {
    if std::fs::symlink_metadata(dir).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        bail!("tile directory {} is a symbolic link", dir.display());
    }
    std::fs::create_dir_all(dir)
        .with_context(|| format!("create tile directory {}", dir.display()))?;
    let resolved = dir
        .canonicalize()
        .with_context(|| format!("resolve tile directory {}", dir.display()))?;
    let mut name = resolved.into_os_string();
    name.push(".writer-lock");
    let path = PathBuf::from(name);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(&path)
        .with_context(|| format!("open tile writer gate {}", path.display()))?;
    LedgerLock::acquire(&file, &path)
}

impl LedgerLock {
    fn acquire(gate: &File, log: &Path) -> Result<Self> {
        Self::acquire_as(gate, log, LockMode::Write)
    }

    /// Take the signer's lock through a descriptor opened for reading.
    fn acquire_shared(log_file: &File, log: &Path) -> Result<Self> {
        Self::acquire_as(log_file, log, LockMode::Read)
    }

    fn acquire_as(opened_log: &File, log: &Path, mode: LockMode) -> Result<Self> {
        if !opened_log
            .metadata()
            .context("inspect ledger file")?
            .is_file()
        {
            bail!("ledger must be a regular file: {}", log.display());
        }
        let gate = opened_log
            .try_clone()
            .context("duplicate ledger lock descriptor")?;
        match mode {
            LockMode::Write => FileExt::lock_exclusive(&gate),
            #[cfg(unix)]
            LockMode::Read => Ok(()),
            #[cfg(not(unix))]
            LockMode::Read => FileExt::lock_shared(&gate),
        }
        .with_context(|| format!("lock ledger {}", log.display()))?;
        let mut guard = Self {
            gate: Some(gate),
            record: None,
            #[cfg(unix)]
            slot: None,
            owner: std::process::id(),
            paths: LedgerPaths::of(log)?,
        };
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

            let opened = opened_log.metadata().context("inspect ledger lock inode")?;
            if opened.nlink() != 1 {
                bail!(
                    "ledger requires one link for writer exclusion: {}",
                    log.display()
                );
            }
            let resolved = guard
                .paths
                .log
                .metadata()
                .context("inspect resolved ledger")?;
            if (opened.dev(), opened.ino()) != (resolved.dev(), resolved.ino()) {
                bail!(
                    "ledger path changed while acquiring its lock: {}",
                    log.display()
                );
            }
            guard.slot = Some(process_slot::Slot::claim((opened.dev(), opened.ino())));
            let record = open_record_lock(&guard.paths.record_lock, mode)
                .with_context(|| format!("open record lock for ledger {}", log.display()))?;
            let info = record.metadata().context("inspect ledger record lock")?;
            if !info.is_file() || info.nlink() != 1 {
                bail!(
                    "ledger record lock requires a regular file with one link: {}",
                    log.display()
                );
            }
            // All-zero offsets cover the whole file; flock's scalar fields
            // have valid zero representations on the supported Unix targets.
            let mut lock: libc::flock = unsafe { std::mem::zeroed() };
            lock.l_type = match mode {
                LockMode::Write => libc::F_WRLCK,
                LockMode::Read => libc::F_RDLCK,
            } as _;
            lock.l_whence = libc::SEEK_SET as _;
            loop {
                // The descriptor stays open in this guard and lock points to
                // a fully initialized flock for the duration of fcntl.
                let result = unsafe { libc::fcntl(record.as_raw_fd(), libc::F_SETLKW, &lock) };
                if result != -1 {
                    break;
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error)
                        .with_context(|| format!("record-lock ledger {}", log.display()));
                }
            }
            guard.record = Some(record);

            fn open_record_lock(path: &Path, mode: LockMode) -> std::io::Result<File> {
                let mut options = OpenOptions::new();
                options
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                if mode == LockMode::Write {
                    return options.write(true).create(true).truncate(false).open(path);
                }
                match options.open(path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    opened => return opened,
                }
                // No writer has created the lock yet. A signer that can create
                // it does; one that cannot must not sign unlocked, because a
                // writer may create it and append at any moment.
                match options.clone().write(true).create_new(true).open(path) {
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        options.open(path)
                    }
                    Err(error) => Err(std::io::Error::new(
                        error.kind(),
                        format!(
                            "{} does not exist and cannot be created ({error}); \
                             a writer creates it on its first append",
                            path.display()
                        ),
                    )),
                    created => created,
                }
            }
        }
        Ok(guard)
    }
}

impl Drop for LedgerLock {
    fn drop(&mut self) {
        // Closing the sidecar releases the record lock before another local
        // contender can open it. A forked child must not unlock its parent.
        drop(self.record.take());
        #[cfg(unix)]
        drop(self.slot.take());
        if let Some(gate) = self.gate.take() {
            if self.owner == std::process::id() {
                let _ = FileExt::unlock(&gate);
            }
        }
    }
}

/// Move the bytes after the last newline, an append that never finished,
/// into a new quarantine file beside the log, flush it and the directory,
/// and only then cut them from the log. Runs under the writer lock.
fn quarantine_tail(
    file: &File,
    paths: &LedgerPaths,
    committed: u64,
    tail: &[u8],
) -> Result<PathBuf> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mode = file
            .metadata()
            .context("inspect ledger")?
            .permissions()
            .mode()
            & 0o666;
        options.mode(mode).custom_flags(libc::O_NOFOLLOW);
    }
    let stamp = Utc::now().format("%Y%m%dT%H%M%S%9fZ");
    let mut attempt = 0u32;
    let (path, mut quarantine) = loop {
        let mut name = paths.quarantine.clone();
        name.push(format!("{stamp}.{}.{attempt}", std::process::id()));
        let path = paths.directory.join(name);
        match options.open(&path) {
            Ok(opened) => break (path, opened),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt < 1000 => {
                attempt += 1;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("create ledger quarantine {}", path.display()))
            }
        }
    };
    let kept = quarantine
        .write_all(tail)
        .and_then(|()| quarantine.sync_all())
        .with_context(|| format!("write ledger quarantine {}", path.display()))
        .and_then(|()| sync_directory(&paths.directory));
    if let Err(error) = kept {
        drop(quarantine);
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    file.set_len(committed)
        .and_then(|()| file.sync_all())
        .with_context(|| {
            format!(
                "cut the interrupted tail from ledger {}; it is kept in {}",
                paths.log.display(),
                path.display()
            )
        })?;
    Ok(path)
}

/// Write one line and flush it. A failed or short write is cut back to
/// `start`, so the log never keeps a partial line this writer left.
fn write_line(file: &mut File, start: u64, line: &str, log: &Path) -> Result<()> {
    let written = file
        .write_all(format!("{line}\n").as_bytes())
        .and_then(|()| file.sync_all());
    let Err(error) = written else {
        return Ok(());
    };
    let error = anyhow!(error).context(format!("append to ledger {}", log.display()));
    match file.set_len(start).and_then(|()| file.sync_all()) {
        Ok(()) => Err(error),
        Err(rollback) => Err(error.context(format!(
            "the partial line could not be cut back ({rollback}); the next append quarantines it"
        ))),
    }
}

/// Append one record to the log under an exclusive lock on the log file.
///
/// A producer can persist a unique `build.event_id` for each event packet.
/// An identical retry acknowledges the existing entry and writes a current
/// signed head without appending another record. A reused identifier with
/// different content is refused. An expected head must match the history;
/// only an acknowledged retry can accept a matching prefix instead of the tip.
pub fn append(request: &AppendRequest) -> Result<AppendReport> {
    if request.head_out.is_some() != request.key.is_some() {
        return Err(crate::cli_parse::usage(
            "--key and --head-out go together: the key signs the head that --head-out names",
        ));
    }
    let key = match &request.key {
        Some(path) => Some(open_signing_key(path, &request.log)?),
        None => None,
    };
    check_tile_namespace(
        &request.log,
        request.tiles.as_deref(),
        key.as_ref(),
        request.head_out.as_deref(),
    )?;
    let text = std::fs::read_to_string(&request.record)
        .with_context(|| format!("read record {}", request.record.display()))?;
    let record: Value = serde_json::from_str(&text)
        .with_context(|| format!("parse record {}", request.record.display()))?;
    let expected = match &request.expect_head {
        Some(path) => Some(SignedHead::read(path)?),
        None => None,
    };

    let mut options = std::fs::OpenOptions::new();
    options.read(true).append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options
        .open(&request.log)
        .with_context(|| format!("open ledger {}", request.log.display()))?;
    let lock = LedgerLock::acquire(&file, &request.log)?;
    if let Some(out) = &request.head_out {
        check_head_target(out, &lock.paths, key.as_ref())?;
    }
    let mut current = Vec::new();
    file.seek(SeekFrom::Start(0))?;
    file.read_to_end(&mut current)?;
    let committed = current
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |at| at + 1);
    if committed < current.len() && serde_json::from_slice::<Value>(&current[committed..]).is_ok() {
        bail!(
            "the last line of ledger {} is complete JSON without a newline; it may be a \
             committed record, so it is neither repaired nor appended after",
            request.log.display()
        );
    }
    let mut entries = std::str::from_utf8(&current[..committed])
        .context("the log is not UTF-8")
        .and_then(parse_log)
        .with_context(|| format!("ledger {}", request.log.display()))?;
    let id = match record.as_object() {
        Some(record) => event_id(record)?,
        None => None,
    };
    let previous = if let Some(id) = id {
        let mut matching = entries.iter().filter(|entry| {
            entry
                .record
                .get("build")
                .and_then(|build| build.get("event_id"))
                .and_then(Value::as_str)
                == Some(id)
        });
        let previous = matching.next().cloned();
        if matching.next().is_some() {
            bail!("ledger contains multiple records for build.event_id {id:?}");
        }
        previous
    } else {
        None
    };
    let replayed = previous.is_some();
    let now = now_utc();
    let entry = if let Some(previous) = previous {
        let original_ts = previous.record["ts"].as_str().unwrap_or(&now);
        let retry = seal_record(&record, previous.index, original_ts)
            .with_context(|| format!("record {}", request.record.display()))?;
        if retry.line != previous.line {
            bail!(
                "build.event_id {:?} is already recorded with different content",
                id
            );
        }
        previous
    } else {
        seal_record(&record, entries.len(), &now)
            .with_context(|| format!("record {}", request.record.display()))?
    };
    if let Some(expected) = &expected {
        let current_leaves = leaves(&entries);
        let prefix_matches = expected.size <= current_leaves.len()
            && expected.root == rfc9162::Head::over(&current_leaves[..expected.size]).root;
        if expected.domain != DOMAIN
            || !prefix_matches
            || (!replayed && expected.size != entries.len())
        {
            let head = rfc9162::Head::over(&current_leaves);
            bail!(
                "ledger is at size {} root {}, not the expected head size {} root {}",
                head.size,
                head.root,
                expected.size,
                expected.root
            );
        }
    }
    if let Some(first) = entries.first() {
        if first.record["stack"] != entry.record["stack"] {
            bail!(
                "record is stack {} but ledger {} is stack {}",
                entry.record["stack"],
                request.log.display(),
                first.record["stack"]
            );
        }
    }
    let quarantined = if committed < current.len() {
        Some(quarantine_tail(
            &file,
            &lock.paths,
            committed as u64,
            &current[committed..],
        )?)
    } else {
        None
    };
    if replayed {
        // A replay is acknowledged only after its record is on stable storage.
        file.sync_all()
            .with_context(|| format!("flush ledger {}", request.log.display()))?;
    } else {
        write_line(&mut file, committed as u64, &entry.line, &request.log)?;
        entries.push(entry.clone());
    }
    sync_directory(&lock.paths.directory)?;
    let signed = key
        .as_ref()
        .map(|key| SignedHead::sign(&entries, &key.key, &now));
    if let (Some(signed), Some(out)) = (&signed, &request.head_out) {
        signed.write(out)?;
    }
    let dir = crate::ledger_tiles::tiles_dir(&request.log, request.tiles.as_deref());
    let (tiles, checkpoint) = match &key {
        Some(key) => {
            let origin = crate::ledger_tiles::origin(entries.first(), request.origin.as_deref())?;
            let (tiles, checkpoint) =
                crate::ledger_tiles::checkpoint(&entries, &dir, &key.key, &origin)?;
            (tiles, Some(checkpoint))
        }
        None => (crate::ledger_tiles::sync(&entries, &dir)?, None),
    };
    drop(lock);
    Ok(AppendReport {
        index: entry.index,
        id: entry.id,
        line: entry.line,
        head: rfc9162::Head::over(&leaves(&entries)),
        signed,
        replayed,
        quarantined: quarantined.map(|path| (current.len() - committed, path)),
        tiles,
        checkpoint,
    })
}

/// What `ledger head` needs.
#[derive(Debug, Clone)]
pub struct HeadRequest {
    /// The JSON Lines log.
    pub log: PathBuf,
    /// Ed25519 PKCS#8 PEM key.
    pub key: PathBuf,
    /// The head file to write.
    pub out: PathBuf,
    /// Signing time, RFC 3339 UTC. Defaults to now.
    pub ts: Option<String>,
    /// The tile log, else the log's path with `.tiles`.
    pub tiles: Option<PathBuf>,
    /// The checkpoint origin, else `eb-stack-ledger/<stack>/<year>`.
    pub origin: Option<String>,
}

/// What `ledger head` signed.
#[derive(Debug, Clone)]
pub struct HeadReport {
    /// The signed head over the JSON Lines log.
    pub head: SignedHead,
    /// The tile log after its sync.
    pub tiles: crate::ledger_tiles::SyncReport,
    /// The tile log's checkpoint. `None` for an empty log signed without
    /// `--origin`: a size-0 head has no record to derive an origin from and
    /// nothing to checkpoint.
    pub checkpoint: Option<crate::ledger_tiles::CheckpointReport>,
}

/// Sign the log's current head, and the tile log's checkpoint with the
/// same key. The log is read and the head written under the signer's side of
/// the writer lock, so the head is the log between two appends and a head an
/// append wrote is never replaced by an older one. The signer opens the log
/// and its record lock for reading only.
pub fn sign_head(request: &HeadRequest) -> Result<HeadReport> {
    let key = open_signing_key(&request.key, &request.log)?;
    check_tile_namespace(
        &request.log,
        request.tiles.as_deref(),
        Some(&key),
        Some(&request.out),
    )?;
    let ts = match &request.ts {
        Some(ts) => {
            let parsed = DateTime::parse_from_rfc3339(ts)
                .map_err(|_| crate::cli_parse::usage(format!("--ts {ts:?} is not RFC 3339")))?;
            if parsed.offset().local_minus_utc() != 0 {
                return Err(crate::cli_parse::usage(format!("--ts {ts:?} is not UTC")));
            }
            ts.clone()
        }
        None => now_utc(),
    };
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options
        .open(&request.log)
        .with_context(|| format!("read ledger {}", request.log.display()))?;
    let lock = LedgerLock::acquire_shared(&file, &request.log)?;
    check_head_target(&request.out, &lock.paths, Some(&key))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .with_context(|| format!("read ledger {}", request.log.display()))?;
    let entries = parse_log(&text).with_context(|| format!("ledger {}", request.log.display()))?;
    let head = SignedHead::sign(&entries, &key.key, &ts);
    head.write(&request.out)?;
    let dir = crate::ledger_tiles::tiles_dir(&request.log, request.tiles.as_deref());
    let (tiles, checkpoint) = if entries.is_empty() && request.origin.is_none() {
        (crate::ledger_tiles::sync(&entries, &dir)?, None)
    } else {
        let origin =
            crate::ledger_tiles::origin(entries.first(), request.origin.as_deref())?;
        let (tiles, checkpoint) =
            crate::ledger_tiles::checkpoint(&entries, &dir, &key.key, &origin)?;
        (tiles, Some(checkpoint))
    };
    drop(lock);
    Ok(HeadReport {
        head,
        tiles,
        checkpoint,
    })
}

/// What `ledger show` needs.
#[derive(Debug, Clone)]
pub struct ShowRequest {
    /// The JSON Lines log.
    pub log: PathBuf,
    /// A module (`PETSc/3.25.0-foss-2026.1`) or a software name (`PETSc`).
    pub package: Option<String>,
    /// Print the records as JSON instead of a table.
    pub json: bool,
}

/// What `ledger show` found.
#[derive(Debug, Clone)]
pub struct ShowReport {
    /// Matching records, newest first.
    pub records: Vec<Value>,
    /// What the command prints.
    pub stdout: String,
}

/// List records, newest first, optionally for one package.
pub fn show(request: &ShowRequest) -> Result<ShowReport> {
    let entries = read_log(&request.log)?;
    let matches = |entry: &Entry| match &request.package {
        None => true,
        Some(name) => {
            let module = entry.module();
            module == name || module.split('/').next() == Some(name.as_str())
        }
    };
    let selected = entries
        .iter()
        .rev()
        .filter(|entry| matches(entry))
        .collect::<Vec<_>>();
    let records = selected
        .iter()
        .map(|entry| Value::Object(entry.record.clone()))
        .collect::<Vec<_>>();
    let stdout = if request.json {
        let mut text = serde_json::to_string_pretty(&records)?;
        text.push('\n');
        text
    } else {
        let mut text = String::from("index\tts\taction\ttarget\tmodule\tactor\n");
        for entry in &selected {
            let field = |name: &str| entry.record[name].as_str().unwrap_or_default().to_string();
            let _ = writeln!(
                text,
                "{}\t{}\t{}\t{}\t{}\t{}",
                entry.index,
                field("ts"),
                field("action"),
                field("target"),
                field("module"),
                field("actor")
            );
        }
        let _ = writeln!(text, "{} of {} records", selected.len(), entries.len());
        text
    };
    Ok(ShowReport { records, stdout })
}

/// Which proof `ledger verify` checks.
#[derive(Debug, Clone)]
pub enum VerifyMode {
    /// Record `record` is in the signed head `head`.
    Inclusion {
        /// Leaf number.
        record: usize,
        /// Head file.
        head: PathBuf,
    },
    /// The signed head `to` extends the signed head `from`.
    Consistency {
        /// The earlier head file.
        from: PathBuf,
        /// The later head file.
        to: PathBuf,
    },
}

/// What `ledger verify` needs.
#[derive(Debug, Clone)]
pub struct VerifyRequest {
    /// The JSON Lines log.
    pub log: PathBuf,
    /// The public key heads must be signed by.
    pub public_key: PathBuf,
    /// The proof to check.
    pub mode: VerifyMode,
    /// The tile log, else the log's path with `.tiles`.
    pub tiles: Option<PathBuf>,
    /// The checkpoint origin, else derived from the log.
    pub origin: Option<String>,
}

/// A proof that checked.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    /// `inclusion` or `consistency`.
    pub kind: String,
    /// The proof nodes, lower-case hex, innermost first.
    pub proof: Vec<String>,
    /// Entries in the tile log, which extends the head.
    pub tiles_size: u64,
    /// The tile log's root, lower-case hex.
    pub tiles_root: String,
    /// What the command prints.
    pub stdout: String,
}

/// Check an inclusion or consistency proof computed from the log against
/// signed heads, then that the tile log extends the head and holds the
/// same lines. Any failure is an error.
pub fn verify(request: &VerifyRequest) -> Result<VerifyReport> {
    let trusted = read_public_key(&request.public_key)?;
    let entries = read_log(&request.log)?;
    let all = leaves(&entries);
    let mut report = verify_proof(request, &trusted, &entries, &all)?;
    let (size, root) = match &request.mode {
        VerifyMode::Inclusion { head, .. } => {
            let head = SignedHead::read(head)?;
            (head.size, head.root)
        }
        VerifyMode::Consistency { to, .. } => {
            let to = SignedHead::read(to)?;
            (to.size, to.root)
        }
    };
    if entries.is_empty() && request.origin.is_none() {
        // A size-0 head over an empty log was signed without a checkpoint,
        // and there is no record to derive an origin from: nothing to audit.
        report.tiles_size = 0;
        report.tiles_root = root;
        return Ok(report);
    }
    let origin = crate::ledger_tiles::origin(entries.first(), request.origin.as_deref())?;
    let dir = crate::ledger_tiles::tiles_dir(&request.log, request.tiles.as_deref());
    let (tiles_size, tiles_root) =
        crate::ledger_tiles::audit(&entries, &dir, size, &root, &origin, &trusted)?;
    let _ = writeln!(
        report.stdout,
        "tile log size {tiles_size} root {tiles_root} extends the head and holds the same lines"
    );
    report.tiles_size = tiles_size;
    report.tiles_root = tiles_root;
    Ok(report)
}

fn verify_proof(
    request: &VerifyRequest,
    trusted: &VerifyingKey,
    entries: &[Entry],
    all: &[[u8; 32]],
) -> Result<VerifyReport> {
    match &request.mode {
        VerifyMode::Inclusion { record, head } => {
            let head = SignedHead::read(head)?;
            head.check(trusted)?;
            if head.size > all.len() {
                bail!(
                    "head covers {} records but the ledger has {}",
                    head.size,
                    all.len()
                );
            }
            if *record >= head.size {
                bail!(
                    "record {record} is past the head, which covers {} records",
                    head.size
                );
            }
            let covered = &all[..head.size];
            let proof = rfc9162::inclusion_proof(covered, *record)
                .ok_or_else(|| anyhow!("no inclusion proof for record {record}"))?;
            if !rfc9162::verify_inclusion(&covered[*record], *record, head.size, &proof, &head.root)
            {
                bail!(
                    "record {record} is not in the head of size {} root {}",
                    head.size,
                    head.root
                );
            }
            Ok(VerifyReport {
                kind: "inclusion".to_string(),
                proof: proof.iter().map(|node| rfc9162::hex(node)).collect(),
                tiles_size: 0,
                tiles_root: String::new(),
                stdout: format!(
                    "record {record} id {} is in the head of size {} root {}\n",
                    entries[*record].id, head.size, head.root
                ),
            })
        }
        VerifyMode::Consistency { from, to } => {
            let from = SignedHead::read(from)?;
            let to = SignedHead::read(to)?;
            from.check(trusted).context("--from head")?;
            to.check(trusted).context("--to head")?;
            if from.size > to.size {
                bail!(
                    "the --from head covers {} records and the --to head {}; a log only grows",
                    from.size,
                    to.size
                );
            }
            if to.size > all.len() {
                bail!(
                    "the --to head covers {} records but the ledger has {}",
                    to.size,
                    all.len()
                );
            }
            let covered = &all[..to.size];
            let proof = if from.size == 0 {
                // The empty tree is a prefix of every tree; the only thing to
                // check is that the earlier head is the empty head.
                if from.root != rfc9162::hex(&rfc9162::root(&[])) {
                    bail!("the --from head has size 0 but not the empty root");
                }
                if rfc9162::hex(&rfc9162::root(covered)) != to.root {
                    bail!("the ledger does not hash to the --to head");
                }
                Vec::new()
            } else {
                let proof = rfc9162::consistency_proof(covered, from.size)
                    .ok_or_else(|| anyhow!("no consistency proof from size {}", from.size))?;
                if !rfc9162::verify_consistency(from.size, &from.root, to.size, &to.root, &proof) {
                    bail!(
                        "the head of size {} root {} does not extend the head of size {} root {}",
                        to.size,
                        to.root,
                        from.size,
                        from.root
                    );
                }
                proof
            };
            Ok(VerifyReport {
                kind: "consistency".to_string(),
                proof: proof.iter().map(|node| rfc9162::hex(node)).collect(),
                tiles_size: 0,
                tiles_root: String::new(),
                stdout: format!(
                    "the head of size {} root {} extends the head of size {} root {}\n",
                    to.size, to.root, from.size, from.root
                ),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(module: &str, ts: &str) -> Value {
        json!({
            "ts": ts,
            "actor": "jenkins/run/1",
            "stack": "2026.06",
            "target": "x86_64/amd/zen4",
            "module": module,
            "location": format!("/cvmfs/software.eessi.io/{module}"),
            "action": "install",
        })
    }

    #[test]
    fn canonical_json_sorts_keys_and_drops_whitespace() {
        let value: Value =
            serde_json::from_str(r#"{ "b" : [1, {"y": 2, "x": "é\n"}], "a": null }"#).unwrap();
        assert_eq!(
            canonical(&value).unwrap(),
            r#"{"a":null,"b":[1,{"x":"é\n","y":2}]}"#
        );
    }

    #[test]
    fn canonical_json_refuses_floats() {
        assert!(canonical(&json!({"a": 1.5})).is_err());
    }

    #[test]
    fn a_sealed_record_parses_back() {
        let entry =
            seal_record(&record("zlib/1.3.1", "2026-11-03T10:00:00Z"), 0, "unused").unwrap();
        let parsed = parse_log(&format!("{}\n", entry.line)).unwrap();
        assert_eq!(parsed[0].id, entry.id);
        assert_eq!(parsed[0].line, entry.line);
    }

    #[test]
    fn build_inodes_peak_is_a_whole_number() {
        let mut with_peak = record("CP2K/2025.2-foss-2025a", "2026-11-03T10:00:00Z");
        with_peak["build"] = json!({"job": "snellius/4411", "inodes_peak": 412_345});
        let entry = seal_record(&with_peak, 0, "unused").unwrap();
        assert_eq!(entry.record["build"]["inodes_peak"], 412_345);
        let mut negative = with_peak.clone();
        negative["build"]["inodes_peak"] = json!(-1);
        let error = seal_record(&negative, 0, "unused").unwrap_err().to_string();
        assert!(error.contains("build.inodes_peak"), "{error}");
        let mut text = with_peak.clone();
        text["build"]["inodes_peak"] = json!("many");
        assert!(seal_record(&text, 0, "unused").is_err());
        // A build object without the field, and a build string, still pass.
        let mut plain = with_peak.clone();
        plain["build"] = json!("log sha256:00");
        assert!(seal_record(&plain, 0, "unused").is_ok());
    }

    #[test]
    fn a_missing_ts_is_filled() {
        let mut bare = record("zlib/1.3.1", "x");
        bare.as_object_mut().unwrap().remove("ts");
        let entry = seal_record(&bare, 0, "2026-11-03T10:00:00Z").unwrap();
        assert_eq!(entry.record["ts"], "2026-11-03T10:00:00Z");
    }

    #[test]
    fn records_outside_the_format_are_refused() {
        let mut unknown = record("zlib/1.3.1", "2026-11-03T10:00:00Z");
        unknown["licence_key"] = json!("secret");
        assert!(seal_record(&unknown, 0, "").is_err());
        let mut action = record("zlib/1.3.1", "2026-11-03T10:00:00Z");
        action["action"] = json!("reinstall");
        assert!(seal_record(&action, 0, "").is_err());
        let local = record("zlib/1.3.1", "2026-11-03T10:00:00+01:00");
        assert!(seal_record(&local, 0, "").is_err());
        let mut correction = record("zlib/1.3.1", "2026-11-03T10:00:00Z");
        correction["action"] = json!("correction");
        assert!(seal_record(&correction, 0, "").is_err());
        let wrong_index = {
            let mut value = record("zlib/1.3.1", "2026-11-03T10:00:00Z");
            value["index"] = json!(4);
            value
        };
        assert!(seal_record(&wrong_index, 0, "").is_err());
    }

    #[test]
    fn a_line_out_of_canonical_form_is_refused() {
        let entry = seal_record(&record("zlib/1.3.1", "2026-11-03T10:00:00Z"), 0, "").unwrap();
        let spaced = entry.line.replacen(':', ": ", 1);
        assert!(parse_log(&format!("{spaced}\n")).is_err());
        assert!(parse_log(&entry.line).is_err(), "no trailing newline");
    }

    #[test]
    fn a_head_signature_binds_domain_size_root_and_time() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let entry = seal_record(&record("zlib/1.3.1", "2026-11-03T10:00:00Z"), 0, "").unwrap();
        let head = SignedHead::sign(&[entry], &key, "2026-11-03T10:00:00Z");
        head.check(&key.verifying_key()).unwrap();
        for tamper in [
            SignedHead {
                size: 2,
                ..head.clone()
            },
            SignedHead {
                ts: "2026-11-04T10:00:00Z".to_string(),
                ..head.clone()
            },
            SignedHead {
                domain: "deedar-log-v1".to_string(),
                ..head.clone()
            },
        ] {
            assert!(tamper.check(&key.verifying_key()).is_err());
        }
        let other = SigningKey::from_bytes(&[8u8; 32]);
        assert!(head.check(&other.verifying_key()).is_err());
        assert!(
            SignedHead::material(1, &head.root, &head.ts).starts_with(b"eb-stack-ledger-v1\t1\t")
        );
    }

    /// Whether a second guard taken in this process waits for `first`.
    #[cfg(unix)]
    fn second_guard_waits(first: LockMode, second: LockMode) {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        use std::time::Duration;

        let root = tempfile::tempdir().unwrap();
        let log = root.path().join("stack.jsonl");
        std::fs::write(&log, "").unwrap();
        let open = || OpenOptions::new().read(true).open(&log).unwrap();
        let held = LedgerLock::acquire_as(&open(), &log, first).unwrap();
        let taken = Arc::new(AtomicBool::new(false));
        let waiter = {
            let (log, taken, file) = (log.clone(), Arc::clone(&taken), open());
            std::thread::spawn(move || {
                let guard = LedgerLock::acquire_as(&file, &log, second).unwrap();
                taken.store(true, Ordering::SeqCst);
                drop(guard);
            })
        };
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !taken.load(Ordering::SeqCst),
            "a second guard in the same process shared the record lock"
        );
        drop(held);
        waiter.join().unwrap();
        assert!(taken.load(Ordering::SeqCst));
    }

    #[cfg(unix)]
    #[test]
    fn a_signer_in_the_writer_process_waits_for_the_writer() {
        second_guard_waits(LockMode::Write, LockMode::Read);
    }

    #[cfg(unix)]
    #[test]
    fn signers_in_one_process_do_not_share_the_record_lock() {
        second_guard_waits(LockMode::Read, LockMode::Read);
    }

    #[cfg(unix)]
    #[test]
    fn a_writer_in_the_signer_process_waits_for_the_signer() {
        second_guard_waits(LockMode::Read, LockMode::Write);
    }
}
