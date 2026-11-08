"""Research evidence shared by blind teams and renewed after material changes."""
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

from review_core.aasm_records import prepare_result
from review_core.authorization import record_authorization
from review_core.cli import _coordinator
from review_core.delegation import plan_review
from review_core.recovery import handoff
from review_core import research
from review_core.runner import run_next_task
from review_core.snapshots import create_snapshot
from review_core.store import Store


class ResearchWorkflowTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        project = self.root / 'project'
        project.mkdir()
        (project / 'brief.md').write_text('Compare options using captured evidence.\n')
        self.store = Store(self.root / 'runs' / 'research', create=True)
        snapshot = create_snapshot(project, ['brief.md'], self.root / 'runs', 'research')
        self.revision = snapshot['revision']
        for key, value in dict(snapshot=snapshot, revision=self.revision, project_root=str(project),
                               paths=['brief.md'], approval_scope='report-ready', status='ready',
                               budget_seconds=300, spent_seconds=0, max_workers=2).items():
            self.store.set_meta(key, value)
        record_authorization(self.store, self.revision, ['review', 'inspect'], ['brief.md'])
        self.coordinator = _coordinator(self.store)
        research.add_source(self.store, 'researcher', self.revision, 'https://example.org/spec',
                            '2026-11-08', content='Option B meets the requirement.', source_id='source')
        research.add_claim(self.store, 'researcher', self.revision, 'Option B is supported',
                           source_ids=['source'], claim_id='claim')
        research.add_recommendation(self.store, 'researcher', self.revision, 'Choose B', ['claim'],
                                    'The case requires supported behavior', recommendation_id='recommendation')
        self.tasks = plan_review(self.store, self.coordinator)

    def tearDown(self):
        self.store.close()
        for directory, _, _ in os.walk(self.root):
            Path(directory).chmod(0o755)
        self.temp.cleanup()

    def complete(self, worker, prefix=''):
        task = self.coordinator.claim_task(worker)
        body = prefix + f'COV id={worker}-coverage paths=["brief.md"] status=reviewed evidence=["Checked brief and research"]\n'
        body += f'VOTE verdict=APPROVED evidence=["{worker}-coverage"]'
        return self.coordinator.complete_task(task['id'], worker, task['data']['lease_id'],
                                              prepare_result(self.store, worker, body))

    def test_adapter_receives_frozen_common_research_without_later_peer_records(self):
        research.add_claim(self.store, 'reviewer-2', self.revision, 'PRIVATE TEAM DISCOVERY',
                           source_ids=['source'], claim_id='private', impact='not-decision-changing')
        output = self.root / 'request.json'
        code = ('import json,sys,pathlib; request=json.load(sys.stdin); '
                f'pathlib.Path({str(output)!r}).write_text(json.dumps(request)); '
                'print(\'COV id=adapter-coverage paths=["brief.md"] status=reviewed evidence=["Read research"]\'); '
                'print(\'VOTE verdict=APPROVED evidence=["adapter-coverage"]\')')
        task = run_next_task(self.coordinator, 'reviewer-1', [sys.executable, '-c', code], 5)
        self.assertEqual('completed', task['data']['status'])
        request = json.loads(output.read_text())
        context = request['task']['research_context']
        self.assertEqual('report-ready', request['task']['approval_scope'])
        self.assertEqual('Choose B', context['records']['recommendation'][0]['data']['text'])
        self.assertEqual('Option B meets the requirement.', context['records']['source'][0]['data']['content'])
        self.assertNotIn('PRIVATE TEAM DISCOVERY', json.dumps(request))
        self.assertEqual(context, handoff(self.store, worker='reviewer-2')['research_context'])

    def test_common_claim_confirmation_succeeds_in_either_completion_order(self):
        for worker in ('reviewer-2', 'reviewer-1'):
            task = self.complete(worker, 'CONFIRM id=claim evidence=["Independently read source"]\n')
            self.assertEqual('completed', task['data']['status'])
        self.assertEqual([], research.research_gaps(self.store))
        self.assertEqual(['reviewer-1', 'reviewer-2'], self.coordinator.readiness()['approved_reviewers'])

    def test_later_private_claim_cannot_be_confirmed_through_common_visibility(self):
        research.add_claim(self.store, 'reviewer-2', self.revision, 'Private claim',
                           source_ids=['source'], claim_id='private')
        before = self.store.events()
        with self.assertRaisesRegex(ValueError, 'inaccessible'):
            self.complete('reviewer-1', 'CONFIRM id=private evidence=["Claimed reading"]\n')
        self.assertEqual('unverified', self.store.get_record('claim', 'private')['data']['verification'])
        self.assertFalse(any(event['kind'] == 'claim.verified' for event in self.store.events(len(before))))

    def test_material_change_requires_two_explicit_current_reassessments(self):
        completed = [self.complete(worker, 'CONFIRM id=claim evidence=["Read source"]\n')
                     for worker in ('reviewer-1', 'reviewer-2')]
        context = handoff(self.store, worker='reviewer-1')['research_context']
        research.add_source(self.store, 'researcher', self.revision, 'https://example.org/new',
                            '2026-11-08', content='New evidence', source_id='new-source')
        self.assertEqual([], self.coordinator.readiness()['approved_reviewers'])
        current = handoff(self.store, worker='reviewer-1')['research_context']
        self.assertNotEqual(context['fingerprint'], current['fingerprint'])
        for index, task in enumerate(completed, 1):
            worker = task['data']['worker']
            with self.assertRaisesRegex(ValueError, 'fingerprint'):
                self.coordinator.record_review(worker, self.revision, 'APPROVED',
                    [worker + '-coverage'], task_id=task['id'], research_fingerprint=context['fingerprint'])
            self.coordinator.record_review(worker, self.revision, 'APPROVED',
                [worker + '-coverage'], task_id=task['id'], research_fingerprint=current['fingerprint'])
            self.assertEqual(index, len(self.coordinator.readiness()['approved_reviewers']))

    def test_reassessment_cannot_bypass_the_blind_gate(self):
        task = self.complete('reviewer-1')
        context = handoff(self.store, worker='reviewer-1')['research_context']
        with self.assertRaisesRegex(ValueError, 'blind'):
            self.coordinator.record_review('reviewer-1', self.revision, 'APPROVED',
                ['reviewer-1-coverage'], task_id=task['id'], research_fingerprint=context['fingerprint'])

    def test_reassessment_requires_own_reviewed_coverage_of_the_task_scope(self):
        from review_core.findings import create_coverage

        task = self.complete('reviewer-1')
        self.complete('reviewer-2')
        create_coverage(self.store, author='reviewer-1', revision=self.revision, paths=['brief.md'],
                        status='partial', evidence=['Not yet checked'], coverage_id='partial')
        fingerprint = handoff(self.store, worker='reviewer-1')['research_context']['fingerprint']
        for identity in ('missing', 'reviewer-2-coverage', 'partial'):
            with self.subTest(identity=identity):
                with self.assertRaisesRegex(ValueError, 'coverage'):
                    self.coordinator.record_review('reviewer-1', self.revision, 'APPROVED', [identity],
                        task_id=task['id'], research_fingerprint=fingerprint)

    def test_reassessment_requires_current_review_authorization(self):
        from review_core.authorization import revoke_authorization

        task = self.complete('reviewer-1')
        self.complete('reviewer-2')
        fingerprint = handoff(self.store, worker='reviewer-1')['research_context']['fingerprint']
        for grant in self.store.list_records('authorization'):
            revoke_authorization(self.store, grant['id'])
        with self.assertRaises(PermissionError):
            self.coordinator.record_review('reviewer-1', self.revision, 'APPROVED',
                ['reviewer-1-coverage'], task_id=task['id'], research_fingerprint=fingerprint)

    def test_report_readiness_and_challenge_require_current_research_reviews(self):
        from review_core.challenge import schedule_challenge
        from review_core.cli import readiness
        from review_core.discussions import schedule_discussions
        from review_core.exports import export_run

        for worker in ('reviewer-1', 'reviewer-2'):
            self.complete(worker, 'CONFIRM id=claim evidence=["Independently read source"]\n')
        for task in schedule_discussions(self.store, self.coordinator):
            worker = task['data']['payload']['worker']
            claimed = self.coordinator.claim_task(worker, task_id=task['id'])
            question = task['data']['payload']['question_ids'][0]
            body = f'A id=answer-{worker} question={question} text="Cross-checked evidence" evidence=["Read source and peer review"]'
            self.coordinator.complete_task(task['id'], worker, claimed['data']['lease_id'],
                                          prepare_result(self.store, worker, body))
        challenge = schedule_challenge(self.store, self.coordinator)
        claimed = self.coordinator.claim_task('challenger', task_id=challenge['id'])
        body = ('COV id=challenge-proof paths=["brief.md"] status=reviewed evidence=["Traced recommendation to verified source"]\n'
                'VOTE verdict=APPROVED evidence=["challenge-proof"]')
        self.coordinator.complete_task(claimed['id'], 'challenger', claimed['data']['lease_id'],
                                      prepare_result(self.store, 'challenger', body))
        self.assertEqual('APPROVED', readiness(self.store)['result'])
        summary = Path(export_run(self.store)['SUMMARY.md']).read_text()
        self.assertIn('Choose B', summary)
        self.assertIn('**APPROVED**', summary)

        research.retire_recommendation(self.store, 'recommendation', 'researcher', ['Changed case'])
        self.assertEqual('BLOCKED', readiness(self.store)['result'])
        with self.assertRaisesRegex(ValueError, 'reassessment'):
            schedule_challenge(self.store, self.coordinator)

    def test_followup_adapter_receives_current_research_after_blind_reviews(self):
        from review_core.discussions import schedule_discussions

        for worker in ('reviewer-1', 'reviewer-2'):
            self.complete(worker)
        research.add_source(self.store, 'researcher', self.revision, 'https://example.org/new',
                            '2026-11-08', content='NEW FOLLOWUP EVIDENCE', source_id='new-source')
        tasks = schedule_discussions(self.store, self.coordinator)
        task = next(row for row in tasks if row['data']['payload']['worker'] == 'reviewer-1')
        question = task['data']['payload']['question_ids'][0]
        code = ('import json,sys; request=json.load(sys.stdin); '
                'assert "NEW FOLLOWUP EVIDENCE" in json.dumps(request["task"]["research_context"]); '
                f'print(\'A id=checked question={question} text="Reviewed current evidence" evidence=["Read new source"]\')')
        result = run_next_task(self.coordinator, 'reviewer-1', [sys.executable, '-c', code], 5, task_id=task['id'])
        self.assertEqual('completed', result['data']['status'])


if __name__ == '__main__':
    unittest.main()
