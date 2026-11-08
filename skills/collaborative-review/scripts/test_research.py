"""Research provenance and readiness support tests."""
import hashlib
from pathlib import Path
import tempfile
import unittest

from review_core.store import Store
from review_core import research as r


class ResearchTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = Store(Path(self.temp.name) / 'run', create=True, metadata={'revision': 'rev'})
        self.addCleanup(self.store.close)

    def source(self, **kwargs):
        return r.add_source(self.store, author='a', revision='rev', locator='https://example.org/doc', access_date='2026-11-06', **kwargs)

    def claim(self, **kwargs):
        return r.add_claim(self.store, author='a', revision='rev', text='Result', **kwargs)

    def recommend(self, ids):
        return r.add_recommendation(self.store, author='a', revision='rev', text='Choose A', claim_ids=ids, case_basis='Project needs A')

    def test_capture_hash_excerpt_immutable_idempotent(self):
        first = self.source(content='actual supplied text', excerpt='supplied', source_id='s')
        self.assertEqual(hashlib.sha256(b'actual supplied text').hexdigest(), first['data']['sha256'])
        self.assertEqual(first, self.source(content='actual supplied text', excerpt='supplied', source_id='s'))
        with self.assertRaises(ValueError):
            self.source(content='changed', source_id='s')
        with self.assertRaises(ValueError):
            self.source(content='text', excerpt='fabricated')
        path = Path(self.temp.name) / 'source.txt'
        path.write_text('local evidence', encoding='utf-8')
        self.assertEqual('local evidence', self.source(file=str(path))['data']['content'])

    def test_independent_verification_progress_and_idempotence(self):
        self.source(content='source', source_id='s')
        self.claim(source_ids=['s'], claim_id='c', inference=True)
        self.recommend(['c'])
        self.assertTrue(r.research_gaps(self.store))
        with self.assertRaises(ValueError):
            r.verify_claim(self.store, 'c', 'a', ['checked source'])
        first = r.verify_claim(self.store, 'c', 'b', ['checked source'])
        self.assertEqual(first, r.verify_claim(self.store, 'c', 'b', ['checked source']))
        self.assertEqual('inference', first['data']['label'])
        self.assertEqual([], r.research_gaps(self.store))

    def test_contradictions_resolution_and_exhaustive_gaps(self):
        self.source(content='source', source_id='s')
        self.claim(source_ids=['s'], claim_id='c', verification='verified', evidence=['checked'], verified_by='b')
        self.claim(source_ids=['s'], claim_id='other', contradictions=['c'], impact='not-decision-changing')
        self.recommend(['c'])
        self.assertTrue(any('contradictions' in gap['reason'] for gap in r.research_gaps(self.store)))
        r.verify_claim(self.store, 'other', 'b', ['comparison resolves disagreement'], contradictions=[])
        self.assertEqual([], r.research_gaps(self.store))
        for index in range(12):
            self.claim(claim_id=f'missing-{index}')
        self.assertEqual(24, len(r.research_gaps(self.store)))

    def test_recommendation_dependencies_cannot_bypass_verification(self):
        self.claim(claim_id='c', impact='not-decision-changing')
        self.recommend(['c'])
        self.assertTrue(r.research_gaps(self.store))

    def test_unsourced_claim_can_gain_audited_support(self):
        self.claim(claim_id='c')
        self.recommend(['c'])
        self.source(content='captured', source_id='s')
        with self.assertRaises(ValueError):
            r.verify_claim(self.store, 'c', 'b', ['checked'], source_ids=['missing'])
        self.assertEqual([], self.store.get_record('claim', 'c')['data']['source_ids'])
        verified = r.verify_claim(self.store, 'c', 'b', ['checked captured source'], source_ids=['s'])
        self.assertEqual(['s'], verified['data']['source_ids'])
        self.assertEqual('sourced-claim', verified['data']['label'])
        self.assertEqual([], r.research_gaps(self.store))
        self.assertEqual('claim.verified', self.store.events()[-1]['kind'])

    def test_creation_retry_after_verification_preserves_current_record(self):
        self.source(content='captured', source_id='s')
        self.claim(claim_id='c', source_ids=['s'])
        verified = r.verify_claim(self.store, 'c', 'b', ['checked'])
        self.assertEqual(verified, self.claim(claim_id='c', source_ids=['s']))
        with self.assertRaises(ValueError):
            self.claim(claim_id='c', source_ids=['s'], inference=True)

    def test_references_validation_and_revision(self):
        with self.assertRaises(ValueError):
            self.claim(source_ids=['absent'])
        with self.assertRaises(ValueError):
            self.claim(verification='verified', verified_by='b', evidence=['checked'])
        self.source(content='source', source_id='s')
        self.claim(source_ids=['s'], claim_id='c')
        self.store.set_meta('revision', 'next')
        with self.assertRaises(ValueError):
            r.add_claim(self.store, author='a', revision='next', text='Stale', source_ids=['s'])
        with self.assertRaises(ValueError):
            r.add_recommendation(self.store, author='a', revision='next', text='Choose', claim_ids=['c'], case_basis='case')
        self.assertEqual([{'id': None, 'reason': 'No current recommendation'}], r.research_gaps(self.store))
        self.assertIsNone(self.store.get_record('claim', 'absent'))
        with Store(self.store.run_dir, readonly=True) as readonly:
            self.assertEqual(r.research_gaps(self.store), r.research_gaps(readonly))

    def test_retire_refuted_claim_and_replace_its_published_recommendation(self):
        self.source(content='Option B works; option A does not', source_id='s')
        self.claim(source_ids=['s'], claim_id='old')
        old_recommendation = self.recommend(['old'])
        r.verify_claim(self.store, 'old', 'b', ['Source refutes A'], verification='disputed')
        self.claim(source_ids=['s'], claim_id='correct', verification='verified', evidence=['Checked B'], verified_by='b')
        current = self.recommend(['correct'])
        retired = r.retire_claim(self.store, 'old', 'b', ['Refuted by captured source'], replacement_id='correct')
        self.assertTrue(any('retired' in gap['reason'] for gap in r.research_gaps(self.store)))
        r.retire_recommendation(self.store, old_recommendation['id'], 'a', ['Use corrected recommendation'],
                                replacement_id=current['id'])
        self.assertEqual([], r.research_gaps(self.store))
        self.assertEqual(retired, r.retire_claim(self.store, 'old', 'b', ['Refuted by captured source'], replacement_id='correct'))
        self.assertEqual(retired, self.claim(source_ids=['s'], claim_id='old'))
        with self.assertRaisesRegex(ValueError, 'retired'):
            r.verify_claim(self.store, 'old', 'b', ['Try to revive'])
        with self.assertRaisesRegex(ValueError, 'retired'):
            self.recommend(['old'])

    def test_retirement_validates_replacement_and_keeps_audit_history(self):
        self.claim(claim_id='old')
        self.claim(claim_id='replacement')
        for replacement in ('old', 'missing'):
            with self.subTest(replacement=replacement):
                with self.assertRaises(ValueError):
                    r.retire_claim(self.store, 'old', 'a', ['Checked'], replacement_id=replacement)
        r.retire_claim(self.store, 'replacement', 'a', ['No longer applicable'])
        with self.assertRaises(ValueError):
            r.retire_claim(self.store, 'old', 'a', ['Checked'], replacement_id='replacement')
        with self.assertRaises(ValueError):
            r.retire_claim(self.store, 'old', 'a', [])
        row = r.retire_claim(self.store, 'old', 'b', ['Refuted'])
        self.assertEqual('a', row['data']['author'])
        self.assertEqual('b', row['data']['retired_by'])
        self.assertEqual('claim.retired', self.store.events()[-1]['kind'])
        with self.assertRaises(ValueError):
            r.retire_claim(self.store, 'old', 'b', ['Different reason'])

    def test_retired_incoming_contradiction_no_longer_blocks_active_support(self):
        self.source(content='Source', source_id='s')
        self.claim(source_ids=['s'], claim_id='supported', verification='verified', evidence=['Checked'], verified_by='b')
        self.claim(claim_id='refuted', contradictions=['supported'])
        self.recommend(['supported'])
        self.assertTrue(r.research_gaps(self.store))
        r.retire_claim(self.store, 'refuted', 'b', ['Contradiction disproved'])
        self.assertEqual([], r.research_gaps(self.store))


if __name__ == '__main__':
    unittest.main()
