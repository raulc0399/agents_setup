import { start } from './app.js';
import { tauriBackend } from './backend.js';

start(tauriBackend(), document.getElementById('app'));
