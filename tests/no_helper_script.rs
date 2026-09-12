//! A helper overlay script is not compose evidence.

use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn golden_compose_has_no_helper_script() {
    let root = repo();
    assert!(
        !root.join("skills/golden-replay/replay.sh").exists(),
        "the helper script must stay deleted; a 4/4 GREEN on it is not compose"
    );
    for rel in [
        "skills/golden-replay/SKILL.md",
        "skills/golden-replay/run-on-builder.sh",
        "skills/annual-bump/SKILL.md",
    ] {
        let text = std::fs::read_to_string(root.join(rel)).expect(rel);
        assert!(
            !text.contains("replay.sh"),
            "{rel} still names the helper script"
        );
    }
}
