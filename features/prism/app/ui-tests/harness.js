// Drives the product UI (ui/js/app.js) with the fake backend. `?scenario=<name>` picks the
// fixture and the user input to replay; `?scenario=checks`, `?scenario=diagram-checks`,
// `?scenario=view-checks` and `?scenario=action-checks` run the behaviour checks of the list,
// the diagram, the view per document with its side pane, and the item actions,
// `?scenario=status-checks` those of the connection status, `?scenario=document-checks`
// closing and reopening documents; each writes PASS or the failures into #result.
import { start } from '../ui/js/app.js';
import { DROPPED_NOTE } from '../ui/js/list.js';
import { questionBlock } from '../ui/js/panel.js';
import { fakeBackend } from './fake-backend.js';
import { CONTENTS_FOR_TESTS, KEYS, RELEASE_IDS, VERSION_PROBLEM, fixture } from './fixtures.js';

const scenario = new URLSearchParams(location.search).get('scenario') ?? 'list';
// Under a virtual time budget headless Chromium hardly runs animation frames: timers stand in.
const FRAME_MS = 16;
window.requestAnimationFrame = (callback) => setTimeout(() => callback(performance.now()), FRAME_MS);
window.cancelAnimationFrame = (frame) => clearTimeout(frame);
const CHECKS = { checks, 'diagram-checks': diagramChecks, 'view-checks': viewChecks, 'narrow-checks': narrowChecks, 'action-checks': actionChecks, 'status-checks': statusChecks, 'document-checks': documentChecks };
const settle = () => new Promise((resolve) => setTimeout(resolve, 30));
const key = (name, options = {}) =>
  document.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, ...options }));

/** Line segments of every arrow in the shown diagram, from the `d` of its line. */
function arrowSegments(svg) {
  return [...svg.querySelectorAll('[data-kind="arrow"] .prism-line')].map((line) => {
    const points = line
      .getAttribute('d')
      .split(/[ML]/)
      .map((point) => point.trim())
      .filter(Boolean)
      .map((point) => point.split(',').map(Number));
    return points.slice(1).map((point, index) => [points[index], point]).filter(([a, b]) => a[0] !== b[0] || a[1] !== b[1]);
  });
}

/** Pairs of arrows that run on one line: collinear, closer than 12 units, less than 16 apart end to end. */
function sharedLines(svg) {
  const arrows = arrowSegments(svg);
  const shared = [];
  arrows.forEach((segments, index) => {
    for (const others of arrows.slice(index + 1)) {
      for (const [a, b] of segments) {
        for (const [c, d] of others) {
          const horizontal = a[1] === b[1];
          if (horizontal !== (c[1] === d[1])) {
            continue;
          }
          const [axis, across] = horizontal ? [0, 1] : [1, 0];
          const overlap = Math.min(Math.max(a[axis], b[axis]), Math.max(c[axis], d[axis])) - Math.max(Math.min(a[axis], b[axis]), Math.min(c[axis], d[axis]));
          if (Math.abs(a[across] - c[across]) < 12 && overlap > -16) {
            shared.push([a, b, c, d]);
          }
        }
      }
    }
  });
  return shared;
}

/** Selects characters `start` to `end` of the text inside `node`, as a drag would; returns them. */
function selectText(node, start, end) {
  const range = document.createRange();
  range.setStart(node.firstChild, start);
  range.setEnd(node.firstChild, end);
  const selection = window.getSelection();
  selection.removeAllRanges();
  selection.addRange(range);
  return range.toString();
}

/** Settles until `condition` holds; the diagram layout arrives over the network. */
async function waitFor(condition, attempts = 100) {
  for (let attempt = 0; attempt < attempts && !condition(); attempt += 1) {
    await settle();
  }
  return condition();
}

async function replay(backend) {
  await settle();
  if (scenario === 'list') {
    key('j');
  } else if (scenario === 'ask') {
    key('j');
    await settle();
    key('Enter');
    await settle();
    const { key: session, id } = backend.sent.at(-1);
    backend.reply(session, id);
  } else if (scenario === 'accepted') {
    key('j');
    key('a');
    await settle();
    const { key: session, id } = backend.sent.at(-1);
    backend.reply(session, id);
  } else if (scenario === 'select') {
    for (const step of ['j', ' ', 'j', 'j', ' ', 'j', 'j', ' ']) {
      key(step);
      await settle();
    }
  } else if (scenario === 'diagram-ask') {
    await waitFor(() => document.querySelector('.diagram-svg [data-id]'));
    selectText(document.querySelector('.detail-panel .panel-body'), 3, 47);
    key('Enter');
    await settle();
    const { key: session, id } = backend.sent.at(-1);
    backend.reply(session, id);
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
  } else if (scenario === 'closed-menu') {
    document.querySelector('.closed-documents').click();
  } else if (scenario.startsWith('diagram')) {
    await waitFor(() => document.querySelector('.diagram-svg [data-id]'));
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

  // A content update rebuilds the focused card in place.
  key('j');
  await settle();
  const top = document.querySelector('.card.focused').getBoundingClientRect().top;
  const updated = structuredClone(CONTENTS_FOR_TESTS[KEYS.LIVE]);
  const plan = updated.documents.find((document) => document.id === KEYS.PLAN);
  plan.elements['a0000000-0000-4000-8000-000000000002'].accepted_revision = null;
  backend.update(KEYS.LIVE, updated);
  await settle();
  expect(!document.querySelector('.card.focused .badge.accepted'), 'the focused card was rebuilt with the update');
  expect(document.querySelector('.card.focused').getBoundingClientRect().top === top, 'no layout shift');
  key('k');
  await settle();
  expect(focusedTitle() === 'Goal', `back on the first card, got ${focusedTitle()}`);

  key('j');
  key('j');
  await settle();
  expect(focusedTitle() === 'Windows and tabs', `j moves focus, got ${focusedTitle()}`);

  // Ask goes straight to the CLI prompt box: no editor, the whole item quoted, no typed question.
  key('Enter');
  await settle();
  const ask = backend.sent.at(-1);
  expect(ask?.command.type === 'ask', 'Enter sends the ask at once');
  expect(!document.querySelector('.detail-panel textarea, .ask'), 'without a question editor');
  expect(ask?.command.items.length === 1 && ask.command.items[0].number === 3 && ask.command.items[0].title === 'Windows and tabs', 'ask names item 3');
  expect(!('question' in (ask?.command ?? {})), 'the question is typed in the CLI, not sent');
  expect(ask?.command.items[0].quote.startsWith('One tab per conversation'), 'ask quotes the item');
  expect(JSON.stringify(ask?.command.selection.ids) === '["a0000000-0000-4000-8000-000000000003"]', 'ask selects the item');
  expect(document.querySelector('.statusbar .message').textContent === 'Sharing…', 'while it is on its way');
  backend.reply(ask.key, ask.id);
  await settle();
  expect(document.querySelector('.statusbar .message').textContent === 'Shared item 3 with the CLI prompt box.', 'the reply is shown');

  // Hide is local and moves focus to the neighbour.
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
  expect(document.querySelector('.detail-panel .panel-actions .button.primary').disabled, 'ask is disabled while disconnected');
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

async function diagramChecks(backend, app) {
  const failures = [];
  const expect = (condition, message) => {
    if (!condition) {
      failures.push(message);
    }
  };
  const { item, link, group } = RELEASE_IDS;
  const root = document.getElementById('app');
  const svg = (scope = root) => scope.querySelector('.diagram-svg');
  const shape = (kind, id) => svg()?.querySelector(`[data-kind="${kind}"][data-id="${id}"]`);
  const viewBox = (scope = root) => svg(scope).getAttribute('viewBox').split(' ').map(Number);
  const panel = (part) => root.querySelector(`.detail-panel ${part}`)?.textContent ?? '';
  const click = (node) => node.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  const canvas = () => root.querySelector('.diagram-canvas');
  const close = (a, b) => Math.abs(a - b) < 0.5;
  // The zoom and the detail level the controls show, and pixels per world unit on screen.
  const label = () => root.querySelector('.diagram-controls .zoom').textContent;
  const level = () => root.querySelector('.diagram-controls .segmented [aria-pressed="true"]')?.textContent;
  const levelButton = (name) => [...root.querySelectorAll('.diagram-controls .segmented button')].find((button) => button.textContent === name);
  const scale = () => canvas().clientWidth / viewBox()[2];
  const near = (a, b) => Math.abs(a - b) < 1e-6;
  const view = () => app.diagramViews.get(KEYS.LIVE);
  const outline = (number) => shape('box', item(number)).querySelector('.prism-shape').getBoundingClientRect();
  // Until a zoom animation ended, and a level change got its layout.
  const animated = () => waitFor(() => !view().animating());
  const settled = () => waitFor(() => backend.pending === 0 && !view().animating());
  const zoomKey = async (name, options = {}) => {
    key(name, options);
    await settled();
  };
  const textsInside = () =>
    [...svg().querySelectorAll('[data-kind="box"]')].filter((box) => {
      const outline = box.querySelector('.prism-shape').getBBox();
      return [...box.querySelectorAll('text')].some((text) => {
        const bounds = text.getBBox();
        return bounds.x < outline.x || bounds.x + bounds.width > outline.x + outline.width || bounds.y < outline.y || bounds.y + bounds.height > outline.y + outline.height;
      });
    });
  const releaseBody = (number) => CONTENTS_FOR_TESTS[KEYS.LIVE].documents.find((candidate) => candidate.id === KEYS.RELEASE).elements[item(number)].body;
  await settle();

  // V switches the document to its diagram; the choice is view state.
  expect(root.querySelector('.cards'), 'the document starts in the list, as chosen before');
  const listNumbers = new Map([...root.querySelectorAll('.card')].map((card) => [card.dataset.id, card.querySelector('.number').textContent]));
  // A link drawn as an arrow shows its step as its kind, from the sizes alone; a link drawn as a badge stays a link.
  const linkKinds = () => JSON.stringify([...root.querySelectorAll('.card.kind-link')].map((card) => card.querySelector('.kind').textContent));
  const allSteps = JSON.stringify(['Step 1', 'Step 2', 'Step 3', 'Step 4', 'Step 5', 'Link', 'Link']);
  expect(await waitFor(() => linkKinds() === allSteps), `the list shows the arrows' steps: ${linkKinds()}`);
  expect(JSON.stringify(backend.layouts) === '[null]', `the list asks for the sizes alone: ${JSON.stringify(backend.layouts)}`);
  const listSteps = new Map([...root.querySelectorAll('.card.kind-link')].map((card) => [card.dataset.id, card.querySelector('.kind').textContent]));
  expect(listSteps.get(link(6)) === 'Link' && listSteps.get(link(7)) === 'Link', `badge links show no step: ${listSteps.get(link(6))}, ${listSteps.get(link(7))}`);
  // The steps follow what the list shows, as the diagram would draw it: a hidden or filtered-out
  // box leaves its arrows out and the rest count on; a new revision brings its own steps.
  click(root.querySelector(`.card[data-id="${item(1)}"]`));
  key('h');
  expect(await waitFor(() => linkKinds() === JSON.stringify(['Link', 'Step 1', 'Step 2', 'Step 3', 'Step 4', 'Link', 'Link'])), `hiding a box renumbers the steps: ${linkKinds()}`);
  key('H');
  expect(await waitFor(() => linkKinds() === allSteps), `showing it again restores them: ${linkKinds()}`);
  const typeFilter = (text) => {
    const input = root.querySelector('.filter');
    input.value = text;
    input.dispatchEvent(new Event('input'));
  };
  typeFilter('batch');
  expect(await waitFor(() => linkKinds() === JSON.stringify(['Link', 'Step 1', 'Link', 'Step 2'])), `filtering renumbers the steps: ${linkKinds()}`);
  typeFilter('');
  expect(await waitFor(() => linkKinds() === allSteps), `clearing the filter restores them: ${linkKinds()}`);
  const listedContent = structuredClone(CONTENTS_FOR_TESTS[KEYS.LIVE]);
  const relinked = structuredClone(listedContent);
  const relinkedFlow = relinked.documents.find((candidate) => candidate.id === KEYS.RELEASE);
  relinkedFlow.revision = 7;
  const second = 'b4000000-0000-4000-8000-00000000000a';
  // A second link makes the finding a box, so its first link becomes an arrow too.
  relinkedFlow.relationships[second] = { ...relinkedFlow.relationships[link(6)], id: second, to: item(2), label: 'affects too' };
  backend.update(KEYS.LIVE, relinked);
  expect(await waitFor(() => linkKinds() === JSON.stringify(['Step 1', 'Step 2', 'Step 3', 'Step 4', 'Step 5', 'Step 6', 'Link', 'Step 7'])), `a new revision brings its steps: ${linkKinds()}`);
  backend.update(KEYS.LIVE, listedContent);
  expect(await waitFor(() => linkKinds() === allSteps), `the restored content shows its steps again: ${linkKinds()}`);
  click(root.querySelector(`.card[data-id="${link(6)}"]`));
  await settle();
  expect(panel('.panel-tag') === `Link${listNumbers.get(link(6))}`, `a badge link's panel tag shows no step: ${panel('.panel-tag')}`);
  const listed = backend.layouts.length;
  key('v');
  expect(await waitFor(() => svg()?.querySelector('[data-id]')), 'V shows the diagram');
  // Without a saved level, one line of text, as the list's cards, fitted whole.
  expect(level() === 'Summary' && scale() <= 1, `it starts with one line of text, fitted: ${level()} ${label()}`);
  expect(JSON.stringify(backend.layouts.slice(listed)) === '["subtitles"]', `it lays out its level once: ${JSON.stringify(backend.layouts.slice(listed))}`);
  expect(root.querySelector('.pane-controls .segmented [aria-pressed="true"]')?.textContent === 'Diagram', 'the switch shows the diagram');

  // 5.1 Lanes per group, boxes, numbered pills, one dashed manual arrow, badges; themed by CSS.
  expect(svg().querySelectorAll('[data-kind="lane"]').length === 3, 'a lane per group');
  expect(svg().querySelectorAll('[data-kind="box"]').length === 6, 'six boxes');
  expect(svg().querySelectorAll('[data-kind="gate"]').length === 2, 'the finding and the question are badges');
  const pills = [...svg().querySelectorAll('[data-kind="pill"] .prism-label')].map((label) => label.textContent);
  expect(pills.every((label, index) => label.startsWith(String(index + 1))), `pills are numbered: ${pills}`);
  // Every pill's number is the step its list row and its panel tag show.
  const pillSteps = [...svg().querySelectorAll('[data-kind="pill"]')].map((pill) => [pill.dataset.id, pill.querySelector('.prism-label').textContent.split(' ')[0]]);
  expect(pillSteps.length === 5 && pillSteps.every(([id, step]) => listSteps.get(id) === `Step ${step}`), `list rows show the pills' steps: ${JSON.stringify(pillSteps)}`);
  expect(svg().querySelectorAll('.prism-arrow.prism-manual [stroke-dasharray]').length === 1, 'the manual arrow is dashed');
  // Boxes read like cards: number, kind, title and one clipped line of text, all inside.
  const boxes = [...svg().querySelectorAll('[data-kind="box"]')];
  const boxNumbers = boxes.map((box) => [box.querySelector('.prism-number')?.textContent, listNumbers.get(box.dataset.id)]);
  expect(boxNumbers.every(([shown, listed]) => shown === listed), `boxes show the list's numbers: ${JSON.stringify(boxNumbers)}`);
  expect(shape('box', item(1)).querySelector('.prism-kind')?.textContent === 'STEP', 'the kind label in capitals');
  expect(shape('box', item(1)).querySelector('.prism-subtitle')?.textContent === 'Sensor app records a reading e…', 'one clipped line of text');
  expect(textsInside().length === 0, `box texts stay inside their box: ${textsInside().map((box) => box.dataset.id)}`);
  const paint = (node, property) => getComputedStyle(node).getPropertyValue(property);
  expect(paint(shape('box', item(1)).querySelector('.prism-shape'), 'fill') === 'rgb(255, 255, 255)', 'boxes take the surface token');
  expect(paint(shape('gate', item(7)).querySelector('.prism-shape'), 'fill') === 'rgb(232, 160, 32)', 'badges take the amber token');
  const frame = canvas().getBoundingClientRect();
  const inView = () =>
    [...svg().querySelectorAll('[data-kind="box"], [data-kind="gate"]')].every((node) => {
      const rect = node.getBoundingClientRect();
      return rect.left >= frame.left && rect.right <= frame.right && rect.top >= frame.top && rect.bottom <= frame.bottom;
    });
  expect(inView() && scale() <= 1, `the whole diagram fits at start: ${label()}`);
  const shared = sharedLines(svg());
  expect(shared.length === 0, `no two arrows run on one line: ${JSON.stringify(shared)}`);

  // 5.3 Clicks focus an item and fill the panel; Esc clears.
  click(shape('box', item(4)));
  await settle();
  expect(panel('.panel-tag') === 'Step4', `the panel tag names kind and number, got ${panel('.panel-tag')}`);
  expect(panel('.panel-title') === 'Review anomalies', 'the panel shows the title');
  expect(panel('.panel-body').includes('An operator checks'), 'and the text');
  expect(panel('.card-question .q') === 'Can small batches skip the review?' && panel('.card-question .a').startsWith('No: every batch'), 'and the question with its answer');
  expect(shape('box', item(4)).classList.contains('focused'), 'the focused box is highlighted');
  expect(document.activeElement === shape('box', item(4)), 'the box takes the keyboard');
  click(shape('box', item(1)));
  await settle();
  expect(panel('.card-source') === 'device/src/capture.rs', 'the panel shows the source');
  click(shape('lane', group(2)));
  await settle();
  expect(panel('.panel-tag').startsWith('Group') && panel('.panel-title') === 'Server staging', 'a lane focuses its group');
  click(shape('arrow', link(4)).querySelector('.prism-line'));
  await settle();
  expect(panel('.panel-title') === 'approve release', 'an arrow focuses its link');
  expect(document.activeElement === shape('pill', link(4)), 'its pill takes the keyboard');
  expect(shape('arrow', link(4)).classList.contains('focused') && shape('pill', link(4)).classList.contains('focused'), 'line and pill are highlighted');
  const tagSteps = [];
  for (const [id] of pillSteps) {
    click(shape('pill', id));
    await settle();
    tagSteps.push([panel('.panel-tag'), `${listSteps.get(id)}· link ${listNumbers.get(id)}`]);
  }
  expect(tagSteps.every(([shown, wanted]) => shown === wanted), `panel tags show step and list number: ${JSON.stringify(tagSteps)}`);
  click(shape('gate', item(7)));
  await settle();
  expect(panel('.panel-title') === 'Clock drift on old devices', 'a badge focuses its finding');
  key('Escape');
  await settle();
  expect(!svg().querySelector('.focused') && panel('.panel-tag') === 'Diagram', 'Esc clears the focus');
  expect(document.activeElement === canvas(), 'the keyboard stays in the diagram');

  // Keyboard: one stop per lane, box, badge and arrow (on its pill), in the layout's order.
  const stops = [...svg().querySelectorAll('[tabindex="0"]')].map((node) => node.dataset.id);
  const order = [group(1), group(2), group(3), item(1), item(7), item(2), item(3), item(4), item(8), item(5), item(6), link(1), link(2), link(3), link(4), link(5)];
  expect(JSON.stringify(stops) === JSON.stringify(order), `tab order ${stops.length} stops`);
  expect([...svg().querySelectorAll('[data-kind="arrow"]')].every((node) => node.getAttribute('tabindex') === '-1'), 'arrow lines are no tab stops');
  expect(shape('pill', link(4)).getAttribute('aria-label') === 'Step 4, link 12, approve release: Review anomalies to Publish dashboard', `pills are named by step and list number: ${shape('pill', link(4)).getAttribute('aria-label')}`);
  expect(shape('pill', link(4)).querySelector('title').textContent === 'Step 4, link 12, approve release', 'and so is their tooltip');
  key('j');
  key('j');
  await settle();
  expect(document.activeElement === shape('gate', item(7)) && panel('.panel-title') === 'Clock drift on old devices', 'J follows the keyboard order');
  shape('pill', link(2)).focus();
  await settle();
  expect(panel('.panel-title') === 'upload', 'Tab onto a pill focuses its link');
  // On an arrow, J and K go to the next and the previous step; at the last one J stays.
  const focusedStep = () => panel('.panel-tag .kind');
  const walked = [focusedStep()];
  for (const name of ['j', 'j', 'j', 'j', 'k']) {
    key(name);
    await settle();
    walked.push(focusedStep());
  }
  expect(JSON.stringify(walked) === JSON.stringify(['Step 2', 'Step 3', 'Step 4', 'Step 5', 'Step 5', 'Step 4']), `J and K follow the steps: ${walked}`);
  expect(document.activeElement === shape('pill', link(4)), 'and the pill takes the keyboard');
  shape('pill', link(2)).focus();
  await settle();

  // 5.2 Zoom is free and continuous between 10% and 400%; only the level control changes the
  // detail. Buttons and keys zoom animated, around the canvas center or the focused box; 0 fits.
  await zoomKey('0');
  const fit = viewBox();
  expect(level() === 'Summary', `0 fits and keeps the level, got ${level()}`);
  key('+');
  expect(view().animating(), '+ zooms animated');
  await animated();
  let box = viewBox();
  expect(Math.abs(fit[2] / box[2] - 1.15) < 1e-6 && close(box[0] + box[2] / 2, fit[0] + fit[2] / 2), `+ zooms in one step around the center: ${label()}`);
  key('+');
  key('+');
  await animated();
  expect(Math.abs(box[2] / viewBox()[2] - 1.15 ** 2) < 1e-6, `steps in a row add up: ${label()}`);

  // Pinch and wheel zoom at once, across the old 80% and 125% limits: no layout, no level change.
  const middle = (number) => {
    const rect = outline(number);
    return [rect.left + rect.width / 2, rect.top + rect.height / 2];
  };
  const moved = (from, to) => Math.hypot(from[0] - to[0], from[1] - to[1]);
  const wheelAt = (point, deltaY) => canvas().dispatchEvent(new WheelEvent('wheel', { deltaY, ctrlKey: true, bubbles: true, cancelable: true, ...point }));
  const worldAt = (point, vb = viewBox()) => {
    const rect = canvas().getBoundingClientRect();
    return [vb[0] + ((point.clientX - rect.left) / rect.width) * vb[2], vb[1] + ((point.clientY - rect.top) / rect.height) * vb[3]];
  };
  // How far, in screen pixels, the world point under `point` moves through a wheel sequence.
  const drift = (point, deltas) => {
    const start = worldAt(point);
    let most = 0;
    for (const deltaY of deltas) {
      wheelAt(point, deltaY);
      const now = worldAt(point);
      most = Math.max(most, Math.hypot(now[0] - start[0], now[1] - start[1]) * scale());
    }
    return most;
  };
  await zoomKey('0');
  const asked = backend.layouts.length;
  // Mouse events carry whole pixels.
  const [px, py] = middle(4).map(Math.round);
  const onBox = { clientX: px, clientY: py };
  const scales = [];
  const levels = new Set();
  for (const deltaY of [...Array(40).fill(-30), ...Array(70).fill(30), ...Array(20).fill(-30)]) {
    wheelAt(onBox, deltaY);
    scales.push(scale());
    levels.add(level());
  }
  expect(Math.min(...scales) < 0.5 && Math.max(...scales) > 2, `the zoom ran across the old limits: ${Math.min(...scales)} to ${Math.max(...scales)}`);
  expect(backend.layouts.length === asked && levels.size === 1 && level() === 'Summary', `continuous zoom lays nothing out and keeps the level: ${backend.layouts.slice(asked)} ${[...levels]}`);
  await zoomKey('0');
  const kept = drift(onBox, [...Array(30).fill(-40), ...Array(50).fill(40), ...Array(10).fill(-100)]);
  expect(kept < 0.5, `the point under the pointer stays under it: ${kept} px`);
  // Near the canvas edge, with the diagram filling the canvas: zooming out and back in.
  view().zoomAt(2.5, frame.left + frame.width / 2, frame.top + frame.height / 2);
  const edge = { clientX: Math.round(frame.right - 4), clientY: Math.round(frame.top + frame.height / 2) };
  const atEdge = drift(edge, [...Array(40).fill(40), ...Array(40).fill(-40)]);
  expect(atEdge < 0.5, `and near the edge: ${atEdge} px`);
  const corner = { clientX: Math.round(frame.left + 3), clientY: Math.round(frame.top + 3) };
  const atCorner = drift(corner, [...Array(20).fill(60), ...Array(20).fill(-60)]);
  expect(atCorner < 0.5, `and in the corner: ${atCorner} px`);
  // With most of the diagram panned out of view: zooming out on its last pixels keeps them there.
  view().zoomAt(1, frame.left + frame.width / 2, frame.top + frame.height / 2);
  for (let steps = 0; steps < 40; steps += 1) {
    view().panStep(1, 0);
  }
  const rightEnd = { clientX: Math.round(frame.left + 30), clientY: Math.round(frame.top + frame.height / 2) };
  const atEnd = drift(rightEnd, [...Array(40).fill(60), ...Array(40).fill(-60)]);
  expect(atEnd < 0.5, `and at the end of a diagram panned away: ${atEnd} px`);
  for (let notches = 0; notches < 30; notches += 1) {
    wheelAt(onBox, -100);
  }
  expect(near(scale(), 4), `zoom stops at 400%, got ${label()}`);
  for (let notches = 0; notches < 60; notches += 1) {
    wheelAt(onBox, 100);
  }
  expect(near(scale(), 0.1) && label() === '10%', `and at 10%, got ${label()}`);
  // One wheel notch is one zoom step, a larger delta is capped to it.
  await zoomKey('0');
  box = viewBox();
  wheelAt(onBox, -100);
  let zoomed = viewBox();
  expect(Math.abs(box[2] / zoomed[2] - 1.15) < 1e-6, `one wheel notch is one zoom step, got ${box[2] / zoomed[2]}`);
  wheelAt(onBox, -600);
  expect(Math.abs(zoomed[2] / viewBox()[2] - 1.15) < 1e-6, `a large wheel delta is capped to one step: ${label()}`);
  // A pinch zooms around its center from the zoom it started at.
  const gesture = (type, gestureScale) => canvas().dispatchEvent(Object.assign(new Event(type, { bubbles: true, cancelable: true }), { scale: gestureScale, ...onBox }));
  await zoomKey('0');
  box = viewBox();
  gesture('gesturestart', 1);
  gesture('gesturechange', 1.1);
  zoomed = viewBox();
  expect(close(zoomed[2], box[2] / 1.1) && close(worldAt(onBox, zoomed)[0], worldAt(onBox, box)[0]), 'a pinch gesture zooms');
  gesture('gesturechange', 0.4);
  expect(close(viewBox()[2], box[2] / 0.4) && level() === 'Summary' && backend.layouts.length === asked, `pinching out keeps the level: ${label()}`);
  gesture('gestureend', 0.4);

  // The level control: one layout, the focused box stays in place at the same zoom; 1 to 3 too.
  await zoomKey('0');
  click(shape('box', item(4)));
  await settle();
  let anchored = middle(4);
  const zoomShown = scale();
  click(levelButton('Full text'));
  await settled();
  expect(level() === 'Full text' && JSON.stringify(backend.layouts.slice(asked)) === '["full"]', `the control lays out the full text once: ${JSON.stringify(backend.layouts.slice(asked))}`);
  expect(moved(middle(4), anchored) < 1 && near(scale(), zoomShown), `the focused box stays in place at the same zoom: ${moved(middle(4), anchored)} ${label()}`);
  const fullText = [...shape('box', item(1)).querySelectorAll('.prism-body')].map((line) => line.textContent);
  expect(fullText.length > 1 && fullText.join(' ') === releaseBody(1), `the whole text, wrapped: ${fullText}`);
  expect(textsInside().length === 0, `full texts stay inside their box: ${textsInside().map((node) => node.dataset.id)}`);
  const saved = () => app.view(KEYS.LIVE).cameras[KEYS.RELEASE];
  expect(saved()?.detail === 'full' && near(saved().scale, zoomShown), `the level is saved with the camera: ${JSON.stringify(saved())}`);
  key('1');
  await settled();
  expect(level() === 'Titles' && backend.layouts.at(-1) === 'names' && !svg().querySelector('.prism-subtitle, .prism-body'), `1 shows titles only: ${level()}`);
  expect(moved(middle(4), anchored) < 1 && textsInside().length === 0, 'with the box in place');
  const laidOut = backend.layouts.length;
  key('3');
  await settled();
  key('2');
  await settled();
  expect(level() === 'Summary' && backend.layouts.length === laidOut && moved(middle(4), anchored) < 1, `levels shown before come without a new layout: ${backend.layouts.slice(laidOut)}`);
  expect(shape('box', item(1)).querySelector('.prism-subtitle')?.textContent === 'Sensor app records a reading e…', 'one clipped line of text');
  key('Escape');

  // Fit shows the whole diagram at every level, at most at 100%.
  for (const name of ['Titles', 'Full text', 'Summary']) {
    click(levelButton(name));
    await settled();
    await zoomKey('0');
    expect(inView() && scale() <= 1, `fit shows the whole diagram with ${name}: ${label()}`);
  }

  // Lines keep their width on screen; arrowheads scale with the diagram up to 100%, not beyond.
  const strokes = () =>
    ['[data-kind="arrow"] .prism-line', '[data-kind="arrow"] .prism-hit', '[data-kind="box"] .prism-shape', '.prism-lane .prism-shape']
      .map((selector) => getComputedStyle(svg().querySelector(selector)))
      .map((style) => `${style.vectorEffect} ${style.strokeWidth}`);
  const head = () => Number(svg().querySelector('marker').getAttribute('markerWidth')) * scale();
  const center = [frame.left + frame.width / 2, frame.top + frame.height / 2];
  view().zoomAt(0.3, ...center);
  const far = strokes();
  const farHead = head();
  view().zoomAt(2.5, ...center);
  expect(far.every((stroke) => stroke.startsWith('non-scaling-stroke')) && JSON.stringify(far) === JSON.stringify(strokes()), `line widths stay on screen: ${far} ${strokes()}`);
  expect(Math.abs(farHead - 11.5 * 0.3) < 0.01 && Math.abs(head() - 11.5) < 0.01, `arrowheads: ${farHead} px at 30%, ${head()} px at 250%`);

  // Zoomed out, text lines fade, then the kind and number line; titles remain. A band changes
  // past its limit by a margin, so zooming around it does not flicker.
  const band = () => svg().dataset.zoom;
  const bands = [];
  for (const zoom of [1, 0.6, 0.56, 0.54, 0.53, 0.52, 0.54, 0.56, 0.57, 0.58, 0.6]) {
    view().zoomAt(zoom, ...center);
    bands.push(band());
  }
  expect(bands.join() === 'near,near,near,near,near,mid,mid,mid,mid,near,near', `bands with hysteresis: ${bands}`);
  const faded = () => ['.prism-subtitle', '.prism-kind', '.prism-number', '.prism-name'].map((selector) => getComputedStyle(svg().querySelector(selector)).opacity).join();
  // Headless Chromium under virtual time hardly runs transitions: the fades are finished here.
  const fade = async () => {
    await settle();
    svg()
      .getAnimations({ subtree: true })
      .forEach((animation) => animation.finish());
  };
  view().zoomAt(0.45, ...center);
  await fade();
  expect(faded() === '0,1,1,1', `below 55% the text lines fade: ${faded()}`);
  view().zoomAt(0.3, ...center);
  await fade();
  expect(faded() === '0,0,0,1', `below 45% the kind and number too, the titles remain: ${faded()}`);
  view().zoomAt(1, ...center);
  await fade();
  expect(faded() === '1,1,1,1', `zoomed in, every text returns: ${faded()}`);

  // Pills and badges keep 80% of their size on screen when zoomed out, growing at most twice
  // in the world; further out they shrink too.
  const height = (kind, id) => shape(kind, id).querySelector('.prism-shape').getBoundingClientRect().height;
  view().zoomAt(0.45, ...center);
  expect(Math.abs(height('pill', link(1)) - 20 * 0.8) < 0.1 && Math.abs(height('gate', item(7)) - 18 * 0.8) < 0.1, `pills and badges keep a readable size: ${height('pill', link(1))} ${height('gate', item(7))}`);
  view().zoomAt(0.3, ...center);
  expect(Math.abs(height('pill', link(1)) - 20 * 0.6) < 0.1, `far out, they grow at most twice: ${height('pill', link(1))}`);
  view().zoomAt(1.5, ...center);
  expect(Math.abs(height('pill', link(1)) - 30) < 0.1 && !shape('pill', link(1)).hasAttribute('transform'), `zoomed in, they scale with the diagram: ${height('pill', link(1))}`);

  // Input stops a zoom on its way where it is: a press, the wheel, a key. With reduced motion there is none.
  const stopsOn = async (start, input) => {
    await zoomKey('0');
    const from = viewBox();
    key(start);
    await new Promise((resolve) => setTimeout(resolve, 50));
    const running = view().animating();
    input();
    const stopped = viewBox();
    await new Promise((resolve) => setTimeout(resolve, 300));
    return running && !view().animating() && JSON.stringify(viewBox().slice(2)) === JSON.stringify(stopped.slice(2)) && viewBox()[2] !== from[2];
  };
  const pointerAt = (type) => canvas().dispatchEvent(new PointerEvent(type, { pointerId: 9, button: 0, bubbles: true, clientX: center[0], clientY: center[1] }));
  expect(
    await stopsOn('+', () => {
      pointerAt('pointerdown');
      pointerAt('pointerup');
    }),
    'a press stops the zoom',
  );
  expect(await stopsOn('-', () => canvas().dispatchEvent(new WheelEvent('wheel', { deltaY: 10, bubbles: true, cancelable: true }))), 'the wheel stops the zoom');
  expect(await stopsOn('+', () => key('ArrowRight')), 'a key stops the zoom');
  expect(near(saved()?.scale, scale()), `where it stopped is saved: ${JSON.stringify(saved())} ${scale()}`);
  const matchMedia = window.matchMedia;
  window.matchMedia = (query) => ({ matches: query.includes('reduced-motion'), media: query });
  box = viewBox();
  key('+');
  expect(!view().animating() && Math.abs(box[2] / viewBox()[2] - 1.15) < 1e-6, 'with reduced motion, + zooms at once');
  key('0');
  expect(!view().animating() && inView(), 'and 0 fits at once');
  window.matchMedia = matchMedia;

  // A double-click zooms to a box, centered at 100%; ⌘/Ctrl+0 zooms to 100%.
  await zoomKey('0');
  shape('box', item(2)).dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
  expect(view().animating(), 'a double-click zooms animated');
  await animated();
  const area = frame.top + 24 + (frame.height - 48 - root.querySelector('.diagram-controls').offsetHeight) / 2;
  expect(near(scale(), 1) && moved(middle(2), [center[0], area]) < 1, `to the box, centered at 100%: ${label()} ${middle(2)} ${[center[0], area]}`);
  await zoomKey('0');
  key('0', { ctrlKey: true });
  await animated();
  expect(near(scale(), 1), `Ctrl+0 zooms to 100%: ${label()}`);
  await zoomKey('0');

  // A failed layout says so and keeps the level shown; zooming goes on, and asking again works.
  // As published; `update` replaces the fixture's content.
  const published = structuredClone(CONTENTS_FOR_TESTS[KEYS.LIVE]);
  const revise = (revision, number, title) => {
    const content = structuredClone(published);
    const flow = content.documents.find((candidate) => candidate.id === KEYS.RELEASE);
    flow.revision = revision;
    flow.elements[item(number)].title = title;
    backend.update(KEYS.LIVE, content);
  };
  const title = (number) => shape('box', item(number))?.querySelector('.prism-name')?.textContent;
  revise(10, 2, 'Upload again');
  expect(await waitFor(() => title(2) === 'Upload again') && level() === 'Summary', `a new revision is laid out anew: ${level()}`);
  const message = () => root.querySelector('.diagram-message');
  backend.failing.add('full');
  click(levelButton('Full text'));
  await settled();
  expect(level() === 'Summary' && svg().querySelector('.prism-subtitle') && !message().hidden && message().textContent.includes('could not be laid out'), `a failed layout is reported, the level stays: ${level()} ${message().textContent}`);
  const failedAt = label();
  await zoomKey('-');
  expect(label() !== failedAt, `zooming still works after a failed layout: ${label()}`);
  backend.failing.clear();
  click(levelButton('Full text'));
  await settled();
  expect(level() === 'Full text' && svg().querySelector('.prism-body') && message().hidden, `asked again, the level comes: ${level()}`);

  // A new revision while a level change waits: the change lands on the new layout, anchored.
  backend.layoutDelay = 150;
  await zoomKey('0');
  click(shape('box', item(4)));
  await settle();
  anchored = middle(4);
  click(levelButton('Titles'));
  expect(backend.pending > 0 && level() === 'Titles' && svg().querySelector('.prism-body'), 'a level change waits for its layout; the level shown stays until it comes');
  revise(11, 4, 'Review again');
  await settled();
  expect(title(4) === 'Review again' && !svg().querySelector('.prism-subtitle, .prism-body'), `the waiting change shows the new revision: ${level()} ${title(4)}`);
  expect(moved(middle(4), anchored) < 1, `with the focused box kept in place: ${moved(middle(4), anchored)}`);
  backend.layoutDelay = 0;
  backend.update(KEYS.LIVE, structuredClone(published));
  expect(await waitFor(() => title(4) === 'Review anomalies'), 'shown again, as published');
  key('Escape');
  click(levelButton('Summary'));
  await settled();
  await zoomKey('0');
  const at = { clientX: frame.left + 200, clientY: frame.top + 150 };
  click(shape('box', item(3)));
  await settle();
  box = viewBox();
  const pointer = (type, dx) =>
    canvas().dispatchEvent(new PointerEvent(type, { pointerId: 7, button: 0, bubbles: true, clientX: at.clientX + dx, clientY: at.clientY }));
  pointer('pointerdown', 0);
  pointer('pointermove', 40);
  pointer('pointerup', 40);
  canvas().dispatchEvent(new MouseEvent('click', { bubbles: true }));
  await settle();
  expect(viewBox()[0] < box[0], 'dragging pans');
  expect(panel('.panel-title') === 'Validate batch', 'the click after a drag does not clear the focus');

  // A press on a shape that becomes a drag does not select it, even when it takes the keyboard.
  const press = (node, type, dx) =>
    node.dispatchEvent(new PointerEvent(type, { pointerId: 8, button: 0, bubbles: true, clientX: at.clientX + dx, clientY: at.clientY }));
  press(shape('box', item(5)), 'pointerdown', 0);
  shape('box', item(5)).focus();
  press(canvas(), 'pointermove', 40);
  press(canvas(), 'pointerup', 40);
  click(shape('box', item(5)));
  await settle();
  expect(panel('.panel-title') === 'Validate batch', 'a drag from a box does not select it');
  // A cancelled drag (the system took the pointer) leaves the next click working.
  press(shape('box', item(6)), 'pointerdown', 0);
  press(canvas(), 'pointermove', 40);
  canvas().dispatchEvent(new PointerEvent('pointercancel', { pointerId: 8, bubbles: true }));
  click(shape('box', item(6)));
  await settle();
  expect(panel('.panel-title') === 'Archive raw batches', 'after a cancelled drag a click selects');
  expect(!canvas().classList.contains('dragging'), 'and the drag ended');

  // Arrow keys in the panel scroll it instead of panning.
  box = viewBox();
  const details = root.querySelector('.detail-panel');
  details.focus();
  details.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
  expect(JSON.stringify(viewBox()) === JSON.stringify(box), 'arrow keys in the panel do not pan');
  click(shape('box', item(3)));
  await settle();

  // Hidden items stay out of the diagram, and come back with Shift+H.
  key('h');
  expect(await waitFor(() => !shape('box', item(3))), 'a hidden box leaves the diagram');
  expect(!svg().querySelector(`[data-id="${link(2)}"]`) && !svg().querySelector(`[data-id="${link(5)}"]`), 'so do its arrows');
  click(shape('gate', item(8)));
  await settle();
  key('h');
  expect(await waitFor(() => !shape('gate', item(8))), 'a hidden badge leaves the diagram');
  key('H');
  expect(await waitFor(() => shape('box', item(3)) && shape('gate', item(8))), 'Shift+H shows them again');

  // A dropped box keeps its place and number, faded, with its arrow dashed lightly; undo draws it as before.
  const place = (id) => {
    const outline = shape('box', id)?.querySelector('.prism-shape').getBBox();
    return outline ? [outline.x, outline.y].join() : null;
  };
  const placed = place(item(6));
  const original = structuredClone(CONTENTS_FOR_TESTS[KEYS.LIVE]);
  const dropped = structuredClone(original);
  const flow = dropped.documents.find((candidate) => candidate.id === KEYS.RELEASE);
  flow.revision = 3;
  flow.elements[item(6)].archived = true;
  flow.relationships[link(5)].archived = true;
  backend.update(KEYS.LIVE, dropped);
  expect(await waitFor(() => shape('box', item(6))?.classList.contains('prism-dropped')), 'a dropped box stays in the diagram');
  const droppedBox = shape('box', item(6));
  expect(place(item(6)) === placed && droppedBox.querySelector('.prism-number')?.textContent === '6' && droppedBox.querySelector('.prism-dropped-label')?.textContent === 'DROPPED', 'in its place, with its number and the dropped label');
  expect(Number(getComputedStyle(droppedBox).opacity) < 0.5 && droppedBox.getAttribute('aria-label') === 'Text: Archive raw batches (dropped)', 'faded and named dropped');
  expect(shape('arrow', link(5))?.classList.contains('prism-dropped') && shape('arrow', link(5)).querySelector('.prism-line').getAttribute('stroke-dasharray') === '2 4' && shape('pill', link(5)), 'its arrow stays with its pill, dashed lightly');
  click(droppedBox);
  await settle();
  expect(root.querySelector('.detail-panel .badge.dropped') && [...root.querySelectorAll('.detail-panel .panel-actions .button')].every((button) => button.disabled), 'the panel marks it and disables its actions');
  backend.update(KEYS.LIVE, structuredClone(original));
  expect(await waitFor(() => shape('box', item(6)) && !shape('box', item(6)).classList.contains('prism-dropped')), 'restored, it is drawn as before');

  // 5.4 Export: a standalone SVG with the theme written into attributes.
  key('s');
  await settle();
  const exported = backend.exports.at(-1);
  expect(exported?.title === 'Release flow', 'the export is named after the document');
  expect(exported?.svg.startsWith('<?xml') && exported.svg.includes('data-id="'), 'the export is an SVG with its item IDs');
  const file = new DOMParser().parseFromString(exported?.svg ?? '', 'image/svg+xml').documentElement;
  const [fileWidth, fileHeight] = [Number(file.getAttribute('width')), Number(file.getAttribute('height'))];
  expect(file.namespaceURI === 'http://www.w3.org/2000/svg' && file.nodeName === 'svg', 'the export is in the SVG namespace');
  expect(fileWidth > 0 && fileHeight > 0 && file.getAttribute('viewBox') === `0 0 ${fileWidth} ${fileHeight}`, 'it has its size and a viewBox of the whole diagram');
  expect(exported?.svg.includes('fill="rgb(255, 255, 255)"') && !exported.svg.includes('var('), 'with resolved colors');
  expect(!exported?.svg.includes('focused'), 'without the focus highlight');
  expect(root.querySelector('.statusbar .message').textContent.includes('Saved the diagram'), 'the saved path is shown');
  expect(level() === 'Summary' && fileWidth === backend.sizes.subtitles.width && exported.svg.includes('class="prism-subtitle"') && !exported.svg.includes('class="prism-body"'), 'the export is of the level shown');
  // Above 100% arrowheads keep their size on screen through a round trip of levels, and in the export.
  view().zoomAt(2.5, ...center);
  for (const name of ['Full text', 'Summary', 'Full text', 'Summary']) {
    click(levelButton(name));
    await settled();
  }
  expect(near(scale(), 2.5) && Math.abs(head() - 11.5) < 0.01, `arrowheads after a round trip at 250%: ${head()} px`);
  key('s');
  await settle();
  const roundTrip = new DOMParser().parseFromString(backend.exports.at(-1).svg, 'image/svg+xml').documentElement;
  expect([...roundTrip.querySelectorAll('marker')].every((marker) => marker.getAttribute('markerWidth') === '11.5'), 'and the export has them at their size');
  // Zoomed out, the export still has every text at full opacity, pills, badges and arrowheads at their size.
  view().zoomAt(0.3, ...center);
  await fade();
  const shownTexts = () => [...svg().querySelectorAll('text')].map((text) => getComputedStyle(text).opacity).join();
  const before = shownTexts();
  key('s');
  await settle();
  expect(shownTexts() === before && band() === 'far' && svg().getAnimations({ subtree: true }).length === 0, 'the export leaves the SVG shown as it is, no fade runs');
  const small = new DOMParser().parseFromString(backend.exports.at(-1).svg, 'image/svg+xml').documentElement;
  const opacities = new Set([...small.querySelectorAll('text')].map((text) => text.getAttribute('opacity')));
  expect(band() === 'far' && JSON.stringify([...opacities]) === '["1"]', `zoomed out, the export shows every text: ${[...opacities]}`);
  expect(!small.querySelector('[transform]') && [...small.querySelectorAll('marker')].every((marker) => marker.getAttribute('markerWidth') === '11.5') && !small.hasAttribute('data-zoom'), 'without the zoom on pills, badges and arrowheads');
  click(levelButton('Full text'));
  await settled();
  key('s');
  await settle();
  const full = backend.exports.at(-1).svg;
  expect(level() === 'Full text' && full.includes('class="prism-body"') && !full.includes('class="prism-subtitle"'), 'with the full text shown, the export has it');
  await zoomKey('0');

  // Each document keeps its own zoom and pan.
  key('+');
  await animated();
  const release = viewBox();
  const showDocument = (title) => [...root.querySelectorAll('.documents button')].find((button) => button.textContent === title).click();
  showDocument('App plan');
  expect(await waitFor(() => svg()?.querySelectorAll('[data-kind="box"]').length === 6 && !svg().querySelector(`[data-id="${item(1)}"]`)), 'another document has its own diagram');
  const other = viewBox();
  showDocument('Release flow');
  expect(await waitFor(() => svg()?.querySelector(`[data-id="${item(1)}"]`)), 'back on the release flow');
  expect(JSON.stringify(viewBox()) === JSON.stringify(release), 'its camera is kept');
  expect(other[2] !== release[2], 'the other document was fitted');

  // A zoom on its way: a new revision lets it end where it was heading, a fit too; another
  // document or tab keeps where it got to, or where it was heading.
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  await zoomKey('0');
  const fitScale = scale();
  key('+');
  await wait(50);
  revise(12, 2, 'Upload once more');
  expect(await waitFor(() => title(2) === 'Upload once more') && (await animated()), 'a revision arrives while zooming');
  expect(Math.abs(scale() / fitScale - 1.15) < 1e-6 && Math.abs(saved().scale / fitScale - 1.15) < 1e-6, `the zoom ends where it was heading: ${scale() / fitScale}`);
  key('+');
  await animated();
  key('0');
  await wait(50);
  revise(13, 2, 'Upload batch');
  expect(await waitFor(() => title(2) === 'Upload batch') && (await animated()), 'a revision arrives while fitting');
  expect(inView() && saved()?.x === undefined && saved()?.detail === 'full', `the fit ends fitted: ${JSON.stringify(saved())}`);
  key('+');
  await wait(50);
  showDocument('App plan');
  const reached = saved()?.scale;
  expect(await waitFor(() => !svg().querySelector(`[data-id="${item(1)}"]`)) && !view().animating(), 'another document stops the zoom');
  expect(reached > fitScale && reached < fitScale * 1.15, `the release flow keeps where it got to: ${reached / fitScale}`);
  showDocument('Release flow');
  expect(await waitFor(() => svg()?.querySelector(`[data-id="${item(1)}"]`)) && near(scale(), reached), `and shows it again: ${label()}`);
  await zoomKey('0');
  key('+');
  await wait(30);
  key('Tab', { ctrlKey: true });
  await wait(400);
  expect(Math.abs(saved().scale / fitScale - 1.15) < 1e-6, `in a hidden tab the zoom ends where it was heading: ${saved().scale / fitScale}`);
  key('Tab', { ctrlKey: true, shiftKey: true });
  expect(await waitFor(() => svg()?.querySelector(`[data-id="${item(1)}"]`)) && Math.abs(scale() / fitScale - 1.15) < 1e-6, `and shows it: ${label()}`);
  // A key that only modifies others does not stop a zoom.
  key('+');
  await wait(30);
  key('Shift', { shiftKey: true });
  expect(view().animating(), 'Shift alone does not stop the zoom');
  await animated();
  // Fades run only without reduced motion: every rule with a transition for the zoom bands is inside that query.
  const fadeRules = [...document.styleSheets]
    .flatMap((sheet) => [...sheet.cssRules])
    .flatMap((rule) => (rule.cssRules ? [...rule.cssRules].map((inner) => [rule, inner]) : [[null, rule]]))
    .filter(([, rule]) => rule.selectorText?.includes('data-zoom') && rule.style.transition);
  expect(fadeRules.length > 0 && fadeRules.every(([media]) => media?.conditionText === '(prefers-reduced-motion: no-preference)'), `fades only without reduced motion: ${fadeRules.map(([media]) => media?.conditionText)}`);

  // The view choice and the cameras are view state, restored when the tab opens again.
  key('v');
  await settle();
  expect(root.querySelector('.cards'), 'V goes back to the list');
  key('v');
  await waitFor(() => svg());
  await zoomKey('0');
  key('+');
  await animated();
  await new Promise((resolve) => setTimeout(resolve, 400));
  const views = backend.views[KEYS.LIVE];
  const camera = views?.cameras?.[KEYS.RELEASE];
  expect(views?.modes?.[KEYS.RELEASE] === 'diagram' && camera?.detail === 'full' && Number.isFinite(camera.scale), `mode, level and camera are saved: ${JSON.stringify(camera)}`);
  // Opens the tab again in a new app with the saved view state, its release flow's camera replaced.
  const reopen = async (replaced = null) => {
    const restored = fixture('diagram-checks');
    restored.state.arrangement.views = structuredClone(backend.views);
    if (replaced) {
      restored.state.arrangement.views[KEYS.LIVE].cameras[KEYS.RELEASE] = replaced;
    }
    const again = document.createElement('div');
    again.className = 'app';
    document.body.append(again);
    start(fakeBackend(restored), again);
    await waitFor(() => svg(again)?.querySelector('[data-id]'));
    const reopened = viewBox(again);
    const shown = {
      x: reopened[0],
      y: reopened[1],
      scale: again.querySelector('.diagram-canvas').clientWidth / reopened[2],
      zoom: again.querySelector('.diagram-controls .zoom').textContent,
      level: again.querySelector('.diagram-controls .segmented [aria-pressed="true"]')?.textContent,
      body: Boolean(again.querySelector('.diagram-svg .prism-body')),
      subtitle: Boolean(again.querySelector('.diagram-svg .prism-subtitle')),
    };
    again.remove();
    return shown;
  };
  root.hidden = true;
  let shown = await reopen();
  expect(close(shown.x, camera.x) && close(shown.y, camera.y) && close(shown.scale, camera.scale) && shown.zoom === `${Math.round(camera.scale * 100)}%`, `the reopened tab has the saved zoom and pan: ${JSON.stringify(shown)}`);
  expect(shown.level === 'Full text' && shown.body, `at the saved level: ${JSON.stringify(shown)}`);
  shown = await reopen({ detail: 'names' });
  expect(shown.level === 'Titles' && !shown.subtitle && shown.scale <= 1, `a fitted camera keeps its level: ${JSON.stringify(shown)}`);
  // As saved by the zoom that changed the level (commit ba53878): its level, zoom and pan.
  shown = await reopen({ detail: 'subtitles', x: 20, y: -20, scale: 1.2 });
  expect(shown.level === 'Summary' && shown.subtitle && close(shown.x, 20) && shown.zoom === '120%', `an older camera opens as saved: ${JSON.stringify(shown)}`);
  shown = await reopen({ x: 20, y: -20, scale: 1.2 });
  expect(shown.level === 'Summary' && shown.scale <= 1, `one without a level opens fitted: ${JSON.stringify(shown)}`);

  // A tab that leaves the window takes its views along.
  root.hidden = false;
  expect(app.diagramViews.has(KEYS.LIVE), 'the tab has a diagram view');
  const leaving = view();
  key('+');
  await backend.moveTab(KEYS.LIVE, null);
  expect(await waitFor(() => !app.diagramViews.has(KEYS.LIVE) && !app.listViews.has(KEYS.LIVE)), 'its list and diagram views are dropped');
  expect(!leaving.animating(), 'with no zoom left running');

  document.getElementById('result').textContent = failures.length === 0 ? 'PASS' : `FAIL: ${failures.join('; ')}`;
}

/**
 * Batch 6: multi-selection, ask about several items and from the diagram panel, question
 * states, accept, drop and undo with their replies, and the tab's filter and hidden items.
 */
async function actionChecks(backend, app) {
  const failures = [];
  const expect = (condition, message) => {
    if (!condition) {
      failures.push(message);
    }
  };
  const { item } = RELEASE_IDS;
  const plan = (number) => `a0000000-0000-4000-8000-00000000000${number}`;
  const card = (id) => document.querySelector(`.card[data-id="${id}"]`);
  const selectedCards = () => [...document.querySelectorAll('.card.selected')].map((node) => node.dataset.id);
  const buttons = (scope) => [...document.querySelectorAll(`${scope} .button`)].map((node) => node.textContent);
  const message = () => document.querySelector('.statusbar .message');
  const ctrlClick = (node) => node.dispatchEvent(new MouseEvent('click', { bubbles: true, ctrlKey: true }));
  const saved = () => new Promise((resolve) => setTimeout(() => resolve(backend.views[KEYS.LIVE]), 400));
  const content = () => backend.content(KEYS.LIVE);
  const planOf = (snapshot) => snapshot.documents.find((document) => document.id === KEYS.PLAN);
  // The fake backend keeps the content it was last given; this is the fixture's.
  const original = structuredClone(CONTENTS_FOR_TESTS[KEYS.LIVE]);
  const changed = (change) => {
    const next = structuredClone(original);
    change(planOf(next));
    backend.update(KEYS.LIVE, next);
  };
  await settle();

  // While disconnected, Accept and Drop are disabled and their keys send nothing.
  key('Tab', { ctrlKey: true });
  await settle();
  const offline = () => [...document.querySelectorAll('.detail-panel .panel-actions .button')].map((node) => `${node.textContent}:${node.disabled}`);
  expect(offline().join('|') === 'Ask:true|Accept:true|Drop:true', `shared actions are disabled: ${offline()}`);
  for (const name of ['a', 'x', 'u']) {
    key(name);
    await settle();
    expect(backend.sent.length === 0 && message().textContent === 'Read-only until the bridge reconnects', `${name} is blocked while disconnected: ${message().textContent}`);
  }
  key('Tab', { ctrlKey: true, shiftKey: true });
  await settle();

  // 6.1 Space and ⌘/Ctrl+click toggle the selection; the focused card's buttons count it.
  key('j');
  key(' ');
  key('j');
  key('j');
  key(' ');
  await settle();
  expect(JSON.stringify(selectedCards()) === JSON.stringify([plan(2), plan(4)]), `Space selects, got ${selectedCards()}`);
  expect(buttons('.detail-panel .panel-actions').join('|') === 'Ask about 2|Accept 2|Drop 2', `buttons name the selection: ${buttons('.detail-panel .panel-actions')}`);
  expect(document.querySelector('.pane-controls .selection')?.textContent === '2 selected · clear', 'the pane counts the selection');
  ctrlClick(card(plan(6)));
  await settle();
  expect(selectedCards().length === 3 && document.querySelector('.card.focused').dataset.id === plan(6), 'Ctrl+click selects and focuses');
  ctrlClick(card(plan(4)));
  key('j');
  key('j');
  key(' ');
  await settle();
  expect(JSON.stringify(selectedCards()) === JSON.stringify([plan(2)]), `toggling again deselects, got ${selectedCards()}`);
  expect(JSON.stringify((await saved())?.selected) === JSON.stringify([plan(2)]), 'the selection is view state');
  key('Escape');
  await settle();
  expect(selectedCards().length === 0 && !document.querySelector('.pane-controls .selection'), 'Esc clears the selection');
  key('A');
  await settle();
  expect(selectedCards().length === 8, 'Shift+A selects every item shown');
  key('Escape');
  await settle();

  // 6.1 Ask about two items: one `ask` at once, every item named, the text selected in the focused one quoted.
  key('k');
  key('k');
  key('k');
  key('k');
  key(' ');
  key('j');
  key('j');
  key(' ');
  await settle();
  const quote = selectText(document.querySelector('.detail-panel .panel-body'), 2, 30);
  key('Enter');
  await settle();
  const ask = backend.sent.at(-1);
  const items = ask?.command.items ?? [];
  expect(ask?.command.type === 'ask' && JSON.stringify(ask.command.selection) === JSON.stringify({ document_id: KEYS.PLAN, revision: 3, ids: [plan(2), plan(4)] }), 'one ask selects both items');
  expect(JSON.stringify(items.map((entry) => [entry.number, entry.title])) === JSON.stringify([[2, 'Listener, pairing file, session registry'], [4, 'Liveness of a connection is not part of the protocol']]), 'and names both');
  expect(items[0]?.quote.endsWith('\nSource: features/prism/app/host/src/registry.rs') && items[1]?.quote === quote, 'the other item is quoted in full, the focused one by the selection');
  expect(!('question' in (ask?.command ?? {})) && !document.querySelector('.detail-panel textarea, .ask'), 'without a typed question or an editor');
  backend.reply(ask.key, ask.id);
  await settle();
  expect(message().textContent === 'Shared items 2, 4 with the CLI prompt box.', `the reply is shown: ${message().textContent}`);
  expect(selectedCards().length === 2, 'asking keeps the selection');

  // Enter pressed again before the reply, or held down, shares nothing more.
  const asked = backend.sent.length;
  key('Enter');
  key('Enter');
  key('Enter', { repeat: true });
  await settle();
  expect(backend.sent.length === asked + 1, `one ask while one is on its way, got ${backend.sent.length - asked}`);
  const pendingAsk = backend.sent.at(-1);
  backend.reply(pendingAsk.key, pendingAsk.id);
  await settle();
  key('Enter', { repeat: true });
  await settle();
  expect(backend.sent.length === asked + 1, 'a repeated Enter asks nothing');

  // An error reply shows the error.
  key('Enter');
  await settle();
  const failing = backend.sent.at(-1);
  backend.reply(failing.key, failing.id, { code: 'failed', message: 'binding retired' });
  await settle();
  expect(message().classList.contains('error') && message().textContent === 'binding retired', `and shows the error: ${message().textContent}`);

  // Ask reads the items as shown now: a new title goes along.
  changed((document) => {
    document.revision = 4;
    document.elements[plan(2)].title = 'Listener and pairing';
  });
  await settle();
  key('Enter');
  await settle();
  const resent = backend.sent.at(-1);
  expect(resent?.command.items[0].title === 'Listener and pairing' && resent.command.selection.revision === 4, `the ask reads the items again: ${JSON.stringify(resent?.command)}`);
  backend.reply(resent.key, resent.id);
  await settle();

  // A dropped item keeps its card, number and place, greyed with a badge, and stays selected.
  const numbers = () => [...document.querySelectorAll('.card')].map((node) => [node.dataset.id, node.querySelector('.number').textContent]);
  const numbered = JSON.stringify(numbers());
  changed((document) => {
    document.revision = 5;
    document.elements[plan(2)].archived = true;
  });
  await settle();
  expect(JSON.stringify(numbers()) === numbered, `a drop shifts no number: ${JSON.stringify(numbers())}`);
  expect(card(plan(2))?.classList.contains('dropped') && card(plan(2)).querySelector('.badge.dropped')?.textContent === 'dropped', 'the dropped card is marked');
  expect(JSON.stringify(selectedCards()) === JSON.stringify([plan(2), plan(4)]), `and stays selected: ${selectedCards()}`);
  const hint = () => document.querySelector('.detail-panel .panel-actions .hint')?.textContent;
  expect(buttons('.detail-panel .panel-actions').join('|') === 'Ask about 1|Accept 1|Drop 1' && hint() === '1 dropped item takes no part', `the buttons leave it out: ${buttons('.detail-panel .panel-actions')} ${hint()}`);
  key('Enter');
  await settle();
  const partial = backend.sent.at(-1);
  expect(JSON.stringify(partial?.command.selection.ids) === JSON.stringify([plan(4)]) && partial.command.items.length === 1 && partial.command.items[0].number === 4, `ask leaves the dropped item out: ${JSON.stringify(partial?.command)}`);
  backend.reply(partial.key, partial.id);
  await settle();

  // Only dropped items picked: Ask, Accept and Drop are disabled and their keys refuse.
  const disabled = () => [...document.querySelectorAll('.detail-panel .panel-actions .button')].map((node) => `${node.textContent}:${node.disabled}`).join('|');
  for (const view of [{ selected: [plan(2)], focus: plan(4) }, { selected: [], focus: plan(2) }]) {
    app.updateView(KEYS.LIVE, view);
    await settle();
    expect(disabled() === 'Ask:true|Accept:true|Drop:true' && hint() === DROPPED_NOTE, `${JSON.stringify(view)}: disabled with the reason: ${disabled()} ${hint()}`);
    const sentBefore = backend.sent.length;
    for (const name of ['Enter', 'a', 'x']) {
      app.setMessage('');
      key(name);
      await settle();
      expect(backend.sent.length === sentBefore && message().textContent === DROPPED_NOTE && message().classList.contains('error'), `${name} refuses a dropped item: ${message().textContent}`);
    }
  }

  // U still undoes the drop, and the item loses its mark.
  key('u');
  await settle();
  const undoDrop = backend.sent.at(-1);
  expect(undoDrop?.command.type === 'undo' && undoDrop.command.expected_revision === 5, 'U undoes the drop');
  backend.reply(undoDrop.key, undoDrop.id);
  backend.update(KEYS.LIVE, structuredClone(original));
  await settle();
  expect(!card(plan(2)).classList.contains('dropped') && !card(plan(2)).querySelector('.badge.dropped') && JSON.stringify(numbers()) === numbered, 'the restored item is unmarked, in its place');

  // A partial publish, as the bridge sends it: the link and the group stored without a
  // position get the one they were shown at, one item is revised, an element and a link are
  // added after the last record. No number changes; the new ones come last.
  const appended = 'a0000000-0000-4000-8000-000000000009';
  const appendedLink = 'b0000000-0000-4000-8000-000000000009';
  changed((document) => {
    document.revision = 6;
    Object.values(document.relationships)[0].order = 6;
    Object.values(document.groups)[0].order = 7;
    document.elements[plan(3)].body = 'Revised by a partial publish.';
    document.elements[plan(3)].semantic_revision = 6;
    document.elements[appended] = { ...document.elements[plan(1)], id: appended, title: 'Appended', order: 8, semantic_revision: 6 };
    document.relationships[appendedLink] = { ...Object.values(document.relationships)[0], id: appendedLink, from: appended, label: 'appended link', order: 9, semantic_revision: 6 };
  });
  await settle();
  const after = numbers();
  const expectedAfter = [...JSON.parse(numbered), [appended, '9'], [appendedLink, '10']];
  expect(JSON.stringify(after) === JSON.stringify(expectedAfter), `every number is stable after a partial publish: ${JSON.stringify(after)}`);
  backend.update(KEYS.LIVE, structuredClone(original));
  await settle();
  app.updateView(KEYS.LIVE, { selected: [plan(2), plan(4)], focus: plan(4) });
  await settle();

  // 6.4 Accept: one `action` on the selection; the note says acceptance is no tool permission.
  expect(document.querySelector('.detail-panel .panel-actions .button:nth-child(2)').title.includes('never permission to run tools'), 'the Accept button says what acceptance is');
  key('a');
  await settle();
  const accept = backend.sent.at(-1);
  expect(accept?.command.type === 'action' && accept.command.action.action === 'accept' && accept.command.expected_revision === 3, 'accept is an action at the revision');
  expect(JSON.stringify(accept?.command.action.ids) === JSON.stringify([plan(2), plan(4)]), 'on the selected items');
  expect(message().textContent === 'Accepting…', 'while it is on its way');
  backend.reply(accept.key, accept.id);
  await settle();
  expect(message().textContent === 'Accepted 2 items. Acceptance is feedback for the agent, never permission to run tools.', `the note: ${message().textContent}`);
  expect(selectedCards().length === 0, 'a committed action clears its items from the selection');
  let next = await content();
  planOf(next).revision = 4;
  planOf(next).elements[plan(4)].accepted_revision = 4;
  backend.update(KEYS.LIVE, next);
  await settle();
  expect(card(plan(4)).querySelector('.badge.accepted') && !card(plan(4)).querySelector('.badge.stale'), 'the card shows accepted');
  next = await content();
  planOf(next).revision = 5;
  planOf(next).elements[plan(4)].semantic_revision = 5;
  backend.update(KEYS.LIVE, next);
  await settle();
  expect(card(plan(4)).querySelector('.badge.stale')?.textContent === 'changed since accepted' && !card(plan(4)).querySelector('.badge.accepted'), 'and changed since accepted after a revision');
  key('a');
  await settle();
  const stale = backend.sent.at(-1);
  expect(JSON.stringify(stale?.command.action.ids) === JSON.stringify([plan(4)]) && stale.command.expected_revision === 5, 'without a selection accept takes the focused item');
  backend.reply(stale.key, stale.id, { code: 'stale_revision', current: 6 });
  await settle();
  expect(message().classList.contains('error') && message().textContent.includes('revision 6'), 'a stale revision is explained');

  // 6.3 Drop the selection with one `action`, then undo through every reply.
  key('k');
  key(' ');
  key('j');
  key('j');
  key(' ');
  await settle();
  key('x');
  await settle();
  const drop = backend.sent.at(-1);
  expect(drop?.command.action.action === 'archive' && JSON.stringify(drop.command.action.ids) === JSON.stringify([plan(3), plan(5)]), 'drop archives the selection in one action');
  backend.reply(drop.key, drop.id);
  await settle();
  expect(message().textContent === 'Dropped 2 items. U undoes.', `drop is confirmed: ${message().textContent}`);
  next = await content();
  planOf(next).revision = 6;
  planOf(next).elements[plan(3)].archived = true;
  planOf(next).elements[plan(5)].archived = true;
  backend.update(KEYS.LIVE, next);
  await settle();
  expect(card(plan(3))?.classList.contains('dropped') && card(plan(5))?.querySelector('.badge.dropped') && document.querySelectorAll('.card').length === 8, 'the dropped items stay in the list, marked');
  const undo = async (error, expected) => {
    key('u');
    await settle();
    const sent = backend.sent.at(-1);
    expect(sent?.command.type === 'undo' && sent.command.document_id === KEYS.PLAN && sent.command.expected_revision === 6, 'undo names the document and its revision');
    expect(message().textContent === 'Undoing…', 'undo is on its way');
    backend.reply(sent.key, sent.id, error);
    await settle();
    expect(message().textContent.includes(expected) && message().classList.contains(error ? 'error' : 'success'), `undo reply ${error?.code}: ${message().textContent}`);
  };
  await undo({ code: 'undo_conflict' }, 'conflicts with later changes and was discarded; U undoes the one before it');
  await undo({ code: 'not_found', message: 'action to undo for this document' }, 'Nothing to undo in this document.');
  await undo({ code: 'stale_revision', current: 7 }, 'revision 7');
  await undo(null, 'Undid the latest action on this document.');
  backend.update(KEYS.LIVE, structuredClone(original));
  await settle();
  expect(!card(plan(3)).classList.contains('dropped') && !card(plan(5)).classList.contains('dropped'), 'the undone items lose their mark');

  // A reply that arrives before `send` returned the command ID is still matched.
  const realSend = backend.send;
  backend.send = async function (session, command) {
    const id = await realSend.call(this, session, command);
    app.onReply({ type: 'reply', key: session, id, error: null });
    return id;
  };
  key('u');
  await settle();
  backend.send = realSend;
  expect(message().textContent === 'Undid the latest action on this document.', `an early reply is matched: ${message().textContent}`);

  // Selected items hidden or filtered out take no part in Ask, Accept and Drop.
  app.updateView(KEYS.LIVE, { selected: [plan(2), plan(4), plan(6)], hidden: [plan(6)], filter: 'pairing' });
  await settle();
  expect(document.querySelector('.pane-controls .selection')?.textContent === '1 selected (2 not shown) · clear', `the pane counts what is not shown: ${document.querySelector('.pane-controls .selection')?.textContent}`);
  for (const name of ['a', 'x']) {
    key(name);
    await settle();
    const sent = backend.sent.at(-1);
    expect(JSON.stringify(sent?.command.action?.ids) === JSON.stringify([plan(2)]), `${name} acts on the shown selection only: ${JSON.stringify(sent?.command)}`);
    backend.reply(sent.key, sent.id, { code: 'stale_revision', current: 3 });
    await settle();
  }
  key('Enter');
  await settle();
  const shownAsk = backend.sent.at(-1);
  expect(JSON.stringify(shownAsk?.command.selection?.ids) === JSON.stringify([plan(2)]) && shownAsk.command.items[0].number === 1, `ask too: ${JSON.stringify(shownAsk?.command)}`);
  backend.reply(shownAsk.key, shownAsk.id);
  await settle();
  app.updateView(KEYS.LIVE, { selected: [], hidden: [], filter: '' });
  await settle();

  // The key map leaves out its least important hints first when a long message needs room.
  app.setMessage('A long message about what just happened, long enough to need most of the status bar at this window width, so that hints must go.');
  const hints = document.querySelector('.statusbar .keys');
  expect(hints.scrollWidth <= hints.clientWidth && hints.textContent.includes('move') && hints.textContent.includes('ask') && !hints.textContent.includes('all'), `whole hints only, the important ones kept: ${hints.textContent}`);

  // 6.2 Question states, the latest first, on the card and for every outcome.
  card(plan(5)).click();
  await settle();
  const blocks = () => [...document.querySelectorAll('.detail-panel .card-question')].map((node) => [node.querySelector('.q').textContent, node.querySelector('.state').textContent]);
  const expected = [
    ['Should the filter be shared as well?', 'Placed in the CLI prompt box'],
    ['Can hide follow the tab when it moves?', 'Answered'],
    ['Does the tab list need the same rule?', 'Shared with the conversation · asked about 2 items'],
    ['Would a per-window hide confuse users?', 'Failed: binding retired; explicitly resume the conversation'],
    ['Is this still open?', 'Refused: native sending from Prism is disabled for Claude'],
  ];
  expect(JSON.stringify(blocks()) === JSON.stringify(expected), `questions latest first with their states: ${JSON.stringify(blocks())}`);
  expect(document.querySelector('.detail-panel .card-question.tone-answered .a')?.textContent.startsWith('Yes: hidden items'), 'the answer is shown');
  const actionsRow = document.querySelector('.detail-panel .panel-actions');
  expect(actionsRow && actionsRow.compareDocumentPosition(document.querySelector('.detail-panel .card-question')) & Node.DOCUMENT_POSITION_FOLLOWING, 'the actions come before the questions');
  expect(card(plan(3)).querySelector('.badge.asked')?.textContent === 'asked · shared', `a collapsed card shows its latest state: ${card(plan(3)).querySelector('.badge.asked')?.textContent}`);
  next = await content();
  next.questions.find((question) => question.request_id.endsWith('000000000004')).answer = 'Not yet: the filter is a private reading aid.';
  backend.update(KEYS.LIVE, next);
  await settle();
  expect(blocks()[0][1] === 'Answered' && card(plan(5)).querySelector('.badge.asked')?.textContent === 'asked ×5 · answered', 'an answer that arrives replaces the state');
  const outcome = (state, extra = {}, answer = null) => questionBlock({ ...original.questions[2], outcome: { state, ...extra }, answer });
  const states = [
    [outcome('pending'), 'shared', 'Shared with the conversation'],
    [outcome('submitted'), 'prompt', 'Placed in the CLI prompt box'],
    [outcome('queued', { reason: null }), 'warning', 'Queued; start not confirmed'],
    [outcome('queued', { reason: 'a turn is running' }), 'warning', 'Queued; start not confirmed: a turn is running'],
    [outcome('started', { turn_id: 'turn-7' }), 'started', 'Started: turn-7'],
    [outcome('failed', { reason: 'bridge stopped' }), 'error', 'Failed: bridge stopped'],
    [outcome('refused', { reason: 'not bound' }), 'error', 'Refused: not bound'],
    [outcome('dropped', { reason: 'conversation cleared' }), 'error', 'Dropped: conversation cleared'],
    [outcome('unknown', { reason: 'no confirmation' }), 'warning', 'Unknown: no confirmation. Sending again may duplicate it.'],
    [outcome('submitted', {}, 'Because.'), 'answered', 'Answered'],
  ];
  for (const [block, tone, text] of states) {
    expect(block.classList.contains(`tone-${tone}`) && block.querySelector('.state').textContent === text, `outcome shown as ${text}, got ${block.className} ${block.querySelector('.state').textContent}`);
  }
  expect(states.at(-1)[0].querySelector('.a')?.textContent === 'Because.', 'an answered question shows its answer');

  // 6.3 The filter: local to the tab, over kind, title, text and ID; typing keeps the keyboard.
  key('/');
  const filter = document.querySelector('.pane-controls .filter');
  expect(document.activeElement === filter, '/ goes to the filter');
  const untouched = [backend.sent.length, JSON.stringify(app.view(KEYS.LIVE))];
  for (const name of ['a', 'x', 'u', 'h', 'H', 'A', ' ', 'j', 'v', '/']) {
    filter.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true }));
  }
  await settle();
  expect(backend.sent.length === untouched[0] && JSON.stringify(app.view(KEYS.LIVE)) === untouched[1] && document.activeElement === filter, 'keys typed into the filter do nothing else');
  filter.value = 'PAIRING';
  filter.dispatchEvent(new Event('input'));
  await settle();
  const titles = () => [...document.querySelectorAll('.card .title')].map((node) => node.textContent);
  expect(JSON.stringify(titles()) === JSON.stringify(['Listener, pairing file, session registry', 'arrangement uses sessions', 'Batch 4']), `the filter matches text case-insensitively: ${titles()}`);
  expect(document.activeElement === filter && filter.classList.contains('active'), 'the filter keeps the keyboard and shows it is active');
  filter.value = 'finding';
  filter.dispatchEvent(new Event('input'));
  await settle();
  expect(JSON.stringify(titles()) === JSON.stringify(['Liveness of a connection is not part of the protocol']) && document.querySelector('.card .number').textContent === '1', `and the kind, numbered as shown: ${titles()}`);
  filter.value = 'no such words';
  filter.dispatchEvent(new Event('input'));
  await settle();
  expect(document.querySelector('.empty h2')?.textContent === 'No items match “no such words”', 'an empty result says so');
  document.querySelector('.empty .button').click();
  await settle();
  expect(document.querySelectorAll('.card').length === 8 && filter.value === '', 'Clear filter shows everything');
  expect(backend.sent.filter((sent) => sent.command.type !== 'ask' && sent.command.type !== 'action' && sent.command.type !== 'undo').length === 0, 'filtering sends nothing');

  // The diagram respects filter and hide; both persist in the tab's view state.
  [...document.querySelectorAll('.documents button')].find((button) => button.textContent === 'Release flow').click();
  await settle();
  key('/');
  filter.value = 'batch';
  filter.dispatchEvent(new Event('input'));
  filter.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  await settle();
  expect(document.activeElement === document.querySelector('.view'), 'Enter leaves the filter for the list');
  key('v');
  const boxes = () => [...document.querySelectorAll('.diagram-svg [data-kind="box"]')].map((node) => node.dataset.id).sort();
  expect(await waitFor(() => boxes().length === 3), `the diagram shows only matching boxes: ${boxes()}`);
  expect(JSON.stringify(boxes()) === JSON.stringify([item(2), item(3), item(6)]), `namely ${boxes()}`);
  document.querySelector(`.diagram-svg [data-kind="box"][data-id="${item(3)}"]`).dispatchEvent(new MouseEvent('click', { bubbles: true }));
  await settle();
  expect(document.querySelector('.detail-panel .badge.accepted'), 'the panel shows an accepted item');
  key('h');
  expect(await waitFor(() => boxes().length === 2), 'a hidden box leaves the filtered diagram');
  const view = await saved();
  expect(view?.filter === 'batch' && JSON.stringify(view.hidden) === JSON.stringify([item(3)]) && view.modes?.[KEYS.RELEASE] === 'diagram', `filter and hidden are view state: ${JSON.stringify(view)}`);
  const restored = fixture('list');
  restored.state.arrangement.views = structuredClone(backend.views);
  const root = document.getElementById('app');
  root.hidden = true;
  const again = document.createElement('div');
  again.className = 'app';
  document.body.append(again);
  start(fakeBackend(restored), again);
  const againBoxes = () => [...again.querySelectorAll('.diagram-svg [data-kind="box"]')].map((node) => node.dataset.id).sort();
  expect(await waitFor(() => againBoxes().length === 2), 'a reopened tab keeps filter and hidden in the diagram');
  expect(again.querySelector('.pane-controls .filter')?.value === 'batch', 'and shows its filter');
  again.remove();
  root.hidden = false;
  key('H');
  key('/');
  filter.value = '';
  filter.dispatchEvent(new Event('input'));
  filter.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  const shape = (id) => document.querySelector(`.diagram-svg [data-kind="box"][data-id="${id}"]`);
  expect(await waitFor(() => boxes().length === 6), 'all boxes again');

  // 6.1 Ask from the diagram panel: Enter sends at once, with the list's payload.
  shape(item(5)).dispatchEvent(new MouseEvent('click', { bubbles: true }));
  await settle();
  expect(document.querySelector('.detail-panel .badge.stale')?.textContent === 'changed since accepted', 'the panel shows changed since accepted');
  shape(item(4)).dispatchEvent(new MouseEvent('click', { bubbles: true }));
  await settle();
  expect(buttons('.detail-panel .panel-actions').join('|') === 'Ask|Accept|Drop', `the panel has the item actions: ${buttons('.detail-panel .panel-actions')}`);
  const panelQuote = selectText(document.querySelector('.detail-panel .panel-body'), 3, 40);
  key('Enter');
  await settle();
  const panelAsk = backend.sent.at(-1);
  expect(JSON.stringify(panelAsk?.command) === JSON.stringify({ type: 'ask', selection: { document_id: KEYS.RELEASE, revision: 2, ids: [item(4)] }, items: [{ number: 4, title: 'Review anomalies', quote: panelQuote }] }), `the panel sends the list's payload, quoting the selection: ${JSON.stringify(panelAsk?.command)}`);
  expect(!document.querySelector('.detail-panel textarea') && document.activeElement === shape(item(4)), 'no editor: the box keeps the keyboard');
  backend.reply(panelAsk.key, panelAsk.id);
  await settle();

  // Selection in the diagram: Space and Ctrl+click; the panel asks about all of them.
  key(' ');
  ctrlClick(shape(item(2)));
  await settle();
  expect(shape(item(4)).classList.contains('selected') && shape(item(2)).classList.contains('selected'), 'selected boxes are marked');
  expect(shape(item(4)).getAttribute('aria-label').startsWith('Selected: ') && !shape(item(3)).getAttribute('aria-label').startsWith('Selected'), 'and named selected');
  expect(buttons('.detail-panel .panel-actions')[0] === 'Ask about 2', 'the panel counts the selection');
  document.querySelector('.detail-panel .panel-actions .button.primary').click();
  await settle();
  const both = backend.sent.at(-1);
  expect(JSON.stringify(both?.command.selection.ids) === JSON.stringify([item(2), item(4)]) && both.command.items[1].quote === 'An operator checks flagged readings before they are released.', `the Ask button asks about both, quoting each in full: ${JSON.stringify(both?.command)}`);
  backend.reply(both.key, both.id);
  await settle();
  key('Escape');
  await settle();
  expect(!document.querySelector('.diagram-svg .selected') && document.querySelector('.detail-panel .panel-title')?.textContent === 'Upload batch', 'Esc clears the selection first');
  key('Escape');
  await settle();
  expect(document.querySelector('.detail-panel .panel-tag')?.textContent === 'Diagram', 'then the focus');
  expect(!shape(item(4)).getAttribute('aria-label').startsWith('Selected'), 'a deselected box gets its name back');
  key('Enter');
  await settle();
  expect(message().textContent.startsWith('Pick an item first'), `Enter with nothing picked says so: ${message().textContent}`);

  document.getElementById('result').textContent = failures.length === 0 ? 'PASS' : `FAIL: ${failures.join('; ')}`;
}

/**
 * The view per document and the side pane: a document with arrows or groups opens in the
 * diagram, one without in a list without a switch; V's choice is saved per document; the
 * pane holds the view's controls in both views; neighboring arrows differ in color.
 */
async function viewChecks(backend, app) {
  const failures = [];
  const expect = (condition, message) => {
    if (!condition) {
      failures.push(message);
    }
  };
  const { link } = RELEASE_IDS;
  const svg = () => document.querySelector('.diagram-svg');
  const boxes = () => [...document.querySelectorAll('.diagram-svg [data-kind="box"]')].map((node) => node.dataset.id);
  const controls = (part) => document.querySelector(`.side-pane .pane-controls ${part}`);
  const pressed = () => controls('.segmented [aria-pressed="true"]')?.textContent;
  const showDocument = (title) => [...document.querySelectorAll('.documents button')].find((button) => button.textContent === title).click();
  const filter = () => controls('.filter');
  const typeFilter = async (text) => {
    filter().value = text;
    filter().dispatchEvent(new Event('input'));
    await settle();
  };
  const saved = () => new Promise((resolve) => setTimeout(() => resolve(backend.views[KEYS.LIVE]), 400));
  await settle();

  // A document with an arrow and a group opens in the diagram; the controls are in the pane.
  expect(await waitFor(() => svg()?.querySelector('[data-id]')), 'the app plan opens in the diagram');
  expect(pressed() === 'Diagram', `the switch shows the diagram, got ${pressed()}`);
  const toolbar = document.querySelector('.toolbar');
  const toolbarEmpty = () => toolbar.children.length === 1 && toolbar.firstChild.classList.contains('documents') && !toolbar.querySelector('.filter, .segmented, .info, .button');
  expect(toolbarEmpty(), 'the toolbar holds only the documents');
  expect(filter() && controls('.info')?.textContent === '8 items · revision 3', `the pane has the filter and the counts: ${controls('.info')?.textContent}`);
  expect([...document.querySelectorAll('.pane-controls .button')].some((button) => button.textContent === 'Export SVG'), 'and the export in the diagram');
  expect(document.querySelector('.side-pane .detail-panel .panel-tag')?.textContent === 'Diagram', 'below the controls, the overview');

  // The filter works from the pane in the diagram; / still goes there.
  key('/');
  expect(document.activeElement === filter(), '/ goes to the filter in the pane');
  await typeFilter('pairing');
  expect(await waitFor(() => boxes().length === 1), `the diagram shows only matching boxes: ${boxes()}`);
  expect(controls('.info')?.textContent === '3 items · revision 3' && document.activeElement === filter(), 'the counts follow, the filter keeps the keyboard');
  await typeFilter('');
  filter().dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  expect(await waitFor(() => boxes().length === 6), 'all boxes again');
  expect(document.activeElement === document.querySelector('.diagram-canvas'), 'Esc leaves the filter for the diagram');

  // Switching documents keeps each one's focus and selection.
  const planFocus = app.view(KEYS.LIVE).focus;
  const shape = svg().querySelector('[data-kind="box"]');
  shape.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  await settle();
  const picked = app.view(KEYS.LIVE).focus;
  app.toggleSelection(KEYS.LIVE, picked);
  await settle();
  expect(picked && picked !== planFocus && app.view(KEYS.LIVE).selected.includes(picked), `a box is focused and selected: ${picked}`);
  showDocument('Release flow');
  await settle();
  expect(app.view(KEYS.LIVE).focus !== picked && app.view(KEYS.LIVE).selected.length === 0, 'the other document starts without them');
  showDocument('App plan');
  expect(await waitFor(() => svg()?.querySelector(`[data-id="${picked}"].focused`)), 'back on the app plan, the box is focused again');
  expect(app.view(KEYS.LIVE).selected.includes(picked), 'and still selected');
  // Back to where the later checks start: nothing selected, the earlier focus.
  app.updateView(KEYS.LIVE, { focus: planFocus, selected: [] });
  key('Escape');
  await settle();

  // Arrows sharing a box or a lane pair differ in color; line, head and pill agree.
  showDocument('Release flow');
  expect(await waitFor(() => svg()?.querySelector(`[data-id="${link(5)}"]`)), 'the release flow opens in the diagram too');
  const colorClass = (kind, id) => [...svg().querySelector(`[data-kind="${kind}"][data-id="${id}"]`).classList].find((name) => name.startsWith('prism-arrow-c'));
  const stroke = (id) => getComputedStyle(svg().querySelector(`[data-kind="arrow"][data-id="${id}"] .prism-line`)).stroke;
  const pillFill = (id) => getComputedStyle(svg().querySelector(`[data-kind="pill"][data-id="${id}"] .prism-shape`)).fill;
  const classes = [1, 2, 3, 4, 5].map((number) => colorClass('arrow', link(number)));
  expect(JSON.stringify(classes) === JSON.stringify(['prism-arrow-c0', 'prism-arrow-c1', 'prism-arrow-c0', 'prism-arrow-c1', 'prism-arrow-c2']), `neighboring arrows differ: ${classes}`);
  expect([1, 2, 3, 4, 5].every((number) => colorClass('pill', link(number)) === classes[number - 1]), 'pills carry their arrow color');
  expect(stroke(link(4)) !== stroke(link(5)), `arrows between the same lanes differ: ${stroke(link(4))}, ${stroke(link(5))}`);
  expect([4, 5].every((number) => pillFill(link(number)) === stroke(link(number))), 'each pill takes its line color');
  const head = getComputedStyle(svg().querySelector('#prism-head-c2 path')).fill;
  expect(head === stroke(link(5)), `the arrowhead too: ${head}`);
  expect(svg().querySelector(`[data-kind="arrow"][data-id="${link(4)}"]`).classList.contains('prism-manual') && svg().querySelector(`[data-id="${link(4)}"] [stroke-dasharray]`), 'the manual arrow stays dashed');

  // Pane buttons keep the keyboard: a new revision updates the counts in place, a rebuild refocuses.
  const control = (name) => controls(`[data-control="${name}"]`);
  control('export').focus();
  const exportButton = control('export');
  const revised = structuredClone(CONTENTS_FOR_TESTS[KEYS.LIVE]);
  revised.documents.find((candidate) => candidate.id === KEYS.RELEASE).revision = 3;
  backend.update(KEYS.LIVE, revised);
  expect(await waitFor(() => controls('.info')?.textContent === '18 items · revision 3'), 'a new revision updates the counts');
  expect(document.activeElement === exportButton, 'the focused button keeps the keyboard');
  control('view-list').focus();
  control('view-list').click();
  await settle();
  expect(document.querySelector('.cards') && document.activeElement === control('view-list'), 'the rebuilt switch keeps the keyboard on its button');
  control('view-diagram').click();
  expect(await waitFor(() => svg()?.querySelector('[data-id]')), 'and switches back');
  key('+');
  expect(await waitFor(() => !app.diagramViews.get(KEYS.LIVE).animating()), 'the zoom animation ends');
  const releaseCamera = () => backend.views[KEYS.LIVE]?.cameras?.[KEYS.RELEASE];

  // V on the app plan: the list, saved for that document only.
  showDocument('App plan');
  await waitFor(() => svg());
  key('v');
  await settle();
  expect(document.querySelector('.cards') && pressed() === 'List', 'V shows the app plan as a list');
  expect(toolbarEmpty() && filter() && controls('.info')?.textContent === '8 items · revision 3' && controls('.segmented'), 'the list has the same controls in the pane');
  expect(![...document.querySelectorAll('.pane-controls .button')].some((button) => button.textContent === 'Export SVG'), 'without the export');
  expect(document.querySelector('.detail-panel .panel-title')?.textContent === document.querySelector('.card.focused .title')?.textContent, 'the pane shows the focused card');
  expect(document.querySelector('.detail-panel .panel-body')?.textContent.length > 0 && !document.querySelector('.card.focused .panel-body, .card.focused .button'), 'with its text and actions; the card stays compact');
  await typeFilter('pairing');
  expect(document.querySelectorAll('.card').length === 3, 'the filter works from the pane in the list');
  await typeFilter('');
  const modesSaved = (modes) => modes?.[KEYS.PLAN] === 'list' && modes[KEYS.RELEASE] === 'diagram' && Object.keys(modes).length === 2;
  expect(modesSaved((await saved())?.modes), `the choice is saved per document: ${JSON.stringify(backend.views[KEYS.LIVE]?.modes)}`);
  showDocument('Release flow');
  expect(await waitFor(() => svg()?.querySelector('[data-id]')) && pressed() === 'Diagram', 'the release flow keeps its own view');
  showDocument('App plan');
  await settle();
  expect(document.querySelector('.cards'), 'and the app plan its list');

  // Dropped arrows and groups are still drawn where they were: the switch stays.
  const archived = structuredClone(revised);
  const plan = archived.documents.find((candidate) => candidate.id === KEYS.PLAN);
  plan.elements[KEYS.TABS_ITEM].archived = true;
  for (const item of [...Object.values(plan.relationships), ...Object.values(plan.groups)]) {
    item.archived = true;
  }
  backend.update(KEYS.LIVE, archived);
  expect(await waitFor(() => document.querySelectorAll('.cards .card.dropped').length === 3), 'the dropped item, arrow and group stay in the list');
  expect(controls('.segmented'), 'and the view switch stays');
  backend.update(KEYS.LIVE, revised);
  expect(await waitFor(() => document.querySelectorAll('.cards .card.dropped').length === 0 && controls('.segmented')), 'undone, nothing is marked');

  // A document without arrows or groups: a list, no switch, and V only says so.
  showDocument('Review findings');
  await settle();
  expect(document.querySelector('.cards') && !controls('.segmented') && filter(), 'a plain list has the filter but no view switch');
  expect(!document.querySelector('.statusbar .keys').textContent.includes('diagram'), `and no V hint: ${document.querySelector('.statusbar .keys').textContent}`);
  key('v');
  await settle();
  expect(document.querySelector('.cards') && document.querySelector('.statusbar .message').textContent === 'Nothing to draw: this document has no arrows or groups.', `V says why: ${document.querySelector('.statusbar .message').textContent}`);
  expect(modesSaved((await saved())?.modes), 'and saves nothing');

  // A reopened tab shows each document as chosen, with its camera; a `mode` saved per tab before is ignored.
  const camera = releaseCamera();
  const restored = fixture('view-checks');
  restored.state.arrangement.views = structuredClone(backend.views);
  Object.assign(restored.state.arrangement.views[KEYS.LIVE], { document: KEYS.RELEASE, mode: 'list' });
  const root = document.getElementById('app');
  root.hidden = true;
  const again = document.createElement('div');
  again.className = 'app';
  document.body.append(again);
  const reopened = fakeBackend(restored);
  start(reopened, again);
  expect(await waitFor(() => again.querySelector('.diagram-svg')?.getAttribute('viewBox')), 'the reopened tab shows the release flow as a diagram');
  const box = again.querySelector('.diagram-svg').getAttribute('viewBox').split(' ').map(Number);
  const width = again.querySelector('.diagram-canvas').clientWidth;
  expect(camera?.scale > 0 && Math.abs(box[0] - camera.x) < 0.5 && Math.abs(width / box[2] - camera.scale) < 0.01, `with its camera: ${box} ${JSON.stringify(camera)}`);
  [...again.querySelectorAll('.documents button')].find((button) => button.textContent === 'App plan').click();
  expect(await waitFor(() => again.querySelector('.cards')), 'and the app plan as a list');
  const resaved = await new Promise((resolve) => setTimeout(() => resolve(reopened.views[KEYS.LIVE]), 400));
  expect(resaved && !('mode' in resaved) && modesSaved(resaved.modes), `the old mode is not saved again: ${JSON.stringify(resaved)}`);
  again.remove();
  root.hidden = false;

  document.getElementById('result').textContent = failures.length === 0 ? 'PASS' : `FAIL: ${failures.join('; ')}`;
}

/** A bridge problem shows in every empty state and the sessions panel until it clears; the version is always shown. */
async function statusChecks(backend) {
  const failures = [];
  const expect = (condition, message) => {
    if (!condition) {
      failures.push(message);
    }
  };
  const text = (selector) => document.querySelector(selector)?.textContent ?? '';
  const problemShown = (selector) => [...document.querySelectorAll(`${selector} .fact.bad`)].some((fact) => fact.textContent === VERSION_PROBLEM);
  await settle();

  expect(text('.empty h2') === 'No conversations connected yet', `the empty state: ${text('.empty h2')}`);
  expect(problemShown('.empty'), 'the empty state names both protocols and the older side');
  expect(text('.empty .facts .about') === 'Prism 0.1.0 · app protocol 2', `the version line: ${text('.empty .facts .about')}`);
  expect(document.querySelector('.strip-actions .button .dot.disconnected')?.title === VERSION_PROBLEM, 'the sessions button is marked');

  // Another conversation is connected, but not in this window.
  backend.change((state) => {
    state.sessions = [
      { key: 'claude:c1', host: 'd_claude', project: '/workspace/website', agent: 'claude', conversation: 'c1', link: { state: 'live' }, loaded: true },
    ];
    state.arrangement.closed = ['claude:c1'];
  });
  await settle();
  expect(text('.empty h2') === 'No tabs in this window', `the empty state with a session: ${text('.empty h2')}`);
  expect(problemShown('.empty'), 'the problem stays in the empty state with a session');
  document.querySelector('.strip-actions .button').click();
  await settle();
  expect(problemShown('.popover .status'), 'the sessions panel shows the problem');
  expect(text('.popover .status .about') === 'Prism 0.1.0 · app protocol 2', 'and the version line');

  // A bridge connected: the host cleared the problem.
  backend.change((state) => {
    state.status.problem = null;
  });
  await settle();
  expect(!document.querySelector('.popover .fact.bad') && !document.querySelector('.empty .fact.bad'), 'the problem is gone');
  expect(!document.querySelector('.strip-actions .button .dot'), 'and so is the mark');

  document.getElementById('result').textContent = failures.length === 0 ? 'PASS' : `FAIL: ${failures.join('; ')}`;
}

/** At a narrow window: the side pane goes below the canvas or the list and nothing scrolls the page. */
/** Closing and reopening documents: shared state from the store, shown the same in every window. */
async function documentChecks(backend) {
  const failures = [];
  const expect = (condition, message) => {
    if (!condition) {
      failures.push(message);
    }
  };
  const strip = () => [...document.querySelectorAll('.documents [role="tab"]')].map((node) => node.textContent);
  const shown = () => document.querySelector('.documents [aria-selected="true"]')?.textContent;
  const closedControl = () => document.querySelector('.closed-documents');
  const message = () => document.querySelector('.statusbar .message').textContent;
  const original = structuredClone(CONTENTS_FOR_TESTS[KEYS.LIVE]);
  const revisions = Object.fromEntries(original.documents.map((document) => [document.id, document.revision]));
  // The bridge's update after a commit: the revised document and the whole closed set.
  const published = (closed, revised = []) => {
    for (const id of revised) {
      revisions[id] += 1;
    }
    const next = structuredClone(original);
    for (const document of next.documents) {
      document.revision = revisions[document.id];
    }
    next.closed = closed;
    backend.update(KEYS.LIVE, next);
  };
  const lastCommand = () => backend.sent.at(-1)?.command;
  const commits = (closed, revised) => {
    const { key: session, id } = backend.sent.at(-1);
    published(closed, revised);
    backend.reply(session, id);
  };
  await settle();
  expect(JSON.stringify(strip()) === JSON.stringify(['App plan', 'Notes', 'Release flow', 'Review findings']), `four documents: ${strip()}`);
  expect(!closedControl(), 'no Closed control while nothing is closed');
  const selectedClose = document.querySelector('.document.selected .close');
  expect(selectedClose && getComputedStyle(selectedClose).visibility === 'visible', 'the shown document has its close control');
  const otherClose = document.querySelector('.document:not(.selected) .close');
  expect(otherClose && getComputedStyle(otherClose).visibility === 'hidden', 'the others only on hover or focus');
  const tab = document.querySelector('.document.selected [role="tab"]').getBoundingClientRect();
  const control = selectedClose.getBoundingClientRect();
  expect(control.right <= tab.right && control.left >= tab.left && control.top >= tab.top && control.bottom <= tab.bottom, 'the close control sits inside its tab');

  // Close via ×: one `close` action at the document's revision; the strip follows the store.
  const reviewClose = [...document.querySelectorAll('.document')].find((node) => node.textContent.startsWith('Review findings')).querySelector('.close');
  reviewClose.click();
  await settle();
  expect(JSON.stringify(lastCommand()) === JSON.stringify({ type: 'action', document_id: KEYS.REVIEW, expected_revision: 1, action: { action: 'close' } }), `× sends close: ${JSON.stringify(lastCommand())}`);
  expect(strip().includes('Review findings'), 'nothing changes before the store says so');
  commits([KEYS.REVIEW], [KEYS.REVIEW]);
  expect(await waitFor(() => !strip().includes('Review findings')), 'the closed document leaves the strip');
  expect(closedControl()?.textContent === 'Closed (1)', `Closed (1) at the end of the strip: ${closedControl()?.textContent}`);
  expect(message() === 'Closed “Review findings” for everyone. U reopens it.', `the reply is shown: ${message()}`);

  // U right after the close undoes it, on the closed document.
  key('u');
  await settle();
  expect(JSON.stringify(lastCommand()) === JSON.stringify({ type: 'undo', document_id: KEYS.REVIEW, expected_revision: 2 }), `U undoes the close: ${JSON.stringify(lastCommand())}`);
  commits([], [KEYS.REVIEW]);
  expect(await waitFor(() => strip().includes('Review findings') && !closedControl()), 'undo reopens it');
  expect(await waitFor(() => shown() === 'Review findings'), `and shows it: ${shown()}`);
  // The next U is an ordinary undo of the document shown.
  key('u');
  await settle();
  expect(JSON.stringify(lastCommand()) === JSON.stringify({ type: 'undo', document_id: KEYS.REVIEW, expected_revision: 3 }), `then U undoes on the shown document: ${JSON.stringify(lastCommand())}`);
  backend.reply(backend.sent.at(-1).key, backend.sent.at(-1).id, { code: 'not_found', message: 'action to undo for this document' });
  await settle();

  // Close via W: the document shown; the next one takes its place.
  document.querySelector('.documents [role="tab"]').click();
  await settle();
  key('w');
  await settle();
  expect(JSON.stringify(lastCommand()) === JSON.stringify({ type: 'action', document_id: KEYS.PLAN, expected_revision: 3, action: { action: 'close' } }), `W closes the shown document: ${JSON.stringify(lastCommand())}`);
  commits([KEYS.PLAN], [KEYS.PLAN]);
  expect(await waitFor(() => !strip().includes('App plan') && shown() === 'Notes'), `the next document is shown: ${shown()}`);

  // Closed (N) lists the closed documents; picking one reopens and shows it.
  closedControl().click();
  await settle();
  const items = () => [...document.querySelectorAll('.menu:not([hidden]) [role="menuitem"]')].map((node) => node.textContent);
  expect(JSON.stringify(items()) === JSON.stringify(['App plan']), `the menu lists the closed documents: ${items()}`);
  const menu = document.querySelector('.menu').getBoundingClientRect();
  expect(menu.top >= closedControl().getBoundingClientRect().bottom && menu.right <= window.innerWidth, 'the menu opens below its control, inside the window');
  document.querySelector('.menu [role="menuitem"]').click();
  await settle();
  expect(document.querySelector('.menu').hidden, 'the menu closes');
  expect(JSON.stringify(lastCommand()) === JSON.stringify({ type: 'action', document_id: KEYS.PLAN, expected_revision: 4, action: { action: 'reopen' } }), `the item sends reopen: ${JSON.stringify(lastCommand())}`);
  commits([], [KEYS.PLAN]);
  expect(await waitFor(() => strip().includes('App plan') && shown() === 'App plan'), `the reopened document is shown: ${shown()}`);

  // Closed elsewhere (the agent, or another window's command): it disappears here too.
  published([KEYS.RELEASE, KEYS.NOTES], [KEYS.RELEASE, KEYS.NOTES]);
  expect(await waitFor(() => !strip().includes('Release flow') && !strip().includes('Notes')), `documents closed by the bridge leave the strip: ${strip()}`);
  expect(closedControl()?.textContent === 'Closed (2)', 'and are counted');
  // U after a close from elsewhere is an ordinary undo of the shown document.
  key('u');
  await settle();
  expect(lastCommand().type === 'undo' && lastCommand().document_id === KEYS.PLAN, 'U does not reopen what another view closed');

  // The shown document closed elsewhere: the next one takes over with its own focus and
  // selection, and the closed one gets its own back when it reopens.
  const selectedCards = () => [...document.querySelectorAll('.card.selected')].map((node) => node.dataset.id);
  const focusedCard = () => document.querySelector('.card.focused')?.dataset.id;
  const showTab = (title) => [...document.querySelectorAll('.documents [role="tab"]')].find((node) => node.textContent === title).click();
  key('j');
  key(' ');
  await settle();
  const planState = [focusedCard(), selectedCards()];
  showTab('Review findings');
  await settle();
  key('j');
  key(' ');
  await settle();
  const reviewState = [focusedCard(), selectedCards()];
  expect(reviewState[1].length === 1 && planState[1].length === 1, `one item selected in each: ${planState} / ${reviewState}`);
  published([KEYS.RELEASE, KEYS.NOTES, KEYS.REVIEW], [KEYS.REVIEW]);
  expect(await waitFor(() => shown() === 'App plan'), `the next open document is shown: ${shown()}`);
  expect(JSON.stringify([focusedCard(), selectedCards()]) === JSON.stringify(planState), `with its own focus and selection: ${focusedCard()} ${selectedCards()}`);
  closedControl().click();
  await settle();
  [...document.querySelectorAll('.menu [role="menuitem"]')].find((node) => node.textContent === 'Review findings').click();
  await settle();
  commits([KEYS.RELEASE, KEYS.NOTES], [KEYS.REVIEW]);
  expect(await waitFor(() => shown() === 'Review findings'), `reopened: ${shown()}`);
  expect(JSON.stringify([focusedCard(), selectedCards()]) === JSON.stringify(reviewState), `the reopened document has its focus and selection back: ${focusedCard()} ${selectedCards()}`);

  // A close that something changed since (here the agent revised the closed document): U is an ordinary undo.
  key('w');
  await settle();
  commits([KEYS.RELEASE, KEYS.NOTES, KEYS.REVIEW], [KEYS.REVIEW]);
  expect(await waitFor(() => shown() === 'App plan'), 'W closed the shown document');
  published([KEYS.RELEASE, KEYS.NOTES, KEYS.REVIEW], [KEYS.REVIEW]);
  await settle();
  key('u');
  await settle();
  expect(lastCommand().type === 'undo' && lastCommand().document_id === KEYS.PLAN, `U undoes on the shown document once the closed one changed: ${JSON.stringify(lastCommand())}`);

  // Another command before the close's reply: U is an ordinary undo too.
  published([KEYS.RELEASE, KEYS.NOTES], [KEYS.REVIEW]);
  await settle();
  key('w');
  await settle();
  const closing = backend.sent.at(-1);
  key('a');
  await settle();
  expect(lastCommand().action?.action === 'accept', `the second command went out: ${JSON.stringify(lastCommand())}`);
  published([KEYS.RELEASE, KEYS.NOTES, KEYS.PLAN], [KEYS.PLAN]);
  backend.reply(closing.key, closing.id);
  await settle();
  key('u');
  await settle();
  expect(lastCommand().type === 'undo' && lastCommand().document_id !== KEYS.PLAN, `U does not reopen after another command: ${JSON.stringify(lastCommand())}`);
  published([KEYS.RELEASE, KEYS.NOTES], [KEYS.PLAN]);
  await settle();

  // Every document closed: the tab says so and keeps the control.
  published([KEYS.PLAN, KEYS.REVIEW, KEYS.RELEASE, KEYS.NOTES], [KEYS.PLAN, KEYS.REVIEW]);
  expect(await waitFor(() => document.querySelector('.empty h2')?.textContent === 'All documents are closed'), 'all closed is explained');
  expect(closedControl()?.textContent === 'Closed (4)' && strip().length === 0, 'with Closed (4) and no strip');
  // A disconnected tab cannot close: W says why.
  key('Tab', { ctrlKey: true });
  await settle();
  key('w');
  await settle();
  expect(message() === 'Read-only until the bridge reconnects', `W is blocked while disconnected: ${message()}`);
  document.getElementById('result').textContent = failures.length === 0 ? 'PASS' : `FAIL: ${failures.join('; ')}`;
}

async function narrowChecks() {
  const failures = [];
  const expect = (condition, message) => {
    if (!condition) {
      failures.push(message);
    }
  };
  expect(await waitFor(() => document.querySelector('.diagram-svg [data-id]')), 'the diagram is shown');
  const zoom = document.querySelector('.diagram-controls').getBoundingClientRect();
  // Fit shows the whole diagram, however small, above the zoom controls.
  const zoomLabel = () => document.querySelector('.diagram-controls .zoom').textContent;
  const fitted = zoomLabel();
  const frame = document.querySelector('.diagram-canvas').getBoundingClientRect();
  const shapes = [...document.querySelectorAll('.diagram-svg [data-kind="box"], .diagram-svg [data-kind="gate"]')].map((node) => node.getBoundingClientRect());
  const inside = shapes.every((rect) => rect.left >= frame.left && rect.right <= frame.right && rect.top >= frame.top && rect.bottom <= zoom.top);
  expect(shapes.length === 8 && inside && Number(fitted.replace('%', '')) < 100, `the whole diagram, clear of the zoom controls: ${fitted}`);
  expect(zoom.left >= frame.left && zoom.right <= frame.right, 'the zoom controls fit the canvas');
  key('-');
  await new Promise((resolve) => setTimeout(resolve, 300));
  expect(Number(zoomLabel().replace('%', '')) < Number(fitted.replace('%', '')), `zooming out goes below the fit, got ${zoomLabel()}`);
  const canvas = document.querySelector('.diagram-canvas').getBoundingClientRect();
  const panel = document.querySelector('.side-pane').getBoundingClientRect();
  const svg = document.querySelector('.diagram-svg').getBoundingClientRect();
  expect(window.innerWidth < 760, `a narrow window, got ${window.innerWidth}`);
  expect(panel.top >= canvas.bottom - 1 && panel.height > 0, 'the pane is below the canvas');
  expect(canvas.height > 150 && svg.width === canvas.width && svg.height === canvas.height, 'the SVG fills its frame');
  key('+');
  key('+');
  await settle();
  const page = document.scrollingElement;
  expect(page.scrollHeight <= window.innerHeight && page.scrollWidth <= window.innerWidth, 'zooming in does not scroll the page');
  expect(document.querySelector('.diagram-svg').getBoundingClientRect().width === canvas.width, 'the diagram pans inside its frame');
  key('v');
  await settle();
  const list = document.querySelector('.view').getBoundingClientRect();
  const pane = document.querySelector('.side-pane').getBoundingClientRect();
  expect(pane.top >= list.bottom - 1 && list.height > 150 && pane.width === list.width, 'in the list too');
  expect(page.scrollHeight <= window.innerHeight && page.scrollWidth <= window.innerWidth, 'and the page does not scroll');
  document.getElementById('result').textContent = failures.length === 0 ? 'PASS' : `FAIL: ${failures.join('; ')}`;
}

const backend = fakeBackend(fixture(scenario === 'checks' ? 'list' : scenario));
const app = start(backend, document.getElementById('app'));
if (CHECKS[scenario]) {
  CHECKS[scenario](backend, app).catch((error) => {
    document.getElementById('result').textContent = `FAIL: ${error}`;
  });
} else {
  replay(backend);
}
