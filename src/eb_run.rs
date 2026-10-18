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
static ERROR_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^ERROR:|EasyBuildError|\berror:|undefined reference|cannot find -l|\bFAILED\b|Traceback \(most recent call last\))")
        .expect("regex")
});

/// The step a line starts, when it starts one.
pub fn step_started(line: &str) -> Option<String> {
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

/// Commands EasyBuild reports running, in order.
pub fn commands_in(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| COMMAND.captures(line).map(|found| found[1].to_string()))
        .collect()
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
    let mut out: Vec<DryRunStep> = Vec::new();
    let mut current = "start".to_string();
    for line in text.lines() {
        if let Some(step) = step_started(line) {
            current = step;
            continue;
        }
        if let Some(found) = COMMAND.captures(line) {
            match out.last_mut() {
                Some(last) if last.step == current => last.commands.push(found[1].to_string()),
                _ => out.push(DryRunStep {
                    step: current.clone(),
                    commands: vec![found[1].to_string()],
                }),
            }
        }
    }
    out
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
