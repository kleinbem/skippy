"""Offline regressions for native recipe identity and bounded upgrade repair."""

import os
import importlib.util
import shlex
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from lib.ci_artifacts import read_json
from lib.upgrade_source import git
from lib.upgrade_repair import repair
from lib.upgrade_preimages import seed_patch_preimages

ROOT = Path(__file__).resolve().parents[2]
SHA = "a" * 40


class Repair(unittest.TestCase):
    def test_packaged_recipe_has_source_identity_without_an_architecture_roster(self):
        from lib.native_recipe import write_native_recipe
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            files = {"llama_cpp/upstream.txt": SHA,
                     "llama_cpp/patches/0001.patch": "fixture patch",
                     "llama_cpp/patches/model_support/series": "0001.patch\n",
                     "llama_cpp/patches/model_support/0001.patch": "model patch",
                     "llama_cpp/patches/generated/series": "0001.patch\n",
                     "llama_cpp/patches/generated/0001.patch": "generated patch"}
            for name, text in files.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text)
            (root / "crates/skippy-api/src").mkdir(parents=True)
            write_native_recipe(root)
            recipe = read_json(root / "crates/skippy-api/src/native-recipe.json")
            self.assertEqual(set(recipe), {"llama_upstream_sha", "patch_queue_sha256"})
            self.assertEqual(recipe["llama_upstream_sha"], SHA)
            before = recipe["patch_queue_sha256"]
            (root / "llama_cpp/patches/0001.patch").write_text("changed patch")
            write_native_recipe(root)
            self.assertNotEqual(read_json(root / "crates/skippy-api/src/native-recipe.json")["patch_queue_sha256"], before)

    def checkout(self, root):
        git(root, "init", "--initial-branch=main")
        (root / "llama_cpp").mkdir()
        (root / "llama_cpp/upstream.txt").write_text(SHA + "\n")
        git(root, "add", ".")
        git(root, "-c", "user.name=Fixture", "-c", "user.email=fixture@skippy.local",
            "-c", "commit.gpgsign=false", "commit", "-m", "base")

    def test_agent_cannot_change_qualification_or_selected_upstream(self):
        for path in ("scripts/upgrade-publish.py", "llama_cpp/upstream.txt"):
            with self.subTest(path=path), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                self.checkout(root)
                def agent(*_args, **_kwargs):
                    target = root / path
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text("changed\n")
                with patch.dict(os.environ, SKIPPY_UPGRADE_REPAIR_COMMAND='["agent"]'), \
                     patch("lib.upgrade_repair.command", side_effect=agent):
                    with self.assertRaisesRegex(ValueError, "allowlist|upstream pin"):
                        repair(root, root)

    def test_missing_agent_command_is_an_explicit_failure(self):
        with patch.dict(os.environ, SKIPPY_UPGRADE_REPAIR_COMMAND="[]"):
            with self.assertRaisesRegex(ValueError, "installed agent"):
                repair(ROOT, ROOT)


class RunnerTools(unittest.TestCase):
    def module(self):
        spec = importlib.util.spec_from_file_location("upgrade_runner", ROOT / "scripts/upgrade-runner-check.py")
        result = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(result)
        return result

    def test_missing_repair_executable_fails_before_any_session(self):
        runner = self.module()
        with patch.dict(os.environ, SKIPPY_UPGRADE_REPAIR_COMMAND='["goose","run","--text"]'), \
             patch.object(runner.shutil, "which", return_value=None), patch.object(runner, "command") as command:
            with self.assertRaisesRegex(ValueError, "repair executable is not installed"):
                runner.check()
            command.assert_not_called()

    def test_preflight_checks_local_tools_and_version_without_starting_agent(self):
        runner = self.module()
        with tempfile.TemporaryDirectory() as tmp:
            llvm = Path(tmp)
            (llvm / "bin").mkdir()
            (llvm / "bin/clang").touch()
            (llvm / "lib").mkdir()
            with patch.dict(os.environ, SKIPPY_UPGRADE_REPAIR_COMMAND='["goose","run","--text"]',
                            SKIPPY_REWRITER_LLVM_PREFIX=str(llvm), LIBCLANG_PATH=str(llvm / "lib")), \
                 patch.object(runner.shutil, "which", side_effect=lambda name: "/usr/bin/" + name), \
                 patch.object(runner, "command") as command:
                runner.check()
                command.assert_called_once_with(["/usr/bin/goose", "--version"], timeout=30)
                (llvm / "bin/clang").unlink()
                with self.assertRaisesRegex(ValueError, "LLVM/Clang"):
                    runner.check()


class PatchPreimages(unittest.TestCase):
    def commit(self, root, message):
        git(root, "add", ".")
        git(root, "-c", "user.name=Fixture", "-c", "user.email=fixture@skippy.local",
            "-c", "commit.gpgsign=false", "commit", "-m", message)
        return git(root, "rev-parse", "HEAD")

    def test_cold_queue_needs_old_patch_results_and_seed_preserves_selected_pin(self):
        with tempfile.TemporaryDirectory() as tmp:
            sandbox = Path(tmp)
            upstream = sandbox / "upstream"
            upstream.mkdir()
            git(upstream, "init", "--initial-branch=main")
            original = [f"line {index}\n" for index in range(40)]
            (upstream / "native.cpp").write_text("".join(original))
            base = self.commit(upstream, "upstream base")
            git(upstream, "checkout", "-b", "patches")
            first = original.copy()
            first[2] = "first patch\n"
            (upstream / "native.cpp").write_text("".join(first))
            patch1 = self.commit(upstream, "first patch")
            preimage = git(upstream, "rev-parse", patch1 + ":native.cpp")
            second = first.copy()
            second[20] = "second patch\n"
            (upstream / "native.cpp").write_text("".join(second))
            patch2 = self.commit(upstream, "second patch")
            git(upstream, "checkout", "main")
            updated = original.copy()
            updated[23] = "new upstream context\n"
            (upstream / "native.cpp").write_text("".join(updated))
            target = self.commit(upstream, "upstream changed patch context")

            checkout = sandbox / "checkout"
            patches = checkout / "llama_cpp/patches"
            patches.mkdir(parents=True)
            for index, commit in enumerate((patch1, patch2), 1):
                (patches / f"{index:04d}-fixture.patch").write_text(
                    git(upstream, "format-patch", "-1", commit, "--stdout") + "\n")
            pin = checkout / "llama_cpp/upstream.txt"
            pin.write_text(target + "\n")
            (checkout / "Justfile").write_text(
                "llama-prepare:\n    @bash " + shlex.quote(str(ROOT / "scripts/prepare-llama.sh")) + "\n")
            native = checkout / ".deps/llama.cpp"
            native.mkdir(parents=True)
            git(native, "init", "--initial-branch=main")
            git(native, "remote", "add", "origin", str(upstream))
            git(native, "fetch", "--no-tags", "origin", base, target)
            git(native, "checkout", "--detach", target)
            git(native, "config", "user.name", "Fixture")
            git(native, "config", "user.email", "fixture@skippy.local")
            self.assertNotEqual(subprocess.run(
                ["git", "-C", str(native), "cat-file", "-e", preimage], capture_output=True).returncode, 0)
            mail = [str(path) for path in sorted(patches.glob("*.patch"))]
            cold = subprocess.run(["git", "-C", str(native), "am", "--3way", *mail],
                                  capture_output=True, text=True)
            self.assertNotEqual(cold.returncode, 0)
            self.assertIn("could not build fake ancestor", cold.stderr)
            with patch.dict(os.environ, LLAMA_UPSTREAM_URL=str(upstream)):
                seed_patch_preimages(checkout, base, patches)
            self.assertEqual(pin.read_text().strip(), target)
            git(native, "cat-file", "-e", preimage)
            git(native, "checkout", "--detach", target)
            git(native, "am", "--3way", *mail)
            expected = second.copy()
            expected[23] = updated[23]
            self.assertEqual((native / "native.cpp").read_text(), "".join(expected))

    def test_unchanged_pin_needs_no_seed_or_native_checkout(self):
        with tempfile.TemporaryDirectory() as tmp:
            checkout = Path(tmp)
            (checkout / "llama_cpp").mkdir()
            (checkout / "llama_cpp/upstream.txt").write_text(SHA + "\n")
            with patch("lib.upgrade_preimages.command") as command:
                seed_patch_preimages(checkout, SHA, checkout / "llama_cpp/patches")
            command.assert_not_called()


if __name__ == "__main__":
    unittest.main()
