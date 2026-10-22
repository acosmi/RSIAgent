"""Finite non-Rust source gates and byte-exact console receipt controls."""
from __future__ import annotations

import base64
import copy
import io
import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import non_rust_delivery as delivery

ROOT = Path(__file__).resolve().parents[1]


class NonRustDeliveryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="rsia-non-rust-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        for relative in delivery.FRONTEND_PATHS + delivery.PYTHON_PATHS + delivery.CI_PATHS:
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / relative, path)
        self.environment = patch.dict(os.environ, {}, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.source_sha = "a" * 40
        self.register_raw = (self.root / delivery.REGISTER).read_bytes()
        self.register_sha = delivery._sha(self.register_raw)

    def edit_json(self, path, change):
        target = self.root / path
        value = json.loads(target.read_bytes())
        change(value)
        target.write_text(json.dumps(value))

    def fake_git(self, argv, **kwargs):
        self.assertEqual(kwargs["cwd"], self.root)
        self.assertIs(kwargs["capture_output"], True)
        self.assertIs(kwargs["check"], False)
        self.assertNotIn("shell", kwargs)
        self.assertEqual(argv[:5], ["git", "-c", "core.hooksPath=" + os.devnull, "-c", "core.fsmonitor=false"])
        env = kwargs["env"]
        self.assertEqual(env["GIT_CONFIG_NOSYSTEM"], "1")
        self.assertEqual(env["GIT_CONFIG_GLOBAL"], os.devnull)
        self.assertEqual(env["GIT_CONFIG_SYSTEM"], os.devnull)
        self.assertEqual(env["GIT_OPTIONAL_LOCKS"], "0")
        for key in ("GIT_DIR", "GIT_WORK_TREE", "GIT_CONFIG_COUNT", "GIT_TEMPLATE_DIR"):
            self.assertNotIn(key, env)
        outputs = {
            ("rev-parse", "--show-toplevel"): os.fsencode(str(self.root)) + b"\n",
            ("rev-parse", "--verify", "HEAD"): self.source_sha.encode() + b"\n",
            ("status", "--porcelain=v1", "-z", "--untracked-files=all"): b"",
            ("show", "HEAD:" + delivery.REGISTER): self.register_raw,
        }
        return subprocess.CompletedProcess(argv, 0, outputs[tuple(argv[5:])], b"")

    def restore(self, console):
        return delivery.restore_console(console, expected_source_sha=self.source_sha, expected_register_sha256=self.register_sha)

    def verify(self, receipt):
        return delivery.verify_local_copies(self.root, receipt, expected_source_sha=self.source_sha, expected_register_sha256=self.register_sha)

    def fake_child(self, argv, **kwargs):
        self.assertEqual(kwargs["cwd"], self.root)
        self.assertIs(kwargs["capture_output"], True)
        self.assertIs(kwargs["check"], False)
        self.assertNotIn("shell", kwargs)
        if argv == ["node", "--version"]:
            return subprocess.CompletedProcess(argv, 0, b"v26.10.0\n", b"")
        if "discover" in argv:
            return subprocess.CompletedProcess(argv, 0, b"\x00python stdout\xff\r\n", b"..s\nRan 3 tests in 0.01s\n\nOK (skipped=1)\n")
        return subprocess.CompletedProcess(argv, 0, b"\x00\xffcomplete\r\n" * 4000, b"original stderr\xfe\n")

    def execute_fake(self, child=None, git=None):
        output = io.StringIO()
        def dispatch(argv, **kwargs):
            # Source Git is a separate read-only preflight, not a Node/Python case.
            if argv[0] == "git":
                return (git or self.fake_git)(argv, **kwargs)
            return (child or self.fake_child)(argv, **kwargs)
        with patch.object(delivery.subprocess, "run", side_effect=dispatch):
            code, receipt = delivery.run_delivery(self.root, console=output)
        return code, receipt, output.getvalue()

    def change_final(self, console, change):
        lines = console.splitlines()
        index = next(i for i, line in enumerate(lines) if line.startswith(delivery.RECEIPT_MARKER))
        receipt = json.loads(lines[index][len(delivery.RECEIPT_MARKER):])
        change(receipt)
        lines[index] = delivery.RECEIPT_MARKER + json.dumps(receipt)
        return "\n".join(lines) + "\n"

    def test_exact_declaration_covers_eight_modules_and_separate_evidence(self):
        self.assertEqual(delivery.inspect_contract(self.root), [])
        self.assertEqual(len(delivery.NODE_SOURCES), 8)
        self.assertEqual(sum("--check" in argv for _, argv in delivery.commands()), 8)
        declared = json.loads((self.root / delivery.REGISTER).read_bytes())
        self.assertIs(declared["evidence"]["declaration_only"], True)
        self.assertEqual(declared["evidence"]["new_verified_subscopes"], [])
        self.assertNotIn("actual_exit_code", declared)
        self.assertNotIn("upload-artifact", delivery.WORKFLOW)
        self.assertIn("package-manager-cache: false", delivery.WORKFLOW)

    def test_missing_frontend_workflow_is_a_real_contract_failure(self):
        (self.root / delivery.WORKFLOW_PATH).unlink()
        self.assertTrue(delivery.inspect_contract(self.root))

    def test_missing_module_or_unregistered_python_source_refuses(self):
        (self.root / delivery.NODE_SOURCES[0]).unlink()
        self.assertTrue(delivery.inspect_contract(self.root))
        shutil.copyfile(ROOT / delivery.NODE_SOURCES[0], self.root / delivery.NODE_SOURCES[0])
        (self.root / "scripts/undeclared.py").write_text("pass\n")
        self.assertTrue(delivery.inspect_contract(self.root))

    def test_symlink_source_is_refused_without_changing_the_referent(self):
        path = self.root / delivery.NODE_SOURCES[0]
        outside = self.root / "outside"
        outside.write_bytes(path.read_bytes())
        before = outside.read_bytes()
        path.unlink()
        path.symlink_to(outside)
        self.assertTrue(delivery.inspect_contract(self.root))
        self.assertEqual(outside.read_bytes(), before)

    def test_unknown_workflow_extra_sender_or_mutable_action_refuses(self):
        path = self.root / delivery.WORKFLOW_PATH
        source = path.read_text()
        for changed in (
            source + "      - run: curl https://unapproved.invalid\n",
            source.replace("249970729cb0ef3589644e2896645e5dc5ba9c38", "v6"),
            source.replace("        run: python3 -B", "        continue-on-error: true\n        run: python3 -B"),
            source.replace("        run: python3 -B", "        if: false\n        run: python3 -B"),
        ):
            with self.subTest(changed=changed):
                path.write_text(changed)
                self.assertTrue(delivery.inspect_contract(self.root))
        path.write_text(source)
        (self.root / ".github/workflows/unknown.yaml").write_text("name: hidden\n")
        self.assertTrue(delivery.inspect_contract(self.root))

    def test_no_entry_and_self_confirmed_second_entry_do_not_authorize_egress(self):
        path = self.root / "docs/ci-egress-register.json"
        original = path.read_bytes()
        self.edit_json("docs/ci-egress-register.json", lambda value: value.update(entries=[]))
        self.assertTrue(delivery.inspect_contract(self.root))
        path.write_bytes(original)
        self.edit_json("docs/ci-egress-register.json", lambda value: value["entries"].append(dict(value["entries"][0], id="new_sender", confirmed=True)))
        self.assertTrue(delivery.inspect_contract(self.root))

    def test_each_approved_egress_field_is_independent_of_self_reported_hash(self):
        path = self.root / "docs/ci-egress-register.json"
        original = path.read_bytes()
        for key, value in (
            ("recipient", "https://other.invalid"),
            ("data_scope", "only a title"),
            ("secret_names", ["OTHER_SECRET"]),
            ("trigger", "all closed PRs"),
            ("purpose", "different purpose"),
            ("workflow_sha256", "f" * 64),
            ("job", "other"),
            ("step_index", 1),
            ("confirmed", 1),
            ("confirmation", {"authority": "self", "observed_at": "now", "scope": "all"}),
        ):
            with self.subTest(key=key):
                path.write_bytes(original)
                self.edit_json("docs/ci-egress-register.json", lambda data: data["entries"][0].update({key: value}))
                self.assertTrue(delivery.inspect_contract(self.root))

    def test_notify_header_body_secret_recipient_and_hash_self_update_refuse(self):
        workflow = self.root / delivery.APPROVED_NOTIFY["workflow"]
        original = workflow.read_text()
        register = self.root / "docs/ci-egress-register.json"
        original_register = register.read_bytes()
        for old, new in (
            ("if: github.event.pull_request.merged == true", "if: true"),
            ("types: [closed]", "types: [opened, closed]"),
            ("GROKBOT_WEBHOOK_KEY", "OTHER_KEY"),
            ("--data-binary @", "--data @"),
            (delivery.APPROVED_NOTIFY["recipient"], "https://other.invalid"),
            ("Content-Type: application/json", "Content-Type: text/plain"),
        ):
            with self.subTest(old=old):
                workflow.write_text(original.replace(old, new, 1))
                register.write_bytes(original_register)
                self.edit_json("docs/ci-egress-register.json", lambda data: data["entries"][0].update(workflow_sha256=delivery._sha(workflow.read_bytes())))
                self.assertTrue(delivery.inspect_contract(self.root))

    def test_duplicate_unknown_paths_claims_and_types_are_not_accepted(self):
        path = self.root / delivery.REGISTER
        original = path.read_text()
        changes = (
            lambda value: value.update(unexpected=True),
            lambda value: value["groups"]["frontend"]["paths"].append("../outside"),
            lambda value: value["groups"]["python"]["paths"].append(value["groups"]["python"]["paths"][0]),
            lambda value: value["evidence"].update(new_verified_subscopes=["V116"]),
            lambda value: value["evidence"].update(cargo_target_credit=True),
            lambda value: value["evidence"].update(declaration_only=1),
        )
        for change in changes:
            path.write_text(original)
            self.edit_json(delivery.REGISTER, change)
            self.assertTrue(delivery.inspect_contract(self.root))
        path.write_text(original[:-2] + ',"schema_version":"duplicate"}')
        self.assertTrue(delivery.inspect_contract(self.root))

    def test_binary_originals_roundtrip_from_console_and_local_logs(self):
        code, receipt, console = self.execute_fake()
        self.assertEqual(code, 0)
        restored, streams = self.restore(console)
        self.assertEqual(restored, receipt)
        self.assertEqual(streams["node-check-1"]["stdout"], b"\x00\xffcomplete\r\n" * 4000)
        self.assertEqual(streams["node-check-1"]["stderr"], b"original stderr\xfe\n")
        self.assertEqual(receipt["python_unittest_summary"], {"tests": 3, "skipped": 1, "terminal_ok": True})
        self.assertIsNone(receipt["location"]["job_url"])
        self.assertIsNone(receipt["location"]["run_url"])
        self.verify(receipt)

    def test_failed_child_stays_numeric_and_remaining_commands_stay_null(self):
        def child(argv, **kwargs):
            if "--check" in argv:
                return subprocess.CompletedProcess(argv, 17, b"entire failed stdout\xff", b"entire failed stderr\x00")
            return self.fake_child(argv, **kwargs)
        code, receipt, console = self.execute_fake(child)
        self.assertEqual(code, 1)
        self.assertEqual(receipt["commands"][1]["exit_code"], 17)
        for row in receipt["commands"][2:]:
            self.assertEqual(row["state"], "not_run")
            self.assertIsNone(row["exit_code"])
            self.assertIsNone(row["stdout"])
            self.assertIsNone(row["stderr"])
        _, streams = self.restore(console)
        self.assertEqual(streams["node-check-1"]["stderr"], b"entire failed stderr\x00")
        self.verify(receipt)

    def test_launch_refusal_is_not_an_invented_child_exit(self):
        def child(*args, **kwargs):
            raise FileNotFoundError("fixture node absent")
        code, receipt, console = self.execute_fake(child)
        self.assertEqual(code, 1)
        first = receipt["commands"][0]
        self.assertEqual(first["state"], "launch_failed")
        self.assertIsNone(first["exit_code"])
        self.assertIn("fixture node absent", first["launch_error"])
        self.assertEqual(set(self.restore(console)[1]), {name for name, _ in delivery.source_commands()})

    def test_bad_preflight_runs_no_child_and_reports_unrun_null(self):
        (self.root / delivery.WORKFLOW_PATH).unlink()
        output = io.StringIO()
        with patch.object(delivery.subprocess, "run") as child:
            code, receipt = delivery.run_delivery(self.root, console=output)
        child.assert_not_called()
        self.assertEqual(code, 1)
        self.assertTrue(receipt["preflight_errors"])
        self.assertTrue(all(row["exit_code"] is None for row in receipt["commands"]))
        self.restore(output.getvalue())

    def test_wrong_node_version_is_not_hidden_behind_numeric_zero(self):
        def child(argv, **kwargs):
            return subprocess.CompletedProcess(argv, 0, b"v25.0.0\n", b"")
        code, receipt, console = self.execute_fake(child)
        self.assertEqual(code, 1)
        self.assertEqual(receipt["commands"][0]["exit_code"], 0)
        self.assertTrue(receipt["receipt_errors"])
        self.restore(console)

    def test_skip_summary_and_source_credit_tampering_refuse(self):
        _, _, console = self.execute_fake()
        for mutate in (
            lambda value: value.update(new_verified_subscopes=["E00"]),
            lambda value: value["python_unittest_summary"].update(skipped=0),
            lambda value: value["commands"][0]["stdout"].update(sha256="a" * 64),
            lambda value: value["commands"][0].update(exit_code=23),
            lambda value: value["location"].update(job_url="https://github.com/example/repo/actions/runs/1/job/2"),
        ):
            with self.assertRaises(ValueError):
                self.restore(self.change_final(console, mutate))

    def test_missing_markers_and_local_receipt_or_stream_tampering_refuse(self):
        _, receipt, console = self.execute_fake()
        lines = console.splitlines()
        for prefix in (delivery.BEGIN_MARKER, delivery.RESULT_MARKER, delivery.RECEIPT_MARKER):
            index = next(i for i, line in enumerate(lines) if line.startswith(prefix))
            with self.assertRaises(ValueError):
                self.restore("\n".join(lines[:index] + lines[index + 1:]))
        path = self.root / receipt["commands"][0]["stdout"]["path"]
        path.write_bytes(b"changed")
        with self.assertRaises(ValueError):
            self.verify(receipt)
        local_receipt = self.root / receipt["local_receipt_path"]
        local_receipt.unlink()
        with self.assertRaises(OSError):
            self.verify(receipt)

    def test_source_identity_comes_from_git_with_matching_or_absent_github_sha(self):
        for github in (None, self.source_sha):
            with self.subTest(github_sha=github), patch.dict(os.environ, {"GITHUB_SHA": github} if github else {}, clear=True):
                code, receipt, console = self.execute_fake()
                self.assertEqual(code, 0)
                self.assertEqual(receipt["source_sha"], self.source_sha)
                self.assertEqual(receipt["github_sha"], github)
                self.assertEqual(receipt["register_sha256"], self.register_sha)
                self.assertEqual(len(receipt["commands"]), len(delivery.commands()))
                self.assertEqual([row["argv"] for row in receipt["source_git"]], [argv for _, argv in delivery.source_commands()])
                self.assertTrue(all(row["exit_code"] == 0 for row in receipt["source_git"]))
                _, streams = self.restore(console)
                self.assertEqual(streams["source-head"]["stdout"], self.source_sha.encode() + b"\n")
                self.assertEqual(streams["source-register"]["stdout"], self.register_raw)
                self.verify(receipt)

    def test_github_mismatch_or_dirty_checkout_runs_no_delivery_command(self):
        for github, dirty in (("b" * 40, b""), ("", b""), (self.source_sha, b" M scripts/non_rust_delivery.py\x00"), (self.source_sha, b"?? undeclared\x00")):
            with self.subTest(github_sha=github, dirty=dirty), patch.dict(os.environ, {"GITHUB_SHA": github}):
                def git(argv, **kwargs):
                    result = self.fake_git(argv, **kwargs)
                    if "status" in argv:
                        return subprocess.CompletedProcess(argv, 0, dirty, b"")
                    return result
                def no_delivery(*args, **kwargs):
                    self.fail("source refusal must precede every Node/Python command")
                code, receipt, console = self.execute_fake(no_delivery, git)
                self.assertEqual(code, 1)
                self.assertTrue(receipt["preflight_errors"])
                self.assertEqual(receipt["source_sha"], self.source_sha)
                self.assertTrue(all(row["state"] == "not_run" and row["exit_code"] is None and row["stdout"] is None and row["stderr"] is None for row in receipt["commands"]))
                _, streams = self.restore(console)
                self.assertEqual(streams["source-status"]["stdout"], dirty)
                self.verify(receipt)

    def test_git_failure_preserves_numeric_exit_raw_and_remaining_null(self):
        def git(argv, **kwargs):
            if argv[-2:] == ["--verify", "HEAD"]:
                return subprocess.CompletedProcess(argv, 128, b"entire Git stdout\xff", b"entire Git stderr\x00")
            return self.fake_git(argv, **kwargs)
        def no_delivery(*args, **kwargs):
            self.fail("failed source Git cannot run a delivery command")
        code, receipt, console = self.execute_fake(no_delivery, git)
        self.assertEqual(code, 1)
        self.assertIsNone(receipt["source_sha"])
        self.assertIsNone(receipt["register_sha256"])
        self.assertEqual(receipt["source_git"][1]["exit_code"], 128)
        for row in receipt["source_git"][2:] + receipt["commands"]:
            self.assertEqual(row["state"], "not_run")
            self.assertIsNone(row["exit_code"])
            self.assertIsNone(row["stdout"])
            self.assertIsNone(row["stderr"])
        _, streams = self.restore(console)
        self.assertEqual(streams["source-head"], {"stdout": b"entire Git stdout\xff", "stderr": b"entire Git stderr\x00"})
        self.verify(receipt)

    def test_git_launch_refusal_is_null_and_cannot_fabricate_a_source(self):
        def git(*args, **kwargs):
            raise FileNotFoundError("fixture Git absent")
        code, receipt, console = self.execute_fake(git=git)
        self.assertEqual(code, 1)
        self.assertEqual(receipt["source_git"][0]["state"], "launch_failed")
        self.assertIsNone(receipt["source_git"][0]["exit_code"])
        self.assertIsNone(receipt["source_sha"])
        self.assertIsNone(receipt["register_sha256"])
        self.assertEqual(self.restore(console)[1], {})
        self.verify(receipt)

    def test_working_register_must_equal_actual_head_blob(self):
        # Whitespace preserves the static declaration; even a falsely clean
        # status mock cannot substitute different bytes for the committed blob.
        (self.root / delivery.REGISTER).write_bytes(self.register_raw + b"\n")
        code, receipt, console = self.execute_fake()
        self.assertEqual(code, 1)
        self.assertIn("working declaration differs from the register blob at actual HEAD", receipt["preflight_errors"])
        self.assertTrue(all(row["exit_code"] is None for row in receipt["commands"]))
        self.restore(console)
        with self.assertRaises(ValueError):
            self.verify(receipt)

    def test_recovery_and_local_copies_require_independent_source_pins(self):
        _, receipt, console = self.execute_fake()
        for key, value in (("source_sha", "b" * 40), ("register_sha256", "c" * 64)):
            changed = copy.deepcopy(receipt)
            changed[key] = value
            with self.assertRaises(ValueError):
                self.restore(self.change_final(console, lambda final: final.update({key: value})))
            with self.assertRaises(ValueError):
                self.verify(changed)
        for source, register in (("b" * 40, self.register_sha), (self.source_sha, "c" * 64), (None, self.register_sha), (self.source_sha, None)):
            with self.assertRaises(ValueError):
                delivery.restore_console(console, expected_source_sha=source, expected_register_sha256=register)
            with self.assertRaises(ValueError):
                delivery.verify_local_copies(self.root, receipt, expected_source_sha=source, expected_register_sha256=register)

    def test_inherited_git_redirect_config_or_hook_environment_is_not_used(self):
        with patch.dict(os.environ, {"GIT_DIR": "/untrusted", "GIT_WORK_TREE": "/untrusted", "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "core.hooksPath", "GIT_CONFIG_VALUE_0": "/untrusted", "GIT_TEMPLATE_DIR": "/untrusted"}):
            code, receipt, console = self.execute_fake()
        self.assertEqual(code, 0)
        self.assertEqual(receipt["source_sha"], self.source_sha)
        self.restore(console)

    def test_actual_hosted_location_uses_run_attempt_and_job_name_without_api(self):
        env = {"GITHUB_ACTIONS": "true", "GITHUB_SERVER_URL": "https://github.com", "GITHUB_REPOSITORY": "example/repo", "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2", "GITHUB_JOB": "frontend"}
        location = delivery.console_location(env)
        self.assertEqual(location["run_url"], "https://github.com/example/repo/actions/runs/123")
        self.assertEqual(location["attempt_url"], location["run_url"] + "/attempts/2")
        self.assertEqual(location["job_name"], "frontend")
        self.assertIsNone(location["job_url"])
        with self.assertRaises(ValueError):
            delivery.console_location(dict(env, GITHUB_RUN_ID=""))


if __name__ == "__main__":
    unittest.main()
