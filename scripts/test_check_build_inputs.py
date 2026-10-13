"""Build-input counterexamples use isolated Git/FS fixtures, never Cargo/rustup."""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from check_build_inputs import ACTIONS, LOCK_GATE, TOOLCHAIN, inspect_tree, workflow_inputs

ROOT = Path(__file__).resolve().parents[1]


class BuildInputsTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="rsia-build-input-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / ".github/workflows").mkdir(parents=True)
        for path in ["Cargo.lock", "rust-toolchain.toml", ".github/workflows/ci.yml", ".github/workflows/dependency-audit.yml", ".github/workflows/offline-build-kit.yml"]:
            shutil.copyfile(ROOT / path, self.root / path)
        self.env = os.environ.copy()
        self.env.update(GIT_AUTHOR_NAME="Build fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid", GIT_COMMITTER_NAME="Build fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid")
        self.git("init", "-q", "--template=", "--initial-branch=main")
        blob = self.git("hash-object", "-w", "--stdin", data=(self.root / "Cargo.lock").read_bytes()).strip()
        tree = self.git("mktree", data=b"100644 blob " + blob + b"\tCargo.lock\n").strip()
        commit = self.git("-c", "commit.gpgsign=false", "commit-tree", tree.decode(), data=b"isolated lock fixture\n").strip()
        self.git("update-ref", "HEAD", commit.decode())
        self.environment = patch.dict(os.environ, {"RUSTUP_TOOLCHAIN": TOOLCHAIN})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def git(self, *args: str, data: bytes | None = None) -> bytes:
        proc = subprocess.run(["git", "-C", str(self.root), *args], input=data, capture_output=True, env=self.env, check=False)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        return proc.stdout

    def errors(self) -> list[str]:
        return inspect_tree(self.root)[0]

    def mutate_ci(self, old: str, new: str) -> None:
        path = self.root / ".github/workflows/ci.yml"
        text = path.read_text()
        self.assertIn(old, text)
        path.write_text(text.replace(old, new, 1))

    def test_committed_lock_and_exact_inputs_are_read_only(self) -> None:
        before = {str(p.relative_to(self.root)): p.read_bytes() for p in self.root.rglob("*") if p.is_file() and ".git" not in p.parts}
        self.assertEqual(self.errors(), [])
        after = {str(p.relative_to(self.root)): p.read_bytes() for p in self.root.rglob("*") if p.is_file() and ".git" not in p.parts}
        self.assertEqual(after, before)

    def test_missing_lock_is_not_created(self) -> None:
        lock = self.root / "Cargo.lock"
        lock.unlink()
        self.assertTrue(self.errors())
        self.assertFalse(lock.exists())

    def test_symlink_lock_is_refused_without_following_it(self) -> None:
        lock = self.root / "Cargo.lock"
        original = lock.read_bytes()
        outside = self.root / "outside-lock"
        outside.write_bytes(original)
        lock.unlink()
        lock.symlink_to(outside)
        self.assertTrue(any("not a regular file" in x for x in self.errors()))
        self.assertEqual(outside.read_bytes(), original)

    def test_valid_comment_drift_is_refused_against_head(self) -> None:
        lock = self.root / "Cargo.lock"
        original = lock.read_bytes()
        lock.write_bytes(original + b"\n# unapproved working-tree drift\n")
        self.assertTrue(any("differs from the pinned source HEAD" in x for x in self.errors()))
        self.assertEqual(self.git("show", "HEAD:Cargo.lock"), original)

    def test_uncommitted_lock_cannot_replace_missing_git_lock(self) -> None:
        tree = self.git("mktree", data=b"").strip()
        commit = self.git("-c", "commit.gpgsign=false", "commit-tree", tree.decode(), data=b"no committed lock\n").strip()
        self.git("update-ref", "HEAD", commit.decode())
        self.assertTrue(any("committed ordinary file" in x for x in self.errors()))

    def test_floating_and_wrong_toolchain_versions_refuse(self) -> None:
        path = self.root / "rust-toolchain.toml"
        source = path.read_text()
        for value in ["stable", "beta", "nightly", "1.98", "1.98.0"]:
            with self.subTest(value=value):
                path.write_text(source.replace(TOOLCHAIN, value))
                self.assertTrue(any("toolchain must be exact" in x for x in self.errors()))
        path.write_text(source)

    def test_component_loss_and_duplicate_toml_refuse(self) -> None:
        path = self.root / "rust-toolchain.toml"
        source = path.read_text()
        path.write_text(source.replace('["rustfmt", "clippy"]', '["rustfmt"]'))
        self.assertTrue(any("retain rustfmt and clippy" in x for x in self.errors()))
        path.write_text(source + '\nchannel = "stable"\n')
        self.assertTrue(self.errors())

    def test_ambient_toolchain_cannot_override_source(self) -> None:
        with patch.dict(os.environ, {"RUSTUP_TOOLCHAIN": "stable"}):
            self.assertTrue(any("overrides the pinned toolchain" in x for x in self.errors()))

    def test_mutable_wrong_repository_and_dynamic_action_refuse(self) -> None:
        path = self.root / ".github/workflows/ci.yml"
        source = path.read_text()
        exact = "actions/checkout@" + ACTIONS["actions/checkout"]
        for value in ["actions/checkout@v4", "actions/checkout@" + "f" * 40, "other/checkout@" + ACTIONS["actions/checkout"], '"${{ inputs.action }}"', "actions/checkout@" + "a" * 39]:
            with self.subTest(value=value):
                path.write_text(source.replace(exact, value, 1))
                self.assertTrue(self.errors())
        path.write_text(source)

    def test_unsupported_quoted_flow_alias_and_folded_forms_refuse(self) -> None:
        path = self.root / ".github/workflows/ci.yml"
        source = path.read_text()
        exact = "uses: actions/checkout@" + ACTIONS["actions/checkout"]
        for value in ['"uses": "actions/checkout@v4"', '{uses: "actions/checkout@v4"}', 'uses: *hidden', 'uses: >\n          actions/checkout@v4']:
            with self.subTest(value=value):
                path.write_text(source.replace(exact, value, 1))
                self.assertTrue(any("unsupported" in x for x in self.errors()))
        path.write_text(source + '\n  hidden:\n    steps: [{"uses": "actions/checkout@v4"}]\n')
        self.assertTrue(any("unsupported" in x for x in self.errors()))

    def test_literal_quoted_action_value_keeps_immutable_identity(self) -> None:
        exact = "actions/checkout@" + ACTIONS["actions/checkout"]
        self.mutate_ci(exact, '"' + exact + '"')
        self.assertEqual(self.errors(), [])

    def test_dependency_commands_must_be_locked(self) -> None:
        for name in ["ci.yml", "dependency-audit.yml", "offline-build-kit.yml"]:
            path = self.root / ".github/workflows" / name
            source = path.read_text()
            with self.subTest(name=name):
                path.write_text(source.replace("cargo fetch --locked", "cargo fetch", 1))
                self.assertTrue(any("requires --locked" in x for x in self.errors()))
                path.write_text(source)

    def test_automatic_generation_and_gate_omission_refuse(self) -> None:
        path = self.root / ".github/workflows/ci.yml"
        source = path.read_text()
        path.write_text(source.replace("cargo fetch --locked", "cargo generate-lockfile", 1))
        self.assertTrue(any("automatic lock generation" in x for x in self.errors()))
        path.write_text(source.replace("          test -f Cargo.lock\n", "", 1))
        self.assertTrue(any("first run step" in x for x in self.errors()))

    def test_real_workflow_missing_lock_gate_stops_before_cargo(self) -> None:
        for name in ["ci.yml", "dependency-audit.yml", "offline-build-kit.yml"]:
            _, runs = workflow_inputs((self.root / ".github/workflows" / name).read_text())
            self.assertEqual(runs[0], LOCK_GATE)
            directory = self.root / ("missing-" + name)
            directory.mkdir()
            marker = directory / "called"
            fake = directory / "cargo"
            fake.write_text('#!/bin/sh\nprintf called > "$MARKER"\n')
            fake.chmod(0o700)
            env = self.env.copy()
            env.update(PATH=str(directory) + ":" + env.get("PATH", ""), MARKER=str(marker))
            result = subprocess.run(["/bin/bash", "-e", "-c", runs[0] + "\ncargo generate-lockfile"], cwd=directory, env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(marker.exists())
            self.assertFalse((directory / "Cargo.lock").exists())

    def test_missing_workflow_is_not_ignored(self) -> None:
        (self.root / ".github/workflows/offline-build-kit.yml").unlink()
        self.assertTrue(any("workflow missing" in x for x in self.errors()))

    def test_offline_lock_digest_is_bound_to_committed_source(self) -> None:
        errors, info = inspect_tree(self.root)
        self.assertEqual(errors, [])
        info["rustc"] = {"release": TOOLCHAIN, "commit-hash": "a" * 40, "host": "fixture-linux"}
        kit = self.root / "kit"
        kit.mkdir()
        (kit / "Cargo.lock").write_bytes((self.root / "Cargo.lock").read_bytes())
        manifest = kit / "build-inputs.json"
        manifest.write_text(json.dumps(info))
        self.assertEqual(inspect_tree(self.root, kit=kit)[0], [])
        with (kit / "Cargo.lock").open("ab") as stream:
            stream.write(b"\n# package changed\n")
        self.assertTrue(any("offline Cargo.lock differs" in x for x in inspect_tree(self.root, kit=kit)[0]))

    def test_offline_source_schema_and_duplicate_keys_refuse(self) -> None:
        _, info = inspect_tree(self.root)
        info["rustc"] = {"release": TOOLCHAIN, "commit-hash": "a" * 40, "host": "fixture-linux"}
        kit = self.root / "kit"
        kit.mkdir()
        (kit / "Cargo.lock").write_bytes((self.root / "Cargo.lock").read_bytes())
        p = kit / "build-inputs.json"
        for key, value in [("source_sha", "f" * 40), ("toolchain", "stable"), ("schema_version", "other")]:
            changed = dict(info)
            changed[key] = value
            p.write_text(json.dumps(changed))
            self.assertTrue(inspect_tree(self.root, kit=kit)[0])
        p.write_text(json.dumps(info)[:-1] + ',"toolchain":"1.98.1"}')
        self.assertTrue(any("duplicate" in x for x in inspect_tree(self.root, kit=kit)[0]))
        p.write_text('[]')
        self.assertTrue(any("manifest shape" in x for x in inspect_tree(self.root, kit=kit)[0]))


    def test_missing_exact_runtime_is_refused_without_installing(self) -> None:
        original = subprocess.run
        calls = []
        def observed(argv, **kwargs):
            if argv[0] == "rustup":
                calls.append(argv)
                return subprocess.CompletedProcess(argv, 1, stdout="", stderr="fixture: toolchain not installed")
            return original(argv, **kwargs)
        with patch("check_build_inputs.subprocess.run", side_effect=observed):
            self.assertTrue(any("runtime not verified" in x for x in inspect_tree(self.root, runtime=True)[0]))
        self.assertEqual(calls, [["rustup", "run", TOOLCHAIN, "rustc", "--version", "--verbose"]])
        self.assertNotIn("--install", calls[0])

    def test_offline_compiler_identity_matches_runtime_not_only_version(self) -> None:
        _, info = inspect_tree(self.root)
        info["rustc"] = {"release": TOOLCHAIN, "commit-hash": "a" * 40, "host": "fixture-linux"}
        kit = self.root / "kit"
        kit.mkdir()
        (kit / "Cargo.lock").write_bytes((self.root / "Cargo.lock").read_bytes())
        (kit / "build-inputs.json").write_text(json.dumps(info))
        original = subprocess.run
        def observed(argv, **kwargs):
            if argv[0] == "rustup":
                stdout = f"rustc {TOOLCHAIN}\nrelease: {TOOLCHAIN}\ncommit-hash: {'b' * 40}\nhost: fixture-linux\n"
                return subprocess.CompletedProcess(argv, 0, stdout=stdout, stderr="")
            return original(argv, **kwargs)
        with patch("check_build_inputs.subprocess.run", side_effect=observed):
            self.assertTrue(any("identity differs" in x for x in inspect_tree(self.root, runtime=True, kit=kit)[0]))


    def test_extra_job_cannot_hide_behind_first_jobs_gate(self) -> None:
        p = self.root / ".github/workflows/ci.yml"
        p.write_text(p.read_text() + "\n  hidden:\n    steps:\n      - run: cargo build --locked\n")
        self.assertTrue(any("job structure" in x for x in self.errors()))

    def test_step_or_duplicate_toolchain_environment_is_not_a_pin(self) -> None:
        p = self.root / ".github/workflows/ci.yml"
        source = p.read_text()
        for value in ["      RUSTUP_TOOLCHAIN: stable\n", '      "RUSTUP_TOOLCHAIN": stable\n']:
            with self.subTest(value=value):
                p.write_text(source.replace("      RUSTUP_TOOLCHAIN: '1.98.1'\n", "      RUSTUP_TOOLCHAIN: '1.98.1'\n" + value, 1))
                self.assertTrue(any("toolchain override" in x for x in self.errors()))


    def test_root_fixed_inline_run_generation_is_enumerated_and_refused(self) -> None:
        p = self.root / ".github/workflows/ci.yml"
        p.write_text(p.read_text() + "      - run: cargo generate-lockfile\n")
        self.assertTrue(any("automatic lock generation" in x for x in self.errors()))

    def test_root_fixed_step_environment_override_is_refused(self) -> None:
        p = self.root / ".github/workflows/ci.yml"
        p.write_text(p.read_text() + "      - name: Override exact toolchain\n        env:\n          RUSTUP_TOOLCHAIN: stable\n        run: cargo check --locked\n")
        self.assertTrue(any("toolchain override" in x for x in self.errors()))

    def test_command_selectors_cannot_replace_exact_job_toolchain(self) -> None:
        p = self.root / ".github/workflows/ci.yml"
        source = p.read_text()
        for command in ["cargo +stable check --locked", "cargo +${TOOLCHAIN} check --locked", "rustup run stable cargo check --locked", "rustup run ${TOOLCHAIN} cargo check --locked", "RUSTUP_TOOLCHAIN=stable cargo check --locked"]:
            with self.subTest(command=command):
                p.write_text(source + "      - run: " + command + "\n")
                self.assertTrue(any("toolchain" in x for x in self.errors()))
        for command in ["cargo +1.98.1 check --locked", "rustup run 1.98.1 cargo check --locked"]:
            with self.subTest(command=command):
                p.write_text(source + "      - run: " + command + "\n")
                self.assertEqual(self.errors(), [])


    def test_flow_and_quoted_environment_cannot_hide_toolchain_override(self) -> None:
        p = self.root / ".github/workflows/ci.yml"
        source = p.read_text()
        for env in ["env: {RUSTUP_TOOLCHAIN: stable}", '"env": {RUSTUP_TOOLCHAIN: stable}']:
            with self.subTest(env=env):
                p.write_text(source + "      - name: Flow environment override\n        " + env + "\n        run: cargo check --locked\n")
                self.assertTrue(any("unsupported" in x for x in self.errors()))


    def test_staged_lock_comment_is_refused_at_first_gate_and_checker(self) -> None:
        lock = self.root / "Cargo.lock"
        with lock.open("ab") as stream:
            stream.write(b"\n# staged drift\n")
        self.git("add", "Cargo.lock")
        result = subprocess.run(["/bin/bash", "-e", "-c", LOCK_GATE], cwd=self.root, env=self.env, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(any("differs from the pinned source HEAD" in x for x in self.errors()))


# Exact edits derived from independently frozen full R2 YAML inputs.
R3_FIXED_INPUTS = {'conditional_first_gate': {'sha256': '89da62fbc429911a151905054f94e0b7a46964abc7c4a95693aaa100e7ef317c', 'kind': 'new_supported_shape', 'edits': [('after', '      - name: Require committed lock\n', '        if: false\n')]}, 'dynamic_constant_executable': {'sha256': 'b555049533441060f73c02028cda26cb67cd32d04cd5aa09c4da270f3d84996f', 'kind': 'new_supported_shape', 'edits': [('append', "      - run: ${{ 'cargo' }} +stable check --locked\n", '')]}, 'dynamic_event_run': {'sha256': '68d49d8c31540fea969e0ee20acbb393cdd41c63bc6f0a70877e447f69258ced', 'kind': 'unsupported_dynamic_unexecuted', 'edits': [('append', '      - run: ${{ github.event.inputs.command }}\n', '')]}, 'explicit_run_key': {'sha256': 'bb6bd4ad1c2f8f7aa0cc2b22b213bd6e984cda054995c99edf5c55f191a374b9', 'kind': 'unsupported_structural_unexecuted', 'edits': [('append', '      - ? run\n        : cargo check\n', '')]}, 'explicit_uses_key': {'sha256': 'caa09808bcbeeb4019746ca1bbc446d31319a967979f63d475d705e54d186bbb', 'kind': 'unsupported_structural_unexecuted', 'edits': [('append', '      - ? uses\n        : actions/checkout@v4\n', '')]}, 'fake_locked_substitution': {'sha256': 'dc48782cc3ca857751f3bc2ba6009c9a667094d889555afb0e96df29227edefc', 'kind': 'new_supported_shape', 'edits': [('append', '      - run: cargo check $(echo --locked >/dev/null)\n', '')]}, 'input_shell_python': {'sha256': '743b853c1e1bf5ea3efb32002b321ccb58b13b903ff793dca629d32801b86eee', 'kind': 'new_supported_shape', 'edits': [('after', '        id: build_inputs\n', '        shell: python\n')]}, 'line_continued_unlocked': {'sha256': '2b774e4ef6469aa5ada2b83319518b077be6e73f10c374ac758f8f26c0109fd0', 'kind': 'new_supported_shape', 'edits': [('append', '      - run: |\n          cargo \\\n            check\n', '')]}, 'multi_job_alternative_indent': {'sha256': '7119f191e5ece99fc83e5e1d9fb583190fc367a89fb49570abb4123ff4d4d887', 'kind': 'new_supported_shape', 'edits': [('append', '\n  hidden:\n      runs-on: ubuntu-latest\n      steps:\n          - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262\n          - run: cargo check --locked\n', '')]}, 'nonblocking_lock_and_input': {'sha256': '2241c784255abe2f70707093446966a286a517f641b27b0a23d3f30c3e5a7ed7', 'kind': 'new_supported_shape', 'edits': [('after', '      - name: Require committed lock\n', '        continue-on-error: true\n'), ('after', '        id: build_inputs\n', '        continue-on-error: true\n')]}, 'python_precondition_wrong': {'sha256': '6d2088391f6f0cc082bc38c69d186a38d556d5c8889d1b7c4d012fe361d33259', 'kind': 'new_supported_shape', 'edits': [('replace', "          python-version: '3.12'\n", "          python-version: '3.9'\n")]}, 'quoted_cargo_stable_block': {'sha256': 'bb581c4d7b5203fce0a968612fcd67f1a6929ef0556aa26480239a5ce1fe5c89', 'kind': 'new_supported_shape', 'edits': [('append', '      - run: |\n          "cargo" +stable check --locked\n', '')]}, 'quoted_cargo_unlocked_block': {'sha256': '9b3a786e72c1874957acbdae632fbd7798cd6f7d08eca197de378ca2150aab97', 'kind': 'new_supported_shape', 'edits': [('append', '      - run: |\n          "cargo" check\n', '')]}, 'quoted_rustup_stable_block': {'sha256': '8451a3a2ae66f70bfbd4d655fc73ac556a1c301dc3bbfd81f3a557f0b2aa9316', 'kind': 'new_supported_shape', 'edits': [('append', '      - run: |\n          "rustup" run stable cargo check --locked\n', '')]}, 'run_key_space_before_colon': {'sha256': 'c3ec00c0fc07a8f579c713dca3b4e7c4a9acf58e1bb8f433ed3894623d9a1e27', 'kind': 'new_supported_shape', 'edits': [('append', '      - run : cargo check\n', '')]}, 'skipped_toolchain_install': {'sha256': '22a25406c3a6cdc2d88e965cc0e27d4c42ddce6fcd6e8e3e9343116bf41db437', 'kind': 'new_supported_shape', 'edits': [('after', '      - name: Exact Rust toolchain\n', '        if: false\n')]}, 'valid_outer_yaml_quote_cargo': {'sha256': '7e253302342f9ba4635fb6508c40f5a31f02b86ba0e1ad0052f07c25f131bb25', 'kind': 'new_supported_shape', 'edits': [('append', '      - run: \'"cargo" +stable check --locked\'\n', '')]}, 'valid_outer_yaml_quote_rustup': {'sha256': '4a15ae9c309e255abac9228d295a04042c6cb03c422947475e96570c29750d44', 'kind': 'new_supported_shape', 'edits': [('append', '      - run: \'"rustup" run stable cargo check --locked\'\n', '')]}}


def _fixed_r3_case(case):
    def regression(self):
        path = self.root / ".github/workflows/ci.yml"
        text = path.read_text()
        for kind, old, new in case["edits"]:
            if kind == "append":
                text += old
            elif kind == "after":
                self.assertIn(old, text)
                text = text.replace(old, old + new, 1)
            else:
                self.assertIn(old, text)
                text = text.replace(old, new, 1)
        path.write_text(text)
        self.assertTrue(self.errors(), "independent frozen input must refuse")
    return regression


for _name, _case in R3_FIXED_INPUTS.items():
    setattr(BuildInputsTest, "test_r3_frozen_" + _name, _fixed_r3_case(_case))


# R4 single mutations reconstruct the four independently frozen healthy R3 inputs.
R4_STRUCTURE_CASES = {'global_defaults_shell': ('jobs:\n', 'defaults:\n  run:\n    shell: bash --noprofile --norc {0}\njobs:\n', 'unsupported workflow/job execution default'), 'quoted_job_defaults_shell': ('    timeout-minutes: 25\n', '    timeout-minutes: 25\n    "defaults":\n      run:\n        shell: bash --noprofile --norc {0}\n', 'unsupported workflow/job execution default'), 'drop_quality_gate': ('      - name: Quality gate\n        if: always()\n        env:\n          BUILD_INPUTS: ${{ steps.build_inputs.outcome }}\n          LOCKED_INPUTS: ${{ steps.locked_inputs.outcome }}\n          FORMAT: ${{ steps.format.outcome }}\n          CHECK: ${{ steps.check.outcome }}\n          TESTS: ${{ steps.tests.outcome }}\n          CLIPPY: ${{ steps.clippy.outcome }}\n          SMOKE_WORKSPACE: ${{ steps.smoke_workspace.outcome }}\n          SMOKE: ${{ steps.smoke.outcome }}\n          MCP_STDIO: ${{ steps.mcp_stdio.outcome }}\n          PYTHON_TESTS: ${{ steps.python_tests.outcome }}\n          STATISTICS_REFERENCE: ${{ steps.statistics_reference.outcome }}\n          MIGRATION_CHECKSUMS: ${{ steps.migration_checksums.outcome }}\n          INVENTORY: ${{ steps.inventory.outcome }}\n          ARCHIVE: ${{ steps.archive.outcome }}\n        run: |\n          set -e\n          test "$BUILD_INPUTS" = success\n          test "$LOCKED_INPUTS" = success\n          test "$FORMAT" = success\n          test "$CHECK" = success\n          test "$TESTS" = success\n          test "$CLIPPY" = success\n          test "$SMOKE_WORKSPACE" = success\n          test "$SMOKE" = success\n          test "$MCP_STDIO" = success\n          test "$PYTHON_TESTS" = success\n          test "$STATISTICS_REFERENCE" = success\n          test "$MIGRATION_CHECKSUMS" = success\n          test "$INVENTORY" = success\n          test "$ARCHIVE" = success\n', '', 'unsupported required Quality gate'), 'drop_workspace_smoke': ('      - name: Workspace smoke\n        id: smoke_workspace\n        continue-on-error: true\n        run: python3 scripts/smoke_workspace.py > out/smoke-workspace.txt 2>&1\n', '', 'unsupported required Workspace smoke')}


def _fixed_r4_case(case):
    def regression(self):
        self.assertEqual(self.errors(), [], "the current healthy baseline must pass")
        old, new, reason = case
        path = self.root / ".github/workflows/ci.yml"
        text = path.read_text()
        self.assertEqual(text.count(old), 1)
        path.write_text(text.replace(old, new, 1))
        self.assertTrue(any(reason in error for error in self.errors()))
    return regression


for _name, _case in R4_STRUCTURE_CASES.items():
    setattr(BuildInputsTest, "test_r4_current_" + _name, _fixed_r4_case(_case))


def _r4_quoted_positive(command):
    def regression(self):
        self.assertEqual(self.errors(), [])
        path = self.root / ".github/workflows/ci.yml"
        path.write_text(path.read_text() + "      - run: |\n          " + command + "\n")
        self.assertEqual(self.errors(), [])
    return regression


setattr(BuildInputsTest, "test_r4_current_pinned_quoted_cargo", _r4_quoted_positive('"cargo" +1.98.1 check --locked'))
setattr(BuildInputsTest, "test_r4_current_pinned_quoted_rustup", _r4_quoted_positive('"rustup" run 1.98.1 cargo check --locked'))


if __name__ == "__main__":
    unittest.main()
