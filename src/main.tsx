import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { App } from './App';

import './styles.css';

if (import.meta.env.DEV && new URLSearchParams(window.location.search).has('preview')) {
  const { installDevPreview } = await import('./devPreview');
  await installDevPreview();
}

// Tomari runs as one normal macOS settings window. The shell restores the last
// focused tool so repeat visits start at the user's working context.
const container = document.querySelector('#root');
if (container) {
  createRoot(container).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
