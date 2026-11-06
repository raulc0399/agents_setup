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


if __name__ == '__main__':
    unittest.main()
