//! Shared command-line parsing for the CLI and the MCP tools.
//!
//! One usage error, one toolchain parser, and one foreign-format parser.
//! The CLI maps the error to exit status 2. MCP maps it to a tool error.

use crate::domain::Toolchain;
use crate::foreign::ForeignFormat;
use std::error::Error;
use std::fmt::{self, Display};

/// The command line was rejected before the work started.
///
/// Exit status is 2, the same status clap uses for a missing flag. A missing
/// file, a parse failure, or a check that failed is status 1 instead.
#[derive(Debug)]
pub struct UsageError {
    message: String,
}

impl UsageError {
    /// A rejection whose text is `message`.
    pub fn new(message: impl Display) -> Self {
        Self {
            message: message.to_string(),
        }
    }
}

impl Display for UsageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for UsageError {}

/// Wrap a usage rejection so `?` and `anyhow` context keep it in the chain.
pub fn usage(message: impl Display) -> anyhow::Error {
    anyhow::Error::new(UsageError::new(message))
}

/// `name/version` in `spec`, or the split name and version fields.
///
/// `default_name` fills a missing name with `foss`. Bump does not, on the
/// CLI or on MCP.
/// An empty name is a usage error, not a missing name, so it does not
/// become `foss`. An empty version is kept: a SYSTEM recipe has one.
/// `flag` is `--toolchain` on the CLI and `toolchain` on MCP. Passing
/// `spec` together with a split field is a usage error.
pub fn resolve_toolchain(
    name: Option<&str>,
    version: Option<&str>,
    spec: Option<&str>,
    default_name: bool,
    flag: &str,
) -> Result<Toolchain, UsageError> {
    if let Some(spec) = spec {
        if name.is_some() || version.is_some() {
            let (name_flag, version_flag) = split_flags(flag);
            return Err(UsageError::new(format!(
                "{flag} cannot be used with {name_flag} or {version_flag}"
            )));
        }
        return parse_toolchain_spec(spec, flag);
    }
    let (name_flag, _) = split_flags(flag);
    if name.is_some_and(|name| name.is_empty()) {
        return Err(UsageError::new(format!("{name_flag} must not be empty")));
    }
    match (name, version) {
        (Some(name), Some(version)) => Ok(Toolchain {
            name: name.to_string(),
            version: version.to_string(),
        }),
        (None, Some(version)) if default_name => Ok(Toolchain {
            name: "foss".into(),
            version: version.to_string(),
        }),
        _ if default_name && flag.starts_with("--") => Err(UsageError::new(
            "--toolchain-version is required, or pass --toolchain name/version",
        )),
        _ if default_name => Err(UsageError::new(
            "toolchain_version is required, or pass toolchain as name/version",
        )),
        _ if flag.starts_with("--") => Err(UsageError::new(
            "--toolchain-name and --toolchain-version are required, or pass --toolchain name/version",
        )),
        _ => Err(UsageError::new(
            "toolchain_name and toolchain_version are required, or pass toolchain as name/version",
        )),
    }
}

/// Parse `name/version`.
///
/// `system` and `SYSTEM`, and the same names with a trailing slash, are the
/// SYSTEM toolchain. EasyBuild's SYSTEM constant is
/// `{'name': 'system', 'version': 'system'}` (`easyconfig/constants.py` in
/// framework 5.4.1). An empty version is that same toolchain: the framework
/// treats the name `system` as SYSTEM regardless of version. `system/system`
/// stays a name and a version, which is the constant's own spelling.
pub fn parse_toolchain_spec(spec: &str, flag: &str) -> Result<Toolchain, UsageError> {
    if let Some(toolchain) = system_toolchain_spec(spec) {
        return Ok(toolchain);
    }
    let Some((parsed_name, parsed_version)) = spec.split_once('/') else {
        return Err(UsageError::new(format!(
            "{flag} {spec}: expected name/version, for example foss/2026.1"
        )));
    };
    if parsed_name.is_empty() || parsed_version.is_empty() || parsed_version.contains('/') {
        return Err(UsageError::new(format!(
            "{flag} {spec}: expected name/version, for example foss/2026.1"
        )));
    }
    Ok(Toolchain {
        name: parsed_name.to_string(),
        version: parsed_version.to_string(),
    })
}

/// `system`, `SYSTEM`, `system/`, and `SYSTEM/` are the SYSTEM toolchain
/// with an empty version. Any other spelling returns `None`.
fn system_toolchain_spec(spec: &str) -> Option<Toolchain> {
    let (name, version) = match spec.split_once('/') {
        Some((name, version)) => (name, Some(version)),
        None => (spec, None),
    };
    if !name.eq_ignore_ascii_case("system") {
        return None;
    }
    match version {
        None | Some("") => Some(Toolchain {
            name: "system".into(),
            version: String::new(),
        }),
        Some(_) => None,
    }
}

fn split_flags(flag: &str) -> (&'static str, &'static str) {
    if flag.starts_with("--") {
        ("--toolchain-name", "--toolchain-version")
    } else {
        ("toolchain_name", "toolchain_version")
    }
}

/// `auto`, or one foreign recipe language. `conda`, `crates`, `crates.io`,
/// `lua`, and `perl6` are accepted spellings.
pub fn parse_foreign_format(value: &str, flag: &str) -> Result<Option<ForeignFormat>, UsageError> {
    match value {
        "auto" => Ok(None),
        "conda-forge" | "conda" => Ok(Some(ForeignFormat::CondaForge)),
        "spack" => Ok(Some(ForeignFormat::Spack)),
        "pypi" => Ok(Some(ForeignFormat::Pypi)),
        "cran" => Ok(Some(ForeignFormat::Cran)),
        "cargo" | "crates" | "crates.io" => Ok(Some(ForeignFormat::Cargo)),
        "luarocks" | "lua" => Ok(Some(ForeignFormat::Luarocks)),
        "raku" | "perl6" => Ok(Some(ForeignFormat::Raku)),
        _ => Err(UsageError::new(format!(
            "{flag} {value}: expected auto, conda-forge, spack, pypi, cran, cargo, luarocks, or raku"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_toolchain_name_is_not_foss() {
        let cli = resolve_toolchain(Some(""), Some("2026.1"), None, true, "--toolchain");
        assert_eq!(
            cli.unwrap_err().to_string(),
            "--toolchain-name must not be empty"
        );
        let mcp = resolve_toolchain(Some(""), Some("2026.1"), None, true, "toolchain");
        assert_eq!(
            mcp.unwrap_err().to_string(),
            "toolchain_name must not be empty"
        );
        let system = resolve_toolchain(Some("system"), Some(""), None, true, "--toolchain")
            .expect("empty version is the SYSTEM spelling");
        assert_eq!(system.name, "system");
        assert!(system.version.is_empty());
        let missing = resolve_toolchain(None, Some("2026.1"), None, true, "--toolchain")
            .expect("an omitted name still defaults");
        assert_eq!(missing.name, "foss");
        assert_eq!(missing.version, "2026.1");
    }
}
