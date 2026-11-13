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
//!
//! The tile log holds exactly the ledger's lines, so its root is the
//! ledger's. The sorted runs a lookup goes through, and the manifests that
//! describe them, live in a second tile log inside it, the index log at
//! `<tiles>/index` (see [`index_dir`]), with its own checkpoint under
//! `<origin>/index` signed by the same key. Each logged manifest carries the
//! size and root of the tile log it indexed. A find proves a record in the
//! tile log's checkpoint and the run that located it in the index log's.

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use ed25519_dalek::{SigningKey, VerifyingKey};
use rfc9162_log::store::Index;
use rfc9162_log::{AuthenticatedLog, Keys, Store};
use rfc9162_tiles::checkpoint::{self, ED25519_LOG};
use rfc9162_tiles::index::KeyFn;
use rfc9162_tiles::layout::{CHECKPOINT, TILE_WIDTH};
use rfc9162_tiles::{Checkpoint, Note, PosixStore, Reader, Signer, TileStore, Verifier};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
pub fn key_fn() -> KeyFn {
    Box::new(keys_of)
}

/// The same keys for the run store.
pub fn keys_arc() -> Keys {
    Arc::new(keys_of)
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
    /// Sorted runs a lookup goes through after the sync.
    pub runs: u64,
    /// Whether this sync sealed the unsealed tail into a run.
    pub sealed: bool,
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
    /// The index log's checkpoint, signed with the same key under
    /// `<origin>/index`, once a run has been sealed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<Box<CheckpointReport>>,
}

impl CheckpointReport {
    /// One line for stdout, two once the index log has a checkpoint.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut text = format!(
            "checkpoint {} size {} root {} to {}",
            self.origin,
            self.size,
            self.root,
            self.path.display()
        );
        if let Some(index) = &self.index {
            text.push_str(&format!(
                "\nindex checkpoint {} size {} root {} to {}",
                index.origin,
                index.size,
                index.root,
                index.path.display()
            ));
        }
        text
    }
}

fn open_store(dir: &Path) -> Result<PosixStore> {
    PosixStore::open(dir).with_context(|| format!("open tile log {}", dir.display()))
}

/// The index log inside the tile log at `dir`: the run files and the logged
/// run manifests, with their own checkpoint. It lives under the tile log so
/// that moving or copying the tile log takes its index along.
#[must_use]
pub fn index_dir(dir: &Path) -> PathBuf {
    dir.join("index")
}

/// The origin the index log's checkpoint is signed under.
#[must_use]
pub fn index_origin(origin: &str) -> String {
    format!("{origin}/index")
}

/// The split store over the tile log at `dir` and its index log, holding
/// both writer locks.
fn open_split(dir: &Path) -> Result<Store<PosixStore>> {
    Store::open_split(open_store(dir)?, open_store(&index_dir(dir))?, keys_arc())
        .map_err(|error| anyhow!("open tile log {}: {error}", dir.display()))
}

fn stale_path(dir: &Path) -> PathBuf {
    let mut name = dir.as_os_str().to_owned();
    name.push(format!(".stale-{}", Utc::now().format("%Y%m%dT%H%M%SZ")));
    PathBuf::from(name)
}

/// The store over a tile log that holds exactly the ledger's lines, and
/// what it took to get there. The lines are appended as one batch; when the
/// unsealed tail then holds a full bundle of entries it is sealed into a
/// sorted run whose manifest is logged in the index log, so the next head
/// signs the index as well as the entries.
fn open_synced(entries: &[Entry], dir: &Path) -> Result<(Store<PosixStore>, SyncReport)> {
    let leaves = ledger::leaves(entries);
    let mut store = open_split(dir)?;
    let held = usize::try_from(store.size()).unwrap_or(usize::MAX);
    let agrees = held <= leaves.len() && store.root() == rfc9162::root(&leaves[..held]);
    let mut rebuilt_from = None;
    if !agrees {
        drop(store);
        let stale = stale_path(dir);
        std::fs::rename(dir, &stale)
            .with_context(|| format!("move tile log {} to {}", dir.display(), stale.display()))?;
        // The index log describes the tree that was just moved aside, and it
        // sits inside that directory, so it went with it.
        store = open_split(dir)?;
        rebuilt_from = Some(stale);
    }
    let start =
        usize::try_from(store.size()).expect("a synced tile log is not longer than the ledger");
    let appended = store
        .append_batch(
            entries[start..]
                .iter()
                .map(|entry| entry.line.clone().into_bytes()),
        )
        .map_err(|error| anyhow!("append to tile log {}: {error}", dir.display()))?;
    let sealed_through = store
        .live_runs()
        .last()
        .map_or(0, |run| run.entries.end() + 1);
    let mut sealed = false;
    if store.size().saturating_sub(sealed_through) >= TILE_WIDTH {
        store
            .seal()
            .map_err(|error| anyhow!("seal tile log {}: {error}", dir.display()))?;
        sealed = true;
    }
    let report = SyncReport {
        dir: dir.to_path_buf(),
        size: store.size(),
        root: rfc9162::hex(&store.root()),
        appended: appended.end - appended.start,
        rebuilt_from,
        runs: store.live_runs().len() as u64,
        sealed,
    };
    Ok((store, report))
}

/// Bring the tile log at `dir` to the ledger's lines: append what it lacks,
/// or move it aside and rebuild it when its entries disagree.
pub fn sync(entries: &[Entry], dir: &Path) -> Result<SyncReport> {
    let _lock = ledger::tile_writer_lock(dir)?;
    let (_store, report) = open_synced(entries, dir)?;
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
    let (store, synced) = open_synced(entries, dir)?;
    let signer = Signer::new(origin, key.clone())
        .map_err(|error| anyhow!("checkpoint signer for {origin}: {error}"))?;
    let note = store
        .writer()
        .checkpoint(&signer, origin)
        .map_err(|error| anyhow!("checkpoint tile log {}: {error}", dir.display()))?;
    let published = Checkpoint::parse(&note.text)
        .map_err(|error| anyhow!("checkpoint tile log {}: {error}", dir.display()))?;
    // The index log gets its own checkpoint under <origin>/index once it
    // holds a manifest; an empty index log is not signed.
    let index = match store.index_writer() {
        Some(writer) if writer.size() > 0 => {
            let index_origin = index_origin(origin);
            let index_dir = index_dir(dir);
            let signer = Signer::new(&index_origin, key.clone())
                .map_err(|error| anyhow!("checkpoint signer for {index_origin}: {error}"))?;
            let note = writer.checkpoint(&signer, &index_origin).map_err(|error| {
                anyhow!("checkpoint index log {}: {error}", index_dir.display())
            })?;
            let published = Checkpoint::parse(&note.text).map_err(|error| {
                anyhow!("checkpoint index log {}: {error}", index_dir.display())
            })?;
            Some(Box::new(CheckpointReport {
                origin: published.origin,
                size: published.size,
                root: rfc9162::hex(&published.root),
                path: index_dir.join(CHECKPOINT),
                note: note.to_text(),
                index: None,
            }))
        }
        _ => None,
    };
    Ok((
        synced,
        CheckpointReport {
            origin: published.origin,
            size: published.size,
            root: rfc9162::hex(&published.root),
            path: dir.join(CHECKPOINT),
            note: note.to_text(),
            index,
        },
    ))
}

/// The index log's signed state: its checkpoint under `<origin>/index` when
/// it has one and the index log extends it, else the empty tree. A tile log
/// with no sealed run has no index checkpoint, and a find then goes through
/// the unsealed tail alone.
fn read_index_checkpoint(
    dir: &Path,
    origin: &str,
    trusted: &VerifyingKey,
) -> Result<(PosixStore, u64, [u8; 32])> {
    let index_dir = index_dir(dir);
    let store = open_store(&index_dir)?;
    if !store.exists(CHECKPOINT)? {
        return Ok((store, 0, rfc9162::root(&[])));
    }
    let index_origin = index_origin(origin);
    let published = read_checkpoint(&store, &index_dir, &index_origin, trusted)?;
    let reader = Reader::open(store.clone(), key_fn())
        .map_err(|error| anyhow!("open index log {}: {error}", index_dir.display()))?;
    check_prefix(
        &reader,
        &index_dir,
        published.size,
        &rfc9162::hex(&published.root),
        "index checkpoint",
    )?;
    Ok((store, published.size, published.root))
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
    if index_dir(dir).is_dir() {
        read_index_checkpoint(dir, origin, trusted)?;
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
    /// Leaf number, in the index log, of the run manifest the lookup went
    /// through, or none when the record sits in the unsealed tail and was
    /// found by scan.
    pub run: Option<u64>,
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
    let reader = Reader::open(store.clone(), key_fn())
        .map_err(|error| anyhow!("open tile log {}: {error}", dir.display()))?;
    // The hash tiles have to fold to the committed root before any entry is
    // read through them, so a tampered tile is named as such.
    reader
        .hashes()
        .root()
        .map_err(|error| anyhow!("tile log {}: {error}", dir.display()))?;
    let checkpoint_root = rfc9162::hex(&published.root);
    check_prefix(
        &reader,
        &dir,
        published.size,
        &checkpoint_root,
        "checkpoint",
    )?;

    // Lookups at the checkpoints: every answer carries inclusion paths
    // against the signed sizes, the record's against the tile log and the run
    // manifest's against the index log, so both are proven with checkpoint
    // roots and nothing from the store is trusted.
    let (index_store, index_size, index_root) = read_index_checkpoint(&dir, &origin, &trusted)?;
    let index_checkpoint_root = rfc9162::hex(&index_root);
    let cache = rfc9162_log::store::RunCache::default();
    let signed = Index::at_split(
        store.clone(),
        published.size,
        published.root,
        index_store.clone(),
        index_size,
        index_root,
        keys_arc(),
        Arc::clone(&cache),
    )
    .map_err(|error| anyhow!("tile log {} index: {error}", dir.display()))?;
    let index_reader = Reader::open(index_store.clone(), key_fn())
        .map_err(|error| anyhow!("open index log {}: {error}", index_dir(&dir).display()))?;
    let current = Index::at_split(
        store,
        reader.size(),
        reader.root(),
        index_store,
        index_reader.size(),
        index_reader.root(),
        keys_arc(),
        cache,
    )
    .map_err(|error| anyhow!("tile log {} index: {error}", dir.display()))?;
    let mut hits: Option<BTreeMap<u64, rfc9162_log::Located>> = None;
    let mut past: Option<BTreeSet<u64>> = None;
    for key in &request.keys {
        let located = signed
            .find(key.as_bytes())
            .map_err(|error| anyhow!("tile log {}: {error}", dir.display()))?
            .into_iter()
            .map(|hit| (hit.entry.index, hit))
            .collect::<BTreeMap<_, _>>();
        hits = Some(match hits {
            None => located,
            Some(sofar) => sofar
                .into_iter()
                .filter(|(index, _)| located.contains_key(index))
                .collect(),
        });
        let beyond = current
            .find(key.as_bytes())
            .map_err(|error| anyhow!("tile log {}: {error}", dir.display()))?
            .into_iter()
            .map(|hit| hit.entry.index)
            .filter(|index| *index >= published.size)
            .collect::<BTreeSet<_>>();
        past = Some(match past {
            None => beyond,
            Some(sofar) => sofar.intersection(&beyond).copied().collect(),
        });
    }
    let size = usize::try_from(published.size).map_err(|_| anyhow!("size does not fit"))?;
    let mut found = Vec::new();
    for (index, hit) in hits.unwrap_or_default() {
        let entry = signed
            .reader()
            .get(index)
            .map_err(|error| anyhow!("tile log {}: {error}", dir.display()))?
            .ok_or_else(|| anyhow!("tile log {} has no entry {index}", dir.display()))?;
        let at = usize::try_from(index).map_err(|_| anyhow!("index does not fit"))?;
        if !rfc9162::verify_inclusion(
            &rfc9162::leaf_hash(&entry),
            at,
            size,
            &hit.entry.path,
            &checkpoint_root,
        ) {
            bail!(
                "tile log {} entry {index} is not in the checkpoint of size {size} root {checkpoint_root}",
                dir.display()
            );
        }
        let run = match hit.run {
            Some(run) => {
                let manifest = signed
                    .manifest_reader()
                    .get(run.index)
                    .map_err(|error| anyhow!("index log {}: {error}", index_dir(&dir).display()))?
                    .ok_or_else(|| {
                        anyhow!(
                            "index log {} has no run manifest {}",
                            index_dir(&dir).display(),
                            run.index
                        )
                    })?;
                let at = usize::try_from(run.index).map_err(|_| anyhow!("index does not fit"))?;
                let index_size =
                    usize::try_from(index_size).map_err(|_| anyhow!("size does not fit"))?;
                if !rfc9162::verify_inclusion(
                    &rfc9162::leaf_hash(&manifest),
                    at,
                    index_size,
                    &run.path,
                    &index_checkpoint_root,
                ) {
                    bail!(
                        "index log {} run manifest {} is not in the index checkpoint of size {index_size}",
                        index_dir(&dir).display(),
                        run.index
                    );
                }
                Some(run.index)
            }
            None => None,
        };
        let record: Value = serde_json::from_slice(&entry)
            .with_context(|| format!("tile log {} entry {index} is not JSON", dir.display()))?;
        found.push(Found { index, record, run });
    }
    let unsigned = past.unwrap_or_default().len() as u64;
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
            "build": {"job": "site/4411", "log": "sha256:00"},
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
            "job=site/4411",
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
    fn a_full_bundle_seals_a_ledger_run_into_the_index_log() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("2026.06.jsonl.tiles");
        let entries: Vec<Entry> = (0..300)
            .map(|i| {
                let mut value = record();
                value["module"] = json!(format!("Mod{i}/1.0"));
                value["target"] = json!(if i % 2 == 0 { "zen2" } else { "zen4" });
                ledger::seal_record(&value, i, "2026-11-08T10:00:00Z").unwrap()
            })
            .collect();
        let (store, report) = open_synced(&entries, &dir).unwrap();
        assert_eq!(report.size, 300, "the tile log holds the lines alone");
        assert!(report.sealed && report.runs == 1, "{report:?}");
        assert_eq!(
            store.root(),
            rfc9162::root(&ledger::leaves(&entries)),
            "the tile log root is the ledger root"
        );
        let index = store.index_writer().expect("split store");
        assert_eq!(index.size(), 1, "one manifest in the index log");
        assert!(
            index_dir(&dir).is_dir(),
            "the index log sits inside the tile log"
        );
        let manifest = Reader::open(index.store().clone(), key_fn())
            .unwrap()
            .get(0)
            .unwrap()
            .expect("manifest entry");
        let logged = rfc9162_log::store::Logged::decode(&manifest, 0, false).unwrap();
        assert_eq!(logged.data_size, 300);
        assert_eq!(logged.data_root, store.root());
        assert_eq!(*logged.run.entries.end(), 299, "the run covers every entry");
        let hits = store.find(b"target=zen4").unwrap();
        assert_eq!(hits.len(), 150);
        assert!(
            hits.iter()
                .all(|h| h.run.as_ref().is_some_and(|r| r.tree_size == 1)),
            "every hit went through the run, proven against the index log"
        );
        drop(store);
        // the same lines again: nothing appended, nothing resealed
        let (_store, again) = open_synced(&entries, &dir).unwrap();
        assert_eq!((again.appended, again.sealed, again.runs), (0, false, 1));
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
