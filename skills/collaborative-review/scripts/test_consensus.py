"""Point-by-point deliberation, independent approvals and automatic rounds."""
import json
import sys
import unittest

from review_core.aasm import parse_document
from review_core.aasm_records import apply_operations, prepare_result
from review_core.consensus import consensus_gaps, point_state, schedule_consensus
from review_core.findings import update_finding
import test_auto_communication


class ConsensusTests(unittest.TestCase):
    setUp = test_auto_communication.AutoCommunicationTests.setUp
    tearDown = test_auto_communication.AutoCommunicationTests.tearDown
    complete_review = test_auto_communication.AutoCommunicationTests.complete_review

    def apply(self, body, worker='reviewer-1'):
        return apply_operations(self.store, worker, self.store.get_meta('revision'), parse_document(body))

    def start(self):
        self.complete_review('reviewer-1')
        self.complete_review('reviewer-2')

    def point(self, identity='retry', refs=()):
        self.apply(f'POINT id={identity} text="Retry behavior disputed" paths=["code.py"] '
                   'evidence=["code.py:1"] refs=' + json.dumps(list(refs)))
        return point_state(self.store, identity)

    def action(self, op, identity='retry', worker='reviewer-1', text='Use the observed value'):
        digest = point_state(self.store, identity)['digest']
        body = f'{op} id={identity} digest={digest} evidence=["code.py:1 checked"]'
        if op != 'AGREE':
            body += ' text=' + json.dumps(text)
        self.apply(body, worker)
        return point_state(self.store, identity)

    def test_each_point_requires_two_explicit_approvals(self):
        self.start()
        self.point('retry')
        self.point('timeout')
        self.action('PROPOSE')
        self.assertEqual(point_state(self.store, 'retry')['approved_by'], [])
        self.action('AGREE')
        self.assertEqual(point_state(self.store, 'retry')['status'], 'open')
        self.action('AGREE', worker='reviewer-2')
        self.assertEqual(point_state(self.store, 'retry')['status'], 'agreed')
        self.assertEqual([gap['id'] for gap in consensus_gaps(self.store)], ['timeout'])
        self.action('PROPOSE', 'timeout')
        self.action('AGREE', 'timeout')
        self.action('AGREE', 'timeout', worker='reviewer-2')
        self.assertEqual(consensus_gaps(self.store), [])

    def test_rejection_reopens_only_its_point_and_schedules_other_lead(self):
        self.start()
        for identity in ('retry', 'timeout'):
            self.point(identity)
            self.action('PROPOSE', identity)
            self.action('AGREE', identity)
            self.action('AGREE', identity, worker='reviewer-2')
        old = point_state(self.store, 'retry')['digest']
        self.action('CONTEST', worker='reviewer-2', text='This ignores the failure path')
        state = point_state(self.store, 'retry')
        self.assertEqual(state['approved_by'], [])
        self.assertEqual(point_state(self.store, 'timeout')['status'], 'agreed')
        with self.assertRaisesRegex(ValueError, 'changed'):
            self.apply(f'AGREE id=retry digest={old} evidence=["old review"]')
        task = schedule_consensus(self.store, self.coordinator)[0]
        self.assertEqual(task['data']['payload']['worker'], 'reviewer-1')
        self.assertIn('This ignores the failure path', task['data']['payload']['brief'])

    def test_changed_linked_evidence_invalidates_only_dependent_approvals(self):
        self.start()
        self.apply('D id=f at=code.py:1 claim="Value mismatch" fix="Check value" evidence=["old trace"]')
        for identity, refs in (('retry', ['f']), ('timeout', [])):
            self.point(identity, refs)
            self.action('PROPOSE', identity)
            self.action('AGREE', identity)
            self.action('AGREE', identity, worker='reviewer-2')
        old = point_state(self.store, 'retry')['digest']
        update_finding(self.store, finding_id='f', actor='reviewer-2', evidence=['new trace'])
        self.assertNotEqual(point_state(self.store, 'retry')['digest'], old)
        self.assertEqual(point_state(self.store, 'retry')['approved_by'], [])
        self.assertEqual(point_state(self.store, 'timeout')['status'], 'agreed')

    def test_retirement_replacement_support_invalidates_only_dependent_points(self):
        from review_core import research
        self.start()
        revision = self.store.get_meta('revision')
        research.add_source(self.store, 'researcher', revision, 'https://example.org/spec',
                            '2026-11-08', content='Current evidence', source_id='source')
        for identity in ('old', 'new'):
            research.add_claim(self.store, 'researcher', revision, identity, source_ids=['source'], claim_id=identity)
        research.retire_claim(self.store, 'old', 'researcher', ['Replaced'], replacement_id='new')
        for identity, refs in (('research', ['old']), ('unrelated', [])):
            self.point(identity, refs)
            self.action('PROPOSE', identity)
            self.action('AGREE', identity)
            self.action('AGREE', identity, worker='reviewer-2')
        research.verify_claim(self.store, 'new', 'verifier', ['Independently checked new evidence'])
        self.assertEqual([], point_state(self.store, 'research')['approved_by'])
        self.assertEqual('agreed', point_state(self.store, 'unrelated')['status'])

    def test_dispute_requires_both_approvals_even_after_question_answer(self):
        self.start()
        self.apply('D id=f at=code.py:1 claim="Value mismatch" fix="Check value" evidence=["trace"]\n'
                   'DISPUTE id=q finding=f reason="Value actually matches" evidence=["counterexample"]')
        self.apply('A id=answer question=q text="Initial response" evidence=["trace"]', 'reviewer-2')
        self.assertEqual([gap['id'] for gap in consensus_gaps(self.store)], ['point-q'])
        self.action('PROPOSE', 'point-q')
        self.action('AGREE', 'point-q')
        self.action('AGREE', 'point-q', worker='reviewer-2')
        self.assertEqual(consensus_gaps(self.store), [])

    def test_dispute_question_closes_only_after_both_agree(self):
        self.start()
        self.apply('D id=f at=code.py:1 claim="Value mismatch" fix="Check value" evidence=["trace"]\n'
                   'DISPUTE id=q finding=f reason="Value matches" evidence=["counterexample"]')
        self.action('PROPOSE', 'point-q')
        self.action('AGREE', 'point-q')
        self.assertEqual(self.store.get_record('question', 'q')['data']['status'], 'unanswered')
        self.action('AGREE', 'point-q', worker='reviewer-2')
        self.assertEqual(self.store.get_record('question', 'q')['data']['status'], 'answered')
        self.assertEqual(consensus_gaps(self.store), [])

    def test_blind_or_foreign_approval_is_rejected(self):
        self.point()
        with self.assertRaisesRegex(ValueError, 'independent'):
            self.action('PROPOSE')
        self.start()
        self.action('PROPOSE')
        with self.assertRaises(PermissionError):
            self.action('AGREE', worker='challenger')

    def test_poll_does_not_leak_a_blind_teams_points(self):
        from review_core.dispatcher import poll_worker
        self.point('private-disagreement')
        self.complete_review('reviewer-1')
        result = poll_worker(self.store, 'reviewer-2', wait_seconds=.01)
        self.assertEqual(result['state'], 'ready')
        self.assertNotIn('private-disagreement', json.dumps(result))
        self.assertNotIn('Retry behavior disputed', json.dumps(result))

    def test_pending_approval_task_is_superseded_when_lead_already_approved(self):
        self.start()
        self.point()
        self.action('PROPOSE')
        previous = schedule_consensus(self.store, self.coordinator)[0]
        self.action('AGREE', worker=previous['data']['payload']['worker'])
        replacement = schedule_consensus(self.store, self.coordinator)[0]
        self.assertNotEqual(previous['id'], replacement['id'])
        self.assertEqual(self.store.get_record('task', previous['id'])['data']['status'], 'cancelled')
        self.assertTrue(self.store.get_record('task', previous['id'])['data']['consensus_superseded'])

    def test_transitive_claim_evidence_reopens_a_recommendation_point(self):
        self.start()
        self.apply('SRC id=s locator=https://example.org date=2026-11-08 content="captured fact"\n'
                   'F id=c text="Precise claim" sources=["s"]\n'
                   'REC id=r text="Recommendation" claims=["c"] case="Case constraints"')
        self.point(refs=['r'])
        self.action('PROPOSE')
        self.action('AGREE')
        self.action('AGREE', worker='reviewer-2')
        self.apply('CONFIRM id=c evidence=["independent support"]', 'reviewer-2')
        self.assertEqual(point_state(self.store, 'retry')['approved_by'], [])

    def test_budget_and_external_gap_stop_without_forcing_approval(self):
        from review_core.dispatcher import poll_worker
        self.start()
        self.point()
        schedule_consensus(self.store, self.coordinator)
        task = self.coordinator.claim_task('reviewer-2')
        result = prepare_result(self.store, 'reviewer-2', 'GAP id=missing text="Need unavailable caller trace"')
        self.coordinator.complete_task(task['id'], 'reviewer-2', task['data']['lease_id'], result)
        before = len(self.store.list_records('task'))
        polled = poll_worker(self.store, 'reviewer-2', wait_seconds=.01)
        self.assertEqual(polled['state'], 'blocked')
        self.assertTrue(any(blocker['code'] == 'consensus' for blocker in polled['blockers']))
        self.assertEqual(point_state(self.store, 'retry')['approved_by'], [])
        scheduled = len(self.store.list_records('task'))
        self.assertGreaterEqual(scheduled, before)
        poll_worker(self.store, 'reviewer-2', wait_seconds=.01)
        self.assertEqual(len(self.store.list_records('task')), scheduled)

    def test_exhausted_budget_preserves_open_point(self):
        from review_core.dispatcher import poll_worker
        self.start()
        self.point()
        self.store.set_meta('spent_seconds', self.store.get_meta('budget_seconds'))
        result = poll_worker(self.store, 'reviewer-1', wait_seconds=.01)
        self.assertEqual(result['state'], 'blocked')
        self.assertTrue(any(blocker['code'] == 'budget' for blocker in result['blockers']))
        self.assertEqual(point_state(self.store, 'retry')['status'], 'open')

    def test_superseded_running_response_has_no_effects(self):
        self.start()
        self.point()
        schedule_consensus(self.store, self.coordinator)
        task = self.coordinator.claim_task('reviewer-2')
        old = task['data']['payload']['point_digest']
        self.action('PROPOSE', worker='reviewer-1')
        body = f'PROPOSE id=retry digest={old} text="Obsolete response" evidence=["old trace"]'
        result = self.coordinator.complete_task(task['id'], 'reviewer-2', task['data']['lease_id'],
                                                prepare_result(self.store, 'reviewer-2', body))
        self.assertEqual(result['data']['status'], 'cancelled')
        self.assertTrue(result['data']['consensus_superseded'])
        self.assertNotIn('Obsolete', json.dumps(point_state(self.store, 'retry')))

    def test_consensus_task_cannot_approve_two_points(self):
        self.start()
        for identity in ('retry', 'timeout'):
            self.point(identity)
            self.action('PROPOSE', identity)
        task = schedule_consensus(self.store, self.coordinator)[0]
        worker = task['data']['payload']['worker']
        claimed = self.coordinator.claim_task(worker, task_id=task['id'])
        body = '\n'.join(f'AGREE id={identity} digest={point_state(self.store, identity)["digest"]} evidence=["checked"]'
                         for identity in ('retry', 'timeout'))
        with self.assertRaisesRegex(ValueError, 'assigned point'):
            self.coordinator.complete_task(task['id'], worker, claimed['data']['lease_id'],
                                            prepare_result(self.store, worker, body))
        self.assertEqual(self.store.list_records('point_approval'), [])

    def test_direct_challenge_waits_for_running_point_response(self):
        from review_core.challenge import schedule_challenge
        from review_core.discussions import schedule_discussions
        self.start()
        for task in schedule_discussions(self.store, self.coordinator):
            worker = task['data']['payload']['worker']
            claimed = self.coordinator.claim_task(worker, task_id=task['id'])
            question = task['data']['payload']['question_ids'][0]
            body = f'A id=answer-{worker} question={question} text="Compared review" evidence=["code.py:1"]'
            self.coordinator.complete_task(task['id'], worker, claimed['data']['lease_id'],
                                           prepare_result(self.store, worker, body))
        self.point()
        self.action('PROPOSE')
        task = schedule_consensus(self.store, self.coordinator)[0]
        self.coordinator.claim_task(task['data']['payload']['worker'], task_id=task['id'])
        self.action('AGREE')
        self.action('AGREE', worker='reviewer-2')
        with self.assertRaisesRegex(ValueError, 'cross-checks'):
            schedule_challenge(self.store, self.coordinator)

    def test_dispatch_iterates_research_rebuttal_and_individual_agreement(self):
        from review_core.dispatcher import dispatch_run
        code = '''import json,sys
r=json.load(sys.stdin)
t=r['task']
if t['kind']=='discussion':
    context=json.loads(t['brief'].split('Source context: ',1)[1])
    for q in context['question_records']:
        print('A id=answer-'+q['id']+' question='+q['id']+' text="Compared evidence" evidence=["code.py:1"]')
    if t['worker']=='reviewer-1':
        for identity in ('retry','timeout'):
            print('POINT id='+identity+' text="Disputed '+identity+' behavior" paths=["code.py"] evidence=["code.py:1"]')
elif t['kind']=='consensus':
    c=json.loads(t['brief'].split('Context: ',1)[1])
    p=c['point']; d=p['data']
    base=' id='+p['id']+' digest='+c['digest']+' evidence=["code.py:1 independently checked"]'
    if d['needs_revision']:
        print('PROPOSE'+base+' text="Resolution after checking the failure path"')
    elif p['id']=='retry' and not d['challenge'] and t['worker']=='reviewer-1':
        print('CONTEST'+base+' text="Research the failure path with a fresh team"')
    else:
        print('AGREE'+base)
else:
    if t['kind']=='challenge':
        c=json.loads(t['brief'].split('Context: ',1)[1])
        assert len(c['records']['point_approval']) == 4
    proof=t['id']+'-coverage'
    print('COV id='+proof+' paths=["code.py"] status=reviewed evidence=["code.py:1"]')
    print('VOTE verdict=APPROVED evidence=["'+proof+'"]')
'''
        result = dispatch_run(self.store.run_dir, [sys.executable, '-c', code], timeout_seconds=2)
        self.assertEqual(result['state'], 'complete', result)
        self.assertEqual(result['approval']['result'], 'APPROVED', result)
        self.assertEqual(consensus_gaps(self.store), [])
        points = self.store.list_records('consensus_point')
        self.assertEqual(len(points), 2)
        self.assertEqual(point_state(self.store, 'retry')['point']['data']['content_version'], 4)
        self.assertEqual(point_state(self.store, 'timeout')['point']['data']['content_version'], 2)


if __name__ == '__main__':
    unittest.main()
