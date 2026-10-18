//! Run EasyBuild with easyblocks from the workspace, and read what it wrote.
//!
//! An easyblock change is only proven by EasyBuild running it: the commands
//! the easyblock generates (`eb -x`, which compiles nothing) and the build
//! and test steps of a real build. These helpers start `eb` with
//! `--include-easyblocks` on workspace files and turn its output into the
//! parts an agent needs: the commands per step, the test step's output, and
//! the error lines. Real builds are refused unless `EB_STACK_ALLOW_BUILD=1`,
//! so a server on a machine that must not build never starts one.

use crate::workspace_fs::resolve_inside;
use regex::Regex;
use serde::Serialize;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Environment variable that must be `1` before [`run_eb`] starts a build.
pub const ALLOW_BUILD_ENV: &str = "EB_STACK_ALLOW_BUILD";

/// EasyBuild's step announcements (`== configuring...`) and the step names
/// its log uses (`Starting configure step`).
const STEP_MARKERS: &[(&str, &str)] = &[
    ("fetching files", "fetch"),
    ("creating build dir, resetting environment", "ready"),
    ("unpacking", "extract"),
    ("patching", "patch"),
    ("preparing", "prepare"),
    ("configuring", "configure"),
    ("building", "build"),
    ("testing", "test"),
    ("installing", "install"),
    ("taking care of extensions", "extensions"),
    ("restore after iterating", "restore"),
    ("postprocessing", "postproc"),
    ("sanity checking", "sanitycheck"),
    ("cleaning up", "cleanup"),
    ("creating module", "module"),
    ("permissions", "permissions"),
    ("packaging", "package"),
    ("testing installation", "testcases"),
];

static ANNOUNCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^== ([a-z][a-z ,]*?)\.\.\.").expect("regex"));
static LOG_STEP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bStarting (\w+) step\b").expect("regex"));
static METHOD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\[(\w+?)_step method\]").expect("regex"));
static COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)running (?:shell )?command:?\s*["'](.+)["']"#).expect("regex")
});
/// `eb -x` wraps a long command after the opening quote (`running shell command "./configure`).
static COMMAND_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)running (?:shell )?command:?\s*"(.*)$"#).expect("regex"));
/// EasyBuild 5.4 `--trace` puts the command on the next line:
/// `Running shell command in <dir>:` / `>> running shell command:`.
static COMMAND_ANNOUNCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)running (?:shell )?command\b[^"']*:\s*$"#).expect("regex"));
static ANSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").expect("regex"));
static ERROR_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^ERROR:|EasyBuildError|\berror:|undefined reference|cannot find -l|\bFAILED\b|Traceback \(most recent call last\))")
        .expect("regex")
});

/// Drop ANSI color so step markers match when EasyBuild's rich console is on.
fn visible(line: &str) -> String {
    ANSI.replace_all(line, "").into_owned()
}

/// The step a line starts, when it starts one.
pub fn step_started(line: &str) -> Option<String> {
    let line = visible(line);
    let line = line.as_str();
    if let Some(found) = METHOD.captures(line) {
        return Some(found[1].to_string());
    }
    if let Some(found) = ANNOUNCE.captures(line) {
        let said = found[1].trim();
        return Some(
            STEP_MARKERS
                .iter()
                .find(|(announce, _)| *announce == said)
                .map_or_else(|| said.replace(' ', "_"), |(_, step)| (*step).to_string()),
        );
    }
    LOG_STEP.captures(line).map(|found| found[1].to_string())
}

/// One step's span in a log: `first` and `end` are 0-based line indexes,
/// `end` exclusive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogStep {
    /// Step name (`configure`, `test`, ...).
    pub step: String,
    /// First line of the step, 1-based.
    pub first_line: usize,
    /// Number of lines in the step.
    pub lines: usize,
}

/// Split a log into steps. A step runs until the next step starts; lines
/// before the first step belong to `start`. A step that starts twice (the
/// announcement and the log line) is one span.
pub fn split_steps(text: &str) -> Vec<LogStep> {
    let mut steps: Vec<LogStep> = Vec::new();
    let total = text.lines().count();
    for (index, line) in text.lines().enumerate() {
        let Some(step) = step_started(line) else {
            continue;
        };
        if steps.last().is_some_and(|last| last.step == step) {
            continue;
        }
        if let Some(last) = steps.last_mut() {
            last.lines = index + 1 - last.first_line;
        } else if index > 0 {
            steps.push(LogStep {
                step: "start".into(),
                first_line: 1,
                lines: index,
            });
        }
        steps.push(LogStep {
            step,
            first_line: index + 1,
            lines: 0,
        });
    }
    if let Some(last) = steps.last_mut() {
        last.lines = total + 1 - last.first_line;
    }
    steps
}

/// Lines of `text` that look like errors, deduplicated, at most `limit`.
pub fn error_lines(text: &str, limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if ERROR_LINE.is_match(line) {
            let line = line.trim().chars().take(400).collect::<String>();
            if !out.contains(&line) {
                out.push(line);
                if out.len() == limit {
                    break;
                }
            }
        }
    }
    out
}

/// A shell command starting at `lines[index]`, and the index of its last line.
///
/// EasyBuild 5.4 prints either a quoted command (`running shell command "make check"`,
/// sometimes wrapped before the closing quote) or a trace announcement whose next
/// line is the command (`Running shell command in <dir>:` then `make check`).
fn command_from(lines: &[String], index: usize) -> Option<(usize, String)> {
    let line = &lines[index];
    if let Some(found) = COMMAND.captures(line) {
        return Some((index, found[1].to_string()));
    }
    if let Some(found) = COMMAND_OPEN.captures(line) {
        let mut parts = vec![found[1].trim().to_string()];
        let mut end = index;
        while end + 1 < lines.len() && end - index < 40 {
            end += 1;
            let trimmed = lines[end].trim();
            if command_closed(lines, end) {
                let body = trimmed.trim_end_matches('"').trim();
                if !body.is_empty() {
                    parts.push(body.to_string());
                }
                return Some((end, parts.join(" ")));
            }
            parts.push(trimmed.to_string());
        }
        return None;
    }
    if COMMAND_ANNOUNCE.is_match(line) {
        let cmd = lines.get(index + 1)?.trim();
        if !cmd.is_empty() && !cmd.starts_with("==") && !cmd.starts_with('[') {
            return Some((index + 1, cmd.to_string()));
        }
    }
    None
}

/// The closing quote of a wrapped `eb -x` command: the line ends with `"` and
/// the next line is the `(in <dir>)` note, a step marker, or a blank line.
/// A configure flag such as `--with-blas-libs="a b"` also ends in `"` but is
/// followed by another flag, so it stays inside the command.
fn command_closed(lines: &[String], index: usize) -> bool {
    let trimmed = lines[index].trim();
    if !trimmed.ends_with('"') {
        return false;
    }
    let next = lines
        .get(index + 1)
        .map(String::as_str)
        .unwrap_or("")
        .trim();
    next.is_empty() || next.starts_with("(in ") || next.starts_with('[') || next.starts_with("==")
}

/// Commands EasyBuild reports running, in order.
pub fn commands_in(text: &str) -> Vec<String> {
    let lines: Vec<String> = text.lines().map(visible).collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if let Some((end, command)) = command_from(&lines, index) {
            out.push(command);
            index = end + 1;
        } else {
            index += 1;
        }
    }
    out
}

/// The text of one step (the last span with that name), or `None`.
pub fn step_text<'a>(text: &'a str, steps: &[LogStep], step: &str) -> Option<Vec<&'a str>> {
    let span = steps.iter().rev().find(|span| span.step == step)?;
    Some(
        text.lines()
            .skip(span.first_line - 1)
            .take(span.lines)
            .collect(),
    )
}

/// The last `n` of `lines`, joined.
pub fn tail(lines: &[&str], n: usize) -> String {
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Commands per step from `eb -x` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DryRunStep {
    /// Step name.
    pub step: String,
    /// Commands the easyblock would run in it.
    pub commands: Vec<String>,
}

/// Group the commands of an `eb -x` run by step.
pub fn parse_dry_run(text: &str) -> Vec<DryRunStep> {
    let lines: Vec<String> = text.lines().map(visible).collect();
    let mut out: Vec<DryRunStep> = Vec::new();
    let mut current = "start".to_string();
    let mut index = 0;
    while index < lines.len() {
        if let Some(step) = step_started(&lines[index]) {
            current = step;
            index += 1;
            continue;
        }
        if let Some((end, command)) = command_from(&lines, index) {
            match out.last_mut() {
                Some(last) if last.step == current => last.commands.push(command),
                _ => out.push(DryRunStep {
                    step: current.clone(),
                    commands: vec![command],
                }),
            }
            index = end + 1;
            continue;
        }
        index += 1;
    }
    out
}

/// The build environment an `eb -x` run reports (`export NAME="value"` and
/// `setting environment variable NAME to "value"` lines): what the toolchain
/// and the loaded dependencies define, in the form the easyblock reads it.
/// With `filter`, only names containing it (case-insensitive).
pub fn dry_run_environment(
    text: &str,
    filter: Option<&str>,
) -> std::collections::BTreeMap<String, String> {
    static EXPORT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"^\s*(?:export\s+([A-Za-z_]\w*)=["']?(.*?)["']?\s*$|.*setting environment variable ([A-Za-z_]\w*) to ["'](.*)["'])"#)
            .expect("regex")
    });
    let wanted = filter.map(str::to_ascii_uppercase);
    let mut out = std::collections::BTreeMap::new();
    for line in text.lines() {
        let Some(found) = EXPORT.captures(line) else {
            continue;
        };
        let (name, value) = match (found.get(1), found.get(2), found.get(3), found.get(4)) {
            (Some(name), Some(value), _, _) | (_, _, Some(name), Some(value)) => {
                (name.as_str(), value.as_str())
            }
            _ => continue,
        };
        if wanted
            .as_deref()
            .is_some_and(|wanted| !name.to_ascii_uppercase().contains(wanted))
        {
            continue;
        }
        out.insert(name.to_string(), value.to_string());
    }
    out
}

/// The `toolchainopts` of an easyconfig as written
/// (`toolchainopts = {'pic': True, 'openmp': True}`): option name to the text
/// of its value. Empty when the recipe sets none. Commented entries are skipped.
pub fn recipe_toolchain_options(text: &str) -> std::collections::BTreeMap<String, String> {
    static START: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^toolchainopts\s*=\s*\{").expect("regex"));
    static ENTRY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"['"]([A-Za-z_]\w*)['"]\s*:\s*([^,}\n]+)"#).expect("regex"));
    let mut out = std::collections::BTreeMap::new();
    let Some(found) = START.find(text) else {
        return out;
    };
    let rest = &text[found.end()..];
    let body = &rest[..rest.find('}').unwrap_or(rest.len())];
    for line in body.lines() {
        let code = line.split('#').next().unwrap_or("");
        for entry in ENTRY.captures_iter(code) {
            out.insert(entry[1].to_string(), entry[2].trim().to_string());
        }
    }
    out
}

/// A build-environment variable and its threaded variant, when their values
/// differ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThreadedPair {
    /// The plain name (`LIBX`, `X_STATIC_LIBS`).
    pub plain: String,
    /// The threaded name (`LIBX_MT`, `X_MT_STATIC_LIBS`, `X_STATIC_LIBS_MT`).
    pub threaded: String,
    /// Value of the plain name.
    pub plain_value: String,
    /// Value of the threaded name.
    pub threaded_value: String,
}

/// The plain/threaded pairs of a build environment. EasyBuild's toolchains
/// define a threaded variant next to several library variables, with an `MT`
/// part in the name; which one applies is up to the easyblock, so a dry run
/// of a recipe with a threaded toolchain option reports both the same way.
pub fn threaded_pairs(env: &std::collections::BTreeMap<String, String>) -> Vec<ThreadedPair> {
    let mut out = Vec::new();
    for (name, value) in env {
        let parts: Vec<&str> = name.split('_').collect();
        if parts.len() < 2 || !parts.contains(&"MT") {
            continue;
        }
        let plain = parts
            .iter()
            .copied()
            .filter(|part| *part != "MT")
            .collect::<Vec<_>>()
            .join("_");
        if let Some(plain_value) = env.get(&plain) {
            if plain_value != value {
                out.push(ThreadedPair {
                    plain,
                    threaded: name.clone(),
                    plain_value: plain_value.clone(),
                    threaded_value: value.clone(),
                });
            }
        }
    }
    out
}

/// Toolchain options that ask for a threaded build.
const THREADED_OPTIONS: &[&str] = &["openmp"];

/// What a dry run should tell the agent about threaded variants: `Some` when
/// the recipe turns a threaded toolchain option on and the environment has
/// plain/threaded pairs, since the commands then look the same as without the
/// option unless the easyblock picks the variant itself.
pub fn threaded_note(
    options: &std::collections::BTreeMap<String, String>,
    pairs: &[ThreadedPair],
) -> Option<String> {
    let on = THREADED_OPTIONS
        .iter()
        .filter(|option| {
            options
                .get(**option)
                .is_some_and(|value| matches!(value.as_str(), "True" | "1"))
        })
        .copied()
        .collect::<Vec<_>>();
    if on.is_empty() || pairs.is_empty() {
        return None;
    }
    let names = pairs
        .iter()
        .take(6)
        .map(|pair| format!("{} / {}", pair.plain, pair.threaded))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "this recipe sets toolchainopts {}: True. The build environment defines a threaded variant next to plain variables ({names}, ...; see threaded_pairs) whatever the option says, so the option alone does not change what an easyblock reads. If your easyblock takes a value from one of these variables, check that it uses the threaded one when this option is on, the way merged easyblocks decide it from the toolchain options, and compare this dry run with one of the recipe without the option.",
        on.join(", ")
    ))
}

/// What to build: a recipe, the easyblock modules to include, extra robot
/// paths and `--try-amend` values.
#[derive(Debug, Clone, Default)]
pub struct EbInvocation {
    /// The easyconfig.
    pub recipe: PathBuf,
    /// Easyblock modules for `--include-easyblocks`.
    pub easyblocks: Vec<PathBuf>,
    /// Robot paths searched before the default ones.
    pub robot: Vec<PathBuf>,
    /// `key=value` pairs for `--try-amend`.
    pub try_amend: Vec<String>,
}

impl EbInvocation {
    /// Check the invocation against the workspace: the recipe and every
    /// easyblock resolve inside `root`, easyblocks are `.py` files, and each
    /// `--try-amend` value is `name=value` with a lowercase parameter name.
    pub fn resolved(&self, root: &Path) -> Result<EbInvocation, String> {
        let inside = |path: &Path| resolve_inside(root, path).map(|(_, found)| found);
        let recipe = inside(&self.recipe)?;
        if !recipe.is_file() {
            return Err(format!("recipe {} is not a file", self.recipe.display()));
        }
        let mut easyblocks = Vec::new();
        for path in &self.easyblocks {
            let found = inside(path)?;
            if found.extension().and_then(|value| value.to_str()) != Some("py") || !found.is_file()
            {
                return Err(format!(
                    "easyblock {} is not a .py file in the workspace",
                    path.display()
                ));
            }
            easyblocks.push(found);
        }
        let mut robot = Vec::new();
        for path in &self.robot {
            robot.push(inside(path)?);
        }
        static AMEND: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^[a-z_][a-z0-9_]*=").expect("regex"));
        if let Some(bad) = self.try_amend.iter().find(|value| !AMEND.is_match(value)) {
            return Err(format!(
                "try_amend value {bad:?} is not name=value with an easyconfig parameter name"
            ));
        }
        Ok(EbInvocation {
            recipe,
            easyblocks,
            robot,
            try_amend: self.try_amend.clone(),
        })
    }

    /// `eb` arguments for this invocation followed by `extra`.
    pub fn args(&self, extra: &[String]) -> Vec<String> {
        let mut args = vec![self.recipe.display().to_string()];
        if !self.easyblocks.is_empty() {
            let joined = self
                .easyblocks
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(",");
            args.push(format!("--include-easyblocks={joined}"));
        }
        args.push("--robot".into());
        if !self.robot.is_empty() {
            let joined = self
                .robot
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(":");
            // A trailing ':' keeps EasyBuild's default robot paths after these.
            args.push(format!("--robot-paths={joined}:"));
        }
        for value in &self.try_amend {
            args.push(format!("--try-amend={value}"));
        }
        args.extend(extra.iter().cloned());
        args
    }
}

/// How one `eb` run ended.
#[derive(Debug, Clone, Serialize)]
pub struct EbRun {
    /// The command line, program first.
    pub argv: Vec<String>,
    /// Exit code; `None` when killed.
    pub exit_code: Option<i32>,
    /// True when the run was killed at its time limit.
    pub timed_out: bool,
    /// Wall time in seconds.
    pub seconds: u64,
    /// Where stdout and stderr went.
    pub output_file: PathBuf,
}

/// The `eb` program: `EB_STACK_EB` when set, else `eb` on `PATH`.
pub fn resolve_eb() -> Result<PathBuf, String> {
    if let Some(value) = std::env::var_os("EB_STACK_EB").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!("EB_STACK_EB={} is not a file", path.display()))
        };
    }
    let path = std::env::var_os("PATH").ok_or("PATH is not set")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join("eb"))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| "eb is not on PATH; load an EasyBuild module where eb-stack runs".into())
}

/// Run `eb args...`, stdout and stderr both to `output_file`, killed after
/// `limit`. The caller decides whether the run may build.
///
/// `eb` starts in the directory of `output_file`: `python -m easybuild.main`
/// puts the current directory first on `sys.path`, so starting it at the
/// root of an easyblocks checkout would import that checkout's `easybuild`
/// package instead of the installed framework.
pub fn run_eb(args: &[String], limit: Duration, output_file: &Path) -> Result<EbRun, String> {
    let program = resolve_eb()?;
    let file = File::create(output_file)
        .map_err(|error| format!("create {}: {error}", output_file.display()))?;
    let err = file
        .try_clone()
        .map_err(|error| format!("clone {}: {error}", output_file.display()))?;
    let started = Instant::now();
    let cwd = output_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut child = Command::new(&program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(err))
        .spawn()
        .map_err(|error| format!("start {}: {error}", program.display()))?;
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("wait for eb: {error}"))?
        {
            break Some(status);
        }
        if started.elapsed() >= limit {
            timed_out = true;
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let mut argv = vec![program.display().to_string()];
    argv.extend(args.iter().cloned());
    Ok(EbRun {
        argv,
        exit_code: status.and_then(|status| status.code()),
        timed_out,
        seconds: started.elapsed().as_secs(),
        output_file: output_file.to_path_buf(),
    })
}

/// True when this process may start a real build.
pub fn builds_allowed() -> bool {
    std::env::var(ALLOW_BUILD_ENV).as_deref() == Ok("1")
}

/// The `eb_build_test` description on a server where builds are `allowed`
/// or not, so the tool list itself says whether a build can run here.
pub fn build_test_description(allowed: bool) -> String {
    let common = "Build one recipe for real with your easyblock modules (--include-easyblocks), --rebuild, and return how it ended: exit code, the last step reached, whether the installation completed, the test step's commands and the tail of its output, the error lines, and the log path for eb_build_log. The build directory is kept under .eb-stack/builds/<n>/build, so config.log and test logs can be read with eb_recipe_read. A build runs the whole recipe, so get the commands right with eb_easyblock_dry_run first.";
    if allowed {
        format!("{common} Builds are on for this server: once the dry run shows the commands you intended, build each recipe you were asked about with this tool before calling the change done. Only a build shows link errors and test failures; a dry run never does.")
    } else {
        format!("{common} Builds are off on this server ({ALLOW_BUILD_ENV}=1 is not set), so this tool returns an error; say which builds are still owed.")
    }
}

/// The `next` field of a dry run: what to do after it, given whether builds
/// are `allowed` and whether the dry run `finished`.
pub fn dry_run_next(allowed: bool, finished: bool) -> &'static str {
    match (finished, allowed) {
        (false, _) => "fix what the error lines say, then dry-run again",
        (true, true) => "builds are on: once these commands match the task, run eb_build_test on each recipe you were asked about; a dry run does not link anything or run the tests",
        (true, false) => "builds are off on this server: once these commands match the task, say which builds are still owed",
    }
}

/// Read the end of a file as text, at most `max_bytes` from its end, so a
/// large build log stays bounded.
pub fn read_tail(path: &Path, max_bytes: u64) -> Result<(String, bool), String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = File::open(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let length = file
        .metadata()
        .map_err(|error| format!("read {}: {error}", path.display()))?
        .len();
    let skipped = length > max_bytes;
    if skipped {
        file.seek(SeekFrom::Start(length - max_bytes))
            .map_err(|error| format!("read {}: {error}", path.display()))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok((String::from_utf8_lossy(&bytes).into_owned(), skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "== temporary log file in case of crash /tmp/eb-1/easybuild.log\n\
== configuring...\n\
  running shell command \"./configure --prefix=/sw --with-x-libs=\\\"a b\\\"\"\n\
  (in /tmp/build/src)\n\
== building...\n\
== testing...\n\
  running shell command \"make check\"\n\
  (in /tmp/build/src)\n\
/usr/bin/ld: cannot find -llibfoo.a\n\
ERROR: Shell command failed!\n\
== FAILED: Installation ended unsuccessfully\n";

    #[test]
    fn steps_split_on_announcements_and_log_lines() {
        let steps = split_steps(LOG);
        let names: Vec<&str> = steps.iter().map(|span| span.step.as_str()).collect();
        assert_eq!(names, vec!["start", "configure", "build", "test"]);
        let test = step_text(LOG, &steps, "test").expect("test step");
        assert_eq!(test.len(), 6);
        assert!(test[1].contains("make check"));
        let log = "2026 INFO Starting configure step\nx\n2026 INFO Starting test step\ny\nz\n";
        let steps = split_steps(log);
        assert_eq!(
            steps.iter().map(|s| s.lines).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn errors_and_commands_are_found() {
        let errors = error_lines(LOG, 10);
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors[0].contains("cannot find -llibfoo.a"));
        let commands = commands_in(LOG);
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[1], "make check");
    }

    #[test]
    fn dry_run_commands_group_by_step() {
        let text = "== configuring... [DRY RUN]\n\n[configure_step method]\n  running command \"./configure --with-a=\\\"x y\\\"\"\n  (in /tmp/a)\n\n[test_step method]\n  running command \"make check\"\n";
        let steps = parse_dry_run(text);
        assert_eq!(steps.len(), 2, "{steps:?}");
        assert_eq!(steps[0].step, "configure");
        assert!(steps[0].commands[0].starts_with("./configure"));
        assert_eq!(steps[1].step, "test");
        assert_eq!(steps[1].commands, vec!["make check".to_string()]);
    }

    #[test]
    fn easybuild_5_trace_and_wrapped_dry_run_commands() {
        // Real EasyBuild 5.4.0 `--logtostdout --trace` lines (foss/2026.1 Hypre):
        // `Running shell command in <dir>:` then a tab and `make check`.
        let trace = "== testing...\n\
== 2026-10-18 02:00:25,304 run.py:532 INFO Running shell command in /tmp/src:\n\
\tmake check\n\
\u{1b}[2K  >> running shell command:\n\
        make check\n";
        let commands = commands_in(trace);
        assert_eq!(
            commands,
            vec!["make check".to_string(), "make check".to_string()]
        );
        // `eb -x` wraps the configure command before its closing quote.
        let dry = "\
[configure_step method]\n\
  running shell command \"./configure \n\
--with-blas-libs=\"flexiblas gfortran\" \n\
--with-MPI-include=/usr/include\"\n\
  (in /tmp/src)\n\
[test_step method]\n\
  running shell command \"make check\"\n";
        let steps = parse_dry_run(dry);
        assert_eq!(steps.len(), 2, "{steps:?}");
        assert!(
            steps[0].commands[0].contains("--with-blas-libs=\"flexiblas gfortran\""),
            "{steps:?}"
        );
        assert_eq!(steps[1].commands, vec!["make check".to_string()]);
        let marked = "\u{1b}[1m[\u{1b}[0mconfigure_step method\u{1b}[1m]\u{1b}[0m";
        assert_eq!(step_started(marked).as_deref(), Some("configure"));
    }

    #[test]
    fn build_tool_text_says_whether_builds_run_here() {
        let on = build_test_description(true);
        let off = build_test_description(false);
        assert!(
            on.contains("Builds are on") && !on.contains("is not set"),
            "{on}"
        );
        assert!(off.contains("EB_STACK_ALLOW_BUILD=1 is not set"), "{off}");
        assert!(dry_run_next(true, true).contains("eb_build_test"));
        assert!(dry_run_next(false, true).contains("still owed"));
        assert!(!dry_run_next(true, false).contains("eb_build_test"));
    }

    #[test]
    fn toolchain_options_and_threaded_pairs_are_reported() {
        let recipe = "name = 'x'\ntoolchain = {'name': 'foss', 'version': '1'}\ntoolchainopts = {\n    'pic': True,\n    # 'usempi': True,\n    \"openmp\": True,\n}\nsources = []\n";
        let options = recipe_toolchain_options(recipe);
        assert_eq!(options.get("pic").map(String::as_str), Some("True"));
        assert_eq!(options.get("openmp").map(String::as_str), Some("True"));
        assert!(!options.contains_key("usempi"), "{options:?}");
        assert!(recipe_toolchain_options("name = 'x'\n").is_empty());
        let one_line = recipe_toolchain_options("toolchainopts = {'pic': True, 'openmp': False}\n");
        assert_eq!(one_line.get("openmp").map(String::as_str), Some("False"));

        let env: std::collections::BTreeMap<String, String> = [
            ("LIBX", "-lx"),
            ("LIBX_MT", "-lx -lthr"),
            ("X_STATIC_LIBS", "libx.a"),
            ("X_MT_STATIC_LIBS", "libx.a,libthr.a"),
            ("Y_STATIC_LIBS", "liby.a"),
            ("Y_STATIC_LIBS_MT", "liby.a"),
            ("MT", "1"),
            ("CFLAGS", "-O2"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
        let pairs = threaded_pairs(&env);
        let names: Vec<_> = pairs
            .iter()
            .map(|pair| (pair.plain.as_str(), pair.threaded.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![("LIBX", "LIBX_MT"), ("X_STATIC_LIBS", "X_MT_STATIC_LIBS")],
            "equal values and a bare MT are not pairs"
        );
        let note = threaded_note(&options, &pairs).expect("openmp is on");
        assert!(
            note.contains("openmp") && note.contains("LIBX / LIBX_MT"),
            "{note}"
        );
        assert!(threaded_note(&one_line, &pairs).is_none(), "openmp is off");
        assert!(threaded_note(&options, &[]).is_none(), "no pairs");
    }

    #[test]
    fn dry_run_environment_reads_exports_and_settings() {
        let text = "  export CC=\"gcc\"\n  export LIBX=\"-lx -ly\"\n  >> setting environment variable X_DIR to '/sw/x/lib'\nnoise\n";
        let all = dry_run_environment(text, None);
        assert_eq!(all.len(), 3, "{all:?}");
        assert_eq!(all["LIBX"], "-lx -ly");
        assert_eq!(all["X_DIR"], "/sw/x/lib");
        let some = dry_run_environment(text, Some("libx"));
        assert_eq!(some.keys().collect::<Vec<_>>(), vec!["LIBX"]);
    }

    #[test]
    fn invocation_stays_in_the_workspace_and_builds_arguments() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("easyblocks/x")).expect("dirs");
        std::fs::write(root.join("x.eb"), "name = 'x'\n").expect("recipe");
        std::fs::write(root.join("easyblocks/x/x.py"), "pass\n").expect("block");
        std::fs::write(root.join("notes.txt"), "x\n").expect("notes");
        let good = EbInvocation {
            recipe: "x.eb".into(),
            easyblocks: vec!["easyblocks/x/x.py".into()],
            robot: vec![".".into()],
            try_amend: vec!["runtest=check".into()],
        }
        .resolved(root)
        .expect("resolved");
        let args = good.args(&["-x".to_string()]);
        assert!(args[0].ends_with("x.eb"));
        assert!(
            args[1].starts_with("--include-easyblocks=") && args[1].ends_with("easyblocks/x/x.py")
        );
        assert_eq!(args[2], "--robot");
        assert!(args[3].starts_with("--robot-paths=") && args[3].ends_with(':'));
        assert_eq!(args[4], "--try-amend=runtest=check");
        assert_eq!(args[5], "-x");
        let escape = EbInvocation {
            recipe: "../x.eb".into(),
            ..Default::default()
        };
        assert!(escape.resolved(root).is_err());
        let not_py = EbInvocation {
            recipe: "x.eb".into(),
            easyblocks: vec!["notes.txt".into()],
            ..Default::default()
        };
        assert!(not_py.resolved(root).is_err());
        let option = EbInvocation {
            recipe: "x.eb".into(),
            try_amend: vec!["--installpath=/".into()],
            ..Default::default()
        };
        assert!(option.resolved(root).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn run_eb_captures_output_and_stops_at_the_limit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let fake = dir.path().join("eb");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho \"args: $*\"\necho \"cwd: $(pwd -P)\"\necho oops >&2\n[ \"$1\" = slow ] && sleep 5\nexit 3\n",
        )
        .expect("fake eb");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        std::env::set_var("EB_STACK_EB", &fake);
        let out = dir.path().join("out.txt");
        let run = run_eb(&["a".into(), "b".into()], Duration::from_secs(5), &out).expect("run");
        assert_eq!(run.exit_code, Some(3));
        assert!(!run.timed_out);
        let (text, skipped) = read_tail(&out, 1024).expect("tail");
        assert!(!skipped && text.contains("args: a b") && text.contains("oops"));
        let cwd = dir.path().canonicalize().expect("canonical");
        assert!(text.contains(&format!("cwd: {}", cwd.display())), "{text}");
        let slow = run_eb(&["slow".into()], Duration::from_millis(300), &out).expect("slow");
        assert!(slow.timed_out && slow.exit_code.is_none());
        std::env::remove_var("EB_STACK_EB");
        let (end, skipped) = read_tail(&out, 4).expect("short tail");
        assert!(skipped && end.len() <= 4);
    }
}
