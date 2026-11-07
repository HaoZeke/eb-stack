//! The tile log beside a ledger: the same canonical lines as the JSON Lines
//! file, stored as a C2SP tlog-tiles log with key sidecars through the
//! `rfc9162-tiles` crate.
//!
//! The JSON Lines file is the canonical ledger. The tile log follows it: a
//! sync appends the lines the tile log does not hold yet, and a tile log
//! whose entries disagree with the ledger is moved aside and rebuilt. Each
//! entry's lookup keys come from [`keys_of`], so `ledger find` answers
//! `pkg=PETSc` or `route=kvm` from the sidecars instead of a scan.
//!
//! The head of the tile log is a C2SP checkpoint under the origin
//! `eb-stack-ledger/<stack>/<year>`, signed with the same Ed25519 key as
//! the ledger's JSON head. A find opens the log at its tree state, checks
//! that the signed checkpoint is a prefix of it, and checks the inclusion
//! proof of every record it prints. A verify checks that the tile log
//! extends the signed JSON head and holds the same lines.

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use ed25519_dalek::{SigningKey, VerifyingKey};
use rfc9162_tiles::checkpoint::{self, ED25519_LOG};
use rfc9162_tiles::index::KeyFn;
use rfc9162_tiles::layout::{CHECKPOINT, TILE_WIDTH};
use rfc9162_tiles::{Checkpoint, Note, PosixStore, Reader, Signer, TileStore, Verifier, Writer};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::ledger::{self, Entry};

/// The values `route` can take: where the build ran.
pub const ROUTES: &[&str] = &["native", "kvm", "qemu"];

/// Fields whose value becomes a `field=value` key as it stands.
const KEYED: &[&str] = &["target", "action", "route", "actor", "id"];

/// The lookup keys of one entry, the bytes of a canonical ledger line:
/// `module=<module>`, `pkg=<software name before the slash>`, and
/// `target=`, `action=`, `route=`, `actor=` and `id=` for each of those
/// fields the record carries, plus `job=<build.job>` when `build` is an
/// object with a string `job`. Bytes that are not a JSON object have no
/// keys beyond the identity hash the crate adds.
#[must_use]
pub fn keys_of(entry: &[u8]) -> Vec<Vec<u8>> {
    let Ok(Value::Object(record)) = serde_json::from_slice::<Value>(entry) else {
        return Vec::new();
    };
    let text = |name: &str| {
        record
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let mut keys = Vec::new();
    if let Some(module) = text("module") {
        keys.push(format!("module={module}"));
        if let Some(name) = module.split('/').next().filter(|name| !name.is_empty()) {
            keys.push(format!("pkg={name}"));
        }
    }
    for field in KEYED {
        if let Some(value) = text(field) {
            keys.push(format!("{field}={value}"));
        }
    }
    if let Some(job) = record
        .get("build")
        .and_then(Value::as_object)
        .and_then(|build| build.get("job"))
        .and_then(Value::as_str)
        .filter(|job| !job.is_empty())
    {
        keys.push(format!("job={job}"));
    }
    keys.into_iter().map(String::into_bytes).collect()
}

/// The key function the tile log is written and read with.
#[must_use]
pub fn key_fn() -> KeyFn {
    Box::new(keys_of)
}

/// Where the tile log of `log` lives: `tiles` when given, else the log's
/// path with `.tiles` appended (`ledger/2026.06.jsonl.tiles`).
#[must_use]
pub fn tiles_dir(log: &Path, tiles: Option<&Path>) -> PathBuf {
    match tiles {
        Some(dir) => dir.to_path_buf(),
        None => {
            let mut name = log.as_os_str().to_owned();
            name.push(".tiles");
            PathBuf::from(name)
        }
    }
}

/// The checkpoint origin of a log: `--origin` when given, else
/// `eb-stack-ledger/<stack>/<year>` from the first record's `stack` and the
/// year of its `ts`. The year of the first record keeps the origin the same
/// for the life of the log.
pub fn origin(first: Option<&Entry>, given: Option<&str>) -> Result<String> {
    if let Some(origin) = given {
        if origin.is_empty() || origin.chars().any(|c| c.is_whitespace() || c == '+') {
            return Err(crate::cli_parse::usage(format!(
                "--origin {origin:?} has whitespace or a plus; a checkpoint origin has neither"
            )));
        }
        return Ok(origin.to_string());
    }
    let Some(first) = first else {
        bail!("the ledger is empty, so no origin derives from it; pass --origin");
    };
    let stack = first.record["stack"].as_str().unwrap_or_default();
    let ts = first.record["ts"].as_str().unwrap_or_default();
    let year = ts.get(..4).unwrap_or_default();
    if stack.is_empty() || year.len() != 4 {
        bail!("the first record has no stack or year to derive an origin from; pass --origin");
    }
    Ok(format!("eb-stack-ledger/{stack}/{year}"))
}

/// The origin from the first line of the log file alone, for a find that
/// does not read the whole ledger.
pub fn origin_from_log(log: &Path, given: Option<&str>) -> Result<String> {
    if given.is_some() {
        return origin(None, given);
    }
    let file =
        std::fs::File::open(log).with_context(|| format!("read ledger {}", log.display()))?;
    let mut line = String::new();
    std::io::BufReader::new(file)
        .read_line(&mut line)
        .with_context(|| format!("read ledger {}", log.display()))?;
    let Some(line) = line.strip_suffix('\n') else {
        bail!(
            "ledger {} has no complete first line, so no origin derives from it; pass --origin",
            log.display()
        );
    };
    let first =
        ledger::parse_line(line, 0).with_context(|| format!("ledger {} line 0", log.display()))?;
    origin(Some(&first), None)
}

/// What a sync left the tile log at.
#[derive(Debug, Clone, Serialize)]
pub struct SyncReport {
    /// The tile log directory.
    pub dir: PathBuf,
    /// Entries in the tile log after the sync.
    pub size: u64,
    /// Root over them, lower-case hex.
    pub root: String,
    /// Entries this sync appended.
    pub appended: u64,
    /// Where a tile log that disagreed with the ledger was moved before
    /// the rebuild.
    pub rebuilt_from: Option<PathBuf>,
}

impl SyncReport {
    /// One line for stdout.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "tile log size {} root {} at {}; {} appended",
            self.size,
            self.root,
            self.dir.display(),
            self.appended
        )
    }

    /// The warning a rebuild earns, for stderr.
    #[must_use = "tile rebuild warnings must be surfaced"]
    pub fn warning(&self) -> Option<String> {
        self.rebuilt_from.as_ref().map(|stale| {
            format!(
                "warning: the tile log disagreed with the ledger and was rebuilt from it; the old log is at {}",
                stale.display()
            )
        })
    }
}

/// The checkpoint a head published.
#[derive(Debug, Clone, Serialize)]
pub struct CheckpointReport {
    /// The origin line.
    pub origin: String,
    /// Entries the checkpoint covers.
    pub size: u64,
    /// Root at that size, lower-case hex.
    pub root: String,
    /// The checkpoint file.
    pub path: PathBuf,
    /// The signed note, as written.
    pub note: String,
}

impl CheckpointReport {
    /// One line for stdout.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "checkpoint {} size {} root {} to {}",
            self.origin,
            self.size,
            self.root,
            self.path.display()
        )
    }
}

fn open_store(dir: &Path) -> Result<PosixStore> {
    PosixStore::open(dir).with_context(|| format!("open tile log {}", dir.display()))
}

fn stale_path(dir: &Path) -> PathBuf {
    let mut name = dir.as_os_str().to_owned();
    name.push(format!(".stale-{}", Utc::now().format("%Y%m%dT%H%M%SZ")));
    PathBuf::from(name)
}

/// The writer over a tile log that holds exactly the ledger's lines, and
/// what it took to get there.
fn open_synced(entries: &[Entry], dir: &Path) -> Result<(Writer<PosixStore>, SyncReport)> {
    let leaves = ledger::leaves(entries);
    let mut writer = Writer::open(open_store(dir)?, key_fn())
        .map_err(|error| anyhow!("open tile log {}: {error}", dir.display()))?;
    let held = usize::try_from(writer.size()).unwrap_or(usize::MAX);
    let agrees = held <= leaves.len() && writer.root() == rfc9162::root(&leaves[..held]);
    let mut rebuilt_from = None;
    if !agrees {
        drop(writer);
        let stale = stale_path(dir);
        std::fs::rename(dir, &stale)
            .with_context(|| format!("move tile log {} to {}", dir.display(), stale.display()))?;
        writer = Writer::open(open_store(dir)?, key_fn())
            .map_err(|error| anyhow!("open tile log {}: {error}", dir.display()))?;
        rebuilt_from = Some(stale);
    }
    let start =
        usize::try_from(writer.size()).expect("a synced tile log is not longer than the ledger");
    let appended = writer
        .append_batch(
            entries[start..]
                .iter()
                .map(|entry| entry.line.clone().into_bytes()),
        )
        .map_err(|error| anyhow!("append to tile log {}: {error}", dir.display()))?;
    let report = SyncReport {
        dir: dir.to_path_buf(),
        size: writer.size(),
        root: rfc9162::hex(&writer.root()),
        appended: appended.end - appended.start,
        rebuilt_from,
    };
    Ok((writer, report))
}

/// Bring the tile log at `dir` to the ledger's lines: append what it lacks,
/// or move it aside and rebuild it when its entries disagree.
pub fn sync(entries: &[Entry], dir: &Path) -> Result<SyncReport> {
    let _lock = ledger::tile_writer_lock(dir)?;
    let (_writer, report) = open_synced(entries, dir)?;
    Ok(report)
}

/// Sync the tile log and sign its tree state as a C2SP checkpoint under
/// `origin` with `key`.
pub fn checkpoint(
    entries: &[Entry],
    dir: &Path,
    key: &SigningKey,
    origin: &str,
) -> Result<(SyncReport, CheckpointReport)> {
    let _lock = ledger::tile_writer_lock(dir)?;
    let (writer, synced) = open_synced(entries, dir)?;
    let signer = Signer::new(origin, key.clone())
        .map_err(|error| anyhow!("checkpoint signer for {origin}: {error}"))?;
    let note = writer
        .checkpoint(&signer, origin)
        .map_err(|error| anyhow!("checkpoint tile log {}: {error}", dir.display()))?;
    let published = Checkpoint::parse(&note.text)
        .map_err(|error| anyhow!("checkpoint tile log {}: {error}", dir.display()))?;
    Ok((
        synced,
        CheckpointReport {
            origin: published.origin,
            size: published.size,
            root: rfc9162::hex(&published.root),
            path: dir.join(CHECKPOINT),
            note: note.to_text(),
        },
    ))
}

/// The checkpoint the store holds, once `trusted` has signed it and its
/// origin is `origin`.
fn read_checkpoint(
    store: &PosixStore,
    dir: &Path,
    origin: &str,
    trusted: &VerifyingKey,
) -> Result<Checkpoint> {
    let bytes = store.get(CHECKPOINT)?.ok_or_else(|| {
        anyhow!(
            "tile log {} has no checkpoint; `ledger head` signs one",
            dir.display()
        )
    })?;
    let text = String::from_utf8(bytes)
        .map_err(|_| anyhow!("checkpoint of {} is not UTF-8", dir.display()))?;
    let note =
        Note::parse(&text).map_err(|error| anyhow!("checkpoint of {}: {error}", dir.display()))?;
    let verifier = Verifier::new(origin, ED25519_LOG, *trusted)
        .map_err(|error| anyhow!("checkpoint verifier for {origin}: {error}"))?;
    checkpoint::verify(&note, std::slice::from_ref(&verifier)).map_err(|error| {
        anyhow!(
            "checkpoint of {} is not signed by the trusted key as {origin}: {error}",
            dir.display()
        )
    })?;
    let published = Checkpoint::parse(&note.text)
        .map_err(|error| anyhow!("checkpoint of {}: {error}", dir.display()))?;
    if published.origin != origin {
        bail!(
            "checkpoint of {} is for {}, not {origin}",
            dir.display(),
            published.origin
        );
    }
    Ok(published)
}

/// Check that the tree `reader` is at extends the tree of `size` and
/// `root`: the same first `size` leaves.
fn check_prefix(
    reader: &Reader<PosixStore>,
    dir: &Path,
    size: u64,
    root: &str,
    what: &str,
) -> Result<()> {
    let current = rfc9162::hex(&reader.root());
    if reader.size() < size {
        bail!(
            "tile log {} holds {} entries, the {what} covers {size}; run `ledger tiles`",
            dir.display(),
            reader.size()
        );
    }
    if reader.size() == size {
        if current != root {
            bail!(
                "tile log {} is at root {current} at size {size}, the {what} says {root}",
                dir.display()
            );
        }
        return Ok(());
    }
    let proof = reader
        .consistency(size)
        .map_err(|error| anyhow!("tile log {}: {error}", dir.display()))?
        .ok_or_else(|| {
            anyhow!(
                "tile log {} has no consistency proof from size {size}",
                dir.display()
            )
        })?;
    let size = usize::try_from(size).map_err(|_| anyhow!("size {size} does not fit"))?;
    let current_size = usize::try_from(reader.size()).map_err(|_| anyhow!("size does not fit"))?;
    if !rfc9162::verify_consistency(size, root, current_size, &current, &proof) {
        bail!(
            "tile log {} at size {current_size} root {current} does not extend the {what} of size {size} root {root}",
            dir.display()
        );
    }
    Ok(())
}

/// Check that the tile log at `dir` extends the signed head of `size` and
/// `root` and holds the ledger's lines for those entries, and that a
/// checkpoint it holds is signed by `trusted` as `origin` and is a prefix
/// of it too. Returns the tile log's size and root.
pub fn audit(
    entries: &[Entry],
    dir: &Path,
    size: usize,
    root: &str,
    origin: &str,
    trusted: &VerifyingKey,
) -> Result<(u64, String)> {
    if !dir.is_dir() {
        bail!(
            "no tile log at {}; `ledger tiles --log LOG` writes one",
            dir.display()
        );
    }
    let store = open_store(dir)?;
    let reader = Reader::open(store.clone(), key_fn())
        .map_err(|error| anyhow!("open tile log {}: {error}", dir.display()))?;
    reader
        .hashes()
        .root()
        .map_err(|error| anyhow!("tile log {}: {error}", dir.display()))?;
    check_prefix(&reader, dir, size as u64, root, "head")?;
    if store.exists(CHECKPOINT)? {
        let published = read_checkpoint(&store, dir, origin, trusted)?;
        check_prefix(
            &reader,
            dir,
            published.size,
            &rfc9162::hex(&published.root),
            "checkpoint",
        )?;
    }
    let width = usize::try_from(TILE_WIDTH).expect("256 fits");
    let mut index = 0usize;
    while index < size {
        let bundle = reader
            .bundle(index as u64 / TILE_WIDTH)
            .map_err(|error| anyhow!("tile log {}: {error}", dir.display()))?;
        let stop = (index + width).min(size);
        for (entry, line) in bundle.iter().zip(&entries[index..stop]) {
            if entry.as_slice() != line.line.as_bytes() {
                bail!(
                    "tile log {} entry {} is not ledger line {}",
                    dir.display(),
                    line.index,
                    line.index
                );
            }
        }
        if bundle.len() < stop - index {
            bail!(
                "tile log {} bundle {} holds {} entries, the head needs {}",
                dir.display(),
                index / width,
                bundle.len(),
                stop - index
            );
        }
        index = stop;
    }
    Ok((reader.size(), rfc9162::hex(&reader.root())))
}

/// What `ledger find` needs.
#[derive(Debug, Clone)]
pub struct FindRequest {
    /// The JSON Lines log; its first line gives the origin.
    pub log: PathBuf,
    /// The tile log, else the log's path with `.tiles`.
    pub tiles: Option<PathBuf>,
    /// `field=value` keys; a record matches when it carries every one.
    pub keys: Vec<String>,
    /// The public key the checkpoint must be signed by.
    pub public_key: PathBuf,
    /// The checkpoint origin, else derived from the log.
    pub origin: Option<String>,
    /// Print the records as a JSON array instead of index and line.
    pub json: bool,
}

/// One record a find returned.
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    /// Leaf number.
    pub index: u64,
    /// The record.
    pub record: Value,
}

/// What `ledger find` found.
#[derive(Debug, Clone, Serialize)]
pub struct FindReport {
    /// Records carrying every key, by index, each proven in the checkpoint.
    pub found: Vec<Found>,
    /// Matching records past the checkpoint, which are not printed.
    pub unsigned: u64,
    /// Entries the checkpoint covers.
    pub checkpoint_size: u64,
    /// The checkpoint root, lower-case hex.
    pub checkpoint_root: String,
    /// What the command prints.
    pub stdout: String,
}

/// Records carrying every key, read from the sidecars and proven in the
/// signed checkpoint.
pub fn find(request: &FindRequest) -> Result<FindReport> {
    if request.keys.is_empty() {
        return Err(crate::cli_parse::usage(
            "pass at least one --key field=value",
        ));
    }
    for key in &request.keys {
        if !key
            .split_once('=')
            .is_some_and(|(field, value)| !field.is_empty() && !value.is_empty())
        {
            return Err(crate::cli_parse::usage(format!(
                "--key {key:?} is not field=value"
            )));
        }
    }
    let trusted = ledger::read_public_key(&request.public_key)?;
    let origin = origin_from_log(&request.log, request.origin.as_deref())?;
    let dir = tiles_dir(&request.log, request.tiles.as_deref());
    if !dir.is_dir() {
        bail!(
            "no tile log at {}; `ledger tiles --log LOG` writes one",
            dir.display()
        );
    }
    let store = open_store(&dir)?;
    let published = read_checkpoint(&store, &dir, &origin, &trusted)?;
    let reader = Reader::open(store, key_fn())
        .map_err(|error| anyhow!("open tile log {}: {error}", dir.display()))?;
    let checkpoint_root = rfc9162::hex(&published.root);
    check_prefix(
        &reader,
        &dir,
        published.size,
        &checkpoint_root,
        "checkpoint",
    )?;

    let mut hits: Option<BTreeSet<u64>> = None;
    for key in &request.keys {
        let indices = reader
            .find(key.as_bytes())
            .map_err(|error| anyhow!("tile log {}: {error}", dir.display()))?
            .into_iter()
            .collect::<BTreeSet<_>>();
        hits = Some(match hits {
            None => indices,
            Some(sofar) => sofar.intersection(&indices).copied().collect(),
        });
    }
    let size = usize::try_from(reader.size()).map_err(|_| anyhow!("size does not fit"))?;
    let root = rfc9162::hex(&reader.root());
    let mut found = Vec::new();
    let mut unsigned = 0u64;
    for index in hits.unwrap_or_default() {
        if index >= published.size {
            unsigned += 1;
            continue;
        }
        let entry = reader
            .get(index)
            .map_err(|error| anyhow!("tile log {}: {error}", dir.display()))?
            .ok_or_else(|| anyhow!("tile log {} has no entry {index}", dir.display()))?;
        let proof = reader
            .inclusion(index)
            .map_err(|error| anyhow!("tile log {}: {error}", dir.display()))?
            .ok_or_else(|| {
                anyhow!(
                    "tile log {} has no inclusion proof for {index}",
                    dir.display()
                )
            })?;
        let at = usize::try_from(index).map_err(|_| anyhow!("index does not fit"))?;
        if !rfc9162::verify_inclusion(&rfc9162::leaf_hash(&entry), at, size, &proof, &root) {
            bail!(
                "tile log {} entry {index} is not in its tree of size {size} root {root}",
                dir.display()
            );
        }
        let record: Value = serde_json::from_slice(&entry)
            .with_context(|| format!("tile log {} entry {index} is not JSON", dir.display()))?;
        found.push(Found { index, record });
    }
    let stdout = if request.json {
        let mut text =
            serde_json::to_string_pretty(&found.iter().map(|f| &f.record).collect::<Vec<_>>())?;
        text.push('\n');
        text
    } else {
        let mut text = String::new();
        for hit in &found {
            text.push_str(&format!(
                "{}\t{}\n",
                hit.index,
                ledger::canonical(&hit.record)?
            ));
        }
        text.push_str(&format!(
            "{} of {} records match; checkpoint {origin} size {} root {checkpoint_root}\n",
            found.len(),
            published.size,
            published.size
        ));
        if unsigned > 0 {
            text.push_str(&format!(
                "{unsigned} more past the checkpoint, not shown; `ledger head` signs them\n"
            ));
        }
        text
    };
    Ok(FindReport {
        found,
        unsigned,
        checkpoint_size: published.size,
        checkpoint_root,
        stdout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(value: Value) -> Entry {
        ledger::seal_record(&value, 0, "2026-11-03T10:00:00Z").unwrap()
    }

    fn record() -> Value {
        json!({
            "ts": "2026-11-03T10:00:00Z",
            "actor": "jenkins/run/7",
            "stack": "2026.06",
            "target": "x86_64/amd/zen4",
            "module": "PETSc/3.25.0-foss-2026.1",
            "location": "/cvmfs/software.eessi.io/PETSc/3.25.0-foss-2026.1",
            "action": "install",
            "route": "kvm",
            "build": {"job": "snellius/4411", "log": "sha256:00"},
        })
    }

    fn keys(entry: &Entry) -> Vec<String> {
        keys_of(entry.line.as_bytes())
            .into_iter()
            .map(|key| String::from_utf8(key).unwrap())
            .collect()
    }

    #[test]
    fn keys_name_the_fields_a_record_carries() {
        let sealed = entry(record());
        let keys = keys(&sealed);
        for expected in [
            "module=PETSc/3.25.0-foss-2026.1",
            "pkg=PETSc",
            "target=x86_64/amd/zen4",
            "action=install",
            "route=kvm",
            "actor=jenkins/run/7",
            "job=snellius/4411",
        ] {
            assert!(
                keys.contains(&expected.to_string()),
                "{keys:?} lacks {expected}"
            );
        }
        assert!(keys.contains(&format!("id={}", sealed.id)), "{keys:?}");
        assert_eq!(keys.len(), 8);
    }

    #[test]
    fn absent_fields_give_no_keys() {
        let mut bare = record();
        let object = bare.as_object_mut().unwrap();
        object.remove("route");
        object.remove("build");
        let keys = keys(&entry(bare));
        assert!(!keys.iter().any(|k| k.starts_with("route=")), "{keys:?}");
        assert!(!keys.iter().any(|k| k.starts_with("job=")), "{keys:?}");
        assert_eq!(keys.len(), 6);
        assert!(keys_of(b"not json").is_empty());
        assert!(keys_of(b"[1,2]").is_empty());
    }

    #[test]
    fn a_route_outside_native_kvm_qemu_is_refused() {
        let mut wrong = record();
        wrong["route"] = json!("docker");
        assert!(ledger::seal_record(&wrong, 0, "").is_err());
        for route in ROUTES {
            let mut right = record();
            right["route"] = json!(route);
            ledger::seal_record(&right, 0, "").unwrap();
        }
    }

    #[test]
    fn the_origin_is_the_stack_and_the_first_year() {
        let sealed = entry(record());
        assert_eq!(
            origin(Some(&sealed), None).unwrap(),
            "eb-stack-ledger/2026.06/2026"
        );
        assert_eq!(
            origin(None, Some("example.org/log")).unwrap(),
            "example.org/log"
        );
        assert!(origin(None, None).is_err());
        assert!(origin(None, Some("has space")).is_err());
        assert!(origin(None, Some("a+b")).is_err());
    }

    #[test]
    fn the_default_tile_log_sits_beside_the_ledger() {
        assert_eq!(
            tiles_dir(Path::new("ledger/2026.06.jsonl"), None),
            PathBuf::from("ledger/2026.06.jsonl.tiles")
        );
        assert_eq!(
            tiles_dir(
                Path::new("ledger/2026.06.jsonl"),
                Some(Path::new("/var/tiles"))
            ),
            PathBuf::from("/var/tiles")
        );
    }
}
