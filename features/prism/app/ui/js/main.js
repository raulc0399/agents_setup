import { start } from './app.js';
import { tauriBackend } from './backend.js';

// The web view's own menu (Reload, Inspect Element) has no use in the app; text fields keep
// theirs for copy and paste. The tab menu is the app's own and opens from its handler.
window.addEventListener('contextmenu', (event) => {
  if (!event.target.closest('input, textarea')) {
    event.preventDefault();
  }
});

start(tauriBackend(), document.getElementById('app'));
