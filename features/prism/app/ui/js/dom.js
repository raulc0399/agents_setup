/**
 * Creates an element. Props: `class`, `text`, `on<event>` listeners, `value` and other
 * non-string values as DOM properties, attributes otherwise. Children are nodes or strings (as text, never HTML).
 */
export function h(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(props)) {
    if (value === undefined || value === null || value === false) {
      continue;
    }
    if (name === 'class') {
      node.className = value;
    } else if (name === 'text') {
      node.textContent = value;
    } else if (name.startsWith('on')) {
      node.addEventListener(name.slice(2), value);
    } else if (name === 'value' || (typeof value !== 'string' && name in node)) {
      node[name] = value;
    } else {
      node.setAttribute(name, value === true ? '' : value);
    }
  }
  node.append(...children.flat(Infinity).filter((child) => child !== null && child !== undefined && child !== false));
  return node;
}

/** The modifier key as the platform names it. */
export const MOD_KEY = /Mac|iPhone|iPad/.test(navigator.platform) ? '⌘' : 'Ctrl';

/** Whether the user prefers reduced motion: animations jump to their end. */
export function reducedMotion() {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
}
