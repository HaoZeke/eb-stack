//! Every long flag `--help` prints is named in the CLI reference.

use std::process::Command;

fn help(args: &[&str]) -> String {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let output = Command::new(binary)
        .args(args)
        .arg("--help")
        .output()
        .expect("run --help");
    assert!(
        output.status.success(),
        "help {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf8 help")
}

fn long_flags(text: &str) -> Vec<String> {
    let mut flags = Vec::new();
    let mut options = false;
    for line in text.lines() {
        if line == "Options:" {
            options = true;
            continue;
        }
        if !options {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with(' ') {
            break;
        }
        let trimmed = line.trim();
        if !trimmed.starts_with('-') {
            continue;
        }
        let Some(start) = trimmed.find("--") else {
            continue;
        };
        let name = trimmed[start + 2..]
            .split(|c: char| c.is_whitespace() || c == '=')
            .next()
            .unwrap_or("");
        if !name.is_empty() && name != "help" {
            flags.push(name.to_string());
        }
    }
    flags
}

fn subcommands(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut commands = false;
    for line in text.lines() {
        if line == "Commands:" {
            commands = true;
            continue;
        }
        if !commands {
            continue;
        }
        if line.trim().is_empty() {
            if !names.is_empty() {
                break;
            }
            continue;
        }
        if !line.starts_with("  ") || line.starts_with("   ") {
            if !line.starts_with(' ') {
                break;
            }
            continue;
        }
        let name = line.split_whitespace().next().unwrap_or("");
        if !name.is_empty() && name != "help" {
            names.push(name.to_string());
        }
    }
    names
}

/// A token is named when it is not the interior of a longer flag or command.
fn token_is_named(section: &str, token: &str) -> bool {
    section.match_indices(token).any(|(index, _)| {
        let before = index == 0
            || section[..index]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_ascii_alphanumeric() && c != '-');
        let after = index + token.len();
        let after_ok = section[after..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '-');
        before && after_ok
    })
}

/// Level-1 org headings. The text before the first heading is `preamble`.
fn org_sections(reference: &str) -> Vec<(String, String)> {
    let mut sections = Vec::new();
    let mut title = String::from("preamble");
    let mut body = String::new();
    for line in reference.lines() {
        if let Some(heading) = line.strip_prefix("* ") {
            sections.push((title, body));
            title = heading.trim().trim_matches('~').to_string();
            body = String::new();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    sections.push((title, body));
    sections
}

fn section_body(sections: &[(String, String)], title: &str) -> String {
    if title == "Command map" {
        let preamble = sections
            .iter()
            .find(|(name, _)| name == "preamble")
            .map(|(_, body)| body.as_str())
            .unwrap_or("");
        let map = sections
            .iter()
            .find(|(name, _)| name == "Command map")
            .map(|(_, body)| body.as_str())
            .unwrap_or("");
        return format!("{preamble}\n{map}");
    }
    sections
        .iter()
        .find(|(name, _)| name == title)
        .map(|(_, body)| body.clone())
        .unwrap_or_default()
}

/// The page section that documents this command path.
fn section_for(args: &[String]) -> &'static str {
    match args.first().map(String::as_str) {
        None => "Command map",
        Some("package") => "Package commands",
        Some("recipe") => "Recipe commands",
        Some("stack") => "Stack commands",
        Some("target") => "Target commands",
        Some("campaign") => "Campaign commands",
        Some("repro") => "repro summary",
        Some("pr-overlap") => "pr-overlap",
        Some("rebuild-set") => "rebuild-set",
        Some("component-reach") => "component-reach",
        Some("easyblock-reach") => "easyblock-reach",
        Some("mcp") => "MCP",
        Some(_) => "Command map",
    }
}

#[test]
fn the_cli_reference_names_every_help_flag() {
    let reference = std::fs::read_to_string("docs/orgmode/reference/cli.org").expect("cli.org");
    let sections = org_sections(&reference);
    let mut queue = vec![Vec::<String>::new()];
    let mut seen = Vec::new();
    while let Some(args) = queue.pop() {
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let text = help(&argv);
        let title = section_for(&args);
        let section = section_body(&sections, title);
        assert!(
            !section.is_empty(),
            "docs/orgmode/reference/cli.org has no section {title}"
        );
        let command = if args.is_empty() {
            "eb-stack".to_string()
        } else {
            format!("eb-stack {}", args.join(" "))
        };
        for flag in long_flags(&text) {
            assert!(
                token_is_named(&section, &format!("--{flag}")),
                "section {title} does not name --{flag} from `{command} --help`"
            );
        }
        for name in subcommands(&text) {
            assert!(
                token_is_named(&section, &name),
                "section {title} does not name subcommand {name} of `{command}`"
            );
            let mut next = args.clone();
            next.push(name);
            queue.push(next);
        }
        seen.push(args);
    }
    let map = section_body(&sections, "Command map");
    for command in [
        "package",
        "recipe",
        "stack",
        "target",
        "campaign",
        "repro",
        "mcp",
        "easyblock-reach",
        "pr-overlap",
        "rebuild-set",
        "component-reach",
    ] {
        assert!(
            seen.iter()
                .any(|args| args.first().is_some_and(|arg| arg == command)),
            "help did not list {command}"
        );
        assert!(
            token_is_named(&map, command),
            "Command map does not name {command}"
        );
    }
    for command in ["order", "easystack", "sections", "summary"] {
        assert!(
            seen.iter()
                .any(|args| args.iter().any(|arg| arg == command)),
            "help did not list {command}"
        );
        let owner = seen
            .iter()
            .find(|args| args.iter().any(|arg| arg == command))
            .map(|args| section_for(args))
            .unwrap_or("Command map");
        let section = section_body(&sections, owner);
        assert!(
            token_is_named(&section, command),
            "section {owner} does not name {command}"
        );
    }
}
