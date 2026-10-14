"""Real small archive bytes plus isolated strict-dependency input controls.

Synthetic manifests/locks are test data. They do not claim a production build or
full-lock legal clearance. All filesystem mutations are in TemporaryDirectory.
"""
from __future__ import annotations
import base64
import copy
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import check_dependency_register as checker

FS2_ARCHIVE = base64.b64decode('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')
FS2_CHECKSUM = "9564fc758e15025b46aa6643b1b77d047d1a56a1aea6e01002ac0c7026876213"
FS2_METADATA_SHA = "c47bb59c1d58dc1eb439331bb140e3a4174370f83b0f0b702d41eb64b27cda3f"


class DependencyRegisterTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="rsia-dependency-test-")
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.root = self.base / "repo"
        self.root.mkdir()
        self.home_parent = self.base / "home"
        self.home = self.home_parent / ".cargo"
        self.transport = self.home / "registry/cache/fixture"
        self.transport.mkdir(parents=True)
        self.archive = self.transport / "fs2-0.4.3.crate"
        self.archive.write_bytes(FS2_ARCHIVE)
        self.root.joinpath("docs").mkdir()
        self.root.joinpath("LICENSE").write_text("Private synthetic test license; no release claim.\n")
        root_toml = '[workspace]\nmembers = ' + json.dumps(list(checker.MEMBERS)) + '\nresolver = "2"\n[workspace.package]\nversion = "0.2.0"\nlicense-file = "LICENSE"\npublish = false\n[workspace.dependencies]\nfs2 = "=0.4.3"\n'
        self.root.joinpath("Cargo.toml").write_text(root_toml)
        for member in checker.MEMBERS:
            directory = self.root / member
            directory.mkdir(parents=True)
            name = "rsia" if member == "apps/rsia" else member.split("/")[-1]
            body = f'[package]\nname = "{name}"\nversion.workspace = true\nlicense-file.workspace = true\npublish.workspace = true\n'
            if member == "crates/evo-core":
                body += '[dependencies]\nfs2.workspace = true\n'
            elif member == "apps/rsia":
                body += '[dependencies]\n'
                for package in checker.MEMBERS:
                    if package.startswith("crates/"):
                        body += package.split('/')[-1] + ' = { path = "../../' + package + '" }\n'
            directory.joinpath("Cargo.toml").write_text(body)
        lock = 'version = 4\n'
        for member in checker.MEMBERS:
            name = "rsia" if member == "apps/rsia" else member.split("/")[-1]
            lock += f'\n[[package]]\nname = "{name}"\nversion = "0.2.0"\n'
        lock += f'\n[[package]]\nname = "fs2"\nversion = "0.4.3"\nsource = "{checker.SOURCE}"\nchecksum = "{FS2_CHECKSUM}"\n'
        self.root.joinpath("Cargo.lock").write_text(lock)
        self.write_documents()

    def write_documents(self):
        inputs, locked, locals_, edges = checker.graph(self.root)
        registry = [checker.primary_record(row, self.home) for row in locked if row.get("source")]
        local_license = {"path": "LICENSE", "sha256": checker.digest((self.root / "LICENSE").read_bytes())}
        self.inventory = {"schema_version": checker.INVENTORY_SCHEMA, "inputs": inputs, "registry_packages": registry, "workspace_packages": [dict(row, license_file=local_license) for row in locals_]}
        by_id = {checker.identity(row): row for row in registry}
        grouped = {}
        for edge in edges:
            grouped.setdefault(edge['identity'], {'path': edge['path'], 'consumers': []})['consumers'].append(edge['consumer'])
        external, internal = [], []
        for key, group in sorted(grouped.items()):
            consumers = sorted(group['consumers'], key=checker.consumer_key)
            if key[2]:
                row = by_id[key]
                external.append({**{name: row[name] for name in ('name', 'version', 'source', 'checksum', 'license')}, 'purpose': 'Portable lock test input.', 'alternative': 'Explicit platform adapter.', 'consumers': consumers})
            else:
                internal.append({'name': key[0], 'version': key[1], 'path': group['path'], 'consumers': consumers})
        self.register = {'schema_version': checker.REGISTER_SCHEMA, 'inputs': inputs, 'inventory_sha256': '', 'source_allowlist': [checker.SOURCE], 'license_allowlist': copy.deepcopy(checker.LICENSE_ALLOWLIST), 'legacy_license_exceptions': copy.deepcopy(checker.LEGACY_EXCEPTIONS), 'dependencies': external, 'internal_packages': internal}
        self.save_documents()

    def save_documents(self):
        raw = checker.canonical(self.inventory)
        (self.root / 'docs/dependency-license-inventory.json').write_bytes(raw)
        self.register['inventory_sha256'] = checker.digest(raw)
        (self.root / 'docs/dependency-register.json').write_bytes(checker.canonical(self.register))

    def errors(self, *, complete=True):
        return checker.dependency_register_errors(self.root, complete=complete, fixture_root=self.home)

    def refused(self, category, *, complete=True):
        errors = self.errors(complete=complete)
        self.assertTrue(any(error.startswith(category + ':') for error in errors), errors)

    def crafted(self, extras=(), metadata=None):
        if metadata is None:
            with tarfile.open(fileobj=io.BytesIO(FS2_ARCHIVE), mode='r:gz') as archive:
                metadata = archive.extractfile('fs2-0.4.3/Cargo.toml').read()
        out = io.BytesIO()
        with tarfile.open(fileobj=out, mode='w') as archive:
            info = tarfile.TarInfo('fs2-0.4.3/Cargo.toml')
            info.size = len(metadata)
            archive.addfile(info, io.BytesIO(metadata))
            for member, data in extras:
                if member.isfile():
                    member.size = len(data)
                archive.addfile(member, io.BytesIO(data) if member.isfile() else None)
        return gzip.compress(out.getvalue(), mtime=0)

    def primary(self, raw):
        self.archive.write_bytes(raw)
        row = {'name': 'fs2', 'version': '0.4.3', 'source': checker.SOURCE, 'checksum': checker.digest(raw)}
        return checker.primary_record(row, self.home)

    def test_real_archive_and_primary_metadata_bytes(self):
        self.assertEqual(checker.digest(FS2_ARCHIVE), FS2_CHECKSUM)
        self.assertEqual(self.inventory['registry_packages'][0]['metadata_sha256'], FS2_METADATA_SHA)
        self.assertEqual(self.inventory['registry_packages'][0]['license'], 'MIT/Apache-2.0')
        self.assertEqual(self.inventory['registry_packages'][0]['bundled_text_status'], 'present')
        self.assertEqual(len(self.inventory['registry_packages'][0]['bundled_license_texts']), 2)
        self.assertEqual(self.errors(), [])

    def test_input_only_does_not_read_or_claim_primary(self):
        shutil.rmtree(self.home)
        errors, info = checker.inspect_dependencies(self.root, complete=False)
        self.assertEqual(errors, [])
        self.assertEqual(info['phase'], 'input_only')
        self.assertEqual(info['primary_checked'], 0)
        self.refused('PRIMARY_MISSING')

    def test_standard_cargo_default_requires_no_new_environment(self):
        with patch.dict(os.environ, {'CARGO_HOME': ''}), patch.object(Path, 'home', return_value=self.home_parent):
            self.assertEqual(checker.dependency_register_errors(self.root), [])

    def test_explicit_cargo_home_and_no_silent_default_fallback(self):
        with patch.dict(os.environ, {'CARGO_HOME': str(self.home)}):
            self.assertEqual(checker.dependency_register_errors(self.root), [])
        with patch.dict(os.environ, {'CARGO_HOME': str(self.base / 'absent')}), patch.object(Path, 'home', return_value=self.home_parent):
            errors = checker.dependency_register_errors(self.root)
            self.assertTrue(any(x.startswith('PRIMARY_MISSING:') for x in errors), errors)

    def test_fixture_root_requires_the_same_full_primary_evidence(self):
        self.assertEqual(self.errors(), [])
        self.archive.write_bytes(b'bad copy')
        self.refused('CHECKSUM')

    def test_duplicate_json_field_and_unknown_field_refused(self):
        path = self.root / 'docs/dependency-register.json'
        raw = path.read_text().replace('"schema_version": "rsia.dependency_register.v1"', '"schema_version": "rsia.dependency_register.v1", "schema_version": "rsia.dependency_register.v1"')
        path.write_text(raw)
        self.refused('SCHEMA', complete=False)
        self.save_documents()
        self.register['ignored'] = True
        self.save_documents()
        self.refused('SCHEMA', complete=False)

    def test_missing_and_wrong_typed_fields_refused(self):
        self.register['dependencies'][0]['consumers'][0]['default_features'] = 0
        self.save_documents()
        self.refused('SCHEMA', complete=False)
        self.write_documents()
        del self.inventory['registry_packages'][0]['license_file']
        self.save_documents()
        self.refused('SCHEMA', complete=False)

    def test_inventory_byte_binding_and_manifest_byte_binding(self):
        path = self.root / 'docs/dependency-license-inventory.json'
        path.write_bytes(path.read_bytes() + b'\n')
        self.refused('INPUT_BINDING', complete=False)
        self.save_documents()
        path = self.root / 'crates/evo-core/Cargo.toml'
        path.write_text(path.read_text() + '\n# byte drift\n')
        self.refused('INPUT_BINDING', complete=False)

    def test_removed_direct_identity_and_removed_target_edge(self):
        self.register['dependencies'] = []
        self.save_documents()
        self.refused('EDGE_COVERAGE', complete=False)
        path = self.root / 'crates/evo-core/Cargo.toml'
        path.write_text(path.read_text() + '\n[target.\'cfg(unix)\'.dependencies]\nfs2.workspace = true\n')
        self.write_documents()
        self.assertEqual(self.errors(), [])
        self.register['dependencies'][0]['consumers'] = [x for x in self.register['dependencies'][0]['consumers'] if x['target'] is None]
        self.save_documents()
        self.refused('EDGE_COVERAGE', complete=False)

    def test_dev_features_default_and_build_declarations_are_preserved(self):
        path = self.root / 'crates/evo-core/Cargo.toml'
        path.write_text(path.read_text() + '\n[dev-dependencies]\nfs2 = {workspace=true, features=["declared-test-feature"]}\n[build-dependencies]\nfs2.workspace = true\n')
        self.write_documents()
        self.assertEqual(self.errors(complete=False), [])
        consumers = self.register['dependencies'][0]['consumers']
        self.assertEqual({x['kind'] for x in consumers}, {'regular', 'dev', 'build'})
        dev = next(x for x in consumers if x['kind'] == 'dev')
        self.assertIn('declared-test-feature', dev['features'])
        dev['features'] = []
        self.save_documents()
        self.refused('EDGE_COVERAGE', complete=False)
        self.write_documents()
        self.register['dependencies'][0]['consumers'][0]['default_features'] = False
        self.save_documents()
        self.refused('EDGE_COVERAGE', complete=False)

    def test_self_consistent_license_and_allowlist_cannot_replace_primary(self):
        self.inventory['registry_packages'][0]['license'] = 'MIT'
        self.register['dependencies'][0]['license'] = 'MIT'
        self.save_documents()
        self.assertEqual(self.errors(complete=False), [])
        self.refused('LICENSE_MISMATCH')

    def test_whole_expression_and_exact_legacy_policy(self):
        self.inventory['registry_packages'][0]['license'] = 'GPL-3.0-only AND MIT'
        self.register['dependencies'][0]['license'] = 'GPL-3.0-only AND MIT'
        self.save_documents()
        self.refused('LICENSE_MISMATCH', complete=False)
        self.write_documents()
        self.register['license_allowlist'].append('*')
        self.save_documents()
        self.refused('LICENSE_MISMATCH', complete=False)
        with self.assertRaises(checker.DependencyError):
            checker.permitted_license({'name': 'another-package', 'version': '0.4.3', 'checksum': FS2_CHECKSUM, 'license': 'MIT/Apache-2.0'})

    def test_unapproved_source_wrong_version_duplicate_inventory(self):
        self.register['dependencies'][0]['source'] = 'registry+https://mirror.invalid/index'
        self.save_documents()
        self.refused('SOURCE', complete=False)
        self.write_documents()
        self.inventory['registry_packages'].append(copy.deepcopy(self.inventory['registry_packages'][0]))
        self.save_documents()
        self.refused('IDENTITY', complete=False)
        path = self.root / 'crates/evo-core/Cargo.toml'
        path.write_text(path.read_text().replace('fs2.workspace = true', 'fs2 = "=0.4.4"'))
        self.refused('VERSION', complete=False)

    def test_alias_and_internal_path_name_mismatch(self):
        root = self.root / 'Cargo.toml'
        core = self.root / 'crates/evo-core/Cargo.toml'
        root.write_text(root.read_text().replace('fs2 = "=0.4.3"', 'portable_lock = {version="=0.4.3", package="fs2"}'))
        core.write_text(core.read_text().replace('fs2.workspace', 'portable_lock.workspace'))
        self.write_documents()
        self.assertEqual(self.errors(), [])
        self.assertEqual(self.register['dependencies'][0]['consumers'][0]['alias'], 'portable_lock')
        app = self.root / 'apps/rsia/Cargo.toml'
        app.write_text(app.read_text().replace('../../crates/evo-core', '../../outside-member'))
        self.refused('IDENTITY', complete=False)

    def test_multiple_locked_versions_require_unique_resolution(self):
        root = self.root / 'Cargo.toml'
        root.write_text(root.read_text().replace('"=0.4.3"', '"0.4"'))
        lock = self.root / 'Cargo.lock'
        lock.write_text(lock.read_text() + f'\n[[package]]\nname="fs2"\nversion="0.4.4"\nsource="{checker.SOURCE}"\nchecksum="{FS2_CHECKSUM}"\n')
        self.refused('VERSION', complete=False)

    def test_workspace_integer_is_not_boolean_inheritance(self):
        path = self.root / 'crates/evo-core/Cargo.toml'
        path.write_text(path.read_text().replace('version.workspace = true', 'version.workspace = 1'))
        self.refused('SCHEMA', complete=False)

    def test_missing_metadata_and_symlink_member_refused(self):
        link = tarfile.TarInfo('fs2-0.4.3/link')
        link.type = tarfile.SYMTYPE
        link.linkname = 'Cargo.toml'
        with self.assertRaisesRegex(checker.DependencyError, 'ARCHIVE_FORMAT'):
            self.primary(self.crafted([(link, b'')]))
        out = io.BytesIO()
        with tarfile.open(fileobj=out, mode='w'):
            pass
        with self.assertRaisesRegex(checker.DependencyError, 'ARCHIVE_FORMAT'):
            self.primary(gzip.compress(out.getvalue(), mtime=0))

    def test_duplicate_escape_noncanonical_members_refused(self):
        for name in ('fs2-0.4.3/Cargo.toml', 'fs2-0.4.3/../escape', '/fs2-0.4.3/absolute'):
            with self.subTest(name=name):
                extra = tarfile.TarInfo(name)
                with self.assertRaisesRegex(checker.DependencyError, 'ARCHIVE_FORMAT'):
                    self.primary(self.crafted([(extra, b'x')]))
        extra = tarfile.TarInfo('fs2-0.4.3///')
        extra.type = tarfile.DIRTYPE
        with self.assertRaisesRegex(checker.DependencyError, 'ARCHIVE_FORMAT'):
            self.primary(self.crafted([(extra, b'')]))

    def test_concatenated_tail_archive_cannot_hide_a_second_member(self):
        hidden = tarfile.TarInfo('fs2-0.4.3/hidden-link')
        hidden.type = tarfile.SYMTYPE
        hidden.linkname = 'Cargo.toml'
        trailing = gzip.decompress(self.crafted([(hidden, b'')]))
        combined = gzip.compress(gzip.decompress(FS2_ARCHIVE) + trailing, mtime=0)
        with self.assertRaisesRegex(checker.DependencyError, 'ARCHIVE_FORMAT'):
            self.primary(combined)

    def test_valid_long_archive_path_is_not_confused_with_escape(self):
        extra = tarfile.TarInfo('fs2-0.4.3/' + 'a' * 140)
        result = self.primary(self.crafted([(extra, b'ordinary long file')]))
        self.assertEqual(result['license'], 'MIT/Apache-2.0')
        self.assertEqual(result['bundled_text_status'], 'not_bundled')

    def test_real_license_file_reference_must_exist(self):
        with tarfile.open(fileobj=io.BytesIO(FS2_ARCHIVE), mode='r:gz') as archive:
            metadata = archive.extractfile('fs2-0.4.3/Cargo.toml').read()
        metadata = metadata.replace(b'license = "MIT/Apache-2.0"', b'license = "MIT/Apache-2.0"\nlicense-file = "MISSING"')
        with self.assertRaisesRegex(checker.DependencyError, 'LICENSE_FILE'):
            self.primary(self.crafted(metadata=metadata))
        extra = tarfile.TarInfo('fs2-0.4.3/MISSING')
        result = self.primary(self.crafted([(extra, b'exact referenced body')], metadata=metadata))
        self.assertEqual(result['license_file'], {'path': 'fs2-0.4.3/MISSING', 'sha256': checker.digest(b'exact referenced body')})

    def test_size_count_limits_are_real_read_boundaries(self):
        for name, value in (('MAX_COMPRESSED', 1), ('MAX_INFLATED', 1), ('MAX_MEMBERS', 1), ('MAX_METADATA', 1), ('MAX_TEXT_FILES', 1)):
            with self.subTest(limit=name), patch.object(checker, name, value):
                with self.assertRaisesRegex(checker.DependencyError, 'ARCHIVE_FORMAT'):
                    checker.primary_record({'name': 'fs2', 'version': '0.4.3', 'source': checker.SOURCE, 'checksum': FS2_CHECKSUM}, self.home)

    def test_identical_mirror_copies_coalesce_but_bad_copy_never_skips(self):
        mirror = self.home / 'registry/cache/z-mirror'
        mirror.mkdir()
        mirror_archive = mirror / self.archive.name
        mirror_archive.write_bytes(FS2_ARCHIVE)
        self.assertEqual(self.errors(), [])
        mirror_archive.write_bytes(b'bad mirror')
        self.refused('CHECKSUM')
        mirror_archive.unlink()
        mirror_archive.symlink_to(self.archive)
        self.refused('IDENTITY')

    def test_fifo_candidate_is_refused_without_blocking_open(self):
        if not hasattr(os, 'mkfifo'):
            self.skipTest('FIFO not available on this platform')
        self.archive.unlink()
        os.mkfifo(self.archive)
        command = [sys.executable, str(Path(checker.__file__)), '--repo-root', str(self.root), '--fixture-root', str(self.home)]
        result = subprocess.run(command, capture_output=True, text=True, timeout=5, env={**os.environ, 'PYTHONDONTWRITEBYTECODE': '1'})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('ARCHIVE_FORMAT', result.stderr)

    def test_duplicate_register_consumer_and_local_rows_are_closed(self):
        self.register['dependencies'].append(copy.deepcopy(self.register['dependencies'][0]))
        self.save_documents()
        self.refused('IDENTITY', complete=False)
        self.write_documents()
        self.register['dependencies'][0]['consumers'].append(copy.deepcopy(self.register['dependencies'][0]['consumers'][0]))
        self.save_documents()
        self.refused('EDGE_COVERAGE', complete=False)
        self.write_documents()
        self.inventory['workspace_packages'].append(copy.deepcopy(self.inventory['workspace_packages'][0]))
        self.save_documents()
        self.refused('IDENTITY', complete=False)

    def test_archive_basename_cannot_read_outside_selected_root(self):
        for name, version in (("../../../../outside", "1.0.0"), ("fs2", "../../outside")):
            row = {"name": name, "version": version, "source": checker.SOURCE, "checksum": FS2_CHECKSUM}
            with self.subTest(name=name, version=version), self.assertRaisesRegex(checker.DependencyError, "IDENTITY"):
                checker.archive_bytes(row, self.home)
        outside = self.base / "outside.crate"
        outside.write_bytes(FS2_ARCHIVE)
        escaped = self.home / "registry/cache/fixture/../../../outside.crate"
        with self.assertRaisesRegex(checker.DependencyError, "IDENTITY"):
            checker.read_regular(escaped, "outside.crate", checker.MAX_COMPRESSED, boundary=self.home)

    def test_raw_header_escape_cannot_be_hidden_by_a_safe_pax_path(self):
        with tarfile.open(fileobj=io.BytesIO(FS2_ARCHIVE), mode='r:gz') as archive:
            metadata = archive.extractfile('fs2-0.4.3/Cargo.toml').read()
        out = io.BytesIO()
        with tarfile.open(fileobj=out, mode='w', format=tarfile.PAX_FORMAT) as archive:
            member = tarfile.TarInfo('../../outside')
            member.pax_headers = {'path': 'fs2-0.4.3/Cargo.toml'}
            member.size = len(metadata)
            archive.addfile(member, io.BytesIO(metadata))
        with self.assertRaisesRegex(checker.DependencyError, '^ARCHIVE_FORMAT:'):
            self.primary(gzip.compress(out.getvalue(), mtime=0))

    def test_malformed_deflate_has_the_stable_archive_format_category(self):
        malformed = bytes.fromhex('1f8b0800000000000003070000000000000000')
        self.assertEqual(len(malformed), 19)
        with self.assertRaisesRegex(checker.DependencyError, '^ARCHIVE_FORMAT:'):
            self.primary(malformed)

    def test_raw_name_and_ustar_prefix_reject_nonzero_suffix_after_nul(self):
        with tarfile.open(fileobj=io.BytesIO(FS2_ARCHIVE), mode='r:gz') as archive:
            metadata = archive.extractfile('fs2-0.4.3/Cargo.toml').read()
        for member_name in ('fs2-0.4.3/Cargo.toml\x00../../outside', 'fs2-0.4.3/Cargo.toml'):
            out = io.BytesIO()
            with tarfile.open(fileobj=out, mode='w', format=tarfile.USTAR_FORMAT) as archive:
                member = tarfile.TarInfo(member_name)
                member.size = len(metadata)
                archive.addfile(member, io.BytesIO(metadata))
            raw = bytearray(out.getvalue())
            if '\x00' not in member_name:
                raw[345:500] = b'\x00../../outside'.ljust(155, b'\x00')
                raw[148:156] = b'        '
                raw[148:156] = ('%06o\x00 ' % sum(raw[:512])).encode()
            with self.subTest(member=member_name), self.assertRaisesRegex(checker.DependencyError, '^ARCHIVE_FORMAT:'):
                self.primary(gzip.compress(raw, mtime=0))

    def test_inventory_provenance_SHA_and_text_status_are_not_self_authenticating(self):
        self.inventory['registry_packages'][0]['metadata_sha256'] = '0' * 64
        self.save_documents()
        self.refused('IDENTITY')
        self.write_documents()
        self.inventory['registry_packages'][0]['bundled_license_texts'][0]['sha256'] = '0' * 64
        self.save_documents()
        self.refused('IDENTITY')
        self.write_documents()
        self.inventory['registry_packages'][0]['bundled_text_status'] = 'not_bundled'
        self.save_documents()
        self.refused('SCHEMA', complete=False)


if __name__ == '__main__':
    unittest.main()
