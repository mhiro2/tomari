import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { App } from './App';

import './styles.css';

// Tomari runs as a single, normal macOS window (label "main"): the Keyboard and
// Window feature panels plus the general settings, all under one roof.
const container = document.querySelector('#root');
if (container) {
  createRoot(container).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
