"""Behavioural checks of the public document manifest in llms_leakguard.py.

Run with `python3 -m unittest scripts.test_llms_leakguard` from the repository
root; tests/llms_txt.rs runs it in CI.
"""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import llms_leakguard as guard  # noqa: E402

REPO = "https://github.com/stackhands/eb-stack"


class ManifestTest(unittest.TestCase):
    def manifest(self) -> guard.Manifest:
        return guard.Manifest({"README.md", "CONTRIBUTING.md"}, ("docs/",))

    def test_listed_files_and_prefixed_directories_pass(self):
        allow = self.manifest()
        self.assertTrue(allow.allows(f"{REPO}/tree/main/docs"))
        self.assertFalse(allow.allows(f"{REPO}/tree/main/docs-private"))
        self.assertTrue(allow.allows(f"{REPO}/blob/main/README.md"))
        self.assertTrue(allow.allows(f"{REPO}/blob/main/docs/orgmode/index.org"))
        self.assertTrue(allow.allows(f"{REPO}/blob/v0.4.0/CONTRIBUTING.md"))

    def test_unlisted_repository_files_are_refused(self):
        allow = self.manifest()
        self.assertFalse(allow.allows(f"{REPO}/blob/main/.private/notes.md"))
        self.assertFalse(allow.allows(f"{REPO}/blob/main/scripts/private.py"))

    def test_other_sites_are_not_governed(self):
        allow = self.manifest()
        self.assertTrue(allow.allows("https://docs.easybuild.io/"))
        self.assertTrue(allow.allows("https://eb-stack.rgoswami.me/howto/stack-shard.html.md"))

    def test_traversal_out_of_a_public_directory_is_refused(self):
        allow = self.manifest()
        self.assertFalse(allow.allows(f"{REPO}/blob/main/docs/../.private/notes.md"))
        self.assertFalse(allow.allows(f"{REPO}/blob/main/docs/%2e%2e/.private/notes.md"))
        self.assertFalse(allow.allows(f"{REPO}/blob/main/docs/%252e%252e/.private/notes.md"))
        self.assertFalse(allow.allows(f"{REPO}/blob/main/docs/..%2f.private/notes.md"))
        self.assertFalse(allow.allows(f"{REPO}/blob/main/docs\\..\\.private\\notes.md"))
        self.assertFalse(allow.allows(f"{REPO}/blob/main/docs/./../.private/notes.md"))

    def test_traversal_inside_a_public_directory_is_still_public(self):
        allow = self.manifest()
        self.assertTrue(allow.allows(f"{REPO}/blob/main/docs/orgmode/../howto/stack-shard.org"))
        self.assertTrue(allow.allows(f"{REPO}/blob/main/docs/%6frgmode/index.org"))

    def test_other_refs_and_raw_forms_follow_the_manifest(self):
        allow = self.manifest()
        self.assertTrue(allow.allows(f"{REPO}/raw/main/README.md"))
        self.assertFalse(allow.allows(f"{REPO}/raw/main/.private/notes.md"))
        self.assertFalse(allow.allows(f"{REPO}/blob/feature-x/.private/notes.md"))
        self.assertFalse(allow.allows(f"{REPO}/tree/main/.private"))
        self.assertFalse(allow.allows("https://raw.githubusercontent.com/stackhands/eb-stack/main/.private/notes.md"))
        self.assertTrue(allow.allows("https://raw.githubusercontent.com/stackhands/eb-stack/main/README.md"))
        self.assertFalse(allow.allows("https://www.github.com/stackhands/eb-stack/blob/main/.private/notes.md"))

    def test_host_port_userinfo_and_case_do_not_change_the_repository(self):
        allow = self.manifest()
        self.assertFalse(allow.allows("https://github.com/stackhands/eb-stack/blob/main/.private/notes.md"))
        self.assertFalse(allow.allows("https://GitHub.com/stackhands/EB-STACK/blob/main/.private/notes.md"))
        self.assertFalse(allow.allows("https://github.com:443/stackhands/eb-stack/blob/main/.private/notes.md"))
        self.assertFalse(allow.allows("https://reader@github.com/stackhands/eb-stack/blob/main/.private/notes.md"))
        self.assertFalse(allow.allows("https://reader:token@github.com:443/stackhands/eb-stack/raw/main/.private/notes.md"))
        self.assertFalse(allow.allows("https://RAW.githubusercontent.com/STACKHANDS/eb-stack/main/.private/notes.md"))
        self.assertTrue(allow.allows("https://github.com:443/stackhands/eb-stack/blob/main/README.md"))
        # The document path keeps its case: a differently cased name is another file.
        self.assertFalse(allow.allows(f"{REPO}/blob/main/readme.md"))
        self.assertTrue(allow.allows(f"{REPO}/blob/main/docs/Orgmode/Index.org"))
        # Another repository of the same owner is another site for this manifest.
        self.assertTrue(allow.allows("https://github.com/HaoZeke/other/blob/main/.private/notes.md"))

    def test_degenerate_repository_paths_are_refused(self):
        allow = self.manifest()
        self.assertFalse(allow.allows(f"{REPO}/blob/main/"))
        self.assertFalse(allow.allows(f"{REPO}/blob/main"))
        self.assertFalse(allow.allows(f"{REPO}/blob/main//etc/passwd"))

    def test_violations_name_the_file_and_url(self):
        found = guard.violations(
            "llms.txt",
            f"[a]({REPO}/blob/main/README.md) [b]({REPO}/blob/main/.private/x.md)",
            self.manifest(),
        )
        self.assertEqual(len(found), 1)
        self.assertIn(".private/x.md", found[0])
        self.assertIn("llms.txt", found[0])


if __name__ == "__main__":
    unittest.main()
