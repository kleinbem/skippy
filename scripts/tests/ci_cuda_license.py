"""Offline regressions for the CUDA license retained by slim compiler images."""

import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "ci-preserve-cuda-license.sh"


class CudaLicense(unittest.TestCase):
    def preserve(self, root, version="12.9.2"):
        return subprocess.run(
            ["bash", str(SCRIPT), version, str(root / "cuda"), str(root / "doc")],
            capture_output=True, text=True,
        )

    def test_preserves_vendor_license_bytes_after_documentation_is_pruned(self):
        for source in ("cuda/EULA.txt", "cuda/LICENSE", "cuda/LICENSE.txt",
                       "cuda/doc/EULA.txt", "doc/cuda-cudart-12-9/copyright"):
            with self.subTest(source=source), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                (root / "cuda").mkdir()
                license_path = root / source
                license_path.parent.mkdir(parents=True, exist_ok=True)
                terms = b"Supplied NVIDIA terms\r\nCopyright fixture\n"
                license_path.write_bytes(terms)
                result = self.preserve(root)
                self.assertEqual(result.returncode, 0, result.stderr)
                shutil.rmtree(root / "doc", ignore_errors=True)
                shutil.rmtree(root / "cuda/doc", ignore_errors=True)
                self.assertEqual((root / "cuda/EULA.txt").read_bytes(), terms)

    def test_uses_the_matching_toolkit_package_through_a_symlink(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "cuda-13.1").mkdir()
            (root / "cuda").symlink_to("cuda-13.1", target_is_directory=True)
            for version in ("12-9", "13-1"):
                copyright_path = root / f"doc/cuda-cudart-{version}/copyright"
                copyright_path.parent.mkdir(parents=True)
                copyright_path.write_text(f"Terms for {version}\n")
            result = self.preserve(root, "13.1.2")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / "cuda-13.1/EULA.txt").read_text(), "Terms for 13-1\n")

    def test_missing_empty_or_other_toolkit_license_fails(self):
        for source in (None, "cuda/EULA.txt", "doc/cuda-cudart-13-1/copyright"):
            with self.subTest(source=source), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                (root / "cuda").mkdir()
                if source:
                    path = root / source
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text("" if source == "cuda/EULA.txt" else "Other toolkit terms\n")
                result = self.preserve(root)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("CUDA 12.9.2 redistribution license material was not found", result.stderr)


if __name__ == "__main__":
    unittest.main()
