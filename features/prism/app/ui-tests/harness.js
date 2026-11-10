// Drives the product UI (ui/js/app.js) with the fake backend. `?scenario=<name>` picks the
// fixture and the user input to replay; `?scenario=checks` and `?scenario=diagram-checks` run
// the behaviour checks of the list and the diagram and write PASS or the failures into #result.
import { start } from '../ui/js/app.js';
import { fakeBackend } from './fake-backend.js';
import { CONTENTS_FOR_TESTS, KEYS, RELEASE_IDS, fixture } from './fixtures.js';

const scenario = new URLSearchParams(location.search).get('scenario') ?? 'list';
const CHECKS = { checks, 'diagram-checks': diagramChecks, 'narrow-checks': narrowChecks };
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
  const panel = (part) => root.querySelector(`.diagram-panel ${part}`)?.textContent ?? '';
  const click = (node) => node.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  const canvas = () => root.querySelector('.diagram-canvas');
  const close = (a, b) => Math.abs(a - b) < 0.5;
  await settle();

  // V switches the tab to its diagram; the choice is view state.
  expect(root.querySelector('.cards'), 'the tab starts in the list');
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
  const details = root.querySelector('.diagram-panel');
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
  key(']');
  expect(await waitFor(() => svg()?.querySelectorAll('[data-kind="box"]').length === 2), 'the next document has its own diagram');
  const other = viewBox();
  key('[');
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
  expect(saved?.mode === 'diagram' && camera?.scale > 0, 'mode and camera are saved');
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

/** At a narrow window: the panel goes below the canvas and nothing scrolls the page. */
async function narrowChecks() {
  const failures = [];
  const expect = (condition, message) => {
    if (!condition) {
      failures.push(message);
    }
  };
  expect(await waitFor(() => document.querySelector('.diagram-svg [data-id]')), 'the diagram is shown');
  const canvas = document.querySelector('.diagram-canvas').getBoundingClientRect();
  const panel = document.querySelector('.diagram-panel').getBoundingClientRect();
  const svg = document.querySelector('.diagram-svg').getBoundingClientRect();
  expect(window.innerWidth < 760, `a narrow window, got ${window.innerWidth}`);
  expect(panel.top >= canvas.bottom - 1 && panel.height > 0, 'the panel is below the canvas');
  expect(canvas.height > 150 && svg.width === canvas.width && svg.height === canvas.height, 'the SVG fills its frame');
  key('+');
  key('+');
  await settle();
  const page = document.scrollingElement;
  expect(page.scrollHeight <= window.innerHeight && page.scrollWidth <= window.innerWidth, 'zooming in does not scroll the page');
  expect(document.querySelector('.diagram-svg').getBoundingClientRect().width === canvas.width, 'the diagram pans inside its frame');
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
