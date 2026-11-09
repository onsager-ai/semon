from contextlib import chdir
import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("should-run-expensive-ci.py")
SPEC = importlib.util.spec_from_file_location("ci_classifier", SCRIPT)
CLASSIFIER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CLASSIFIER)


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
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    def classify(self, event="pull_request"):
        head = subprocess.run(
            ["git", "-C", str(self.repo), "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        with chdir(self.repo):
            return CLASSIFIER.should_run(event, self.base, head)

    def test_docs_only_and_mixed_changes(self):
        (self.repo / "guide name\nwith newline.md").write_text("docs\n")
        (self.repo / "docs").mkdir()
        (self.repo / "docs" / "readme.txt").write_text("docs tree\n")
        self.commit("documentation")
        self.assertFalse(self.classify())
        (self.repo / "src.rs").write_text("code\n")
        self.commit("mixed")
        self.assertTrue(self.classify())

    def test_shell_contract_markdown_runs_expensive_suites(self):
        (self.repo / "docs").mkdir()
        contract = self.repo / "docs" / "shell.md"
        contract.write_text("## Classes\noriginal\n")
        self.base = self.commit("add compiled shell contract")
        contract.write_text("## Classes\nchanged\n")
        self.commit("edit only compiled shell contract")
        self.assertTrue(self.classify())

    def test_deletions_and_renames_retain_code_classification(self):
        (self.repo / "old.rs").write_text("code\n")
        self.base = self.commit("add code")
        (self.repo / "old.rs").unlink()
        self.commit("delete code")
        self.assertTrue(self.classify())
        (self.repo / "old.rs").write_text("code\n")
        self.base = self.commit("restore code")
        (self.repo / "docs").mkdir()
        (self.repo / "old.rs").rename(self.repo / "docs" / "renamed.rs")
        self.commit("move code to docs")
        self.assertTrue(self.classify())

    def test_events_and_empty_diff_run_expensive_suites(self):
        self.assertTrue(self.classify())
        self.assertTrue(self.classify("push"))
        self.assertTrue(self.classify("workflow_dispatch"))

    def test_classifier_failure_propagates(self):
        with self.assertRaises(subprocess.CalledProcessError):
            CLASSIFIER.should_run("pull_request", "missing-base", "missing-head")


if __name__ == "__main__":
    unittest.main()
