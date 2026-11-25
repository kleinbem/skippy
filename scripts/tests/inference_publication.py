"""Exercise Parquet, the card and the atomic commit against a local Hub double."""

from pathlib import Path
from runpy import run_path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from inference_nightly import PR_PATH, SOURCE, smoke_receipt
from lib.ci_artifacts import read_json
from lib.inference_parquet import read_run


class Dataset:
    def __init__(self, root):
        self.root = root
        self.version = 1
        self.directory = root / str(self.version)
        self.directory.mkdir()
        (self.directory / "README.md").write_text("Preview", encoding="utf-8")

    @property
    def sha(self):
        return f"{self.version:040d}"

    def dataset_info(self, repo, *, token):
        assert token is False
        return SimpleNamespace(id=repo, private=False, gated=False, sha=self.sha, siblings=[
            SimpleNamespace(rfilename=path.relative_to(self.directory).as_posix())
            for path in self.directory.rglob("*") if path.is_file()
        ])

    def download(self, repo, path, *, repo_type, revision, token):
        assert repo_type == "dataset" and token is False and revision == self.sha
        return self.directory / path

    def create_commit(self, *, repo_id, repo_type, operations, parent_commit, commit_message):
        assert repo_type == "dataset" and parent_commit == self.sha
        self.version += 1
        target = self.root / str(self.version)
        target.mkdir()
        additions = {
            operation["path_in_repo"]: operation.get("path_or_fileobj")
            for operation in operations
        }
        public = {
            path.relative_to(self.directory).as_posix(): path
            for path in self.directory.rglob("*") if path.is_file()
        }
        public.update(additions)
        for name, source in public.items():
            if source is not None:
                destination = target / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(source.read_bytes())
        self.directory = target
        return SimpleNamespace(commit_url=f"https://example.invalid/{self.sha}")


class PublicationTests(unittest.TestCase):
    def test_failed_run_is_published_and_does_not_replace_saved_baseline(self):
        publish = run_path(str(PR_PATH))["publish"]
        with TemporaryDirectory() as directory, patch("lib.inference_nightly_results.print", create=True):
            root = Path(directory)
            dataset = Dataset(root)
            evidence = root / "evidence"

            def release(source, run_id, number, started_at):
                return publish(dataset, dataset.download, dict, dict, evidence,
                               (source, run_id, "1", number, started_at, False))

            first_stamp = "2026-11-21T00:00:00Z"
            smoke_receipt(evidence, SOURCE, "23")
            first = release(SOURCE, "23", 1, first_stamp)
            self.assertEqual(first["outcome"], "failed")
            self.assertEqual(dataset.version, 2)
            initial = read_json(dataset.directory / "baselines.json")
            self.assertEqual(len(initial["baselines"]), 2)
            self.assertEqual(read_run(dataset.directory / "runs/23/1.parquet"), first)
            self.assertTrue((dataset.directory / "charts/qualification-1.png").is_file())
            self.assertEqual(release(SOURCE, "23", 1, first_stamp), first)
            self.assertEqual(dataset.version, 2)

            second_stamp = "2026-11-22T00:00:00Z"
            other_source = "b" * 40
            smoke_receipt(evidence, other_source, "24", value=90)
            second = release(other_source, "24", 2, second_stamp)
            self.assertEqual(second["outcome"], "failed")
            self.assertEqual(second["baseline_revision"], f"{2:040d}")
            current = read_json(dataset.directory / "baselines.json")
            self.assertEqual(current["baselines"], initial["baselines"])
            self.assertEqual(current["history"], ["runs/24/1.parquet", "runs/23/1.parquet"])
            card = (dataset.directory / "README.md").read_text(encoding="utf-8")
            self.assertIn("## Latest results", card)
            self.assertIn("| -10.00% | Regression |", card)
            self.assertIn("## Performance over time", card)
            self.assertNotIn("job_fixture", card)
            self.assertEqual(read_run(dataset.directory / "runs/24/1.parquet"), second)
            with self.assertRaisesRegex(ValueError, "older nightly"):
                release(SOURCE, "22", 1, first_stamp)


if __name__ == "__main__":
    unittest.main()
