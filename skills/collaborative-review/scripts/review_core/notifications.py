"""Revision-bound presentation holds and durable, app-consumable notifications."""
import hashlib
import uuid

from .messages import _name


KINDS = ('status', 'blocked', 'completion', 'decision')


def _identity(revision, recipient):
    return hashlib.sha256((revision + '\0' + recipient).encode()).hexdigest()


def _revision(store):
    return _name(store.get_meta('revision'), 'Revision')


def set_discussion_active(store, recipient, active):
    """Hold notification presentation only; task execution is unaffected."""
    _name(recipient, 'Recipient')
    if type(active) is not bool:
        raise ValueError('active must be boolean')
    with store.transaction():
        revision = _revision(store)
        record_id = _identity(revision, recipient)
        data = dict(revision=revision, recipient=recipient, active=active)
        old = store.get_record('discussion_state', record_id)
        if old and old['data'] == data:
            return old
        return store.put_record('discussion_state', record_id, data,
                                event='discussion.presentation', expected_version=old['version'] if old else 0)


def _sequence(store, record):
    events = [event for event in store.events() if event['kind'] == 'notification.queued'
              and event['record_id'] == record['id']]
    if len(events) != 1:
        raise ValueError('Notification requires exactly one publication event')
    return {**record, 'sequence': events[0]['seq']}


def queue_notification(store, recipient, summary, kind='status', notification_id=None):
    """Publish an immutable concise summary, without contacting the host UI."""
    _name(recipient, 'Recipient')
    _name(summary, 'Summary')
    if kind not in KINDS:
        raise ValueError('Invalid notification kind')
    notification_id = _name('notification-' + uuid.uuid4().hex if notification_id is None else notification_id,
                            'Notification ID')
    with store.transaction():
        data = dict(revision=_revision(store), recipient=recipient, summary=summary, kind=kind)
        old = store.get_record('notification', notification_id)
        if old:
            if old['data'] != data:
                raise ValueError('Notification ID already has a different request')
            return _sequence(store, old)
        record = store.put_record('notification', notification_id, data,
                                  event='notification.queued', expected_version=0)
        return _sequence(store, record)


def read_notifications(store, recipient, after=0):
    """Read without consuming a cursor; held delivery remains replayable."""
    _name(recipient, 'Recipient')
    if type(after) is not int or after < 0:
        raise ValueError('Notification cursor must be a nonnegative integer')
    with store.read_transaction():
        revision = _revision(store)
        state = store.get_record('discussion_state', _identity(revision, recipient))
        if state and state['data']['active']:
            return dict(deferred=True, notifications=[])
        notifications = []
        for event in store.events(after):
            if event['kind'] != 'notification.queued':
                continue
            record = store.get_record('notification', event['record_id'])
            if record is None:
                raise ValueError('Publication references a missing notification')
            if record['data']['revision'] == revision and record['data']['recipient'] == recipient:
                notifications.append({**record, 'sequence': event['seq']})
        return dict(deferred=False, notifications=notifications)
