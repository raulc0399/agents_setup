import { h } from './dom.js';

/**
 * Detail levels of the layout, least first (`Detail` in prism-core's graph.rs): titles only,
 * one line of text, the whole text. Zooming changes the level, so text keeps its size.
 */
const LEVELS = ['names', 'subtitles', 'full'];
const LEVEL_LABELS = { names: 'Titles', subtitles: 'Summary', full: 'Full text' };
/** The request for the sizes of every level without a layout, among the levels asked for. */
const SIZES_REQUEST = 'sizes';
/**
 * Pixels per world unit. At DESIGN_SCALE text has the size of the list's cards; at every
 * level the zoom stays between READABLE_MIN and READABLE_MAX. Zooming past either end
 * switches to the neighboring level at DESIGN_SCALE, more than a zoom step from both ends,
 * so a small zoom back does not switch again. A fit is never larger than DESIGN_SCALE; an
 * overview too large to fit at READABLE_MIN is shown at READABLE_MIN from its top left, or
 * around the focused item, to pan from there.
 */
const DESIGN_SCALE = 1;
const READABLE_MIN = 0.8;
const READABLE_MAX = 1.25;
const FIT_PADDING_PX = 24;
/** Distance of the zoom controls from the canvas edge (`.diagram-controls` in styles.css). */
const CONTROLS_INSET_PX = 12;
const ZOOM_STEP = 1.15;
const PAN_STEP_PX = 80;
/** Ctrl+wheel: one mouse notch (about this many pixels) zooms one ZOOM_STEP; larger deltas
 * are capped to it, the small ones of a trackpad pinch zoom smoothly. */
const WHEEL_NOTCH_PX = 100;
/** A press that moves less than this is a click, not a drag. */
const DRAG_THRESHOLD_PX = 4;
/** Panning keeps at least this much of the diagram in view. */
const KEEP_VISIBLE_PX = 60;
/** Space kept around a shape that keyboard focus brings into view. */
const REVEAL_MARGIN_PX = 32;
/** Shapes whose computed paint and font the export writes as attributes. */
const EXPORT_SHAPES = 'svg, rect, path, text';
const EXPORT_PROPERTIES = [
  'fill',
  'fill-opacity',
  'stroke',
  'stroke-opacity',
  'stroke-width',
  'stroke-dasharray',
  'opacity',
  'font-family',
  'font-size',
  'font-weight',
  'letter-spacing',
];
const XML_PROLOG = '<?xml version="1.0" encoding="UTF-8"?>\n';
/** Classes the view adds to shapes, left out of an export. */
const VIEW_CLASSES = ['focused', 'selected'];

/** What a tab lists, as layouts, sizes and steps are kept for: another revision or other items shown is another key. */
export function layoutKey(document, visible) {
  return JSON.stringify([document.id, document.revision, visible]);
}

/**
 * The diagram view of one tab: the SVG that `prism-core` laid out at one detail level,
 * panned and zoomed through its viewBox, themed by `styles.css`; the tab's side pane shows
 * the focused item. A camera `{detail, x, y, scale}` is the level, the world point at the
 * top left and pixels per world unit; each document has its own, `null` fits the whole
 * diagram at the most detailed level that fits. Layouts of the levels shown are kept until
 * the document or what the tab lists changes.
 *
 * `actions`: layout(document, visibleIds, detail | null) -> Diagram, onFocus(id), onToggle(id),
 * onClear(), onCamera(documentId, camera), onLayout(): a new layout is shown (its `counts()`
 * changed). `panel`: the tab's detail panel, which keeps the keyboard in the diagram as the
 * canvas does.
 */
export class DiagramView {
  constructor(actions, panel) {
    this.actions = actions;
    this.panel = panel;
    this.model = null;
    // The layout shown: its `Layout` from the shell, SVG, level, size and box outlines.
    this.diagram = null;
    this.svg = null;
    this.detail = null;
    this.bounds = null;
    this.boxes = new Map();
    // What is laid out, and the layouts of it per level, arrived or asked for.
    this.layoutKey = null;
    this.source = null;
    this.layouts = new Map();
    this.requests = new Set();
    // The size of the whole diagram per level, from the latest layout of the document.
    this.sizes = null;
    this.camera = null;
    // The camera last shown, while the camera's level is still being laid out.
    this.shown = null;
    // A level change waiting for its layout: {detail, anchor, scale}.
    this.switching = null;
    this.press = null;
    this.suppressClick = false;
    this.pointerFocus = false;
    this.gesture = null;
    this.shownFocus = null;
    this.shownSelection = null;

    this.message = h('div', { class: 'diagram-message', hidden: true });
    this.zoomLabel = h('span', { class: 'zoom', title: 'Detail and zoom' });
    this.controls = h(
      'div',
      { class: 'diagram-controls' },
      h('button', { class: 'button', title: 'Zoom out (-)', 'aria-label': 'Zoom out', text: '−', onclick: () => this.zoomStep(-1) }),
      this.zoomLabel,
      h('button', { class: 'button', title: 'Zoom in (+)', 'aria-label': 'Zoom in', text: '+', onclick: () => this.zoomStep(1) }),
      h('button', { class: 'button', title: 'Fit the whole diagram (0)', text: 'Fit', onclick: () => this.fit() }),
    );
    this.canvas = h('div', {
      class: 'diagram-canvas',
      tabindex: '0',
      role: 'group',
      'aria-label': 'Diagram',
      onpointerdown: (event) => this.onPointerDown(event),
      onpointermove: (event) => this.onPointerMove(event),
      onpointerup: (event) => this.onPointerUp(event),
      onpointercancel: () => this.onPointerCancel(),
      onclick: (event) => this.onClick(event),
      onfocusin: (event) => this.onFocusIn(event),
      // Focus moving to a shape outside the view must not scroll the frame; the camera follows.
      onscroll: () => {
        this.canvas.scrollTop = 0;
        this.canvas.scrollLeft = 0;
      },
    });
    this.canvas.addEventListener('wheel', (event) => this.onWheel(event), { passive: false });
    // Pinch on a WKWebView trackpad (macOS); Chromium sends ctrl+wheel instead.
    this.canvas.addEventListener('gesturestart', (event) => this.onGesture(event));
    this.canvas.addEventListener('gesturechange', (event) => this.onGesture(event));
    this.canvas.addEventListener('gestureend', () => {
      this.gesture = null;
    });
    this.element = h('div', { class: 'diagram-frame' }, this.canvas, this.message, this.controls);
    // A fitted diagram may fit at another level in the new size.
    this.resizes = new ResizeObserver(() => this.showLevel());
    this.resizes.observe(this.canvas);
  }

  /** The tab left this window: stop watching the canvas size. */
  dispose() {
    this.resizes.disconnect();
  }

  /**
   * `model`: {document, rows, focus: the focused item's ID or null, selected: Set of selected
   * item IDs, camera: the camera saved for this document or null, sizes: the sizes of what is
   * shown, when the list already asked for them, or null}.
   */
  render(model) {
    const previousFocus = this.model?.focus ?? null;
    this.model = model;
    const visible = model.rows.map((row) => row.id);
    const key = layoutKey(model.document, visible);
    if (key !== this.layoutKey) {
      // Sizes and a level change of another document do not apply to this one.
      if (this.source?.document.id !== model.document.id) {
        this.sizes = null;
        this.switching = null;
      }
      this.layoutKey = key;
      this.source = { document: model.document, visible };
      this.layouts = new Map();
      this.requests = new Set();
    }
    // Sizes the list asked for spare asking again.
    if (model.sizes) {
      this.sizes = model.sizes;
    }
    // A saved camera this view could not have made does not apply: the diagram starts fitted.
    this.camera = usableCamera(model.camera) ? model.camera : null;
    this.showLevel();
    this.showFocus(model.focus !== previousFocus);
  }

  /** The level a level change, the camera or the fit asks for; null while a fit lacks the sizes or the canvas size. */
  wantedLevel() {
    return this.switching?.detail ?? this.camera?.detail ?? this.fitLevel();
  }

  /** Shows the layout of the wanted level, once it arrived; until then the one shown stays. */
  showLevel() {
    if (!this.model) {
      return;
    }
    const level = this.wantedLevel();
    if (level === null) {
      // A fit picks its level from the sizes, which come without laying out any level.
      if (!this.sizes) {
        this.requestLayout(null);
      }
      return;
    }
    const layout = this.layouts.get(level);
    if (!layout) {
      this.requestLayout(level);
      this.applyCamera();
      return;
    }
    if (layout.diagram !== this.diagram) {
      this.install(layout);
      return;
    }
    this.applyCamera();
  }

  /**
   * Asks the shell for the sizes of what is shown now and the layout of a level, or with a
   * null `detail` for the sizes alone; one request per level at a time. A failed one can be
   * asked again; the level shown stays.
   */
  async requestLayout(detail) {
    const request = detail ?? SIZES_REQUEST;
    if (this.requests.has(request)) {
      return;
    }
    this.requests.add(request);
    const key = this.layoutKey;
    let response = null;
    let failure = null;
    try {
      response = await this.actions.layout(this.source.document, this.source.visible, detail);
    } catch (error) {
      failure = `The diagram could not be laid out: ${error}`;
    }
    if (key !== this.layoutKey) {
      return;
    }
    this.requests.delete(request);
    if (response?.layout) {
      const svg = parseSvg(response.layout.svg);
      if (svg) {
        this.layouts.set(detail, { diagram: response.layout, svg, boxes: boxOutlines(svg), bounds: response.sizes[detail] });
      } else {
        failure = 'The diagram could not be shown.';
      }
    }
    if (failure) {
      this.switching = null;
      this.showMessage(failure);
      return;
    }
    this.sizes = response.sizes;
    this.showLevel();
  }

  install(layout) {
    // Keyboard focus inside the old SVG moves to the same item in the new one.
    const refocus = this.svg?.contains(document.activeElement);
    this.svg?.remove();
    this.svg = layout.svg;
    this.diagram = layout.diagram;
    this.detail = layout.diagram.detail;
    this.bounds = layout.bounds;
    this.boxes = layout.boxes;
    this.canvas.prepend(this.svg);
    this.showMessage(this.diagram.order.length === 0 ? 'Nothing to draw: the items shown here have no boxes.' : null);
    this.shownFocus = null;
    this.shownSelection = null;
    const switching = this.switching;
    this.switching = null;
    if (switching) {
      this.setCamera(this.anchoredCamera(switching));
    } else {
      this.applyCamera();
    }
    this.showFocus(false);
    this.actions.onLayout();
    if (refocus) {
      (this.stop(this.model.focus) ?? this.canvas).focus({ preventScroll: true });
    }
  }

  /** After Esc: the keyboard stays in the diagram, on no item. */
  focusCanvas() {
    this.canvas.focus({ preventScroll: true });
  }

  /** After leaving the filter: back to the focused item, or the canvas. */
  restoreKeyboard() {
    (this.stop(this.model?.focus) ?? this.canvas).focus({ preventScroll: true });
  }

  /** Boxes, arrows and badges of the diagram shown; null before its layout arrived. */
  counts() {
    return this.svg ? ['box', 'pill', 'gate'].map((kind) => this.svg.querySelectorAll(`[data-kind="${kind}"]`).length) : null;
  }

  showMessage(text) {
    this.message.hidden = !text;
    this.message.textContent = text ?? '';
  }

  // Focus

  /** The keyboard stop of an item: its box, badge, lane or, for an arrow, its pill. */
  stop(id) {
    if (!this.svg || !id) {
      return null;
    }
    return [...this.svg.querySelectorAll('[data-id][tabindex="0"]')].find((node) => node.dataset.id === id) ?? null;
  }

  /**
   * Highlights the focused item. When it `moved` by keyboard, the item also takes the
   * keyboard (unless something outside the diagram and its detail panel has it) and comes
   * into view.
   */
  showFocus(moved) {
    if (!this.svg) {
      return;
    }
    const focus = this.model.focus;
    if (focus !== this.shownFocus) {
      for (const node of this.svg.querySelectorAll('.focused')) {
        node.classList.remove('focused');
      }
      for (const node of this.svg.querySelectorAll('[data-id]')) {
        if (node.dataset.id === focus) {
          node.classList.add('focused');
        }
      }
      this.shownFocus = focus;
    }
    const selection = [...this.model.selected].join();
    if (selection !== this.shownSelection) {
      for (const node of this.svg.querySelectorAll('[data-id]')) {
        const selected = this.model.selected.has(node.dataset.id);
        node.classList.toggle('selected', selected);
        // Keyboard stops say it too; the label from prism-core is kept to restore it.
        if (node.hasAttribute('aria-label')) {
          node.dataset.label ??= node.getAttribute('aria-label');
          node.setAttribute('aria-label', selected ? `Selected: ${node.dataset.label}` : node.dataset.label);
        }
      }
      this.shownSelection = selection;
    }
    const stop = this.stop(focus);
    if (!moved || !stop || this.pointerFocus) {
      return;
    }
    const active = document.activeElement;
    if (active === document.body || active === null || this.element.contains(active) || this.panel.contains(active)) {
      stop.focus({ preventScroll: true });
    }
    this.reveal(stop);
  }

  // Camera

  size() {
    return { width: this.canvas.clientWidth, height: this.canvas.clientHeight };
  }

  /** Where a fitted diagram goes: the canvas less padding, above the zoom controls at the bottom. */
  fitArea() {
    const { width, height } = this.size();
    const bottom = Math.max(FIT_PADDING_PX, this.controls.offsetHeight + 2 * CONTROLS_INSET_PX);
    return { top: FIT_PADDING_PX, width: width - 2 * FIT_PADDING_PX, height: height - FIT_PADDING_PX - bottom };
  }

  /** The scale at which a diagram of `size` fills the fit area, at most DESIGN_SCALE. */
  fitScale(size) {
    if (!size || size.width <= 0 || size.height <= 0) {
      return DESIGN_SCALE;
    }
    const area = this.fitArea();
    return Math.min(area.width / size.width, area.height / size.height, DESIGN_SCALE);
  }

  /** The most detailed level whose whole diagram fits readably, else the overview; null before sizes and a canvas size are known. */
  fitLevel() {
    const { width, height } = this.size();
    if (!this.sizes || width === 0 || height === 0) {
      return null;
    }
    return [...LEVELS].reverse().find((level) => this.fitScale(this.sizes[level]) >= READABLE_MIN) ?? LEVELS[0];
  }

  /**
   * The camera that shows the whole diagram shown, centered above the zoom controls. One too
   * large for READABLE_MIN is shown at it, around the focused item or from its top left.
   */
  fitCamera() {
    const bounds = this.bounds ?? { width: 0, height: 0 };
    if (bounds.width <= 0 || bounds.height <= 0) {
      return { x: 0, y: 0, scale: DESIGN_SCALE };
    }
    const area = this.fitArea();
    const scale = clamp(this.fitScale(bounds), READABLE_MIN, DESIGN_SCALE);
    const centered = (box) => ({
      x: box.x + box.width / 2 - this.size().width / 2 / scale,
      y: box.y + box.height / 2 - (area.top + area.height / 2) / scale,
      scale,
    });
    if (this.fitScale(bounds) >= READABLE_MIN) {
      return centered({ x: 0, y: 0, ...bounds });
    }
    const focused = this.stop(this.model?.focus);
    if (focused) {
      return centered(focused.getBBox());
    }
    return { x: -FIT_PADDING_PX / scale, y: -area.top / scale, scale };
  }

  /** The camera of the layout shown: while another level is laid out, the one last shown. */
  current() {
    if (!this.camera) {
      return this.fitCamera();
    }
    return this.camera.detail === this.detail ? this.camera : (this.shown ?? this.fitCamera());
  }

  applyCamera() {
    const { width, height } = this.size();
    if (!this.svg || !this.model || width === 0 || height === 0) {
      return;
    }
    const camera = this.current();
    this.shown = camera;
    this.svg.setAttribute('viewBox', `${camera.x} ${camera.y} ${width / camera.scale} ${height / camera.scale}`);
    this.zoomLabel.textContent = `${LEVEL_LABELS[this.detail]} ${Math.round(camera.scale * 100)}%`;
  }

  /**
   * Moves the camera on the layout shown, keeps part of the diagram in view, shows and reports
   * it. Without a canvas size (the tab is not shown) there is nothing to keep in view: no move.
   */
  setCamera(camera) {
    const { width, height } = this.size();
    if (width === 0 || height === 0) {
      return;
    }
    const bounds = this.bounds ?? { width: 0, height: 0 };
    const keep = KEEP_VISIBLE_PX / camera.scale;
    this.camera = {
      detail: this.detail,
      x: clamp(camera.x, keep - width / camera.scale, bounds.width - keep),
      y: clamp(camera.y, keep - height / camera.scale, bounds.height - keep),
      scale: camera.scale,
    };
    this.applyCamera();
    this.actions.onCamera(this.model.document.id, this.camera);
  }

  /** Zooms by `factor` around a point of the canvas (client coordinates), by default its center. */
  zoomBy(factor, clientX = null, clientY = null) {
    this.zoomTo(this.current().scale * factor, clientX, clientY);
  }

  /**
   * Zooms to `target` pixels per world unit around a point of the canvas. Past the end of the
   * level's band it switches the level instead; until that layout arrives, zooming waits.
   */
  zoomTo(target, clientX = null, clientY = null) {
    if (!this.svg || this.switching) {
      return;
    }
    const rect = this.canvas.getBoundingClientRect();
    const px = clientX === null ? rect.width / 2 : clientX - rect.left;
    const py = clientY === null ? rect.height / 2 : clientY - rect.top;
    const index = LEVELS.indexOf(this.detail);
    if (target > READABLE_MAX && index < LEVELS.length - 1) {
      this.switchLevel(LEVELS[index + 1], px, py);
      return;
    }
    if (target < READABLE_MIN && index > 0) {
      this.switchLevel(LEVELS[index - 1], px, py);
      return;
    }
    const camera = this.current();
    const scale = clamp(target, READABLE_MIN, READABLE_MAX);
    // The world point under the anchor stays under it.
    const worldX = camera.x + px / camera.scale;
    const worldY = camera.y + py / camera.scale;
    this.setCamera({ x: worldX - px / scale, y: worldY - py / scale, scale });
  }

  /** Pans by screen pixels. */
  panBy(dx, dy) {
    const camera = this.current();
    this.setCamera({ x: camera.x + dx / camera.scale, y: camera.y + dy / camera.scale, scale: camera.scale });
  }

  /** Shows another level at DESIGN_SCALE, with the box at the canvas point (px, py) kept there. */
  switchLevel(detail, px, py) {
    this.switching = { detail, anchor: this.anchorAt(px, py), scale: DESIGN_SCALE };
    this.showLevel();
  }

  /**
   * What a level change keeps in place: the box at the canvas point (px, py), or the nearest
   * one, by ID, as positions change between layouts. The point is kept as shares of the box's
   * size, plus how far outside it the point is, in screen pixels; and as the world point, for
   * when the box is not in the new layout.
   */
  anchorAt(px, py) {
    const camera = this.current();
    const world = [camera.x + px / camera.scale, camera.y + py / camera.scale];
    let anchor = { id: null };
    let nearest = Infinity;
    for (const [id, box] of this.boxes) {
      const inside = [clamp(world[0], box.x, box.x + box.width), clamp(world[1], box.y, box.y + box.height)];
      const distance = Math.hypot(world[0] - inside[0], world[1] - inside[1]);
      if (distance < nearest) {
        nearest = distance;
        anchor = {
          id,
          share: [share(inside[0], box.x, box.width), share(inside[1], box.y, box.height)],
          outside: [(world[0] - inside[0]) * camera.scale, (world[1] - inside[1]) * camera.scale],
        };
      }
    }
    return { ...anchor, world, px, py };
  }

  /**
   * The camera of a level change on its new layout: the anchored box at the same canvas
   * point; when the box is gone (hidden or filtered out meanwhile), the same world point.
   */
  anchoredCamera({ anchor, scale }) {
    const box = this.boxes.get(anchor.id);
    const [x, y] = box
      ? [box.x + anchor.share[0] * box.width + anchor.outside[0] / scale, box.y + anchor.share[1] * box.height + anchor.outside[1] / scale]
      : anchor.world;
    return { x: x - anchor.px / scale, y: y - anchor.py / scale, scale };
  }

  /** One zoom step in (1) or out (-1), around the focused box if one is shown, else the center. */
  zoomStep(direction) {
    const box = this.boxes.get(this.model?.focus);
    if (!box) {
      this.zoomBy(ZOOM_STEP ** direction);
      return;
    }
    const camera = this.current();
    const rect = this.canvas.getBoundingClientRect();
    const clientX = rect.left + (box.x + box.width / 2 - camera.x) * camera.scale;
    const clientY = rect.top + (box.y + box.height / 2 - camera.y) * camera.scale;
    this.zoomBy(ZOOM_STEP ** direction, clientX, clientY);
  }

  /** One pan step per axis: -1, 0 or 1. */
  panStep(dx, dy) {
    this.panBy(dx * PAN_STEP_PX, dy * PAN_STEP_PX);
  }

  /** Fits the whole diagram, at the most detailed level that fits readably. */
  fit() {
    this.camera = null;
    this.switching = null;
    this.actions.onCamera(this.model.document.id, null);
    this.showLevel();
  }

  /** Pans the least amount that brings the shape into view, with a margin. */
  reveal(node) {
    const { width, height } = this.size();
    if (width === 0 || height === 0) {
      return;
    }
    const camera = this.current();
    const box = node.getBBox();
    const margin = REVEAL_MARGIN_PX / camera.scale;
    const viewWidth = width / camera.scale;
    const viewHeight = height / camera.scale;
    let { x, y } = camera;
    if (box.x - margin < x) {
      x = box.x - margin;
    } else if (box.x + box.width + margin > x + viewWidth) {
      x = Math.min(box.x - margin, box.x + box.width + margin - viewWidth);
    }
    if (box.y - margin < y) {
      y = box.y - margin;
    } else if (box.y + box.height + margin > y + viewHeight) {
      y = Math.min(box.y - margin, box.y + box.height + margin - viewHeight);
    }
    if (x !== camera.x || y !== camera.y) {
      this.setCamera({ x, y, scale: camera.scale });
    }
  }

  // Pointer, wheel and gestures

  /**
   * While any button is down, keyboard focus that the press moves onto a shape does not
   * select it: only a click does, and a press that becomes a drag never clicks.
   */
  onPointerDown(event) {
    this.pointerFocus = true;
    this.suppressClick = false;
    if (event.button !== 0) {
      return;
    }
    const camera = this.current();
    this.press = { id: event.pointerId, x: event.clientX, y: event.clientY, camera, dragging: false };
  }

  onPointerMove(event) {
    const press = this.press;
    if (!press || press.id !== event.pointerId) {
      return;
    }
    const dx = event.clientX - press.x;
    const dy = event.clientY - press.y;
    if (!press.dragging && Math.hypot(dx, dy) < DRAG_THRESHOLD_PX) {
      return;
    }
    if (!press.dragging) {
      press.dragging = true;
      this.canvas.classList.add('dragging');
      try {
        this.canvas.setPointerCapture(event.pointerId);
      } catch {
        // The pointer is already gone; the drag ends with its pointerup anyway.
      }
    }
    this.setCamera({ x: press.camera.x - dx / press.camera.scale, y: press.camera.y - dy / press.camera.scale, scale: press.camera.scale });
  }

  onPointerUp(event) {
    this.pointerFocus = false;
    if (!this.press || this.press.id !== event.pointerId) {
      return;
    }
    this.suppressClick = this.press.dragging;
    this.endPress();
  }

  /** The system took the pointer (a gesture, a window switch): no click follows. */
  onPointerCancel() {
    this.pointerFocus = false;
    this.suppressClick = false;
    this.endPress();
  }

  endPress() {
    this.press = null;
    this.canvas.classList.remove('dragging');
  }

  /** A click on a shape focuses its item without panning; one on the background clears the focus. */
  onClick(event) {
    if (this.suppressClick) {
      this.suppressClick = false;
      return;
    }
    const shape = event.target.closest?.('[data-id]');
    this.pointerFocus = true;
    if (shape && (event.metaKey || event.ctrlKey)) {
      this.actions.onToggle(shape.dataset.id);
    } else if (shape) {
      // An arrow's line is not a keyboard stop; its pill is.
      this.stop(shape.dataset.id)?.focus({ preventScroll: true });
      this.actions.onFocus(shape.dataset.id);
    } else {
      this.actions.onClear();
    }
    this.pointerFocus = false;
  }

  /** Tab onto a shape focuses its item and brings it into view; a press or click does neither here. */
  onFocusIn(event) {
    const shape = event.target.closest?.('[data-id]');
    if (!shape || this.pointerFocus) {
      return;
    }
    this.reveal(shape);
    this.actions.onFocus(shape.dataset.id);
  }

  onWheel(event) {
    event.preventDefault();
    if (event.ctrlKey || event.metaKey) {
      // A trackpad pinch in Chromium and WebView2, or Ctrl (⌘) with a mouse wheel.
      if (this.gesture === null) {
        const delta = clamp(event.deltaY, -WHEEL_NOTCH_PX, WHEEL_NOTCH_PX);
        this.zoomBy(ZOOM_STEP ** (-delta / WHEEL_NOTCH_PX), event.clientX, event.clientY);
      }
      return;
    }
    const sideways = event.shiftKey && event.deltaX === 0;
    this.panBy(sideways ? event.deltaY : event.deltaX, sideways ? 0 : event.deltaY);
  }

  /** The zoom is the gesture's scale times `base`; after a level change, the gesture goes on from the new scale. */
  onGesture(event) {
    event.preventDefault();
    if (event.type === 'gesturestart' || this.gesture === null || this.gesture.detail !== this.detail) {
      this.gesture = { base: this.current().scale / (event.scale || 1), detail: this.detail };
    }
    if (event.type === 'gesturechange') {
      this.zoomTo(this.gesture.base * event.scale, event.clientX, event.clientY);
    }
  }

  // Export

  /**
   * The whole diagram as a standalone SVG file: what the theme paints is written into
   * presentation attributes, so the file looks the same outside the app.
   */
  exportSvg() {
    if (!this.svg || !this.diagram) {
      return null;
    }
    const marked = VIEW_CLASSES.map((name) => [name, [...this.svg.querySelectorAll(`.${name}`)]]);
    marked.forEach(([name, nodes]) => nodes.forEach((node) => node.classList.remove(name)));
    const copy = this.svg.cloneNode(true);
    const originals = [this.svg, ...this.svg.querySelectorAll(EXPORT_SHAPES)];
    const copies = [copy, ...copy.querySelectorAll(EXPORT_SHAPES)];
    originals.forEach((node, index) => {
      const style = getComputedStyle(node);
      for (const property of EXPORT_PROPERTIES) {
        copies[index].setAttribute(property, style.getPropertyValue(property));
      }
    });
    marked.forEach(([name, nodes]) => nodes.forEach((node) => node.classList.add(name)));
    copy.removeAttribute('class');
    copy.setAttribute('width', String(this.bounds.width));
    copy.setAttribute('height', String(this.bounds.height));
    copy.setAttribute('viewBox', `0 0 ${this.bounds.width} ${this.bounds.height}`);
    return XML_PROLOG + new XMLSerializer().serializeToString(copy);
  }
}

/** A camera of a known level within the readable band. */
function usableCamera(camera) {
  return (
    LEVELS.includes(camera?.detail) &&
    [camera.x, camera.y, camera.scale].every(Number.isFinite) &&
    camera.scale >= READABLE_MIN &&
    camera.scale <= READABLE_MAX
  );
}

/** The SVG of a layout, ready to show, or null when it is not one. */
function parseSvg(text) {
  const parsed = new DOMParser().parseFromString(text, 'image/svg+xml').documentElement;
  if (parsed.nodeName !== 'svg') {
    return null;
  }
  const svg = document.importNode(parsed, true);
  svg.removeAttribute('width');
  svg.removeAttribute('height');
  svg.setAttribute('class', 'diagram-svg');
  return svg;
}

/** Where prism-core put each box, by item ID: the outline of its shape. */
function boxOutlines(svg) {
  const outlines = new Map();
  for (const box of svg.querySelectorAll('[data-kind="box"]')) {
    const shape = box.querySelector('.prism-shape');
    const [x, y, width, height] = ['x', 'y', 'width', 'height'].map((name) => Number(shape.getAttribute(name)));
    outlines.set(box.dataset.id, { x, y, width, height });
  }
  return outlines;
}

/** Where `value` lies from `start` over `length`, from 0 to 1. */
function share(value, start, length) {
  return length > 0 ? (value - start) / length : 0;
}

function clamp(value, min, max) {
  return Math.min(Math.max(value, min), Math.max(min, max));
}
