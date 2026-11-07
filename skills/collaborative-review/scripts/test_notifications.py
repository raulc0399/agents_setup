"""Durable presentation holds never consume or leak queued deliveries."""
from pathlib import Path
import tempfile
import unittest

from review_core.notifications import queue_notification, read_notifications, set_discussion_active
from review_core.store import Store


class NotificationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.path = Path(self.temp.name) / 'run'
        self.store = Store(self.path, create=True, metadata=dict(revision='rev'))

    def tearDown(self):
        self.store.close()
        self.temp.cleanup()

    def test_hold_durable_delivery_cursor_and_recipient_isolation(self):
        set_discussion_active(self.store, 'chat-a', True)
        first = queue_notification(self.store, 'chat-a', 'Review completed', 'completion', 'n1')
        queue_notification(self.store, 'chat-b', 'Other decision', 'decision')
        before = self.store.events()
        self.assertEqual(read_notifications(self.store, 'chat-a'), dict(deferred=True, notifications=[]))
        self.assertEqual(before, self.store.events())
        self.store.close()
        self.store = Store(self.path)
        self.assertTrue(read_notifications(self.store, 'chat-a')['deferred'])
        set_discussion_active(self.store, 'chat-a', False)
        with Store(self.path, readonly=True) as readonly:
            self.assertEqual(read_notifications(readonly, 'chat-a')['notifications'], [first])
            self.assertEqual(read_notifications(readonly, 'chat-a', first['sequence'])['notifications'], [])
            self.assertEqual(read_notifications(readonly, 'stranger')['notifications'], [])

    def test_immutable_idempotence_and_revision_binding(self):
        first = queue_notification(self.store, 'a', 'Finished', notification_id='same')
        events = self.store.events()
        self.assertEqual(queue_notification(self.store, 'a', 'Finished', notification_id='same'), first)
        self.assertEqual(events, self.store.events())
        with self.assertRaises(ValueError):
            queue_notification(self.store, 'b', 'Finished', notification_id='same')
        set_discussion_active(self.store, 'a', True)
        self.store.set_meta('revision', 'next')
        self.assertEqual(read_notifications(self.store, 'a'), dict(deferred=False, notifications=[]))
        with self.assertRaises(ValueError):
            queue_notification(self.store, 'a', 'Finished', notification_id='same')

    def test_validation_writes_nothing(self):
        before = self.store.events()
        for kwargs in (dict(summary=''), dict(summary='ok', kind='peer'), dict(summary='ok', recipient=' ')):
            args = dict(recipient='a', summary='ok')
            args.update(kwargs)
            with self.assertRaises(ValueError):
                queue_notification(self.store, **args)
        for cursor in (-1, True, 1.5):
            with self.assertRaises(ValueError):
                read_notifications(self.store, 'a', cursor)
        with self.assertRaises(ValueError):
            set_discussion_active(self.store, 'a', 1)
        self.assertEqual(before, self.store.events())


if __name__ == '__main__':
    unittest.main()
