#!/usr/bin/env python3
"""Mutation checks for the closed production writer inventory."""
from __future__ import annotations
import sys
import tempfile
import unittest
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
from check_production_store_open import check_tree, lexical_code

ROOT = Path(__file__).resolve().parents[1]


class ProductionOpenTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="rsia-writer-inventory-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        for relative in ("apps/rsia/src/main.rs", "crates/evo-engine/src/bin/formal_local_host.rs", "crates/evo-engine/src/formal_authority.rs", "crates/evo-engine/src/data_directory_lock.rs", "crates/evo-storage/src/lib.rs"):
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text((ROOT / relative).read_text())
        self.formal = self.root / "crates/evo-engine/src/bin/formal_local_host.rs"
        self.authority = self.root / "crates/evo-engine/src/formal_authority.rs"
        self.assertEqual(check_tree(self.root), [])

    def mutate(self, old, new):
        text = self.formal.read_text()
        self.assertIn(old, text)
        self.formal.write_text(text.replace(old, new, 1))

    def extra(self, text):
        path = self.root / "crates/evo-engine/src/extra.rs"
        path.unlink(missing_ok=True)
        self.assertEqual(check_tree(self.root), [])
        path.write_text(text)

    def test_current_production_tree_is_registered(self):
        self.assertEqual(check_tree(ROOT), [])

    def test_new_unregistered_production_writer_is_rejected(self):
        self.extra("async fn rogue() { Store::open(path).await; }")
        self.assertTrue(check_tree(self.root))

    def test_renamed_import_and_type_aliases_are_rejected(self):
        for source in ("use evo_storage::Store as Hidden; async fn rogue() { Hidden::open(path).await; }", "type Hidden = evo_storage::Store; async fn rogue() { Hidden::open(path).await; }", "type First = Store; type Hidden = First; async fn rogue() { Hidden::open(path).await; }"):
            with self.subTest(source=source):
                self.extra(source)
                self.assertTrue(check_tree(self.root))

    def test_alias_reexport_across_source_files_is_rejected(self):
        self.extra("pub use evo_storage::Store as Hidden;")
        other = self.root / "apps/rsia/src/extra.rs"
        other.write_text("use evo_engine::extra::Hidden; async fn rogue() { Hidden::open(path).await; }")
        self.assertTrue(check_tree(self.root))

    def test_function_reference_and_qualified_type_are_rejected(self):
        for source in ("fn rogue() { let open = Store::open; }", "async fn rogue() { <Store>::open(path).await; }"):
            with self.subTest(source=source):
                self.extra(source)
                self.assertTrue(check_tree(self.root))

    def test_test_items_comments_and_literals_do_not_create_writers(self):
        self.extra("""// Store::open(path)
/* nested /* Store::open(x) */ comment */
const TEXT: &str = r###"Store::open(path) { }"###;
const C: char = '{';
#[cfg(test)] mod tests { fn fixture() { Store::open(path); } }
#[cfg(test)] async fn fixture() { Store::open(path).await; }
""")
        self.assertEqual(check_tree(self.root), [])

    def mutate_authority_paths(self, variants):
        for path, old, new in variants:
            with self.subTest(path=path.name,old=old):
                self.assertEqual(check_tree(self.root), [])
                original=path.read_text()
                self.assertIn(old,original)
                start=original.index("pub async fn serve_owned") if path==self.authority else 0
                self.assertIn(old,original[start:])
                path.write_text(original[:start]+original[start:].replace(old,new,1))
                self.assertTrue(check_tree(self.root))
                path.write_text(original)

    def test_missing_guard_is_rejected(self):
        self.mutate_authority_paths([(self.formal,"mount_gate(std::path::Path::new(database), None)","other_gate(std::path::Path::new(database), None)"),
            (self.authority,"mount_gate(&database, None)","other_gate(&database, None)")])

    def test_guard_on_another_directory_is_rejected(self):
        self.mutate_authority_paths([(self.formal,"mount_gate(std::path::Path::new(database), None)","mount_gate(std::path::Path::new(another_database), None)"),
            (self.authority,"mount_gate(&database, None)","mount_gate(&another_database, None)")])

    def test_early_guard_drop_is_rejected(self):
        self.mutate_authority_paths([(self.formal,"let store = Store::open","drop(_data_lock); let store = Store::open"),
            (self.authority,"let store = Store::open","drop(data_lock); let store = Store::open")])

    def test_guard_hidden_in_inner_scope_is_rejected(self):
        for path,token in [(self.formal,"let (_data_lock, gate)"),(self.authority,"let (data_lock, gate)")]:
            self.assertEqual(check_tree(self.root), [])
            original=path.read_text();self.assertIn(token,original)
            start=original.index("pub async fn serve_owned") if path==self.authority else 0
            changed=original[start:].replace(token,"{ "+token,1)
            self.assertIn("let store = Store::open",changed)
            path.write_text(original[:start]+changed.replace("let store = Store::open","} let store = Store::open",1))
            self.assertTrue(check_tree(self.root));path.write_text(original)

    def test_early_authority_socket_is_rejected(self):
        self.mutate_authority_paths([(self.authority,"let service =","let listener = tokio::net::UnixListener::bind(path); let service ="),
            (self.formal,"    evo_engine::formal_authority::PrincipalAuthorityService::serve_owned(","    let listener = tokio::net::UnixListener::bind(socket); evo_engine::formal_authority::PrincipalAuthorityService::serve_owned(")])

    def test_missing_admission_and_store_close_are_rejected(self):
        self.mutate_authority_paths([(path,old,new) for path in (self.formal,self.authority)
            for old,new in (("gate.admit()","gate.skip()"),("store.close().await","store.skip().await"))])

    def test_alternative_lock_identity_is_rejected(self):
        path = self.root / "crates/evo-engine/src/data_directory_lock.rs"
        original = path.read_text()
        for old, new in (("directory.join(DATA_LOCK_FILE)", "directory.join(OTHER_LOCK_FILE)"), (".truncate(false)", ".truncate(true)")):
            with self.subTest(old=old):
                path.write_text(original)
                self.assertEqual(check_tree(self.root), [])
                self.assertIn(old, original)
                path.write_text(original.replace(old, new, 1))
                self.assertTrue(check_tree(self.root))
                path.write_text(original)
        path.write_text(original)

    def test_cli_extra_bootstrap_caller_is_rejected(self):
        path = self.root / "apps/rsia/src/main.rs"
        path.write_text(path.read_text() + "\nasync fn rogue() { bootstrap(&data, &namespace).await; }\n")
        self.assertTrue(check_tree(self.root))

    def test_failed_open_without_awaited_pool_close_is_rejected(self):
        path = self.root / "crates/evo-storage/src/lib.rs"
        original = path.read_text()
        for old, new in (("pool.close().await;", "drop(pool);"), (".connect_lazy_with(opts)", ".connect_with(opts).await?")):
            with self.subTest(old=old):
                path.write_text(original)
                self.assertEqual(check_tree(self.root), [])
                self.assertIn(old, original)
                path.write_text(original.replace(old, new, 1))
                self.assertTrue(check_tree(self.root))
                path.write_text(original)
        path.write_text(original)

    def test_broken_lexical_input_is_rejected(self):
        self.extra('/* unclosed comment')
        self.assertTrue(check_tree(self.root))
        for text in ('"unclosed', 'r##"unclosed'):
            with self.subTest(text=text), self.assertRaises(ValueError):
                lexical_code(text)


if __name__ == "__main__": unittest.main()
