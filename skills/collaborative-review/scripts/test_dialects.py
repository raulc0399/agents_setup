from concurrent.futures import ThreadPoolExecutor
import copy
from pathlib import Path
import tempfile
import unittest

from review_core.aasm import BASE_DIALECT, parse_document, expand_operations
from review_core.dialects import (propose_dialect, test_dialect as readback,
                                  acknowledge_dialect, dialect_status, get_dialect, export_dialect)
from review_core.store import Store


class DialectTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.store = Store(self.root / 'run', create=True, metadata={'policies': {'locked': True}})
        self.addCleanup(self.store.close)
        self.definitions = [{'op': 'OBS', 'meaning': 'Record an observation as a note',
                             'fields': {'text': {'type': 'text', 'required': True}},
                             'expands_to': {'op': 'NOTE', 'args': {'text': {'field': 'text'}}}}]
        body = 'OBS text="Observed behavior"'
        self.examples = [{'body': body, 'expected': expand_operations(
            parse_document(body, self.definitions), self.definitions)}]

    def propose(self, **kwargs):
        args = dict(author='alice', name='review-notes', version=1,
                    participants=['alice', 'bob'], definitions=self.definitions, examples=self.examples)
        args.update(kwargs)
        return propose_dialect(self.store, **args)

    def accept(self):
        dialect_id = self.propose()['id']
        for participant in ['alice', 'bob']:
            readback(self.store, dialect_id, participant, self.examples)
            acknowledge_dialect(self.store, dialect_id, participant)
        return dialect_id

    def test_all_readbacks_and_explicit_acks_required(self):
        dialect_id = self.propose()['id']
        self.assertEqual('pending', dialect_status(self.store, dialect_id)['status'])
        for participant in ['alice', 'bob']:
            with self.assertRaises(ValueError):
                acknowledge_dialect(self.store, dialect_id, participant)
            readback(self.store, dialect_id, participant, self.examples)
        self.assertEqual('tested', dialect_status(self.store, dialect_id)['status'])
        acknowledge_dialect(self.store, dialect_id, 'alice')
        self.assertEqual(['bob'], dialect_status(self.store, dialect_id)['missing_acks'])
        with self.assertRaises(ValueError):
            get_dialect(self.store, dialect_id, 'alice')
        acknowledge_dialect(self.store, dialect_id, 'bob')
        self.assertEqual(self.definitions, get_dialect(self.store, dialect_id, 'alice')['definitions'])
        self.assertEqual({'locked': True}, self.store.get_meta('policies'))

    def test_failed_readbacks_persist_and_cannot_ack(self):
        dialect_id = self.propose()['id']
        for interpretations in [None, [], [{'body': self.examples[0]['body'], 'expected': []}],
                                self.examples + self.examples, [{'body': 'other', 'expected': []}]]:
            result = readback(self.store, dialect_id, 'alice', interpretations)
            self.assertFalse(result['data']['passed'])
            self.assertEqual(interpretations, result['data']['interpretations'])
            with self.assertRaises(ValueError):
                acknowledge_dialect(self.store, dialect_id, 'alice')
        self.assertTrue(readback(self.store, dialect_id, 'alice', self.examples)['data']['passed'])

    def test_malformed_proposals_do_not_write(self):
        for kwargs in [dict(version=True), dict(version=0), dict(participants=['alice']),
                       dict(participants=['alice', 'alice']), dict(participants=['bob', 'carol']),
                       dict(definitions=[{'op': 'NOTE'}]), dict(examples=[]),
                       dict(examples=[{'body': self.examples[0]['body'], 'expected': []}]),
                       dict(examples=[{'body': 'NOTE text=x', 'expected': parse_document('NOTE text=x')}])]:
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                self.propose(**kwargs)
        self.assertEqual([], self.store.list_records('dialect'))

    def test_versions_and_manifest_immutability(self):
        proposal = self.propose()
        for kwargs in [dict(author='bob'), dict(dialect_id='alias'),
                       dict(version=2, dialect_id=proposal['id'])]:
            with self.assertRaises(ValueError):
                self.propose(**kwargs)
        self.propose(version=2)
        proposal['data']['definitions'][0]['meaning'] = 'Mutated outside store'
        self.assertEqual(self.definitions, export_dialect(self.store, proposal['id'])['definitions'])

    def test_idempotent_retries_and_frozen_acknowledged_readback(self):
        dialect_id = self.accept()
        events = self.store.events()
        self.propose()
        for participant in ['alice', 'bob']:
            readback(self.store, dialect_id, participant, self.examples)
            acknowledge_dialect(self.store, dialect_id, participant)
        self.assertEqual(events, self.store.events())
        with self.assertRaises(ValueError):
            readback(self.store, dialect_id, 'alice', [])
        self.assertEqual('accepted', dialect_status(self.store, dialect_id)['status'])

    def integer_readbacks(self):
        definitions = [{'op': 'COUNT', 'meaning': 'Report the observed positive count',
                        'fields': {'count': {'type': 'integer', 'required': True}}}]
        body = 'COUNT count=1'
        examples = [{'body': body, 'expected': parse_document(body, definitions)}]
        dialect_id = self.propose(definitions=definitions, examples=examples)['id']
        invalid = copy.deepcopy(examples)
        invalid[0]['expected'][0]['args']['count'] = True
        return dialect_id, examples, invalid

    def test_boolean_failure_does_not_block_corrected_integer_readback(self):
        dialect_id, examples, invalid = self.integer_readbacks()
        failed = readback(self.store, dialect_id, 'alice', invalid)
        self.assertFalse(failed['data']['passed'])
        corrected = readback(self.store, dialect_id, 'alice', examples)
        self.assertTrue(corrected['data']['passed'])
        self.assertEqual(failed['version'] + 1, corrected['version'])
        acknowledge_dialect(self.store, dialect_id, 'alice')

    def test_boolean_retry_after_integer_ack_is_rejected(self):
        dialect_id, examples, invalid = self.integer_readbacks()
        readback(self.store, dialect_id, 'alice', examples)
        acknowledge_dialect(self.store, dialect_id, 'alice')
        events = self.store.events()
        with self.assertRaises(ValueError):
            readback(self.store, dialect_id, 'alice', invalid)
        self.assertEqual(events, self.store.events())

    def test_participant_mismatch_and_base(self):
        dialect_id = self.accept()
        for participant in ['carol', None]:
            with self.assertRaises(ValueError):
                get_dialect(self.store, dialect_id, participant)
            with self.assertRaises(ValueError):
                readback(self.store, dialect_id, participant, self.examples)
            with self.assertRaises(ValueError):
                acknowledge_dialect(self.store, dialect_id, participant)
        self.assertEqual('accepted', get_dialect(self.store, BASE_DIALECT)['status'])

    def test_export_reuse_requires_new_readbacks(self):
        dialect_id = self.accept()
        manifest = export_dialect(self.store, dialect_id)
        with Store(self.root / 'other', create=True) as other:
            propose_dialect(other, **{key: manifest[key] for key in (
                'author', 'name', 'version', 'participants', 'definitions', 'examples')},
                dialect_id=manifest['id'])
            self.assertEqual('pending', dialect_status(other, dialect_id)['status'])

    def test_three_participant_profile_and_version_isolation(self):
        dialect_id = self.propose(participants=['alice', 'bob', 'carol'])['id']
        for participant in ['alice', 'bob']:
            readback(self.store, dialect_id, participant, self.examples)
            acknowledge_dialect(self.store, dialect_id, participant)
        self.assertEqual(['carol'], dialect_status(self.store, dialect_id)['missing_tests'])
        readback(self.store, dialect_id, 'carol', self.examples)
        acknowledge_dialect(self.store, dialect_id, 'carol')
        self.assertEqual('accepted', dialect_status(self.store, dialect_id)['status'])
        next_id = self.propose(version=2)['id']
        self.assertEqual('pending', dialect_status(self.store, next_id)['status'])

    def test_concurrent_conflicting_proposals_keep_one_manifest(self):
        def propose(author):
            with Store(self.root / 'run') as store:
                try:
                    return propose_dialect(store, author, 'review-notes', 1, ['alice', 'bob'],
                                           self.definitions, self.examples)
                except ValueError:
                    return None
        with ThreadPoolExecutor(2) as pool:
            results = list(pool.map(propose, ['alice', 'bob']))
        self.assertEqual(1, sum(result is not None for result in results))
        self.assertEqual(1, len(self.store.list_records('dialect')))

    def test_concurrent_identical_proposals_and_acks(self):
        def propose(_):
            with Store(self.root / 'run') as store:
                return propose_dialect(store, 'alice', 'review-notes', 1, ['alice', 'bob'],
                                       self.definitions, self.examples)
        with ThreadPoolExecutor(2) as pool:
            proposals = list(pool.map(propose, range(2)))
        self.assertEqual(proposals[0], proposals[1])
        dialect_id = proposals[0]['id']
        for participant in ['alice', 'bob']:
            readback(self.store, dialect_id, participant, self.examples)
        def ack(participant):
            with Store(self.root / 'run') as store:
                return acknowledge_dialect(store, dialect_id, participant)
        with ThreadPoolExecutor(4) as pool:
            list(pool.map(ack, ['alice', 'bob', 'alice', 'bob']))
        self.assertEqual('accepted', dialect_status(self.store, dialect_id)['status'])
        self.assertEqual(2, len(self.store.list_records('dialect_ack')))
        self.assertEqual(1, len(self.store.list_records('dialect')))


if __name__ == '__main__':
    unittest.main()
