import { MOD_KEY, h } from './dom.js';
import { questionBlock } from './list.js';
import { questionsFor } from './rows.js';

/** Zoom limits in pixels per world unit; a fitted diagram is never blown up beyond MAX_FIT_SCALE. */
const MIN_SCALE = 0.1;
const MAX_SCALE = 4;
const MAX_FIT_SCALE = 1.25;
const FIT_PADDING_PX = 24;
const ZOOM_STEP = 1.25;
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

/**
 * The diagram view of one tab: the SVG that `prism-core` laid out, panned and zoomed through
 * its viewBox, themed by `styles.css`, and a side panel for the focused item. A camera
 * `{x, y, scale}` is the world point at the top left and pixels per world unit; each
 * document has its own, `null` fits the whole diagram.
 *
 * `actions`: layout(document, visibleIds) -> Diagram, onFocus(id), onClear(),
 * onCamera(documentId, camera).
 */
export class DiagramView {
  constructor(actions) {
    this.actions = actions;
    this.model = null;
    this.diagram = null;
    this.svg = null;
    this.layoutKey = null;
    this.camera = null;
    this.press = null;
    this.suppressClick = false;
    this.pointerFocus = false;
    this.gestureScale = null;
    this.shownFocus = null;
    this.panelSignature = null;

    this.message = h('div', { class: 'diagram-message', hidden: true });
    this.zoomLabel = h('span', { class: 'zoom' });
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
      this.gestureScale = null;
    });
    // Focusable, so that its text can be scrolled with the keyboard.
    this.panel = h('aside', { class: 'diagram-panel', tabindex: '0', 'aria-label': 'Item details' });
    this.element = h('div', { class: 'diagram-view' }, h('div', { class: 'diagram-frame' }, this.canvas, this.message, this.controls), this.panel);
    this.resizes = new ResizeObserver(() => this.applyCamera());
    this.resizes.observe(this.canvas);
  }

  /** The tab left this window: stop watching the canvas size. */
  dispose() {
    this.resizes.disconnect();
  }

  /**
   * `model`: {document, rows, focus: the focused item's ID or null, questions,
   * camera: the camera saved for this document or null}.
   */
  render(model) {
    const previousFocus = this.model?.focus ?? null;
    this.model = model;
    const visible = model.rows.map((row) => row.id);
    const key = JSON.stringify([model.document.id, model.document.revision, visible]);
    if (key !== this.layoutKey) {
      this.layoutKey = key;
      this.requestLayout(key, model.document, visible);
    }
    // A saved camera this view could not have made does not apply: the diagram starts fitted.
    this.camera = usableCamera(model.camera) ? model.camera : null;
    this.applyCamera();
    this.showFocus(model.focus !== previousFocus);
    this.renderPanel();
  }

  async requestLayout(key, document, visible) {
    let diagram;
    try {
      diagram = await this.actions.layout(document, visible);
    } catch (error) {
      if (key === this.layoutKey) {
        this.showMessage(`The diagram could not be laid out: ${error}`);
      }
      return;
    }
    if (key !== this.layoutKey) {
      return;
    }
    this.install(diagram);
  }

  install(diagram) {
    const parsed = new DOMParser().parseFromString(diagram.svg, 'image/svg+xml').documentElement;
    if (parsed.nodeName !== 'svg') {
      this.showMessage('The diagram could not be shown.');
      return;
    }
    // Keyboard focus inside the old SVG moves to the same item in the new one.
    const refocus = this.svg?.contains(document.activeElement);
    const svg = document.importNode(parsed, true);
    svg.removeAttribute('width');
    svg.removeAttribute('height');
    svg.setAttribute('class', 'diagram-svg');
    this.svg?.remove();
    this.svg = svg;
    this.diagram = diagram;
    this.canvas.prepend(svg);
    this.showMessage(diagram.order.length === 0 ? 'Nothing to draw: the items shown here have no boxes.' : null);
    this.shownFocus = null;
    this.applyCamera();
    this.showFocus(false);
    this.renderPanel();
    if (refocus) {
      (this.stop(this.model.focus) ?? this.canvas).focus({ preventScroll: true });
    }
  }

  /** After Esc: the keyboard stays in the diagram, on no item. */
  focusCanvas() {
    this.canvas.focus({ preventScroll: true });
  }

  showMessage(text) {
    this.message.hidden = !text;
    this.message.textContent = text ?? '';
  }

  // Focus and panel

  /** The keyboard stop of an item: its box, badge, lane or, for an arrow, its pill. */
  stop(id) {
    if (!this.svg || !id) {
      return null;
    }
    return [...this.svg.querySelectorAll('[data-id][tabindex="0"]')].find((node) => node.dataset.id === id) ?? null;
  }

  /**
   * Highlights the focused item. When it `moved` by keyboard, the item also takes the
   * keyboard (unless something outside the diagram has it) and comes into view.
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
    const stop = this.stop(focus);
    if (!moved || !stop || this.pointerFocus) {
      return;
    }
    const active = document.activeElement;
    if (active === document.body || active === null || this.element.contains(active)) {
      stop.focus({ preventScroll: true });
    }
    this.reveal(stop);
  }

  renderPanel() {
    const row = this.model.rows.find((candidate) => candidate.id === this.model.focus) ?? null;
    const questions = row ? questionsFor(this.model.questions, this.model.document.id, row.id) : [];
    const counts = this.svg ? ['box', 'pill', 'gate'].map((kind) => this.svg.querySelectorAll(`[data-kind="${kind}"]`).length) : null;
    const signature = JSON.stringify([row, questions, row ? null : [this.model.document.title, counts]]);
    if (signature === this.panelSignature) {
      return;
    }
    this.panelSignature = signature;
    this.panel.replaceChildren(...(row ? itemPanel(row, questions) : overviewPanel(this.model.document.title, counts)));
  }

  // Camera

  size() {
    return { width: this.canvas.clientWidth, height: this.canvas.clientHeight };
  }

  /** The camera that shows the whole diagram, centered. */
  fitCamera() {
    const { width, height } = this.size();
    const bounds = this.diagram ?? { width: 0, height: 0 };
    if (bounds.width <= 0 || bounds.height <= 0) {
      return { x: 0, y: 0, scale: 1 };
    }
    const scale = clamp(
      Math.min((width - 2 * FIT_PADDING_PX) / bounds.width, (height - 2 * FIT_PADDING_PX) / bounds.height, MAX_FIT_SCALE),
      MIN_SCALE,
      MAX_SCALE,
    );
    return {
      x: bounds.width / 2 - width / 2 / scale,
      y: bounds.height / 2 - height / 2 / scale,
      scale,
    };
  }

  current() {
    return this.camera ?? this.fitCamera();
  }

  applyCamera() {
    const { width, height } = this.size();
    if (!this.svg || !this.model || width === 0 || height === 0) {
      return;
    }
    const camera = this.current();
    this.svg.setAttribute('viewBox', `${camera.x} ${camera.y} ${width / camera.scale} ${height / camera.scale}`);
    this.zoomLabel.textContent = `${Math.round(camera.scale * 100)}%`;
  }

  /** Moves the camera, keeps part of the diagram in view, shows and reports it. */
  setCamera(camera) {
    const { width, height } = this.size();
    const bounds = this.diagram ?? { width: 0, height: 0 };
    const keep = KEEP_VISIBLE_PX / camera.scale;
    this.camera = {
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

  zoomTo(target, clientX = null, clientY = null) {
    const camera = this.current();
    const scale = clamp(target, MIN_SCALE, MAX_SCALE);
    const rect = this.canvas.getBoundingClientRect();
    const px = clientX === null ? rect.width / 2 : clientX - rect.left;
    const py = clientY === null ? rect.height / 2 : clientY - rect.top;
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

  /** One zoom step in (1) or out (-1), around the center. */
  zoomStep(direction) {
    this.zoomBy(ZOOM_STEP ** direction);
  }

  /** One pan step per axis: -1, 0 or 1. */
  panStep(dx, dy) {
    this.panBy(dx * PAN_STEP_PX, dy * PAN_STEP_PX);
  }

  fit() {
    this.camera = null;
    this.applyCamera();
    this.actions.onCamera(this.model.document.id, null);
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
    if (shape) {
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
      if (this.gestureScale === null) {
        const delta = clamp(event.deltaY, -WHEEL_NOTCH_PX, WHEEL_NOTCH_PX);
        this.zoomBy(ZOOM_STEP ** (-delta / WHEEL_NOTCH_PX), event.clientX, event.clientY);
      }
      return;
    }
    const sideways = event.shiftKey && event.deltaX === 0;
    this.panBy(sideways ? event.deltaY : event.deltaX, sideways ? 0 : event.deltaY);
  }

  onGesture(event) {
    event.preventDefault();
    if (event.type === 'gesturestart' || this.gestureScale === null) {
      this.gestureScale = this.current().scale;
    }
    if (event.type === 'gesturechange') {
      this.zoomTo(this.gestureScale * event.scale, event.clientX, event.clientY);
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
    const focused = [...this.svg.querySelectorAll('.focused')];
    focused.forEach((node) => node.classList.remove('focused'));
    const copy = this.svg.cloneNode(true);
    const originals = [this.svg, ...this.svg.querySelectorAll(EXPORT_SHAPES)];
    const copies = [copy, ...copy.querySelectorAll(EXPORT_SHAPES)];
    originals.forEach((node, index) => {
      const style = getComputedStyle(node);
      for (const property of EXPORT_PROPERTIES) {
        copies[index].setAttribute(property, style.getPropertyValue(property));
      }
    });
    focused.forEach((node) => node.classList.add('focused'));
    copy.removeAttribute('class');
    copy.setAttribute('width', String(this.diagram.width));
    copy.setAttribute('height', String(this.diagram.height));
    copy.setAttribute('viewBox', `0 0 ${this.diagram.width} ${this.diagram.height}`);
    return XML_PROLOG + new XMLSerializer().serializeToString(copy);
  }
}

function usableCamera(camera) {
  return (
    [camera?.x, camera?.y, camera?.scale].every(Number.isFinite) &&
    camera.scale >= MIN_SCALE &&
    camera.scale <= MAX_SCALE
  );
}

function clamp(value, min, max) {
  return Math.min(Math.max(value, min), Math.max(min, max));
}

function itemPanel(row, questions) {
  return [
    h('div', { class: `panel-tag kind-${row.kind}` }, h('span', { class: 'kind', text: row.kindLabel }), h('span', { class: 'number', text: String(row.number) })),
    h('h2', { class: 'panel-title', text: row.title }),
    row.body ? h('div', { class: 'panel-body', text: row.body }) : null,
    row.source ? h('div', { class: 'card-source', text: row.source }) : null,
    ...questions.map(questionBlock),
  ].filter(Boolean);
}

function overviewPanel(title, counts) {
  const [boxes, arrows, badges] = counts ?? [0, 0, 0];
  const chip = (count, one, many) => h('span', { class: 'chip', text: `${count} ${count === 1 ? one : many}` });
  return [
    h('div', { class: 'panel-tag' }, h('span', { class: 'kind', text: 'Diagram' })),
    h('h2', { class: 'panel-title', text: title }),
    counts ? h('div', { class: 'chips' }, chip(boxes, 'box', 'boxes'), chip(arrows, 'arrow', 'arrows'), chip(badges, 'badge', 'badges')) : null,
    h(
      'p',
      { class: 'hint' },
      'Click a box, an arrow’s number or a badge, or press ',
      h('kbd', { text: 'Tab' }),
      '. Drag or scroll to pan; ',
      h('kbd', { text: MOD_KEY }),
      ' and scroll or pinch to zoom.',
    ),
  ].filter(Boolean);
}
