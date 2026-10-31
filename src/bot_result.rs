//! What an EESSI bot build job's FAILURE actually was.
//!
//! The EESSI software-layer bot marks a job FAILURE when the job output
//! (`slurm-<id>.out`) holds any line matching `FATAL:`, `ERROR:`,
//! `FAILED:` or `required modules missing:`, or lacks
//! `No missing installations` or a `<tarball> created!` line
//! (`bot/check-build.sh` in EESSI/software-layer-scripts). The test step is
//! marked FAILURE on any `ERROR:` line. Many of those jobs built and
//! packaged everything. This module reads what a job left behind (the bot's
//! comment, the `_bot_job<id>.result` file, the job output) and says which
//! of these it was:
//!
//! - a real failure: a build or a check failed and installations remain
//!   missing, with the cause from [`crate::campaign::classify_build_failure`];
//! - ERROR-only noise: `ERROR:` lines, but `No missing installations` and a
//!   tarball with contents;
//! - an empty tarball: nothing was built, so there is nothing to deploy;
//! - infra: the job never ran the build properly (bot configuration, merge,
//!   scheduler, container, network, missing job output).
//!
//! Anything else stays unclassified, for a human.

use crate::campaign::{classify_build_failure, BuildFindingClass};
use crate::cli_parse::usage;
use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::LazyLock;

/// Which of the four outcomes a bot job had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BotJobClass {
    /// The build or a check failed; installations remain missing.
    RealFailure,
    /// `ERROR:` lines, but every installation is present and the tarball
    /// has contents.
    ErrorOnly,
    /// Nothing was built, so the tarball is empty.
    EmptyTarball,
    /// The job never ran the build properly.
    Infra,
    /// The bot reported success and nothing contradicts it.
    Success,
    /// The inputs match no known shape. Left for a human rather than guessed.
    Unclassified,
}

impl BotJobClass {
    /// Every class, in report order.
    pub const ALL: [BotJobClass; 6] = [
        BotJobClass::RealFailure,
        BotJobClass::ErrorOnly,
        BotJobClass::EmptyTarball,
        BotJobClass::Infra,
        BotJobClass::Success,
        BotJobClass::Unclassified,
    ];

    /// The name the text and JSON output use, `real-failure`.
    pub fn as_str(self) -> &'static str {
        match self {
            BotJobClass::RealFailure => "real-failure",
            BotJobClass::ErrorOnly => "error-only",
            BotJobClass::EmptyTarball => "empty-tarball",
            BotJobClass::Infra => "infra",
            BotJobClass::Success => "success",
            BotJobClass::Unclassified => "unclassified",
        }
    }
}

/// Why a job never ran the build properly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InfraCause {
    /// The bot instance submitted no job for the command, or the job
    /// configuration is missing or names an unknown repository.
    BotConfig,
    /// The bot could not merge the pull request into its target branch.
    Merge,
    /// The job output file is missing.
    MissingOutput,
    /// The scheduler refused the job or lost its node.
    Scheduler,
    /// The build container could not be created.
    Container,
    /// CernVM-FS or the network was unavailable.
    Network,
    /// A tool from the compatibility layer, not the build environment, ran.
    CompatLayer,
}

impl InfraCause {
    fn as_str(self) -> &'static str {
        match self {
            InfraCause::BotConfig => "bot-config",
            InfraCause::Merge => "merge",
            InfraCause::MissingOutput => "missing-output",
            InfraCause::Scheduler => "scheduler",
            InfraCause::Container => "container",
            InfraCause::Network => "network",
            InfraCause::CompatLayer => "compat-layer",
        }
    }
}

/// The sub-class of a real failure or of infra.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum BotSubClass {
    /// What [`classify_build_failure`] makes of the job output.
    Build(BuildFindingClass),
    /// What kept the build from running.
    Infra(InfraCause),
}

impl BotSubClass {
    /// The kebab-case name, `resource` or `bot-config`.
    pub fn as_str(self) -> String {
        match self {
            BotSubClass::Build(class) => serde_json::to_value(class)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| format!("{class:?}").to_ascii_lowercase()),
            BotSubClass::Infra(cause) => cause.as_str().to_string(),
        }
    }
}

/// One input text: a saved bot comment, a `_bot_job<id>.result` file, or
/// a job output file.
#[derive(Debug, Clone)]
pub struct BotInput {
    /// Where the text came from. A file name such as `slurm-327885.out`
    /// or `_bot_job327885.result` gives the job id.
    pub name: String,
    /// The text.
    pub text: String,
}

/// The classification of one bot job.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BotJobResult {
    /// Inputs that describe this job.
    pub sources: Vec<String>,
    /// Slurm job id, when an input names it.
    pub job_id: Option<String>,
    /// Bot instance, `eessi-bot-mc-aws`.
    pub instance: Option<String>,
    /// Bot repository, `eessi.io-2025.06-software`.
    pub repository: Option<String>,
    /// CPU target the job built for, `x86_64/amd/zen2`.
    pub target: Option<String>,
    /// The outcome.
    pub class: BotJobClass,
    /// The cause of a real failure or of infra. A real failure whose
    /// cause is not in the inputs is `unknown`.
    pub sub_class: Option<BotSubClass>,
    /// One sentence on why the job got its class.
    pub reason: String,
    /// The lines the class rests on.
    pub evidence: Vec<String>,
}

/// Arguments for `stack bot-classify`.
pub struct BotClassifyRequest {
    /// Saved bot comments, `_bot_job<id>.result` files, and job output
    /// files. Inputs that name the same job id are read as one job.
    pub files: Vec<PathBuf>,
}

/// What `stack bot-classify` prints.
pub struct BotClassifyReport {
    /// Stdout, including the count line.
    pub stdout: String,
    /// One result per job.
    pub jobs: Vec<BotJobResult>,
}

impl BotClassifyReport {
    /// Jobs per class, every class present.
    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BotJobClass::ALL
            .iter()
            .map(|class| (class.as_str(), 0))
            .collect::<BTreeMap<_, _>>();
        for job in &self.jobs {
            *counts.entry(job.class.as_str()).or_default() += 1;
        }
        counts
    }

    fn count_line(&self) -> String {
        let counts = self.counts();
        let mut fields = BotJobClass::ALL
            .iter()
            .map(|class| format!("{}={}", class.as_str(), counts[class.as_str()]))
            .collect::<Vec<_>>();
        fields.push(format!("jobs={}", self.jobs.len()));
        fields.join(" ")
    }
}

/// Read the files and classify each job they describe.
pub fn run_bot_classify(request: &BotClassifyRequest) -> Result<BotClassifyReport> {
    if request.files.is_empty() {
        return Err(usage("pass at least one bot comment, result, or log file"));
    }
    let mut inputs = Vec::new();
    for path in &request.files {
        if path.is_dir() {
            return Err(usage(format!(
                "{} is a directory; pass the comment, result, or log file",
                path.display()
            )));
        }
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        inputs.push(BotInput {
            name: path.display().to_string(),
            text: String::from_utf8_lossy(&bytes).into_owned(),
        });
    }
    let jobs = classify_bot_inputs(&inputs);
    let mut report = BotClassifyReport {
        stdout: jobs.iter().map(render_job).collect(),
        jobs,
    };
    let count_line = report.count_line();
    report.stdout.push_str(&count_line);
    report.stdout.push('\n');
    Ok(report)
}

fn render_job(job: &BotJobResult) -> String {
    let mut head = job.class.as_str().to_string();
    if let Some(sub) = job.sub_class {
        head.push('/');
        head.push_str(&sub.as_str());
    }
    if let Some(target) = &job.target {
        head.push(' ');
        head.push_str(target);
    }
    if let Some(id) = &job.job_id {
        head.push_str(&format!(" job {id}"));
    }
    if let Some(instance) = &job.instance {
        head.push_str(&format!(" ({instance})"));
    }
    let mut out = format!("{head}: {}\n", job.sources.join(", "));
    out.push_str(&format!("  why: {}\n", job.reason));
    for line in &job.evidence {
        out.push_str(&format!("  evidence: {line}\n"));
    }
    out
}

/// Classify every job the inputs describe.
///
/// A comment holding several job tables, or a bot update with several
/// commands, gives one job each. Inputs that name the same job id are one
/// job: the comment or result file says what the bot checked, and the job
/// output adds the lines that say why.
pub fn classify_bot_inputs(inputs: &[BotInput]) -> Vec<BotJobResult> {
    let mut jobs: Vec<Facts> = Vec::new();
    for input in inputs {
        for facts in parse_input(input) {
            let same = facts.job_id.as_ref().and_then(|id| {
                jobs.iter()
                    .position(|job| job.job_id.as_deref() == Some(id.as_str()))
            });
            match same {
                Some(index) => jobs[index].merge(facts),
                None => jobs.push(facts),
            }
        }
    }
    jobs.into_iter().map(Facts::classify).collect()
}

/// Classify the inputs as one job.
pub fn classify_bot_job(inputs: &[BotInput]) -> BotJobResult {
    let mut facts = Facts::default();
    for input in inputs {
        for parsed in parse_input(input) {
            facts.merge(parsed);
        }
    }
    facts.classify()
}

/// What one bot check row (`finished` or `test result`) says.
#[derive(Debug, Clone, Default)]
struct Row {
    success: Option<bool>,
    reason: Option<String>,
    output_found: Option<bool>,
    fatal: Option<bool>,
    error: Option<bool>,
    failed: Option<bool>,
    req_missing: Option<bool>,
    no_missing: Option<bool>,
    tarball_created: Option<bool>,
    reframe_failed: Option<bool>,
    entries: Option<u64>,
    no_artefacts: bool,
    tarball: Option<String>,
    failed_items: Vec<String>,
}

/// Everything the inputs say about one job.
#[derive(Debug, Clone, Default)]
struct Facts {
    sources: Vec<String>,
    job_id: Option<String>,
    instance: Option<String>,
    repository: Option<String>,
    target: Option<String>,
    build: Option<Row>,
    test: Option<Row>,
    log: Option<LogFacts>,
    no_jobs: Option<String>,
    merge_failed: Vec<String>,
}

/// What the job output says, by the patterns `bot/check-build.sh` greps.
#[derive(Debug, Clone, Default)]
struct LogFacts {
    row: Row,
    infra: Option<(InfraCause, String)>,
    failure: Option<BuildFindingClass>,
    evidence: Vec<String>,
    empty_tarball: bool,
}

impl Facts {
    fn merge(&mut self, other: Facts) {
        self.sources.extend(other.sources);
        self.job_id = self.job_id.take().or(other.job_id);
        self.instance = self.instance.take().or(other.instance);
        self.repository = self.repository.take().or(other.repository);
        self.target = self.target.take().or(other.target);
        self.build = self.build.take().or(other.build);
        self.test = self.test.take().or(other.test);
        self.log = self.log.take().or(other.log);
        self.no_jobs = self.no_jobs.take().or(other.no_jobs);
        self.merge_failed.extend(other.merge_failed);
    }

    fn result(
        &self,
        class: BotJobClass,
        sub_class: Option<BotSubClass>,
        reason: impl Into<String>,
        evidence: Vec<String>,
    ) -> BotJobResult {
        let target = self.target.clone().or_else(|| {
            [self.build.as_ref(), self.log.as_ref().map(|log| &log.row)]
                .into_iter()
                .flatten()
                .find_map(|row| row.tarball.as_deref().and_then(target_from_tarball))
        });
        BotJobResult {
            sources: self.sources.clone(),
            job_id: self.job_id.clone(),
            instance: self.instance.clone(),
            repository: self.repository.clone(),
            target,
            class,
            sub_class,
            reason: reason.into(),
            evidence,
        }
    }

    /// The bot's build checks, from its comment or result file when there
    /// is one, else from the job output.
    fn build_row(&self) -> Option<&Row> {
        self.build
            .as_ref()
            .or_else(|| self.log.as_ref().map(|log| &log.row))
    }

    fn classify(self) -> BotJobResult {
        use BotJobClass::*;
        if let Some(line) = &self.no_jobs {
            return self.result(
                Infra,
                Some(BotSubClass::Infra(InfraCause::BotConfig)),
                "the bot instance submitted no job for the build command",
                vec![line.clone(), "no jobs were submitted".into()],
            );
        }
        if !self.merge_failed.is_empty() {
            return self.result(
                Infra,
                Some(BotSubClass::Infra(InfraCause::Merge)),
                "the bot could not merge the pull request, so no job ran",
                self.merge_failed.clone(),
            );
        }
        if let Some((cause, line)) = self.log.as_ref().and_then(|log| log.infra.clone()) {
            return self.result(
                Infra,
                Some(BotSubClass::Infra(cause)),
                "the job output shows the build environment itself failed",
                vec![line],
            );
        }
        let rows = [self.build.as_ref(), self.test.as_ref()];
        if let Some(row) = rows
            .iter()
            .flatten()
            .find(|row| row.output_found == Some(false))
        {
            return self.result(
                Infra,
                Some(BotSubClass::Infra(InfraCause::MissingOutput)),
                "the bot found no job output file",
                row.failed_items.clone(),
            );
        }
        let log_failure = self.log.as_ref().and_then(|log| log.failure);
        let mut log_evidence = self
            .log
            .as_ref()
            .map(|log| log.evidence.clone())
            .unwrap_or_default();
        match log_failure {
            Some(BuildFindingClass::Transport) => {
                return self.result(
                    Infra,
                    Some(BotSubClass::Infra(InfraCause::Network)),
                    "the job output shows a connection failure",
                    log_evidence,
                )
            }
            Some(BuildFindingClass::Executor) => {
                return self.result(
                    Infra,
                    Some(BotSubClass::Infra(InfraCause::Scheduler)),
                    "the job output shows the scheduler refused the job",
                    log_evidence,
                )
            }
            _ => {}
        }
        let failure = || {
            Some(BotSubClass::Build(
                log_failure.unwrap_or(BuildFindingClass::Unknown),
            ))
        };
        let Some(build) = self.build_row() else {
            if let Some(test) = &self.test {
                return self.classify_test_only(test);
            }
            return self.result(
                Unclassified,
                None,
                "no bot check result or job output was found in the inputs",
                Vec::new(),
            );
        };
        let mut evidence = build.failed_items.clone();
        let hard = [
            (build.fatal, "FATAL: "),
            (build.failed, "FAILED: "),
            (build.req_missing, " required modules missing:"),
        ]
        .into_iter()
        .filter(|(found, _)| *found == Some(true))
        .map(|(_, pattern)| format!("`{}`", pattern.trim()))
        .collect::<Vec<_>>();
        if !hard.is_empty() {
            evidence.append(&mut log_evidence);
            return self.result(
                RealFailure,
                failure(),
                format!("the job output matched {}", hard.join(", ")),
                evidence,
            );
        }
        let test_failures = self.test.as_ref().is_some_and(|test| {
            test.reframe_failed == Some(true)
                || test
                    .reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("produced failures"))
        });
        if test_failures {
            let test = self.test.as_ref().expect("test row");
            let mut evidence = test.failed_items.clone();
            evidence.extend(
                test.reason
                    .iter()
                    .map(|reason| format!("test result: {reason}")),
            );
            return self.result(
                RealFailure,
                Some(BotSubClass::Build(BuildFindingClass::Test)),
                "the EESSI test suite reported failing test cases",
                evidence,
            );
        }
        let empty = build.entries == Some(0) || self.log.as_ref().is_some_and(|l| l.empty_tarball);
        let error = build.error == Some(true);
        let test_error = self
            .test
            .as_ref()
            .is_some_and(|test| test.error == Some(true));
        if build.no_missing == Some(true) && build.tarball_created == Some(true) {
            evidence.push("found `No missing installations`".into());
            evidence.push(tarball_line(build));
            if empty {
                return self.result(
                    EmptyTarball,
                    None,
                    "No missing installations and the tarball has no entries: everything was already installed",
                    evidence,
                );
            }
            if error || test_error {
                if let Some(test) = self.test.as_ref().filter(|_| test_error) {
                    evidence.extend(
                        test.failed_items
                            .iter()
                            .map(|item| format!("test result: {item}")),
                    );
                    evidence.extend(
                        test.reason
                            .iter()
                            .map(|reason| format!("test result: {reason}")),
                    );
                }
                evidence.append(&mut log_evidence);
                return self.result(
                    ErrorOnly,
                    None,
                    "`ERROR: ` matched, but No missing installations and the tarball was created",
                    evidence,
                );
            }
            let test_ok = self
                .test
                .as_ref()
                .is_none_or(|test| test.success != Some(false));
            if build.success != Some(false) && test_ok {
                return self.result(
                    Success,
                    None,
                    "No missing installations and the tarball was created",
                    evidence,
                );
            }
            if let Some(test) = &self.test {
                evidence.extend(
                    test.reason
                        .iter()
                        .map(|reason| format!("test result: {reason}")),
                );
            }
            return self.result(
                Unclassified,
                None,
                "the build checks passed, but the bot reported a failure they do not explain",
                evidence,
            );
        }
        if build.tarball_created == Some(true) && empty && !error {
            evidence.push(tarball_line(build));
            return self.result(
                EmptyTarball,
                None,
                "the tarball has no entries and no check found an error: nothing was built",
                evidence,
            );
        }
        if error && build.no_missing != Some(true) {
            evidence.append(&mut log_evidence);
            return self.result(
                RealFailure,
                failure(),
                "`ERROR: ` matched and the job never reported No missing installations",
                evidence,
            );
        }
        if let Some(class) = log_failure.filter(|class| *class != BuildFindingClass::Unknown) {
            evidence.append(&mut log_evidence);
            return self.result(
                RealFailure,
                Some(BotSubClass::Build(class)),
                "the job output names a build failure",
                evidence,
            );
        }
        if build.no_artefacts {
            evidence.push("No artefacts were created or found.".into());
        }
        self.result(
            Unclassified,
            None,
            "the checks that failed do not say why; read the job output",
            evidence,
        )
    }

    fn classify_test_only(&self, test: &Row) -> BotJobResult {
        let mut evidence = test.failed_items.clone();
        evidence.extend(
            test.reason
                .iter()
                .map(|reason| format!("test result: {reason}")),
        );
        self.result(
            BotJobClass::Unclassified,
            None,
            "only the test step result is in the inputs",
            evidence,
        )
    }
}

fn tarball_line(row: &Row) -> String {
    match (&row.tarball, row.entries) {
        (Some(name), Some(entries)) => format!("{name}: entries: {entries}"),
        (Some(name), None) => format!("{name} created!"),
        (None, Some(entries)) => format!("tarball entries: {entries}"),
        (None, None) => "found `.tar.* created!`".into(),
    }
}

/// The CPU target in a tarball name:
/// `eessi-2025.06-software-linux-x86_64-amd-zen2-17931241740.tar.zst` is
/// `x86_64/amd/zen2`. Target names use `_`, never `-`.
fn target_from_tarball(name: &str) -> Option<String> {
    static TARBALL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"eessi-[0-9.]+-[a-z]+-linux-([A-Za-z0-9_-]+)-[0-9]+\.tar").expect("regex")
    });
    TARBALL
        .captures(name)
        .map(|captures| captures[1].replace('-', "/"))
}

fn capture(pattern: &Regex, text: &str) -> Option<String> {
    pattern.captures(text).map(|c| c[1].to_string())
}

fn parse_input(input: &BotInput) -> Vec<Facts> {
    let text = input.text.replace('\r', "");
    if text
        .lines()
        .any(|line| matches!(line.trim(), "[RESULT]" | "[TEST]"))
        && text.contains("comment_description =")
    {
        return vec![parse_result_file(&input.name, &text)];
    }
    let comment = text.contains("|finished|")
        || text.contains("|test result|")
        || text.contains("New job on instance")
        || text.contains("Updates by the bot instance")
        || text.contains(MERGE_FAILED);
    if comment {
        let jobs = parse_comment(&input.name, &text);
        if jobs.is_empty() {
            return vec![Facts {
                sources: vec![input.name.clone()],
                ..Facts::default()
            }];
        }
        return jobs;
    }
    vec![parse_log(&input.name, &text)]
}

const MERGE_FAILED: &str =
    "Unable to download or merge changes between the source branch and the destination branch";

static JOB_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"job id `([0-9]+)`").expect("regex"));
static SLURM_OUT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"slurm-([0-9]+)\.out").expect("regex"));
static RESULT_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"_bot_job([0-9]+)\.(?:result|test)").expect("regex"));

fn parse_comment(name: &str, text: &str) -> Vec<Facts> {
    let mut jobs = Vec::new();
    let mut segments = text.split("New job on instance").peekable();
    let head = segments.next().unwrap_or("");
    jobs.extend(parse_bot_update(name, head));
    if head.contains(MERGE_FAILED) {
        let mut evidence = head
            .lines()
            .map(|line| line.trim().trim_start_matches('`'))
            .filter(|line| line.starts_with("error:"))
            .map(str::to_string)
            .collect::<Vec<_>>();
        evidence.push(MERGE_FAILED.into());
        jobs.push(Facts {
            sources: vec![name.into()],
            merge_failed: evidence,
            ..Facts::default()
        });
    }
    for segment in segments {
        jobs.push(parse_job_segment(
            name,
            &format!("New job on instance{segment}"),
        ));
    }
    if jobs.is_empty() && (text.contains("|finished|") || text.contains("|test result|")) {
        jobs.push(parse_job_segment(name, text));
    }
    jobs
}

fn parse_job_segment(name: &str, text: &str) -> Facts {
    static INSTANCE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"New job on instance `([^`]+)`").expect("regex"));
    static REPOSITORY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"for repository `([^`]+)`").expect("regex"));
    static TARGET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Building for: `([^`]+)`").expect("regex"));
    static ROW: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^\|[^|]*\|(finished|test result)\|(.*)\|\s*$").expect("regex")
    });
    let mut facts = Facts {
        sources: vec![name.into()],
        job_id: capture(&JOB_ID, text).or_else(|| capture(&SLURM_OUT, text)),
        instance: capture(&INSTANCE, text),
        repository: capture(&REPOSITORY, text),
        target: capture(&TARGET, text),
        ..Facts::default()
    };
    for row in ROW.captures_iter(text) {
        let parsed = parse_description(&row[2]);
        if &row[1] == "finished" {
            facts.build = Some(parsed);
        } else {
            facts.test = Some(parsed);
        }
    }
    facts
}

/// One job per build command a bot instance did not submit:
/// ``- handling command `build architecture:... instance:...` resulted in:``
/// followed by `- no jobs were submitted`.
fn parse_bot_update(name: &str, text: &str) -> Vec<Facts> {
    static HANDLING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"handling command `([^`]+)` resulted in").expect("regex"));
    static FIELD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(architecture|repository|instance):(\S+)").expect("regex"));
    let mut jobs = Vec::new();
    let mut command: Option<(String, String)> = None;
    for line in text.lines() {
        if let Some(captures) = HANDLING.captures(line) {
            command = Some((
                captures[1].to_string(),
                line.trim().trim_start_matches("- ").into(),
            ));
            continue;
        }
        if line.trim().trim_start_matches("- ") == "no jobs were submitted" {
            let Some((command, line)) = command.take() else {
                continue;
            };
            let mut facts = Facts {
                sources: vec![name.into()],
                no_jobs: Some(line),
                ..Facts::default()
            };
            for field in FIELD.captures_iter(&command) {
                let value = Some(field[2].to_string());
                match &field[1] {
                    "architecture" => facts.target = value,
                    "repository" => facts.repository = value,
                    _ => facts.instance = value,
                }
            }
            jobs.push(facts);
        }
    }
    jobs
}

/// The bot's job result description, the HTML `bot/check-build.sh` and
/// `bot/check-test.sh` write into `comment_description`.
fn parse_description(html: &str) -> Row {
    static SUMMARY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<summary>([^<]*)").expect("regex"));
    static REASON: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"_Reason_</dt><dd>(.*?)</dd>").expect("regex"));
    static ITEM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(:white_check_mark:|:x:) (.*?)<br/>").expect("regex"));
    static ENTRIES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"entries: ([0-9]+)").expect("regex"));
    static TARBALL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"<summary><code>([^<]*\.tar[^<]*)</code></summary>").expect("regex")
    });
    let mut row = Row {
        success: SUMMARY.captures(html).and_then(|c| {
            if c[1].contains("SUCCESS") {
                Some(true)
            } else if c[1].contains("FAILURE") {
                Some(false)
            } else {
                None
            }
        }),
        reason: REASON.captures(html).map(|c| strip_tags(&c[1])),
        entries: capture(&ENTRIES, html).and_then(|n| n.parse().ok()),
        no_artefacts: html.contains("No artefacts were created or found"),
        tarball: capture(&TARBALL, html),
        ..Row::default()
    };
    for item in ITEM.captures_iter(html) {
        let ok = &item[1] == ":white_check_mark:";
        let text = strip_tags(&item[2]);
        let found = text.starts_with("found");
        let slot = if text.contains("job output file") {
            Some((&mut row.output_found, ok))
        } else if text.contains("FATAL:") {
            Some((&mut row.fatal, found))
        } else if text.contains("ERROR:") {
            Some((&mut row.error, found))
        } else if text.contains("FAILED:") {
            Some((&mut row.failed, found))
        } else if text.contains("required modules missing:") {
            Some((&mut row.req_missing, found))
        } else if text.contains("No missing installations") {
            Some((&mut row.no_missing, found))
        } else if text.contains("created!") {
            Some((&mut row.tarball_created, found))
        } else if text.contains("test case") {
            Some((&mut row.reframe_failed, found))
        } else {
            None
        };
        if let Some((slot, value)) = slot {
            *slot = Some(value);
        }
        if !ok {
            row.failed_items.push(text);
        }
    }
    row
}

fn strip_tags(html: &str) -> String {
    static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("regex"));
    TAG.replace_all(html, "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(r"\*", "*")
}

/// `_bot_job<id>.result` or `.test`: `[RESULT]` or `[TEST]`, then
/// `comment_description = <details>...</details>`, `status = ...`, and
/// `artefacts =` with one tarball per indented line.
fn parse_result_file(name: &str, text: &str) -> Facts {
    let description = text
        .lines()
        .find_map(|line| line.strip_prefix("comment_description ="))
        .unwrap_or("");
    let mut row = parse_description(description);
    if row.success.is_none() {
        row.success =
            text.lines()
                .find_map(|line| match line.strip_prefix("status =").map(str::trim) {
                    Some("SUCCESS") => Some(true),
                    Some("FAILURE") => Some(false),
                    _ => None,
                });
    }
    if row.tarball.is_none() {
        row.tarball = text
            .lines()
            .skip_while(|line| !line.starts_with("artefacts ="))
            .skip(1)
            .map(str::trim)
            .find(|line| line.contains(".tar"))
            .map(str::to_string);
    }
    let test = text.lines().any(|line| line.trim() == "[TEST]");
    let mut facts = Facts {
        sources: vec![name.into()],
        job_id: capture(&RESULT_NAME, name).or_else(|| capture(&SLURM_OUT, text)),
        ..Facts::default()
    };
    if test {
        facts.test = Some(row);
    } else {
        facts.build = Some(row);
    }
    facts
}

/// Infra signatures in the job output, each a lowercase line substring set
/// that only the named cause prints.
const INFRA_SIGNATURES: &[(InfraCause, &[&str])] = &[
    (
        InfraCause::BotConfig,
        &["job config file", "does not exist or not readable"],
    ),
    (
        InfraCause::BotConfig,
        &["is not an eessi cvmfs repository or it is not known how to mount it"],
    ),
    (InfraCause::Scheduler, &["sbatch: error:"]),
    (InfraCause::Scheduler, &["batch job submission failed"]),
    (InfraCause::Scheduler, &["due to node failure"]),
    (InfraCause::Container, &["container creation failed"]),
    (InfraCause::Container, &["failed to pull", "image"]),
    (
        InfraCause::Network,
        &["failed to initialize root file catalog"],
    ),
    (
        InfraCause::Network,
        &["transport endpoint is not connected"],
    ),
    (InfraCause::Network, &["/cvmfs/", "is not available!"]),
    (
        InfraCause::Network,
        &["temporary failure in name resolution"],
    ),
    (InfraCause::Network, &["could not resolve host"]),
    (
        InfraCause::CompatLayer,
        &["can't locate", "in @inc", "/compat/"],
    ),
];

/// The bot's own markers. `ERROR:` and `FATAL:` say a step failed, not
/// which one, so they are removed before the line is classified.
const BOT_MARKERS: &[&str] = &[
    "FATAL: ",
    "ERROR: ",
    "FAILED: ",
    " required modules missing:",
    "No missing installations",
];

const MAX_EVIDENCE: usize = 12;

fn parse_log(name: &str, text: &str) -> Facts {
    static TARBALL_CREATED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\.tar.* created!").expect("regex"));
    static TARBALL_NAME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(eessi-[^/ ]*\.tar[^/ ]*) created!").expect("regex"));
    let lines = text
        .lines()
        .filter(|line| !line.starts_with(">> searching for "))
        .collect::<Vec<_>>();
    let has = |pattern: &str| Some(lines.iter().any(|line| line.contains(pattern)));
    let mut log = LogFacts {
        row: Row {
            output_found: Some(true),
            fatal: has("FATAL: "),
            error: has("ERROR: "),
            failed: has("FAILED: "),
            req_missing: has(" required modules missing:"),
            no_missing: has("No missing installations"),
            tarball_created: Some(lines.iter().any(|line| TARBALL_CREATED.is_match(line))),
            tarball: lines.iter().find_map(|line| capture(&TARBALL_NAME, line)),
            ..Row::default()
        },
        empty_tarball: has(">> Creating tarball") == Some(true)
            && has("wrote file list to") == Some(false),
        ..LogFacts::default()
    };
    let mut classified = Vec::new();
    let mut continuation = 0_usize;
    for line in &lines {
        let lower = line.to_ascii_lowercase();
        if log.infra.is_none() {
            log.infra = INFRA_SIGNATURES
                .iter()
                .find(|(_, parts)| parts.iter().all(|part| lower.contains(part)))
                .map(|(cause, _)| (*cause, line.trim().to_string()));
        }
        let stripped = line.replace("ERROR: ", "").replace("FATAL: ", "");
        let marker = BOT_MARKERS.iter().any(|marker| line.contains(marker));
        let signal =
            classify_build_failure("build", &stripped, "", None) != BuildFindingClass::Unknown;
        if signal || continuation > 0 {
            classified.push(stripped.trim().to_string());
        }
        continuation = if lower.contains("installation ended unsuccessfully") {
            2
        } else {
            continuation.saturating_sub(1)
        };
        if (marker || signal) && log.evidence.len() < MAX_EVIDENCE {
            let line = line.trim().to_string();
            if !log.evidence.contains(&line) {
                log.evidence.push(line);
            }
        }
    }
    if !classified.is_empty() {
        log.failure = Some(classify_build_failure(
            "build",
            &classified.join("\n"),
            "",
            None,
        ));
    }
    Facts {
        sources: vec![name.into()],
        job_id: capture(&SLURM_OUT, name),
        log: Some(log),
        ..Facts::default()
    }
}
