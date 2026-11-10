// Drives the product UI (ui/js/app.js) with the fake backend. `?scenario=<name>` picks the
// fixture and the user input to replay; `?scenario=checks`, `?scenario=diagram-checks`,
// `?scenario=view-checks` and `?scenario=action-checks` run the behaviour checks of the list,
// the diagram, the view per document with its side pane, and the item actions,
// `?scenario=status-checks` those of the connection status; each writes PASS or the failures
// into #result.
import { start } from '../ui/js/app.js';
import { questionBlock } from '../ui/js/panel.js';
import { fakeBackend } from './fake-backend.js';
import { CONTENTS_FOR_TESTS, KEYS, RELEASE_IDS, VERSION_PROBLEM, fixture } from './fixtures.js';

const scenario = new URLSearchParams(location.search).get('scenario') ?? 'list';
const CHECKS = { checks, 'diagram-checks': diagramChecks, 'view-checks': viewChecks, 'narrow-checks': narrowChecks, 'action-checks': actionChecks, 'status-checks': statusChecks };
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

/** Types into the open ask editor and sends with Ctrl+Enter. */
function typeQuestion(text, send = true) {
  const textarea = document.querySelector('.ask textarea');
  textarea.value = text;
  textarea.dispatchEvent(new Event('input'));
  if (send) {
    textarea.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true }));
  }
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
    const textarea = document.querySelector('.ask textarea');
    textarea.value = 'Is the 0600 mode enough on a shared Mac, or should the file live in the keychain?';
    textarea.dispatchEvent(new Event('input'));
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
    typeQuestion('Who is the operator at night, and can a flagged batch wait until morning?', false);
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
  expect(ask?.command.items.length === 1 && ask.command.items[0].number === 3 && ask.command.items[0].title === 'Windows and tabs', 'ask names item 3');
  expect(ask?.command.question === 'Why per project?', 'ask carries the typed question');
  expect(ask?.command.items[0].quote.startsWith('One tab per conversation'), 'ask quotes the item');
  expect(JSON.stringify(ask?.command.selection.ids) === '["a0000000-0000-4000-8000-000000000003"]', 'ask selects the item');
  expect(document.querySelector('.ask'), 'the editor waits for the reply');
  backend.reply(ask.key, ask.id);
  await settle();
  expect(!document.querySelector('.ask'), 'and closes once the question is shared');
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
  await settle();

  // V switches the document to its diagram; the choice is view state.
  expect(root.querySelector('.cards'), 'the document starts in the list, as chosen before');
  key('v');
  expect(await waitFor(() => svg()?.querySelector('[data-id]')), 'V shows the diagram');
  expect(root.querySelector('.segmented [aria-pressed="true"]')?.textContent === 'Diagram', 'the switch shows the diagram');

  // 5.1 Lanes per group, boxes, numbered pills, one dashed manual arrow, badges; themed by CSS.
  expect(svg().querySelectorAll('[data-kind="lane"]').length === 3, 'a lane per group');
  expect(svg().querySelectorAll('[data-kind="box"]').length === 6, 'six boxes');
  expect(svg().querySelectorAll('[data-kind="gate"]').length === 2, 'the finding and the question are badges');
  const pills = [...svg().querySelectorAll('[data-kind="pill"] .prism-label')].map((label) => label.textContent);
  expect(pills.every((label, index) => label.startsWith(String(index + 1))), `pills are numbered: ${pills}`);
  expect(svg().querySelectorAll('.prism-arrow.prism-manual [stroke-dasharray]').length === 1, 'the manual arrow is dashed');
  expect(shape('box', item(1)).querySelector('.prism-subtitle')?.textContent === 'Sensor app records a reading', 'a subtitle of five words');
  const paint = (node, property) => getComputedStyle(node).getPropertyValue(property);
  expect(paint(shape('box', item(1)).querySelector('.prism-shape'), 'fill') === 'rgb(255, 255, 255)', 'boxes take the surface token');
  expect(paint(shape('gate', item(7)).querySelector('.prism-shape'), 'fill') === 'rgb(232, 160, 32)', 'badges take the amber token');
  const frame = canvas().getBoundingClientRect();
  const inside = [...svg().querySelectorAll('[data-kind="box"], [data-kind="gate"]')].every((node) => {
    const rect = node.getBoundingClientRect();
    return rect.left >= frame.left && rect.right <= frame.right && rect.top >= frame.top && rect.bottom <= frame.bottom;
  });
  expect(inside, 'the whole diagram fits at start');
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
  expect(shape('pill', link(4)).getAttribute('aria-label') === 'Arrow 4 approve release: Review anomalies to Publish dashboard', 'pills are named');
  key('j');
  key('j');
  await settle();
  expect(document.activeElement === shape('gate', item(7)) && panel('.panel-title') === 'Clock drift on old devices', 'J follows the keyboard order');
  shape('pill', link(2)).focus();
  await settle();
  expect(panel('.panel-title') === 'upload', 'Tab onto a pill focuses its link');

  // 5.2 Zoom and pan with keys, wheel, pinch and drag; 0 fits.
  key('0');
  await settle();
  const fit = viewBox();
  key('+');
  let box = viewBox();
  expect(box[2] < fit[2] && close(box[0] + box[2] / 2, fit[0] + fit[2] / 2), '+ zooms in around the center');
  key('-');
  key('-');
  expect(viewBox()[2] > fit[2], '- zooms out');
  box = viewBox();
  key('ArrowRight');
  key('ArrowDown');
  expect(viewBox()[0] > box[0] && viewBox()[1] > box[1], 'arrows pan');
  key('0');
  expect(JSON.stringify(viewBox()) === JSON.stringify(fit), '0 fits again');
  box = viewBox();
  canvas().dispatchEvent(new WheelEvent('wheel', { deltaY: 120, bubbles: true, cancelable: true }));
  expect(viewBox()[1] > box[1] && viewBox()[2] === box[2], 'the wheel pans');
  const at = { clientX: frame.left + 200, clientY: frame.top + 150 };
  const world = (vb) => [vb[0] + (200 / frame.width) * vb[2], vb[1] + (150 / frame.height) * vb[3]];
  box = viewBox();
  canvas().dispatchEvent(new WheelEvent('wheel', { deltaY: -100, ctrlKey: true, bubbles: true, cancelable: true, ...at }));
  let zoomed = viewBox();
  expect(zoomed[2] < box[2] && close(world(zoomed)[0], world(box)[0]) && close(world(zoomed)[1], world(box)[1]), 'ctrl+wheel zooms around the pointer');
  expect(Math.abs(box[2] / zoomed[2] - 1.25) < 1e-6, `one wheel notch is one zoom step, got ${box[2] / zoomed[2]}`);
  canvas().dispatchEvent(new WheelEvent('wheel', { deltaY: -600, ctrlKey: true, bubbles: true, cancelable: true, ...at }));
  expect(Math.abs(zoomed[2] / viewBox()[2] - 1.25) < 1e-6, 'a large wheel delta is capped to one step');
  const gesture = (type, scale) => canvas().dispatchEvent(Object.assign(new Event(type, { bubbles: true, cancelable: true }), { scale, ...at }));
  key('0');
  box = viewBox();
  gesture('gesturestart', 1);
  gesture('gesturechange', 2);
  gesture('gestureend', 2);
  zoomed = viewBox();
  expect(close(zoomed[2], box[2] / 2) && close(world(zoomed)[0], world(box)[0]), 'a pinch gesture zooms');
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

  // Each document keeps its own zoom and pan.
  key('+');
  const release = viewBox();
  const showDocument = (title) => [...root.querySelectorAll('.documents button')].find((button) => button.textContent === title).click();
  showDocument('App plan');
  expect(await waitFor(() => svg()?.querySelectorAll('[data-kind="box"]').length === 6 && !svg().querySelector(`[data-id="${item(1)}"]`)), 'another document has its own diagram');
  const other = viewBox();
  showDocument('Release flow');
  expect(await waitFor(() => svg()?.querySelector(`[data-id="${item(1)}"]`)), 'back on the release flow');
  expect(JSON.stringify(viewBox()) === JSON.stringify(release), 'its camera is kept');
  expect(other[2] !== release[2], 'the other document was fitted');

  // The view choice and the cameras are view state, restored when the tab opens again.
  key('v');
  await settle();
  expect(root.querySelector('.cards'), 'V goes back to the list');
  key('v');
  await waitFor(() => svg());
  key('+');
  await new Promise((resolve) => setTimeout(resolve, 400));
  const saved = backend.views[KEYS.LIVE];
  const camera = saved?.cameras?.[KEYS.RELEASE];
  expect(saved?.modes?.[KEYS.RELEASE] === 'diagram' && camera?.scale > 0, 'mode and camera are saved');
  const restored = fixture('diagram-checks');
  restored.state.arrangement.views = structuredClone(backend.views);
  root.hidden = true;
  const again = document.createElement('div');
  again.className = 'app';
  document.body.append(again);
  start(fakeBackend(restored), again);
  expect(await waitFor(() => svg(again)?.getAttribute('viewBox')), 'the reopened tab shows its diagram');
  const reopened = viewBox(again);
  const width = again.querySelector('.diagram-canvas').clientWidth;
  expect(close(reopened[0], camera.x) && close(reopened[1], camera.y) && close(width / reopened[2], camera.scale), 'with the saved zoom and pan');

  // A tab that leaves the window takes its views along.
  root.hidden = false;
  again.remove();
  expect(app.diagramViews.has(KEYS.LIVE), 'the tab has a diagram view');
  await backend.moveTab(KEYS.LIVE, null);
  expect(await waitFor(() => !app.diagramViews.has(KEYS.LIVE) && !app.listViews.has(KEYS.LIVE)), 'its list and diagram views are dropped');

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
  expect(offline().join('|') === 'Ask:true|Accept:true|Hide:false|Drop:true', `shared actions are disabled: ${offline()}`);
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
  expect(buttons('.detail-panel .panel-actions').join('|') === 'Ask about 2|Accept 2|Hide 2|Drop 2', `buttons name the selection: ${buttons('.detail-panel .panel-actions')}`);
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

  // 6.1 Ask about two items: one `ask`, every item named, the text selected in the focused one quoted.
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
  expect(document.querySelector('.detail-panel .ask .quote')?.textContent === `Asking about 2 items (2, 4), quoting your selection in item 4, “${quote}”, the others in full`, `the editor says what it quotes: ${document.querySelector('.ask .quote')?.textContent}`);
  typeQuestion('How do these two relate?');
  await settle();
  const ask = backend.sent.at(-1);
  const items = ask?.command.items ?? [];
  expect(ask?.command.type === 'ask' && JSON.stringify(ask.command.selection) === JSON.stringify({ document_id: KEYS.PLAN, revision: 3, ids: [plan(2), plan(4)] }), 'one ask selects both items');
  expect(JSON.stringify(items.map((entry) => [entry.number, entry.title])) === JSON.stringify([[2, 'Listener, pairing file, session registry'], [4, 'Liveness of a connection is not part of the protocol']]), 'and names both');
  expect(items[0]?.quote.endsWith('\nSource: features/prism/app/host/src/registry.rs') && items[1]?.quote === quote, 'the other item is quoted in full, the focused one by the selection');
  expect(ask?.command.question === 'How do these two relate?', 'with the typed question');
  backend.reply(ask.key, ask.id);
  await settle();
  expect(message().textContent === 'Shared items 2, 4 with the CLI prompt box.', `the reply is shown: ${message().textContent}`);
  expect(selectedCards().length === 2, 'asking keeps the selection');

  // The editor opens in the first selected card when the focused one is not selected.
  key('j');
  key('j');
  key('Enter');
  await settle();
  expect(document.querySelector('.card.focused')?.dataset.id === plan(2) && document.querySelector('.detail-panel .ask'), 'the first selected card takes the editor');
  document.querySelector('.ask textarea').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  key('j');
  key('j');
  await settle();

  // An error reply keeps the editor and the draft.
  const kept = selectText(document.querySelector('.detail-panel .panel-body'), 2, 20);
  key('Enter');
  await settle();
  typeQuestion('Still there?');
  await settle();
  const failing = backend.sent.at(-1);
  const sendButton = () => document.querySelector('.detail-panel .ask .button.primary');
  expect(sendButton()?.disabled && sendButton().textContent === 'Sending…', 'the editor waits for the reply');
  backend.reply(failing.key, failing.id, { code: 'failed', message: 'binding retired' });
  await settle();
  expect(document.querySelector('.detail-panel .ask textarea')?.value === 'Still there?' && !sendButton().disabled, 'an error reply keeps the editor and the draft');
  expect(message().classList.contains('error') && message().textContent === 'binding retired', `and shows the error: ${message().textContent}`);

  // Sending reads the items again: a new title goes along, an item that left stops the send.
  changed((document) => {
    document.revision = 4;
    document.elements[plan(2)].title = 'Listener and pairing';
  });
  await settle();
  typeQuestion('Still there?');
  await settle();
  const resent = backend.sent.at(-1);
  expect(resent?.command.items[0].title === 'Listener and pairing' && resent.command.selection.revision === 4 && resent.command.items[1].quote === kept, `the send reads the items again, keeping the quote: ${JSON.stringify(resent?.command)}`);
  backend.reply(resent.key, resent.id);
  await settle();
  expect(!document.querySelector('.ask'), 'a confirmed ask closes the editor');
  key('Enter');
  await settle();
  changed((document) => {
    document.revision = 5;
    document.elements[plan(2)].archived = true;
  });
  await settle();
  expect(JSON.stringify(app.view(KEYS.LIVE).selected) === JSON.stringify([plan(4)]) && document.querySelector('.pane-controls .selection')?.textContent === '1 selected · clear', 'a dropped item leaves the selection when the update arrives');
  const sentBefore = backend.sent.length;
  typeQuestion('And now?');
  await settle();
  expect(backend.sent.length === sentBefore && message().textContent.includes('left the list') && document.querySelector('.detail-panel .ask'), `an item that left stops the send: ${message().textContent}`);
  document.querySelector('.ask textarea').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  backend.update(KEYS.LIVE, structuredClone(original));
  await settle();
  key('k');
  key('k');
  key(' ');
  key('j');
  key('j');
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
  expect(!card(plan(3)) && !card(plan(5)) && document.querySelectorAll('.card').length === 6, 'the dropped items leave the list');
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
  expect(card(plan(3)) && card(plan(5)), 'the undone items come back');

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
  expect(document.querySelector('.ask .quote')?.textContent === 'Quoting item 1, Listener, pairing file, session registry, in full', `ask too: ${document.querySelector('.ask .quote')?.textContent}`);
  document.querySelector('.ask textarea').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
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

  // 6.1 Ask from the diagram panel: Enter opens the editor there, with the same payload.
  shape(item(5)).dispatchEvent(new MouseEvent('click', { bubbles: true }));
  await settle();
  expect(document.querySelector('.detail-panel .badge.stale')?.textContent === 'changed since accepted', 'the panel shows changed since accepted');
  shape(item(4)).dispatchEvent(new MouseEvent('click', { bubbles: true }));
  await settle();
  expect(buttons('.detail-panel .panel-actions').join('|') === 'Ask|Accept|Hide|Drop', `the panel has the item actions: ${buttons('.detail-panel .panel-actions')}`);
  const panelQuote = selectText(document.querySelector('.detail-panel .panel-body'), 3, 40);
  key('Enter');
  await settle();
  const editor = document.querySelector('.detail-panel .ask textarea');
  expect(editor && document.activeElement === editor, 'Enter opens the editor in the panel');
  expect(document.querySelector('.detail-panel .ask .quote')?.textContent === `Quoting your selection: “${panelQuote}”`, 'quoting the text selected in the panel');
  typeQuestion('Who is on call at night?');
  await settle();
  const panelAsk = backend.sent.at(-1);
  expect(JSON.stringify(panelAsk?.command) === JSON.stringify({ type: 'ask', selection: { document_id: KEYS.RELEASE, revision: 2, ids: [item(4)] }, items: [{ number: 4, title: 'Review anomalies', quote: panelQuote }], question: 'Who is on call at night?' }), `the panel sends the list's payload: ${JSON.stringify(panelAsk?.command)}`);
  backend.reply(panelAsk.key, panelAsk.id);
  await settle();
  expect(!document.querySelector('.detail-panel .ask') && document.activeElement === shape(item(4)), 'the editor closes and the box has the keyboard');

  // Selection in the diagram: Space and Ctrl+click; the panel asks about all of them.
  key(' ');
  ctrlClick(shape(item(2)));
  await settle();
  expect(shape(item(4)).classList.contains('selected') && shape(item(2)).classList.contains('selected'), 'selected boxes are marked');
  expect(shape(item(4)).getAttribute('aria-label').startsWith('Selected: ') && !shape(item(3)).getAttribute('aria-label').startsWith('Selected'), 'and named selected');
  expect(buttons('.detail-panel .panel-actions')[0] === 'Ask about 2', 'the panel counts the selection');
  document.querySelector('.detail-panel .panel-actions .button.primary').click();
  await settle();
  expect(document.querySelector('.detail-panel .ask .quote')?.textContent === 'Asking about 2 items (2, 4), quoting each in full', `the Ask button asks about both: ${document.querySelector('.detail-panel .ask .quote')?.textContent}`);
  document.querySelector('.detail-panel .ask textarea').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  await settle();
  expect(!document.querySelector('.detail-panel .ask'), 'Esc in the editor cancels');
  key('Escape');
  await settle();
  expect(!document.querySelector('.diagram-svg .selected') && document.querySelector('.detail-panel .panel-title')?.textContent === 'Upload batch', 'Esc clears the selection first');
  key('Escape');
  await settle();
  expect(document.querySelector('.detail-panel .panel-tag')?.textContent === 'Diagram', 'then the focus');
  expect(!shape(item(4)).getAttribute('aria-label').startsWith('Selected'), 'a deselected box gets its name back');
  key('Enter');
  await settle();
  expect(message().textContent.startsWith('Pick an item first') && !document.querySelector('.detail-panel .ask'), `Enter with nothing picked says so: ${message().textContent}`);

  document.getElementById('result').textContent = failures.length === 0 ? 'PASS' : `FAIL: ${failures.join('; ')}`;
}

/**
 * The view per document and the side pane: a document with arrows or groups opens in the
 * diagram, one without in a list without a switch; V's choice is saved per document; the
 * pane holds the view's controls in both views; arrows of one lane pair differ in color.
 */
async function viewChecks(backend) {
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

  // Arrows between the same two lanes differ in color; line, head and pill agree.
  showDocument('Release flow');
  expect(await waitFor(() => svg()?.querySelector(`[data-id="${link(5)}"]`)), 'the release flow opens in the diagram too');
  const colorClass = (kind, id) => [...svg().querySelector(`[data-kind="${kind}"][data-id="${id}"]`).classList].find((name) => name.startsWith('prism-arrow-c'));
  const stroke = (id) => getComputedStyle(svg().querySelector(`[data-kind="arrow"][data-id="${id}"] .prism-line`)).stroke;
  const pillFill = (id) => getComputedStyle(svg().querySelector(`[data-kind="pill"][data-id="${id}"] .prism-shape`)).fill;
  const classes = [1, 2, 3, 4, 5].map((number) => colorClass('arrow', link(number)));
  expect(JSON.stringify(classes) === JSON.stringify(['prism-arrow-c0', 'prism-arrow-c0', 'prism-arrow-c0', 'prism-arrow-c0', 'prism-arrow-c1']), `staging to live has two colors, the other pairs one arrow each: ${classes}`);
  expect([1, 2, 3, 4, 5].every((number) => colorClass('pill', link(number)) === classes[number - 1]), 'pills carry their arrow color');
  expect(stroke(link(4)) !== stroke(link(5)), `the two arrows from staging to live differ: ${stroke(link(4))}, ${stroke(link(5))}`);
  expect([4, 5].every((number) => pillFill(link(number)) === stroke(link(number))), 'each pill takes its line color');
  const head = getComputedStyle(svg().querySelector('#prism-head-c1 path')).fill;
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
  expect(document.querySelector('.detail-panel .panel-body')?.textContent.startsWith('Replace the terminal UI') && !document.querySelector('.card.focused .panel-body, .card.focused .button'), 'with its text and actions; the card stays compact');
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

  // An arrow counts only while both its ends are there: without them and the group, nothing to draw.
  const archived = structuredClone(revised);
  const plan = archived.documents.find((candidate) => candidate.id === KEYS.PLAN);
  plan.elements[KEYS.TABS_ITEM].archived = true;
  Object.values(plan.groups).forEach((group) => {
    group.archived = true;
  });
  backend.update(KEYS.LIVE, archived);
  expect(await waitFor(() => !controls('.segmented') && document.querySelector('.cards')), 'an arrow to an archived item draws nothing: no switch');
  backend.update(KEYS.LIVE, revised);
  expect(await waitFor(() => controls('.segmented')), 'the switch is back with the items');

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
  expect(text('.empty .facts .about') === 'Prism 0.1.0 · app protocol 1', `the version line: ${text('.empty .facts .about')}`);
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
  expect(text('.popover .status .about') === 'Prism 0.1.0 · app protocol 1', 'and the version line');

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
async function narrowChecks() {
  const failures = [];
  const expect = (condition, message) => {
    if (!condition) {
      failures.push(message);
    }
  };
  expect(await waitFor(() => document.querySelector('.diagram-svg [data-id]')), 'the diagram is shown');
  const zoom = document.querySelector('.diagram-controls').getBoundingClientRect();
  const clear = [...document.querySelectorAll('.diagram-svg [data-kind="lane"], .diagram-svg [data-kind="box"]')].every((node) => {
    const rect = node.getBoundingClientRect();
    return rect.bottom <= zoom.top || rect.right <= zoom.left;
  });
  expect(clear, 'the fitted diagram leaves room for the zoom controls');
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
