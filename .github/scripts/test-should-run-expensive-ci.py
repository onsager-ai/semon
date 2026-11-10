from contextlib import chdir
import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path
import tomllib

SCRIPT = Path(__file__).with_name("should-run-expensive-ci.py")
SPEC = importlib.util.spec_from_file_location("ci_classifier", SCRIPT)
CLASSIFIER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CLASSIFIER)
ROOT = SCRIPT.parents[2]


class ClassifierTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.repo = Path(self.temp.name)
        subprocess.run(["git", "init", "-q", str(self.repo)], check=True)
        subprocess.run(["git", "-C", str(self.repo), "config", "user.name", "CI"], check=True)
        subprocess.run(["git", "-C", str(self.repo), "config", "user.email", "ci@example.invalid"], check=True)
        (self.repo / "README.md").write_text("base\n")
        self.base = self.commit("base")

    def tearDown(self):
        self.temp.cleanup()

    def commit(self, message):
        subprocess.run(["git", "-C", str(self.repo), "add", "-A"], check=True)
        subprocess.run(["git", "-C", str(self.repo), "commit", "-qm", message], check=True)
        return subprocess.run(
            ["git", "-C", str(self.repo), "rev-parse", "HEAD"],
            check=True, capture_output=True, text=True,
        ).stdout.strip()

    def classify(self, event="pull_request"):
        head = subprocess.run(
            ["git", "-C", str(self.repo), "rev-parse", "HEAD"],
            check=True, capture_output=True, text=True,
        ).stdout.strip()
        with chdir(self.repo):
            return CLASSIFIER.should_run(event, self.base, head)

    def tier(self, *paths):
        return CLASSIFIER.classify_paths([path.encode() for path in paths])

    def test_docs_asi_full_smoke_tiers(self):
        self.assertEqual(self.tier("docs/guide.md"), "none")
        self.assertEqual(self.tier("crates/semon-asi/src/lib.rs"), "none")
        self.assertEqual(self.tier("ui/src/app.tsx"), "full")
        self.assertEqual(self.tier("tests/ui/fixture-data/sample.json"), "full")
        self.assertEqual(self.tier("crates/semon-sessions/src/viewer.rs"), "full")
        self.assertEqual(self.tier("crates/semon-sessions/tests/viewer_server.rs"), "full")
        self.assertEqual(self.tier(".github/workflows/ui.yml"), "full")
        self.assertEqual(self.tier(".github/scripts/should-run-expensive-ci.py"), "full")
        self.assertEqual(self.tier("Cargo.lock"), "smoke")
        self.assertEqual(self.tier("crates/semon-cli/src/main.rs"), "smoke")
        self.assertEqual(self.tier("crates/semon-asi/src/lib.rs", "Cargo.lock"), "smoke")

    def test_classifier_contract(self):
        self.assertTrue(CLASSIFIER.should_run("pull_request", self.base, self.base))
        self.assertTrue(CLASSIFIER.should_run("push", self.base, self.base))
        self.assertTrue(CLASSIFIER.should_run("workflow_dispatch", self.base, self.base))

    def test_rust_binary_crates_do_not_depend_on_asi(self):
        for crate in ("semon-cli", "semon-push", "semon-sessions"):
            manifest = ROOT / "crates" / crate / "Cargo.toml"
            data = tomllib.loads(manifest.read_text())
            for section in ("dependencies", "dev-dependencies", "build-dependencies"):
                self.assertNotIn("semon-asi", data.get(section, {}), f"{manifest}: {section}")

    def test_classifier_failure_propagates(self):
        with self.assertRaises(subprocess.CalledProcessError):
            CLASSIFIER.should_run("pull_request", "missing-base", "missing-head")


if __name__ == "__main__":
    unittest.main()
