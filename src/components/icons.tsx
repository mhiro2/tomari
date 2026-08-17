import type { ReactNode } from 'react';

/** The sections the sidebar can select. */
export type SectionName = 'keyboard' | 'window' | 'menubar' | 'session' | 'general';

// One drawing standard for every section icon, so the sidebar reads as a single
// column rather than four unrelated marks: a 16×16 box, a 1.5 stroke, no fill,
// and `currentColor` — which is what lets the active row tint its icon amber by
// setting `color` alone. Each shape stays inside roughly 12×10 so the four have
// the same optical weight.
const SHAPES: Record<SectionName, ReactNode> = {
  // A keyboard seen head-on: the case, a row of keys, a space bar.
  keyboard: (
    <>
      <rect x="1.5" y="4" width="13" height="8" rx="1.5" />
      <path d="M4.4 7h.01M6.8 7h.01M9.2 7h.01M11.6 7h.01" />
      <path d="M5.6 9.8h4.8" />
    </>
  ),
  // A window split down the middle — the halves a snap lands in.
  window: (
    <>
      <rect x="2" y="3" width="12" height="10" rx="1.5" />
      <path d="M8 3v10" />
    </>
  ),
  // The divider with icons either side of it — the arrangement the feature is
  // all about.
  menubar: (
    <>
      <path d="M2.6 8h.01M5.1 8h.01" />
      <path d="M8 3.5v9" />
      <path d="M10.9 8h.01M13.4 8h.01" />
    </>
  ),
  // An open eye: the Mac stays awake.
  session: (
    <>
      <path d="M1.8 8s2.4-4 6.2-4 6.2 4 6.2 4-2.4 4-6.2 4-6.2-4-6.2-4Z" />
      <circle cx="8" cy="8" r="1.6" />
    </>
  ),
  // Sliders. The tracks break around each knob so nothing needs a fill.
  general: (
    <>
      <path d="M2 5.5h6.2M11.8 5.5h2.2" />
      <circle cx="10" cy="5.5" r="1.8" />
      <path d="M2 10.5h2.2M7.8 10.5h6.2" />
      <circle cx="6" cy="10.5" r="1.8" />
    </>
  ),
};

/** The icon for one sidebar section. Decorative: the row carries the name. */
export function SectionIcon({ name }: { name: SectionName }) {
  return (
    <svg
      className="nav-item__icon"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {SHAPES[name]}
    </svg>
  );
}
