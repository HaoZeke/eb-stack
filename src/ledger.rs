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

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use ed25519_dalek::pkcs8::{DecodePrivateKey, DecodePublicKey};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

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

/// Fields with structured content: free text or an object of references.
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
            || key == "stage";
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
    for field in LINKS.iter().copied().chain(["stage", "sig"]) {
        if let Some(value) = record.get(field) {
            if !value.as_str().is_some_and(|text| !text.is_empty()) {
                bail!("ledger field {field} must be a non-empty string");
            }
        }
    }
    if let Some(stage) = record.get("stage").and_then(Value::as_str) {
        if !STAGES.contains(&stage) {
            bail!("ledger stage {stage:?} is not one of {}", STAGES.join(", "));
        }
    }
    if action == "correction" && !record.contains_key("corrects") {
        bail!("a correction record names the record it corrects in corrects");
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

fn parse_line(line: &str, index: usize) -> Result<Entry> {
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

    /// Write a head file: the head as JSON, one line.
    pub fn write(&self, path: &Path) -> Result<()> {
        let mut text = serde_json::to_string(self)?;
        text.push('\n');
        std::fs::write(path, text).with_context(|| format!("write head {}", path.display()))
    }
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
        format!(
            "{status} record {} id {}; head size {} root {}",
            self.index, self.id, self.head.size, self.head.root
        )
    }
}

/// A local gate keeps contenders from opening the process-scoped record-lock
/// inode. Closing any descriptor for that inode releases the process's lock.
struct LedgerLock {
    gate: std::fs::File,
    record: Option<std::fs::File>,
    owner: u32,
}

impl LedgerLock {
    fn acquire(gate: &std::fs::File, log: &Path) -> Result<Self> {
        if !gate.metadata().context("inspect ledger file")?.is_file() {
            bail!("ledger must be a regular file: {}", log.display());
        }
        let gate = gate
            .try_clone()
            .context("duplicate ledger lock descriptor")?;
        FileExt::lock_exclusive(&gate).with_context(|| format!("lock ledger {}", log.display()))?;
        let mut guard = Self {
            gate,
            record: None,
            owner: std::process::id(),
        };
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

            let opened = guard.gate.metadata().context("inspect ledger lock inode")?;
            if opened.nlink() != 1 {
                bail!(
                    "ledger requires one link for writer exclusion: {}",
                    log.display()
                );
            }
            let canonical = log.canonicalize().context("resolve ledger lock path")?;
            let resolved = canonical.metadata().context("inspect resolved ledger")?;
            if (opened.dev(), opened.ino()) != (resolved.dev(), resolved.ino()) {
                bail!(
                    "ledger path changed while acquiring its lock: {}",
                    log.display()
                );
            }
            let mut name = canonical.into_os_string();
            name.push(".posix-lock");
            let record = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(PathBuf::from(name))
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
            lock.l_type = libc::F_WRLCK as _;
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
        }
        Ok(guard)
    }
}

impl Drop for LedgerLock {
    fn drop(&mut self) {
        // Closing the sidecar releases the record lock before another local
        // contender can open it. A forked child must not unlock its parent.
        drop(self.record.take());
        if self.owner == std::process::id() {
            let _ = FileExt::unlock(&self.gate);
        }
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
        Some(path) => {
            refuse_key_beside_log(path, &request.log)?;
            Some(read_signing_key(path)?)
        }
        None => None,
    };
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
    let _lock = LedgerLock::acquire(&file, &request.log)?;
    let mut current = String::new();
    file.seek(SeekFrom::Start(0))?;
    file.read_to_string(&mut current)?;
    let mut entries =
        parse_log(&current).with_context(|| format!("ledger {}", request.log.display()))?;
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
    if !replayed {
        file.write_all(format!("{}\n", entry.line).as_bytes())
            .with_context(|| format!("append to ledger {}", request.log.display()))?;
        file.sync_all()?;
        entries.push(entry.clone());
    }
    let signed = key.map(|key| SignedHead::sign(&entries, &key, &now));
    if let (Some(signed), Some(out)) = (&signed, &request.head_out) {
        signed.write(out)?;
    }
    Ok(AppendReport {
        index: entry.index,
        id: entry.id,
        line: entry.line,
        head: rfc9162::Head::over(&leaves(&entries)),
        signed,
        replayed,
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
}

/// Sign the log's current head.
pub fn sign_head(request: &HeadRequest) -> Result<SignedHead> {
    refuse_key_beside_log(&request.key, &request.log)?;
    let key = read_signing_key(&request.key)?;
    let entries = read_log(&request.log)?;
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
    let head = SignedHead::sign(&entries, &key, &ts);
    head.write(&request.out)?;
    Ok(head)
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
}

/// A proof that checked.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    /// `inclusion` or `consistency`.
    pub kind: String,
    /// The proof nodes, lower-case hex, innermost first.
    pub proof: Vec<String>,
    /// What the command prints.
    pub stdout: String,
}

/// Check an inclusion or consistency proof computed from the log against
/// signed heads. Any failure is an error.
pub fn verify(request: &VerifyRequest) -> Result<VerifyReport> {
    let trusted = read_public_key(&request.public_key)?;
    let entries = read_log(&request.log)?;
    let all = leaves(&entries);
    match &request.mode {
        VerifyMode::Inclusion { record, head } => {
            let head = SignedHead::read(head)?;
            head.check(&trusted)?;
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
                stdout: format!(
                    "record {record} id {} is in the head of size {} root {}\n",
                    entries[*record].id, head.size, head.root
                ),
            })
        }
        VerifyMode::Consistency { from, to } => {
            let from = SignedHead::read(from)?;
            let to = SignedHead::read(to)?;
            from.check(&trusted).context("--from head")?;
            to.check(&trusted).context("--to head")?;
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
}
