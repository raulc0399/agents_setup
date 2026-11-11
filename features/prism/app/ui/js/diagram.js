import { h } from './dom.js';

/**
 * Detail levels of the layout, least first (`Detail` in prism-core's graph.rs): titles only,
 * one line of text, the whole text. Only the user changes the level (the level control, keys
 * 1 to 3); zooming never does.
 */
export const LEVELS = ['names', 'subtitles', 'full'];
const LEVEL_LABELS = { names: 'Titles', subtitles: 'Summary', full: 'Full text' };
const LEVEL_TITLES = { names: 'Titles only (1)', subtitles: 'One line of text (2)', full: 'The whole text (3)' };
/** The level of a document the user did not pick one for: one line of text, as the list's cards. */
const DEFAULT_LEVEL = 'subtitles';
/**
 * Pixels per world unit. At DESIGN_SCALE text has the size of the list's cards. Zoom is free
 * between ZOOM_MIN and ZOOM_MAX; a fit is never larger than DESIGN_SCALE and may be smaller
 * than ZOOM_MIN, so that it shows the whole diagram.
 */
const DESIGN_SCALE = 1;
const ZOOM_MIN = 0.1;
const ZOOM_MAX = 4;
/** Buttons, keys, fit and double-click zoom over this long, easing out; pinch and wheel at once. */
const ZOOM_ANIMATION_MS = 200;
/**
 * Zoomed out, small text fades instead of shrinking (`data-zoom` on the SVG, styles.css):
 * below the first limit the text lines, below the second also the kind and number line, so
 * the titles remain. A band changes only past its limit by ZOOM_BAND_HYSTERESIS (a share of
 * it), so a zoom around a limit does not flicker.
 */
const ZOOM_BANDS = ['far', 'mid', 'near'];
const ZOOM_BAND_LIMITS = [0.45, 0.55];
const ZOOM_BAND_HYSTERESIS = 0.05;
/**
 * Zoomed out, pills and badges keep at least this share of their size on screen, growing in
 * the world at most LABEL_MAX_GROWTH times, and only as far as they stay LABEL_GAP apart and
 * badges inside their lane (`labelLimits`): further out they shrink too, so they do not cover
 * each other. Their scale changes in steps of LABEL_SCALE_STEP, which spares most writes.
 */
const LABEL_MIN_SCALE = 0.8;
const LABEL_MAX_GROWTH = 2;
const LABEL_GAP = 2;
const LABEL_SCALE_STEP = 0.02;
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
/** Pills and badges, which keep a readable size when zoomed out. */
const LABEL_SHAPES = '[data-kind="pill"], [data-kind="gate"]';
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
/** Attributes that reference an arrowhead marker, as `url(#id)`. */
const HEAD_REFERENCES = ['marker-start', 'marker-end'];
const HEAD_REFERENCE = /^url\(#(.+)\)$/;
/** The focused arrowhead (prism-core's `HEAD_FOCUS`), and the property styles.css draws it from. */
const FOCUS_HEAD = 'marker.prism-head-focus';
const FOCUS_HEAD_PROPERTY = '--head-focus';
/** Diagram views made so far: each prefixes its marker IDs with its number. */
let viewCount = 0;

/** What a tab lists, as layouts, sizes and steps are kept for: another revision or other items shown is another key. */
export function layoutKey(document, visible) {
  return JSON.stringify([document.id, document.revision, visible]);
}

/**
 * The diagram view of one tab: the SVG that `prism-core` laid out at one detail level,
 * panned and zoomed through its viewBox, themed by `styles.css`; the tab's side pane shows
 * the focused item. A camera `{x, y, scale}` is the world point at the top left and pixels
 * per world unit; `null` fits the whole diagram. Each document saves its level with its
 * camera, `{detail, x, y, scale}`, or `{detail}` when fitted. Layouts of the levels shown are
 * kept until the document or what the tab lists changes.
 *
 * `actions`: layout(document, visibleIds, detail) -> Diagram, onFocus(id), onToggle(id),
 * onClear(), onCamera(documentId, camera), onLayout(): a new layout is shown (its `counts()`
 * changed). `panel`: the tab's detail panel, which keeps the keyboard in the diagram as the
 * canvas does.
 */
export class DiagramView {
  constructor(actions, panel) {
    this.actions = actions;
    this.panel = panel;
    this.model = null;
    // The layout shown: its `Layout` from the shell, SVG, level, size, box outlines, pills and
    // badges with their centers, and arrowheads with their size.
    this.diagram = null;
    this.svg = null;
    this.detail = null;
    this.bounds = null;
    this.boxes = new Map();
    this.labels = [];
    this.heads = [];
    // Marker IDs are unique in the page, so two diagrams shown side by side keep their own.
    viewCount += 1;
    this.headPrefix = `prism-view${viewCount}-`;
    // What is laid out, and the layouts of it per level, arrived or asked for.
    this.layoutKey = null;
    this.source = null;
    this.layouts = new Map();
    this.requests = new Set();
    // The level the user picked for the document; `anchoring` while its layout replaces another.
    this.level = DEFAULT_LEVEL;
    this.anchoring = false;
    // The camera on the layout shown; null fits it.
    this.camera = null;
    // The canvas size last seen while shown, so a resize keeps the view's center in place.
    this.shownSize = null;
    // A zoom on its way: {target, frame}.
    this.animation = null;
    // What the zoom shows now: its scale, the text band, the pills' and badges' scale.
    this.zoomShown = { scale: null, band: null, labels: null, heads: null };
    this.press = null;
    this.suppressClick = false;
    this.pointerFocus = false;
    this.gesture = null;
    this.shownFocus = null;
    this.shownSelection = null;

    this.message = h('div', { class: 'diagram-message', hidden: true });
    this.zoomLabel = h('span', { class: 'zoom', title: 'Zoom' });
    this.levelButtons = LEVELS.map((level) =>
      h('button', { 'data-level': level, title: LEVEL_TITLES[level], text: LEVEL_LABELS[level], onclick: () => this.setLevel(level) }),
    );
    this.controls = h(
      'div',
      { class: 'diagram-controls' },
      h('div', { class: 'segmented', role: 'group', 'aria-label': 'Detail' }, this.levelButtons),
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
      ondblclick: (event) => this.onDoubleClick(event),
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
    this.resizes = new ResizeObserver(() => this.onResize());
    this.resizes.observe(this.canvas);
  }

  /** The tab left this window: stop watching the canvas size and any zoom on its way. */
  dispose() {
    this.resizes.disconnect();
    if (this.animation) {
      cancelAnimationFrame(this.animation.frame);
      this.animation = null;
    }
  }

  /**
   * `model`: {document, rows, focus: the focused item's ID or null, selected: Set of selected
   * item IDs, camera: the camera saved for this document or null}. The saved camera is read
   * when the document is shown; from then on this view keeps it and reports its changes.
   */
  render(model) {
    const previousFocus = this.model?.focus ?? null;
    if (model.document.id !== this.model?.document.id) {
      // A zoom on its way belongs to the document shown before.
      this.stopAnimation();
      this.level = savedLevel(model.camera);
      this.camera = savedPosition(model.camera);
      this.anchoring = false;
    }
    this.model = model;
    const visible = model.rows.map((row) => row.id);
    const key = layoutKey(model.document, visible);
    if (key !== this.layoutKey) {
      this.layoutKey = key;
      this.source = { document: model.document, visible };
      this.layouts = new Map();
      this.requests = new Set();
    }
    this.showLevels();
    this.showLevel();
    this.showFocus(model.focus !== previousFocus);
  }

  /** Shows the layout of the level picked, once it arrived; until then the one shown stays. */
  showLevel() {
    if (!this.model) {
      return;
    }
    const layout = this.layouts.get(this.level);
    if (!layout) {
      this.requestLayout(this.level);
      this.applyCamera();
      return;
    }
    if (layout.diagram !== this.diagram) {
      this.install(layout);
      return;
    }
    // Picked again before its layout replaced it: the level shown stays as it is.
    this.anchoring = false;
    this.applyCamera();
  }

  /** The level control shows the level picked. */
  showLevels() {
    for (const button of this.levelButtons) {
      button.setAttribute('aria-pressed', String(button.dataset.level === this.level));
    }
  }

  /**
   * Shows another level: it is laid out once, and the box at the focused box or the canvas
   * center stays in place at the same zoom (see `install`).
   */
  setLevel(level) {
    if (!this.model || level === this.level) {
      return;
    }
    this.stopAnimation();
    this.level = level;
    this.anchoring = this.svg !== null;
    this.showLevels();
    this.showLevel();
  }

  /**
   * Asks the shell for the layout of a level of what is shown now, which comes with the sizes
   * of every level; one request per level at a time. A failed one can be asked again; the
   * level shown stays.
   */
  async requestLayout(detail) {
    if (this.requests.has(detail)) {
      return;
    }
    this.requests.add(detail);
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
    this.requests.delete(detail);
    if (response?.layout) {
      const svg = parseSvg(response.layout.svg);
      if (svg) {
        renameHeads(svg, (id) => this.headPrefix + id);
        // Arrowhead sizes are read before any zoom changed them.
        const heads = [...svg.querySelectorAll('marker')].map((marker) => ({ marker, size: Number(marker.getAttribute('markerWidth')) }));
        this.layouts.set(detail, { diagram: response.layout, svg, boxes: boxOutlines(svg), labels: labelLimits(svg), heads, bounds: response.sizes[detail] });
      } else {
        failure = 'The diagram could not be shown.';
      }
    }
    if (failure) {
      // A level no longer picked fails quietly.
      if (detail !== this.level) {
        return;
      }
      // A level picked while another is shown goes back to that one.
      if (this.detail !== null && detail !== this.detail) {
        this.level = this.detail;
        this.anchoring = false;
        this.showLevels();
      }
      this.showMessage(failure);
      return;
    }
    this.showLevel();
  }

  install(layout) {
    // Keyboard focus inside the old SVG moves to the same item in the new one.
    const refocus = this.svg?.contains(document.activeElement);
    // A level the user picked keeps the box at the focused box or the canvas center in place.
    const anchor = this.anchoring ? { anchor: this.anchorAt(...this.zoomAnchor()), scale: this.current().scale } : null;
    this.anchoring = false;
    // Otherwise a zoom on its way ends where it was heading, on the new layout.
    const finish = anchor ? null : (this.animation?.done ?? null);
    if (finish) {
      cancelAnimationFrame(this.animation.frame);
      this.animation = null;
    }
    this.stopAnimation();
    this.svg?.remove();
    this.svg = layout.svg;
    this.diagram = layout.diagram;
    this.detail = layout.diagram.detail;
    this.bounds = layout.bounds;
    this.boxes = layout.boxes;
    this.labels = layout.labels;
    this.heads = layout.heads;
    this.zoomShown = { scale: null, band: null, labels: null, heads: null };
    // Set before the SVG is shown, so its texts do not fade in.
    this.showZoom(anchor ? anchor.scale : this.current().scale);
    this.canvas.style.setProperty(FOCUS_HEAD_PROPERTY, `url(#${this.svg.querySelector(FOCUS_HEAD).id})`);
    this.canvas.prepend(this.svg);
    this.showMessage(this.diagram.order.length === 0 ? 'Nothing to draw: the items shown here have no boxes.' : null);
    this.shownFocus = null;
    this.shownSelection = null;
    if (anchor) {
      this.setCamera(this.anchoredCamera(anchor));
    } else if (finish) {
      finish();
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

  /** The scale at which a shape of `size` (by default the whole diagram) fills the fit area, at most DESIGN_SCALE. */
  fitScale(size = this.bounds) {
    const area = this.fitArea();
    if (!size || size.width <= 0 || size.height <= 0 || area.width <= 0 || area.height <= 0) {
      return DESIGN_SCALE;
    }
    return Math.min(area.width / size.width, area.height / size.height, DESIGN_SCALE);
  }

  /** The camera at `scale` with the world rectangle `box` in the middle of the fit area. */
  centered(box, scale) {
    const area = this.fitArea();
    return {
      x: box.x + box.width / 2 - this.size().width / 2 / scale,
      y: box.y + box.height / 2 - (area.top + area.height / 2) / scale,
      scale,
    };
  }

  /** The camera that shows the whole diagram shown, centered above the zoom controls. */
  fitCamera() {
    const bounds = this.bounds ?? { width: 0, height: 0 };
    if (bounds.width <= 0 || bounds.height <= 0) {
      return { x: 0, y: 0, scale: DESIGN_SCALE };
    }
    return this.centered({ x: 0, y: 0, ...bounds }, this.fitScale());
  }

  current() {
    return this.camera ?? this.fitCamera();
  }

  /** The zoom range: down to ZOOM_MIN, or to the fit when the diagram fits only below it. */
  zoomLimits() {
    return [Math.min(ZOOM_MIN, this.fitScale()), ZOOM_MAX];
  }

  /**
   * The canvas changed size (the window, the side pane): a fitted diagram fits the new size; a
   * camera keeps the world point at the canvas center in place, at the same zoom. A hidden
   * canvas (another tab shown) has no size and changes nothing.
   */
  onResize() {
    const size = this.size();
    if (size.width === 0 || size.height === 0) {
      return;
    }
    const before = this.shownSize;
    this.shownSize = size;
    if (this.camera && this.svg && before && (before.width !== size.width || before.height !== size.height)) {
      const { x, y, scale } = this.camera;
      this.camera = { x: x + (before.width - size.width) / 2 / scale, y: y + (before.height - size.height) / 2 / scale, scale };
      this.report();
    }
    this.applyCamera();
  }

  applyCamera() {
    const { width, height } = this.size();
    if (!this.svg || !this.model || width === 0 || height === 0) {
      return;
    }
    const camera = this.current();
    this.svg.setAttribute('viewBox', `${camera.x} ${camera.y} ${width / camera.scale} ${height / camera.scale}`);
    this.zoomLabel.textContent = `${Math.round(camera.scale * 100)}%`;
    this.showZoom(camera.scale);
  }

  /**
   * What the zoom changes besides the viewBox, set per element only when it changes: the text
   * band, the size of pills and badges below LABEL_MIN_SCALE (scaled around their center), and
   * arrowheads, which scale with the diagram up to 100% and keep that size on screen beyond.
   */
  showZoom(scale) {
    const shown = this.zoomShown;
    if (scale === shown.scale) {
      return;
    }
    shown.scale = scale;
    const band = zoomBand(scale, shown.band);
    if (band !== shown.band) {
      this.svg.dataset.zoom = band;
      shown.band = band;
    }
    const labels = Math.max(1, Math.ceil(LABEL_MIN_SCALE / scale / LABEL_SCALE_STEP) * LABEL_SCALE_STEP);
    if (labels !== shown.labels) {
      for (const label of this.labels) {
        const growth = Math.min(labels, label.growth);
        if (growth === label.shown) {
          continue;
        }
        if (growth === 1) {
          label.node.removeAttribute('transform');
        } else {
          label.node.setAttribute('transform', `matrix(${growth} 0 0 ${growth} ${label.x * (1 - growth)} ${label.y * (1 - growth)})`);
        }
        label.shown = growth;
      }
      shown.labels = labels;
    }
    const heads = Math.min(1, 1 / scale);
    if (heads !== shown.heads) {
      for (const { marker, size } of this.heads) {
        marker.setAttribute('markerWidth', String(size * heads));
        marker.setAttribute('markerHeight', String(size * heads));
      }
      shown.heads = heads;
    }
  }

  /**
   * Shows and reports a camera on the layout shown. Before a layout arrived, or without a
   * canvas size (the tab is not shown), there is nothing to show: no move.
   */
  moveCamera(camera) {
    const { width, height } = this.size();
    if (width === 0 || height === 0) {
      return;
    }
    this.placeCamera(camera);
  }

  /** Shows (once the canvas has a size) and reports a camera on the layout shown. */
  placeCamera(camera) {
    if (!this.svg) {
      return;
    }
    this.camera = camera;
    this.applyCamera();
    this.report();
  }

  /** Saves the level with the camera, or with the fit. */
  report() {
    const camera = this.camera ? { detail: this.detail, ...this.camera } : { detail: this.level };
    this.actions.onCamera(this.model.document.id, camera);
  }

  /** Moves the camera with part of the diagram kept in view; a zoom on its way stops. */
  setCamera(camera) {
    this.stopAnimation();
    this.moveCamera(this.limited(camera));
  }

  /**
   * The camera moved as little as keeps KEEP_VISIBLE_PX of the diagram in view, or all of it
   * when it is smaller on screen. A pan `from` a camera already beyond that may stay as far
   * out, never further, so the view never jumps.
   */
  limited(camera, from = null) {
    const { width, height } = this.size();
    const bounds = this.bounds ?? { width: 0, height: 0 };
    const axis = (value, view, extent, start) => {
      const keep = Math.min(KEEP_VISIBLE_PX / camera.scale, extent);
      const low = keep - view / camera.scale;
      const high = extent - keep;
      return start === null ? clamp(value, low, high) : clamp(value, Math.min(low, start), Math.max(high, start));
    };
    return {
      x: axis(camera.x, width, bounds.width, from?.x ?? null),
      y: axis(camera.y, height, bounds.height, from?.y ?? null),
      scale: camera.scale,
    };
  }

  /** The canvas point of a client point; without one, the canvas center. */
  canvasPoint(clientX = null, clientY = null) {
    const rect = this.canvas.getBoundingClientRect();
    return clientX === null ? [rect.width / 2, rect.height / 2] : [clientX - rect.left, clientY - rect.top];
  }

  /**
   * `camera` zoomed to `scale` (within the zoom range) around the canvas point (px, py): the
   * world point there stays there. Zooming out, or in on the diagram, keeps the diagram in
   * view by itself; zooming in on the space beside it is limited as a pan is.
   */
  zoomedCamera(camera, scale, px, py) {
    const [min, max] = this.zoomLimits();
    // A camera below the range (a fit that grew) zooms on from where it is.
    const next = clamp(scale, Math.min(min, camera.scale), max);
    const world = [camera.x + px / camera.scale, camera.y + py / camera.scale];
    const zoomed = { x: world[0] - px / next, y: world[1] - py / next, scale: next };
    const bounds = this.bounds ?? { width: 0, height: 0 };
    const onDiagram = world[0] >= 0 && world[0] <= bounds.width && world[1] >= 0 && world[1] <= bounds.height;
    return next <= camera.scale || onDiagram ? zoomed : this.limited(zoomed);
  }

  /** Pinch and wheel: zooms to `scale` at once, around a client point. */
  zoomAt(scale, clientX, clientY) {
    if (!this.svg) {
      return;
    }
    this.stopAnimation();
    this.moveCamera(this.zoomedCamera(this.current(), scale, ...this.canvasPoint(clientX, clientY)));
  }

  /** The canvas point a zoom by button or key keeps in place: the focused box's middle if one is shown, else the canvas center. */
  zoomAnchor() {
    const box = this.boxes.get(this.model?.focus);
    if (!box) {
      return this.canvasPoint();
    }
    const camera = this.current();
    return [(box.x + box.width / 2 - camera.x) * camera.scale, (box.y + box.height / 2 - camera.y) * camera.scale];
  }

  /** One zoom step in (1) or out (-1), animated; steps in a row add up. */
  zoomStep(direction) {
    this.zoomAnimated((this.animation?.target ?? this.current()).scale * ZOOM_STEP ** direction);
  }

  /** Zooms to 100%, animated. */
  zoomActual() {
    this.zoomAnimated(DESIGN_SCALE);
  }

  zoomAnimated(scale) {
    if (!this.svg) {
      return;
    }
    this.animateTo(this.zoomedCamera(this.current(), scale, ...this.zoomAnchor()));
  }

  /** Pans by screen pixels. */
  panBy(dx, dy) {
    this.stopAnimation();
    const camera = this.current();
    this.moveCamera(this.limited({ x: camera.x + dx / camera.scale, y: camera.y + dy / camera.scale, scale: camera.scale }, camera));
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

  /** One pan step per axis: -1, 0 or 1. */
  panStep(dx, dy) {
    this.panBy(dx * PAN_STEP_PX, dy * PAN_STEP_PX);
  }

  /** Fits the whole diagram, animated; it stays fitted when the canvas changes size. */
  fit() {
    if (!this.svg) {
      return;
    }
    this.animateTo(this.fitCamera(), () => this.placeCamera(null));
  }

  /** Zooms to a box: centered at 100%, or whole when larger; zoomed in further, at that zoom. */
  zoomToBox(id) {
    const box = this.boxes.get(id);
    if (!box) {
      return;
    }
    const scale = Math.max(this.current().scale, Math.min(DESIGN_SCALE, this.fitScale(box)));
    this.animateTo(this.limited(this.centered(box, scale)));
  }

  /**
   * Moves the camera to `target` over ZOOM_ANIMATION_MS, easing out, zooming around the one
   * canvas point both cameras show the same world point at; then `done` (by default the
   * target is shown and reported, also when the tab was hidden meanwhile). With reduced
   * motion, at once.
   */
  animateTo(target, done = () => this.placeCamera(target)) {
    this.stopAnimation();
    const from = this.current();
    if (reducedMotion() || (from.x === target.x && from.y === target.y && from.scale === target.scale)) {
      done();
      return;
    }
    const start = performance.now();
    const frame = (now) => {
      const progress = Math.min(1, (now - start) / ZOOM_ANIMATION_MS);
      if (progress >= 1) {
        this.animation = null;
        done();
        return;
      }
      this.camera = between(from, target, 1 - (1 - progress) ** 3);
      this.applyCamera();
      this.animation.frame = requestAnimationFrame(frame);
    };
    this.animation = { target, done, frame: requestAnimationFrame(frame) };
  }

  /** Stops a zoom on its way where it is, and reports that camera. */
  stopAnimation() {
    if (!this.animation) {
      return;
    }
    cancelAnimationFrame(this.animation.frame);
    this.animation = null;
    if (this.camera) {
      this.report();
    }
  }

  /** Whether a zoom is on its way. */
  animating() {
    return this.animation !== null;
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
    this.stopAnimation();
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
    const next = { x: press.camera.x - dx / press.camera.scale, y: press.camera.y - dy / press.camera.scale, scale: press.camera.scale };
    this.moveCamera(this.limited(next, this.current()));
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

  /** A double-click on a box zooms to it. */
  onDoubleClick(event) {
    const box = event.target.closest?.('[data-kind="box"]');
    if (box) {
      this.zoomToBox(box.dataset.id);
    }
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
        this.zoomAt(this.current().scale * ZOOM_STEP ** (-delta / WHEEL_NOTCH_PX), event.clientX, event.clientY);
      }
      return;
    }
    const sideways = event.shiftKey && event.deltaX === 0;
    this.panBy(sideways ? event.deltaY : event.deltaX, sideways ? 0 : event.deltaY);
  }

  /** The zoom is the gesture's scale times `base`, the zoom when it started. */
  onGesture(event) {
    event.preventDefault();
    if (event.type === 'gesturestart' || this.gesture === null) {
      this.gesture = { base: this.current().scale / (event.scale || 1) };
    }
    if (event.type === 'gesturechange') {
      this.zoomAt(this.gesture.base * event.scale, event.clientX, event.clientY);
    }
  }

  // Export

  /**
   * The whole diagram as a standalone SVG file of the level shown: what the theme paints is
   * written into presentation attributes, so the file looks the same outside the app. What
   * the zoom changes is left out: every text shows, pills, badges and arrowheads at their size.
   */
  exportSvg() {
    if (!this.svg || !this.diagram) {
      return null;
    }
    // The theme is read from a hidden copy without the view's classes and the zoom band, so
    // the SVG shown does not change.
    const staged = this.svg.cloneNode(true);
    VIEW_CLASSES.forEach((name) => staged.querySelectorAll(`.${name}`).forEach((node) => node.classList.remove(name)));
    delete staged.dataset.zoom;
    const stage = h('div', { hidden: true }, staged);
    this.element.append(stage);
    const copy = staged.cloneNode(true);
    renameHeads(copy, (id) => id.slice(this.headPrefix.length));
    copy.querySelectorAll(LABEL_SHAPES).forEach((node) => node.removeAttribute('transform'));
    copy.querySelectorAll('marker').forEach((marker, index) => {
      marker.setAttribute('markerWidth', String(this.heads[index].size));
      marker.setAttribute('markerHeight', String(this.heads[index].size));
    });
    const originals = [staged, ...staged.querySelectorAll(EXPORT_SHAPES)];
    const copies = [copy, ...copy.querySelectorAll(EXPORT_SHAPES)];
    originals.forEach((node, index) => {
      const style = getComputedStyle(node);
      for (const property of EXPORT_PROPERTIES) {
        copies[index].setAttribute(property, style.getPropertyValue(property));
      }
    });
    stage.remove();
    copy.removeAttribute('class');
    copy.setAttribute('width', String(this.bounds.width));
    copy.setAttribute('height', String(this.bounds.height));
    copy.setAttribute('viewBox', `0 0 ${this.bounds.width} ${this.bounds.height}`);
    return XML_PROLOG + new XMLSerializer().serializeToString(copy);
  }
}

/** The level a saved camera names, else DEFAULT_LEVEL. */
function savedLevel(camera) {
  return LEVELS.includes(camera?.detail) ? camera.detail : DEFAULT_LEVEL;
}

/** The zoom and pan of a saved camera of a known level within the zoom range; else null, which fits. */
function savedPosition(camera) {
  const usable =
    LEVELS.includes(camera?.detail) && [camera.x, camera.y, camera.scale].every(Number.isFinite) && camera.scale > 0 && camera.scale <= ZOOM_MAX;
  return usable ? { x: camera.x, y: camera.y, scale: camera.scale } : null;
}

/**
 * The text band of a zoom (ZOOM_BANDS): a band is left, up or down, only past its limit by
 * ZOOM_BAND_HYSTERESIS; without a `current` band, at the limit.
 */
function zoomBand(scale, current) {
  const index = current === null ? null : ZOOM_BANDS.indexOf(current);
  const reached = ZOOM_BAND_LIMITS.filter((limit, position) => {
    if (index === null) {
      return scale >= limit;
    }
    return scale >= limit * (position < index ? 1 - ZOOM_BAND_HYSTERESIS : 1 + ZOOM_BAND_HYSTERESIS);
  });
  return ZOOM_BANDS[reached.length];
}

/**
 * The camera `share` of the way from `from` to `to`: the scale on a log scale, the position so
 * that the canvas point both show the same world point at stays put; a pan without a zoom,
 * straight.
 */
function between(from, to, share) {
  const scale = from.scale * (to.scale / from.scale) ** share;
  const axis = (start, end) => {
    if (from.scale === to.scale) {
      return start + (end - start) * share;
    }
    const fixed = (end - start) / (1 / from.scale - 1 / to.scale);
    return start + fixed / from.scale - fixed / scale;
  };
  return { x: axis(from.x, to.x), y: axis(from.y, to.y), scale };
}

function reducedMotion() {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
}

/** The rectangle of a shape from its attributes. */
function rectOf(shape) {
  const [x, y, width, height] = ['x', 'y', 'width', 'height'].map((name) => Number(shape.getAttribute(name)));
  return { x, y, width, height };
}

/**
 * Pills and badges, each with the middle of its shape and how far it may grow around it:
 * LABEL_GAP apart from every other one growing as much, a badge inside its lane, at most
 * LABEL_MAX_GROWTH; `shown` is the growth shown.
 */
function labelLimits(svg) {
  const labels = [...svg.querySelectorAll(LABEL_SHAPES)].map((node) => {
    const rect = rectOf(node.querySelector('.prism-shape'));
    return { node, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2, half: [rect.width / 2, rect.height / 2], growth: LABEL_MAX_GROWTH, shown: 1 };
  });
  // Two labels growing by g overlap only while both their distances are below g times their half sizes.
  labels.forEach((label, index) => {
    for (const other of labels.slice(index + 1)) {
      const across = (Math.abs(label.x - other.x) - LABEL_GAP) / (label.half[0] + other.half[0]);
      const along = (Math.abs(label.y - other.y) - LABEL_GAP) / (label.half[1] + other.half[1]);
      const growth = Math.max(1, across, along);
      label.growth = Math.min(label.growth, growth);
      other.growth = Math.min(other.growth, growth);
    }
  });
  const lanes = [...svg.querySelectorAll('.prism-lane > .prism-shape')].map(rectOf);
  for (const label of labels.filter(({ node }) => node.dataset.kind === 'gate')) {
    const lane = lanes.find((rect) => label.x >= rect.x && label.x <= rect.x + rect.width && label.y >= rect.y && label.y <= rect.y + rect.height);
    if (!lane) {
      continue;
    }
    const room = Math.min(
      (label.x - lane.x) / label.half[0],
      (lane.x + lane.width - label.x) / label.half[0],
      (label.y - lane.y) / label.half[1],
      (lane.y + lane.height - label.y) / label.half[1],
    );
    label.growth = Math.min(label.growth, Math.max(1, room));
  }
  return labels;
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

/** Renames the arrowhead markers of a diagram SVG, and every reference to them. */
function renameHeads(svg, rename) {
  for (const marker of svg.querySelectorAll('marker')) {
    marker.id = rename(marker.id);
  }
  for (const attribute of HEAD_REFERENCES) {
    for (const node of svg.querySelectorAll(`[${attribute}]`)) {
      const id = node.getAttribute(attribute).match(HEAD_REFERENCE)[1];
      node.setAttribute(attribute, `url(#${rename(id)})`);
    }
  }
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
