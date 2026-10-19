//! The easyblock-fix skill names tools, fields and lint codes the server
//! has, so an agent following it never looks for one that is not there.

use std::path::PathBuf;

fn read(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn easyblock_fix_skill_names_what_the_server_provides() {
    let skill = read("skills/easyblock-fix/SKILL.md");
    let sources = [
        read("src/mcp.rs"),
        read("src/eb_run.rs"),
        read("src/eb_module.rs"),
    ]
    .join("\n");
    let tools: Vec<&str> = skill
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| word.starts_with("eb_") && word.len() > 3)
        .collect();
    assert!(tools.contains(&"eb_build_test"), "{tools:?}");
    for tool in &tools {
        assert!(
            sources.contains(&format!("\"{tool}\"")),
            "the skill names {tool}, which the server does not define"
        );
    }
    for name in [
        "EBM_CFG_OVERRIDE",
        "EBM_TEST_STEP_OWN_CMD",
        "EBM_THREADED_VARIANT",
        "threaded_findings",
        "config_log",
        "toolchain_options",
        "threaded_pairs",
        "failure_kind",
    ] {
        assert!(skill.contains(name), "the skill does not mention {name}");
        assert!(sources.contains(name), "the server does not define {name}");
    }
    assert!(
        skill.contains("still owed\n   only when `eb_build_test` returned an error"),
        "a build is owed only after a refused build"
    );
}
