//! Command bodies the CLI prints and the MCP tools return.
//!
//! Each function is the one implementation of that command. The CLI
//! prints the report. The MCP tool returns the same report, so the two
//! surfaces cannot drift.

use crate::build_order::{self, Choice};
use crate::cli_parse::usage;
use crate::easystack::{self, EasystackOptions};
use crate::eb_easyblock::{charmap_document, encode_class_name};
use crate::eb_engine_export::{hierarchy_table, resolve_templates, template_spec};
use crate::jenkins;
use crate::repro_report::{self, ReproRatchet};
use crate::{
    filter_deps, input_hashes, lock_to_easystack, parse_easyconfig_trees, write_text, ModuleKey,
    StackLock,
};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Default `--out` for `stack order`.
pub const STACK_ORDER_OUT: &str = "build-order.txt";

/// Default `--out` for `stack easystack`.
pub const STACK_EASYSTACK_OUT: &str = "stack.yml";

/// Default `--unfiled` for `stack sections`.
pub const STACK_SECTIONS_UNFILED: &str = "New in this generation";

/// Pretty JSON plus the trailing newline the CLI prints.
pub struct JsonReport {
    /// CLI stdout.
    pub stdout: String,
    /// The document `stdout` renders.
    pub document: Value,
}

fn render_json(document: Value) -> Result<JsonReport> {
    let stdout = format!("{}\n", serde_json::to_string_pretty(&document)?);
    Ok(JsonReport { stdout, document })
}

/// What `recipe easyblock` prints.
pub struct EasyblockReport {
    /// CLI stdout.
    pub stdout: String,
    /// Derived class name, when a software name was given and `--charmap` was not.
    pub class_name: Option<String>,
    /// Encoding table, when `--charmap` was set.
    pub charmap: Option<Value>,
}

/// Print the derived easyblock class, or the encoding table.
///
/// `--charmap` wins when both are set, which is what the CLI does.
pub fn recipe_easyblock(name: Option<&str>, charmap: bool) -> Result<EasyblockReport> {
    if charmap {
        let document = charmap_document();
        let rendered = render_json(document)?;
        return Ok(EasyblockReport {
            stdout: rendered.stdout,
            class_name: None,
            charmap: Some(rendered.document),
        });
    }
    let name = name.ok_or_else(|| usage("a software name, or --charmap"))?;
    let class_name = encode_class_name(name);
    Ok(EasyblockReport {
        stdout: format!("{class_name}\n"),
        class_name: Some(class_name),
        charmap: None,
    })
}

/// Print the template spec, or the values `name` and `version` resolve.
///
/// `--export` wins when a name is also present.
pub fn recipe_templates(
    name: Option<&str>,
    version: Option<&str>,
    export: bool,
) -> Result<JsonReport> {
    if export {
        return render_json(template_spec());
    }
    let name = name.ok_or_else(|| usage("--name, or --export"))?;
    let version = version.ok_or_else(|| usage("--version, or --export"))?;
    render_json(resolve_templates(name, version))
}

/// Print the known toolchain hierarchies.
///
/// `--export` is the spec a page embeds, the same role as
/// `recipe templates --export`. The plain command is the lookup:
/// fixture generations only. The GCC-family rule is synthetic, not one
/// of those generations, so it stays on the export.
pub fn recipe_hierarchy(export: bool) -> Result<JsonReport> {
    let table = hierarchy_table();
    let value = if export {
        table
    } else {
        serde_json::json!({
            "generations": table.get("generations").cloned().unwrap_or_default(),
        })
    };
    render_json(value)
}

/// Arguments for `stack order`.
pub struct StackOrderRequest {
    /// Easyconfig trees. Later trees win on one identity.
    pub easyconfigs: Vec<PathBuf>,
    /// Roots, each a package name or `name==version`.
    pub roots: Vec<String>,
    /// Build-order file to write.
    pub out: PathBuf,
    /// Take the oldest admissible version of an unpinned requirement.
    pub oldest: bool,
    /// Easystack to write in build order.
    pub easystack_out: Option<PathBuf>,
    /// Input-hash file to write.
    pub hashes_out: Option<PathBuf>,
    /// Dependencies to drop the way EasyBuild's `--filter-deps` drops them.
    ///
    /// Empty means read `EASYBUILD_FILTER_DEPS`.
    pub filter_deps: Vec<String>,
}

/// Hash file `stack order --hashes-out` wrote.
pub struct StackOrderHashes {
    /// Path of the hash file.
    pub path: PathBuf,
    /// Modules listed in the file.
    pub modules: usize,
    /// Modules whose recipe could not be read.
    pub incomplete: usize,
}

/// What `stack order` wrote and printed.
pub struct StackOrderReport {
    /// CLI stdout.
    pub stdout: String,
    /// Path of the order file.
    pub out: PathBuf,
    /// Builds in the order.
    pub builds: usize,
    /// Names with more than one build, in name order.
    pub multiple: Vec<(String, Vec<String>)>,
    /// Easystack path when one was written.
    pub easystack: Option<PathBuf>,
    /// Hash-file summary when one was written.
    pub hashes: Option<StackOrderHashes>,
}

/// Order the builds `roots` need and write the files the CLI writes.
pub fn stack_order(request: &StackOrderRequest) -> Result<StackOrderReport> {
    let trees: Vec<&Path> = request.easyconfigs.iter().map(PathBuf::as_path).collect();
    let parsed = parse_easyconfig_trees(&trees).map_err(|error| {
        let listed = request
            .easyconfigs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::anyhow!(error).context(format!("--easyconfigs {listed}"))
    })?;
    let choice = if request.oldest {
        Choice::Oldest
    } else {
        Choice::Newest
    };
    // Same filter `stack solve` applies: the flag, or
    // EASYBUILD_FILTER_DEPS when the flag is absent. Dropping the
    // dependency here keeps its own dependencies out of the order.
    let mut candidates = parsed.candidates;
    let specs = crate::dep_filter::specs_from_cli_or_env(&request.filter_deps);
    filter_deps(&mut candidates, &specs)?;
    let order = build_order::build_order(&candidates, &request.roots, choice)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    std::fs::write(&request.out, build_order::format_order(&order))?;
    let multi = build_order::multi_build_names(&order);
    let multiple: Vec<(String, Vec<String>)> = multi
        .iter()
        .map(|(name, builds)| (name.clone(), builds.clone()))
        .collect();

    let mut stdout = String::new();
    for (name, builds) in &multiple {
        stdout.push_str(&format!(
            "multiple builds of {name}: {}\n",
            builds.join(", ")
        ));
    }

    if let Some(path) = request.easystack_out.as_deref() {
        // Straight from the ordered list: a lock is sorted by name so
        // two locks diff cleanly, and writing that out would discard
        // the sequence this command exists to produce.
        let paths: Vec<&str> = order.iter().map(|c| c.easyconfig_path.as_str()).collect();
        std::fs::write(
            path,
            easystack::easystack_from_paths(&paths, &EasystackOptions::new()),
        )?;
        stdout.push_str(&format!("easystack={}\n", path.display()));
    }

    let hashes = if let Some(path) = request.hashes_out.as_deref() {
        let graph = build_order::build_graph(&candidates, &request.roots, choice)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        let sequence: Vec<ModuleKey> = order.iter().map(ModuleKey::of).collect();
        let recipe_paths: BTreeMap<ModuleKey, String> = order
            .iter()
            .map(|candidate| (ModuleKey::of(candidate), candidate.easyconfig_path.clone()))
            .collect();
        let hashed = input_hashes(&graph, &sequence, &recipe_paths);
        let incomplete = hashed.values().filter(|hash| !hash.complete).count();
        let mut text = String::new();
        for key in &sequence {
            if let Some(hash) = hashed.get(key) {
                text.push_str(&format!("{}  {}\n", hash.hash, key));
            }
        }
        std::fs::write(path, text)?;
        stdout.push_str(&format!(
            "hashes={} modules={} incomplete={}\n",
            path.display(),
            hashed.len(),
            incomplete
        ));
        Some(StackOrderHashes {
            path: path.to_path_buf(),
            modules: hashed.len(),
            incomplete,
        })
    } else {
        None
    };

    stdout.push_str(&format!(
        "order={} builds={}\n",
        request.out.display(),
        order.len()
    ));
    Ok(StackOrderReport {
        stdout,
        out: request.out.clone(),
        builds: order.len(),
        multiple,
        easystack: request.easystack_out.clone(),
        hashes,
    })
}

/// Arguments for `stack easystack`.
pub struct StackEasystackRequest {
    /// Stack lock JSON written by `stack solve`.
    pub lock: PathBuf,
    /// Easystack YAML to write.
    pub out: PathBuf,
    /// Per-easyconfig options, each `<file.eb>:<option>=<value>`.
    pub options: Vec<String>,
}

/// What `stack easystack` wrote and printed.
pub struct StackEasystackReport {
    /// CLI stdout.
    pub stdout: String,
    /// Path of the easystack.
    pub out: PathBuf,
    /// YAML entries written.
    pub entries: usize,
    /// Packages in the lock.
    pub packages: usize,
}

/// Read a stack lock. The path is named once, as `--lock`.
pub fn read_lock(path: &Path) -> Result<StackLock> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("--lock {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("--lock {}", path.display()))
}

/// Write the lock as an easystack, attaching each `--option`.
pub fn stack_easystack(request: &StackEasystackRequest) -> Result<StackEasystackReport> {
    let lock = read_lock(&request.lock)?;
    let mut parsed = EasystackOptions::new();
    for spec in &request.options {
        let (file, rest) = spec.split_once(':').ok_or_else(|| {
            usage(format!(
                "--option wants <file.eb>:<option>=<value>, got {spec}"
            ))
        })?;
        let (key, value) = rest.split_once('=').ok_or_else(|| {
            usage(format!(
                "--option wants <file.eb>:<option>=<value>, got {spec}"
            ))
        })?;
        parsed
            .entry(file.to_string())
            .or_default()
            .insert(key.to_string(), value.to_string());
    }
    let yaml = lock_to_easystack(&lock, &parsed);
    std::fs::write(&request.out, &yaml)?;
    let entries = yaml.lines().filter(|line| line.starts_with("- ")).count();
    let packages = lock.packages.len();
    Ok(StackEasystackReport {
        stdout: format!(
            "easystack={} entries={entries} of={packages}\n",
            request.out.display()
        ),
        out: request.out.clone(),
        entries,
        packages,
    })
}

/// Arguments for `stack sections`.
pub struct StackSectionsRequest {
    /// The build list to regroup.
    pub list: PathBuf,
    /// Previous build list whose headers decide where each entry goes.
    pub from: PathBuf,
    /// Header for entries no previous section names.
    pub unfiled: String,
    /// Grouped build list to write.
    pub out: PathBuf,
}

/// What `stack sections` wrote and printed.
pub struct StackSectionsReport {
    /// CLI stdout.
    pub stdout: String,
    /// Path of the grouped list.
    pub out: PathBuf,
    /// Section headers written, including the unfiled header.
    pub sections: usize,
    /// Previous sections plus the unfiled header.
    pub of: usize,
}

/// Regroup `list` under the section headers of `from`.
pub fn stack_sections(request: &StackSectionsRequest) -> Result<StackSectionsReport> {
    let previous = std::fs::read_to_string(&request.from)
        .with_context(|| format!("reading build list {}", request.from.display()))?;
    let text = std::fs::read_to_string(&request.list)
        .with_context(|| format!("reading build list {}", request.list.display()))?;
    let sections = jenkins::parse_sections(&previous);
    let grouped = jenkins::group_by_sections(&text, &sections, &request.unfiled);
    write_text(&request.out, &grouped)?;
    let headers = grouped
        .lines()
        .filter(|line| line.starts_with("#### "))
        .count();
    let of = sections.len() + 1;
    Ok(StackSectionsReport {
        stdout: format!(
            "list={} sections={headers} of={of}\n",
            request.out.display()
        ),
        out: request.out.clone(),
        sections: headers,
        of,
    })
}

/// Arguments for `repro summary`.
pub struct ReproSummaryRequest {
    /// Directory of per-case score artifacts.
    pub scores: PathBuf,
    /// Committed allowance counts. Absent, the scoreboard is only printed.
    pub ratchet: Option<PathBuf>,
    /// Print the collected scores as JSON instead of a table.
    pub json: bool,
}

/// What `repro summary` prints.
///
/// `body` is the table or the JSON. `ratchet_line` is set when the run
/// holds. `violations` is set when it does not. The CLI prints `body`,
/// then either the ratchet line or the violations.
pub struct ReproSummaryReport {
    /// Table or JSON, including its trailing newline.
    pub body: String,
    /// `ratchet: N cases hold at T` plus a newline, when the run holds.
    pub ratchet_line: Option<String>,
    /// Violation text, when the run does not hold.
    pub violations: Vec<String>,
}

impl ReproSummaryReport {
    /// Stdout for a run that holds. A run with violations prints `body`
    /// and then the violations on stderr.
    pub fn stdout(&self) -> String {
        let mut out = self.body.clone();
        if let Some(line) = &self.ratchet_line {
            out.push_str(line);
        }
        out
    }
}

/// Read the score directory and, when asked, check the ratchet.
pub fn repro_summary(request: &ReproSummaryRequest) -> Result<ReproSummaryReport> {
    let collected = repro_report::read_case_scores(&request.scores)
        .with_context(|| format!("read repro scores from {}", request.scores.display()))?;
    if collected.is_empty() {
        bail!(
            "no score artifacts in {}; run the reproduction suite with EB_REPRO_SCORES set to that directory",
            request.scores.display()
        );
    }
    let body = if request.json {
        format!("{}\n", serde_json::to_string_pretty(&collected)?)
    } else {
        repro_report::render_scoreboard_table(&collected)
    };
    let Some(ratchet_path) = request.ratchet.as_deref() else {
        return Ok(ReproSummaryReport {
            body,
            ratchet_line: None,
            violations: Vec::new(),
        });
    };
    let ratchet = ReproRatchet::read(ratchet_path)
        .with_context(|| format!("read ratchet {}", ratchet_path.display()))?;
    let found = repro_report::check_ratchet(&ratchet, &collected);
    if found.is_empty() {
        return Ok(ReproSummaryReport {
            body,
            ratchet_line: Some(format!(
                "ratchet: {} cases hold at {}\n",
                collected.len(),
                ratchet.total
            )),
            violations: Vec::new(),
        });
    }
    Ok(ReproSummaryReport {
        body,
        ratchet_line: None,
        violations: found.iter().map(ToString::to_string).collect(),
    })
}
