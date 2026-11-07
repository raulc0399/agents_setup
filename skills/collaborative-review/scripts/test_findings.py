"""Regression coverage for typed review records."""
import tempfile
import unittest
from pathlib import Path

from review_core.store import Store, ConflictError
from review_core import findings as f


class FindingsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.store = Store(Path(self.temp.name) / 'run', create=True, metadata={'revision': 'rev', 'snapshot': {'files': [{'path': 'src/a.py'}]}})
        self.addCleanup(self.temp.cleanup)
        self.addCleanup(self.store.close)

    def finding(self, **kwargs):
        return f.create_finding(self.store, **dict(author='reviewer', revision='rev', claim='Wrong result', proposed_fix='Correct result', paths=['src/a.py'], **kwargs))

    def test_optional_typed_locations_preserve_old_callers(self):
        self.assertNotIn('locations', self.finding()['data'])
        row = self.finding(locations=['src/a.py:2-4'])
        self.assertEqual(['src/a.py:2-4'], row['data']['locations'])
        for location in ['src/a.py:0', 'src/a.py:4-2', 'missing.py:2', 'src//a.py:2']:
            with self.subTest(location=location), self.assertRaises(ValueError):
                self.finding(locations=[location])
        with self.assertRaises(ValueError):
            f.update_finding(self.store, finding_id=row['id'], actor='r', paths=[])

    def test_stable_id_and_complete_gap_list(self):
        first = self.finding(finding_id='f1', verified=True, evidence=['a.py:3'])
        self.assertEqual(first, self.finding(finding_id='f1', verified=True, evidence=['a.py:3']))
        for index in range(12):
            self.finding(finding_id=f'other-{index}', verified=True, evidence=['a.py:3'])
        self.assertEqual(13, len(f.finding_gaps(self.store)))
        self.assertEqual(13, len(self.store.events()))
        with self.assertRaises(ValueError):
            self.finding(finding_id='f1')

    def test_evidence_gates_and_conflict(self):
        with self.assertRaises(ValueError):
            self.finding(verified=True)
        row = self.finding(finding_id='f1')
        with self.assertRaises(ValueError):
            f.update_finding(self.store, finding_id='f1', actor='r', status='resolved')
        closed = f.update_finding(self.store, finding_id='f1', actor='r', status='resolved', resolution_evidence=['test passes'])
        self.assertEqual('resolved', closed['data']['status'])
        with self.assertRaises(ConflictError):
            f.update_finding(self.store, finding_id='f1', actor='r', claim='Changed', expected_version=row['version'])

    def test_paths_revision_and_types(self):
        for path in ('../secret', '/etc/passwd', 'src/../a.py', 'src//a.py', 'missing.py', 'C:\\a'):
            with self.subTest(path=path), self.assertRaises(ValueError):
                f.create_coverage(self.store, author='r', revision='rev', paths=[path], status='unreviewed')
        with self.assertRaises(ValueError):
            f.create_question(self.store, author='r', revision='old', text='Why?')
        with self.assertRaises(ValueError):
            self.finding(blocking='yes')

    def test_question_answer_and_option_references_atomic(self):
        question = f.create_question(self.store, author='r', revision='rev', text='Which fix?', question_id='q')
        with self.assertRaises(ValueError):
            f.answer_question(self.store, question_id='q', author='o', revision='rev', text='This', option_id='missing')
        option = f.create_option(self.store, author='r', revision='rev', title='Fix', description='Use fix', option_id='o')
        answer = f.answer_question(self.store, question_id='q', author='o', revision='rev', text='This', option_id=option['id'], answer_id='a')
        self.assertEqual(answer, f.answer_question(self.store, question_id='q', author='o', revision='rev', text='This', option_id='o', answer_id='a'))
        with self.assertRaises(ValueError):
            f.answer_question(self.store, question_id='q', author='o', revision='rev', text='Other', answer_id='b')
        self.assertIsNone(self.store.get_record('answer', 'b'))
        self.assertEqual([], f.finding_gaps(self.store))
        self.assertEqual('unanswered', question['data']['status'])

    def test_updates_and_stale_records(self):
        self.finding(finding_id='f')
        with self.assertRaises(ValueError):
            f.create_question(self.store, author='r', revision='rev', text='?', finding_ids=['absent'])
        f.create_question(self.store, author='r', revision='rev', text='?', question_id='q')
        f.update_question(self.store, question_id='q', actor='r', blocking=False, finding_ids=['f'])
        f.create_option(self.store, author='r', revision='rev', title='A', description='B', option_id='o')
        f.update_option(self.store, option_id='o', actor='r', title='C')
        f.create_coverage(self.store, author='r', revision='rev', paths=['src/a.py'], status='unreviewed', coverage_id='c')
        with self.assertRaises(ValueError):
            f.update_coverage(self.store, coverage_id='c', actor='r', status='reviewed')
        f.update_coverage(self.store, coverage_id='c', actor='r', status='reviewed', evidence=['Read file'])
        self.store.set_meta('revision', 'next')
        self.assertEqual([], f.finding_gaps(self.store))
        with self.assertRaises(ValueError):
            f.update_finding(self.store, finding_id='f', actor='r', claim='Old')

    def test_substantive_edits_invalidate_decisions(self):
        self.finding(finding_id='f', verified=True, evidence=['old proof'], status='resolved', resolution_evidence=['old closure'])
        changed = f.update_finding(self.store, finding_id='f', actor='r', claim='Different claim')
        self.assertEqual('open', changed['data']['status'])
        self.assertEqual([], changed['data']['resolution_evidence'])
        self.assertFalse(changed['data']['verified'])
        self.assertEqual([], changed['data']['evidence'])

    def test_question_edit_invalidates_answer(self):
        f.create_question(self.store, author='r', revision='rev', text='Old?', question_id='q')
        f.answer_question(self.store, question_id='q', author='r', revision='rev', text='Yes', answer_id='a')
        changed = f.update_question(self.store, question_id='q', actor='r', text='Different?')
        self.assertEqual('unanswered', changed['data']['status'])
        self.assertIsNone(changed['data']['answer_id'])

    def test_coverage_scope_and_selected_options_cannot_reuse_decisions(self):
        self.store.set_meta('snapshot', {'files': [{'path': 'src/a.py'}, {'path': 'src/b.py'}]})
        f.create_coverage(self.store, author='r', revision='rev', paths=['src/a.py'], status='reviewed', evidence=['Read a'], coverage_id='c')
        changed = f.update_coverage(self.store, coverage_id='c', actor='r', paths=['src/b.py'])
        self.assertEqual('unreviewed', changed['data']['status'])
        self.assertEqual([], changed['data']['evidence'])

    def test_selected_option_is_frozen(self):
        f.create_question(self.store, author='r', revision='rev', text='Which?', question_id='q')
        f.create_option(self.store, author='r', revision='rev', title='Old', description='Old choice', option_id='o')
        f.answer_question(self.store, question_id='q', author='r', revision='rev', text='Choose old', option_id='o')
        with self.assertRaises(ValueError):
            f.update_option(self.store, option_id='o', actor='r', description='New choice')
        self.assertEqual('Old choice', self.store.get_record('option', 'o')['data']['description'])

    def test_explicit_new_evidence_can_revalidate_changed_records(self):
        self.finding(finding_id='f', verified=True, evidence=['old'], status='resolved', resolution_evidence=['closed'])
        changed = f.update_finding(self.store, finding_id='f', actor='r', claim='New', evidence=['new proof'], verified=True, status='resolved', resolution_evidence=['new closure'])
        self.assertEqual('resolved', changed['data']['status'])
        self.assertTrue(changed['data']['verified'])
        with self.assertRaises(ValueError):
            f.update_finding(self.store, finding_id='f', actor='r', claim='Another', status='resolved')

    def test_old_answer_cannot_be_replayed_after_question_edit(self):
        f.create_question(self.store, author='r', revision='rev', text='Old?', question_id='q')
        f.answer_question(self.store, question_id='q', author='r', revision='rev', text='Yes', answer_id='a')
        f.update_question(self.store, question_id='q', actor='r', text='New?')
        with self.assertRaises(ValueError):
            f.answer_question(self.store, question_id='q', author='r', revision='rev', text='Yes', answer_id='a')
        self.assertEqual('unanswered', self.store.get_record('question', 'q')['data']['status'])
        self.assertEqual(1, len(f.finding_gaps(self.store)))

    def test_reopening_clears_old_closure_evidence(self):
        self.finding(finding_id='f', status='resolved', resolution_evidence=['Old closure'])
        row = f.update_finding(self.store, finding_id='f', actor='r', status='open')
        self.assertEqual([], row['data']['resolution_evidence'])
        with self.assertRaises(ValueError):
            f.update_finding(self.store, finding_id='f', actor='r', status='resolved')
        f.update_finding(self.store, finding_id='f', actor='r', status='resolved', resolution_evidence=['New closure'])


if __name__ == '__main__':
    unittest.main()
