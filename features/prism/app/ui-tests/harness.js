// Drives the product UI (ui/js/app.js) with the fake backend. `?scenario=<name>` picks the
// fixture and the user input to replay; `?scenario=checks` runs the behaviour checks and
// writes PASS or the failures into #result.
import { start } from '../ui/js/app.js';
import { fakeBackend } from './fake-backend.js';
import { CONTENTS_FOR_TESTS, KEYS, fixture } from './fixtures.js';

const scenario = new URLSearchParams(location.search).get('scenario') ?? 'list';
const settle = () => new Promise((resolve) => setTimeout(resolve, 30));
const key = (name, options = {}) =>
  document.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, ...options }));

async function replay(backend) {
  await settle();
  if (scenario === 'list') {
    key('j');
  } else if (scenario === 'ask') {
    key('j');
    await settle();
    key('Enter');
    await settle();
    const textarea = document.querySelector('.ask textarea');
    textarea.value = 'Is the 0600 mode enough on a shared Mac, or should the file live in the keychain?';
    textarea.dispatchEvent(new Event('input'));
  } else if (scenario === 'sessions') {
    document.querySelector('.strip-actions .button').click();
  } else if (scenario === 'menu') {
    const tab = document.querySelectorAll('.tab')[1];
    const rect = tab.getBoundingClientRect();
    tab.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: rect.left + 30, clientY: rect.bottom - 4 }));
  } else if (scenario === 'error') {
    key('x');
    await settle();
    const { key: session, id } = backend.sent.at(-1);
    backend.reply(session, id, { code: 'stale_revision', current: 4 });
  }
  await settle();
}

async function checks(backend) {
  const failures = [];
  const expect = (condition, message) => {
    if (!condition) {
      failures.push(message);
    }
  };
  const focusedTitle = () => document.querySelector('.card.focused .title')?.textContent;
  await settle();

  expect(document.querySelectorAll('.tab').length === 3, 'three tabs in window w1');
  expect(focusedTitle() === 'Goal', `first card focused, got ${focusedTitle()}`);
  expect(document.querySelectorAll('.card').length === 8, 'six elements, one link, one group');
  const tabLabels = [...document.querySelectorAll('.tab .label')].map((label) => label.textContent);
  expect(tabLabels[0].includes('6f1c2a9e') && tabLabels[2].includes('b3e8d4f2'), `same-project tabs name their conversation: ${tabLabels}`);
  expect(!tabLabels[1].includes('019a2b7c'), 'a unique project needs no conversation ID');

  // A content update while a draft is open keeps the draft, the keyboard and the layout.
  key('j');
  await settle();
  key('Enter');
  await settle();
  const draft = document.querySelector('.ask textarea');
  draft.value = 'Half-typed question';
  draft.dispatchEvent(new Event('input'));
  draft.setSelectionRange(4, 4);
  const asking = document.querySelector('.card.focused');
  const top = asking.getBoundingClientRect().top;
  const updated = structuredClone(CONTENTS_FOR_TESTS[KEYS.LIVE]);
  const plan = updated.documents.find((document) => document.id === KEYS.PLAN);
  plan.elements['a0000000-0000-4000-8000-000000000002'].accepted_revision = null;
  backend.update(KEYS.LIVE, updated);
  await settle();
  const rebuilt = document.querySelector('.ask textarea');
  expect(!document.querySelector('.card.focused .badge.accepted'), 'the asking card was rebuilt with the update');
  expect(rebuilt?.value === 'Half-typed question', `the draft survives, got ${rebuilt?.value}`);
  expect(document.activeElement === rebuilt && rebuilt.selectionStart === 4, 'the editor keeps the keyboard and caret');
  expect(document.querySelector('.card.focused').getBoundingClientRect().top === top, 'no layout shift');
  rebuilt.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  await settle();
  key('k');
  await settle();
  expect(focusedTitle() === 'Goal', `back on the first card, got ${focusedTitle()}`);

  key('j');
  key('j');
  await settle();
  expect(focusedTitle() === 'Windows and tabs', `j moves focus, got ${focusedTitle()}`);

  // Ask quotes the whole item and sends the typed question with number and title.
  key('Enter');
  await settle();
  const textarea = document.querySelector('.ask textarea');
  expect(document.activeElement === textarea, 'the ask editor takes the keyboard');
  textarea.value = 'Why per project?';
  textarea.dispatchEvent(new Event('input'));
  textarea.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true }));
  await settle();
  const ask = backend.sent.at(-1);
  expect(ask?.command.type === 'ask', 'ask sent');
  expect(ask?.command.number === 3 && ask.command.title === 'Windows and tabs', 'ask names item 3');
  expect(ask?.command.question === 'Why per project?', 'ask carries the typed question');
  expect(ask?.command.quote.startsWith('One tab per conversation'), 'ask quotes the item');
  expect(JSON.stringify(ask?.command.selection.ids) === '["a0000000-0000-4000-8000-000000000003"]', 'ask selects the item');
  expect(!document.querySelector('.ask'), 'the editor closes after sending');
  backend.reply(ask.key, ask.id);
  await settle();
  expect(document.querySelector('.statusbar .message').textContent.includes('Shared item 3'), 'the reply is shown');

  // Esc keeps nothing open; hide is local and moves focus to the neighbour.
  key('Enter');
  await settle();
  document.querySelector('.ask textarea').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  await settle();
  expect(!document.querySelector('.ask'), 'Esc closes the editor');
  key('h');
  await new Promise((resolve) => setTimeout(resolve, 400));
  expect(focusedTitle() === 'Liveness of a connection is not part of the protocol', `hide focuses the next item, got ${focusedTitle()}`);
  expect(JSON.stringify(backend.views[KEYS.LIVE]?.hidden) === '["a0000000-0000-4000-8000-000000000003"]', 'hide is saved as view state');
  expect(backend.sent.length === 1, 'hide sends no command');

  // Drop archives the focused item at the document's revision.
  key('x');
  await settle();
  const drop = backend.sent.at(-1);
  expect(drop?.command.type === 'action' && drop.command.action.action === 'archive', 'drop is an archive action');
  expect(drop?.command.expected_revision === 3 && drop.command.action.ids[0] === 'a0000000-0000-4000-8000-000000000004', 'drop names item and revision');
  backend.reply(drop.key, drop.id, { code: 'stale_revision', current: 4 });
  await settle();
  expect(document.querySelector('.statusbar .message.error')?.textContent.includes('revision 4'), 'a stale revision is explained');

  // Documents switch with ] and show their own items.
  key(']');
  await settle();
  expect(document.querySelector('.documents [aria-selected="true"]').textContent === 'Notes', 'the next document by title');
  key('H');
  key('[');
  await settle();
  expect(document.querySelectorAll('.card').length === 8, 'Shift+H shows hidden items again');

  // A disconnected tab is read-only; the context menu moves a tab to a new window.
  key('Tab', { ctrlKey: true });
  await settle();
  expect(document.querySelector('.banner')?.textContent.includes('Connection lost'), 'disconnected banner');
  expect(document.querySelector('.card-actions .button.primary').disabled, 'ask is disabled while disconnected');
  const tab = document.querySelectorAll('.tab')[1];
  tab.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 50, clientY: 30 }));
  await settle();
  const items = [...document.querySelectorAll('.menu button')].map((item) => item.textContent);
  expect(items.join('|') === 'Move to new window|Move to window 2|Close tab', `menu items ${items.join('|')}`);
  document.querySelector('.menu button').click();
  await settle();
  expect(document.querySelectorAll('.tab').length === 2, 'the moved tab left this window');

  // The sessions panel reopens a closed tab here.
  document.querySelector('.strip-actions .button').click();
  await settle();
  const closed = [...document.querySelectorAll('.session-row')].find((row) => row.textContent.includes('website'));
  expect(closed?.textContent.includes('Open'), 'a closed session offers Open');
  closed?.click();
  await settle();
  expect(document.querySelectorAll('.tab').length === 3, 'the closed tab reopened here');
  expect(document.querySelector('.tab[aria-selected="true"]').textContent.includes('website'), 'and is shown');

  // A session shown in another window brings that window forward instead of moving.
  document.querySelector('.strip-actions .button').click();
  await settle();
  const elsewhere = [...document.querySelectorAll('.session-row')].find((row) => row.textContent.includes('billing-service'));
  expect(elsewhere?.textContent.includes('Show window 3'), `the other window is named, got ${elsewhere?.textContent}`);
  elsewhere?.click();
  await settle();
  expect(backend.shown.join() === 'w3', 'that window is shown');
  expect(document.querySelectorAll('.tab').length === 3, 'and the tab stays there');

  const result = document.getElementById('result');
  result.textContent = failures.length === 0 ? 'PASS' : `FAIL: ${failures.join('; ')}`;
}

const backend = fakeBackend(fixture(scenario === 'checks' ? 'list' : scenario));
start(backend, document.getElementById('app'));
if (scenario === 'checks') {
  checks(backend).catch((error) => {
    document.getElementById('result').textContent = `FAIL: ${error}`;
  });
} else {
  replay(backend);
}
