"""Generated exports retain provenance without writing review state."""
import re
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from review_core.exports import export_run, EXPORT_NAMES, _anchor
from review_core.store import Store


class ExportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = Store(Path(self.temp.name) / 'run', create=True, metadata={'revision': 'current'})
        self.addCleanup(self.store.close)
        self.approval = {'result': 'BLOCKED', 'revision': 'current', 'blockers': ['challenge missing']}

    def row(self, kind, record_id, **data):
        return self.store.put_record(kind, record_id, data)

    def test_exports_derive_selection_history_and_immutable_decisions(self):
        self.row('option', 'option', revision='current', title='choice')
        self.row('answer', 'unused', revision='current', option_id='option')
        self.row('question', 'question', revision='current', status='unanswered', answer_id=None)
        self.row('source', 'source', revision='current', locator='https://example.test', access_date='2026-11-06', sha256='digest', excerpt='cited excerpt')
        self.row('claim', 'claim', revision='current', verification='verified', inference=True, contradictions=['other'])
        self.row('recommendation', 'recommendation', revision='current', text='proposed action', case_basis='observed case', claim_ids=['claim'])
        self.row('coverage', '../../outside', revision='old', paths=['a'], status='reviewed')
        self.store.append_event('authorization.recorded', 'grant', {'decision': 'approved'}, actor='operator')
        before = self.store.connection.total_changes
        events = self.store.events()
        paths = export_run(self.store, self.approval)
        self.assertEqual(set(EXPORT_NAMES), set(paths))
        self.assertEqual(before, self.store.connection.total_changes)
        self.assertEqual(events, self.store.events())
        self.assertIn('not selected', Path(paths['OPTIONS.md']).read_text())
        self.assertIn('Historical revisions', Path(paths['COVERAGE.md']).read_text())
        self.assertIn('authorization.recorded', Path(paths['DECISIONS.md']).read_text())
        self.assertIn('challenge missing', Path(paths['SUMMARY.md']).read_text())
        for detail in ('cited excerpt', 'https://example.test', 'digest', 'Contradictions', 'observed case', 'claims:'):
            self.assertIn(detail, Path(paths['SUMMARY.md']).read_text())
        self.row('question', 'question', revision='current', status='answered', answer_id='unused')
        export_run(self.store, self.approval)
        self.assertIn('| selected | [unused]', Path(paths['OPTIONS.md']).read_text())

    def test_transcript_escapes_content_and_only_links_existing_safe_anchors(self):
        evil = '\"><script>alert(1)</script><img src=x onerror=alert(2)>'
        self.row('question', evil, revision='current', text=evil, answer_id='javascript:alert(1)')
        self.row('finding', 'finding', revision='current', claim=evil, question_id=evil)
        paths = export_run(self.store, self.approval)
        text = Path(paths['TRANSCRIPT.html']).read_text()
        self.assertNotIn('<script', text)
        self.assertNotIn('<img', text)
        self.assertIn('&lt;script&gt;', text)
        links = re.findall('href="([^"]+)"', text)
        anchors = set(re.findall('id="([^"]+)"', text))
        self.assertTrue(links)
        self.assertTrue(all(link.startswith('#record-') and link[1:] in anchors for link in links))

    def test_markdown_is_inert_and_readable(self):
        evil = '<img src=x onerror=alert(1)>\n# forged | [go](javascript:alert(1))'
        self.store.set_meta('revision', evil)
        self.row('option', evil, revision=evil, title=evil, author='reviewer', description='short option')
        self.row('coverage', 'coverage', revision=evil, paths=['src/a.py'], status='partial', author='reviewer')
        self.row('source', 'source', revision=evil, locator='https://example.test', access_date='2026-11-06',
                 sha256='digest', excerpt='short excerpt', content='UNNEEDED FULL SOURCE')
        self.row('claim', 'claim', revision=evil, text='supported claim', source_ids=['source'], verification='verified')
        self.row('recommendation', 'recommendation', revision=evil, text='recommended action', case_basis='actual case', claim_ids=['claim'])
        paths = export_run(self.store, self.approval)
        for name in ('SUMMARY.md', 'OPTIONS.md'):
            text = Path(paths[name]).read_text()
            self.assertNotIn('<img', text)
            self.assertNotIn('\n# forged', text)
            self.assertNotIn('[go](javascript:', text)
        summary = Path(paths['SUMMARY.md']).read_text()
        self.assertNotIn('UNNEEDED FULL SOURCE', summary)
        self.assertIn('recommended action', summary)
        self.assertIn('actual case', summary)
        self.assertIn('verified', summary)
        self.assertIn('TRANSCRIPT.html#' + _anchor('source', 'source'), summary)
        self.assertIn('| ID | Title | Author | Selection |', Path(paths['OPTIONS.md']).read_text())
        self.assertIn('| Path | Status |', Path(paths['COVERAGE.md']).read_text())

    def test_typed_reference_links_do_not_confuse_duplicate_ids_or_prose(self):
        self.row('question', 'shared', revision='current', text='shared', finding_ids=['shared'])
        self.row('finding', 'shared', revision='current', claim='shared')
        self.row('answer', 'answer', revision='current', question_id='shared', text='shared')
        paths = export_run(self.store, self.approval)
        text = Path(paths['TRANSCRIPT.html']).read_text()
        self.assertIn('<dt>question_id</dt><dd><a href="#' + _anchor('question', 'shared') + '">shared</a>', text)
        self.assertIn('<dt>finding_ids</dt><dd><ul><li><a href="#' + _anchor('finding', 'shared') + '">shared</a>', text)
        self.assertIn('<dt>text</dt><dd>shared</dd>', text)

    def test_capture_remains_consistent_when_writer_commits_during_readiness(self):
        self.row('finding', 'before', revision='current', claim='original finding')
        def check(store):
            revision = store.get_meta('revision')
            with Store(store.run_dir) as writer:
                writer.set_meta('revision', 'next')
                writer.put_record('finding', 'after', {'revision': 'next', 'claim': 'later finding'})
            return {'result': 'BLOCKED', 'revision': revision, 'blockers': []}
        with patch('review_core.cli.readiness', side_effect=check):
            paths = export_run(self.store)
        summary = Path(paths['SUMMARY.md']).read_text()
        self.assertIn('original finding', summary)
        self.assertNotIn('later finding', summary)
        self.assertEqual('next', self.store.get_meta('revision'))

    def test_refuses_root_and_broken_destination_symlinks(self):
        root = self.store.run_dir / 'exports'
        root.symlink_to(Path(self.temp.name) / 'absent', target_is_directory=True)
        with self.assertRaises(ValueError):
            export_run(self.store, self.approval)
        root.unlink()
        root.mkdir()
        (root / 'SUMMARY.md').symlink_to(Path(self.temp.name) / 'absent')
        with self.assertRaises(ValueError):
            export_run(self.store, self.approval)
        self.assertFalse((root / 'OPTIONS.md').exists())

    def test_default_uses_combined_readiness_and_readonly_store(self):
        self.store.close()
        with Store(Path(self.temp.name) / 'run', readonly=True) as store:
            with patch('review_core.cli.readiness', return_value=self.approval) as readiness:
                paths = export_run(store)
            readiness.assert_called_once_with(store)
            self.assertIn('BLOCKED', Path(paths['SUMMARY.md']).read_text())


if __name__ == '__main__':
    unittest.main()
