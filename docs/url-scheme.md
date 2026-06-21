# URL scheme (Raycast / Alfred integration)

Tomari exposes window actions to other tools through the `tomari://` URL
scheme. It is designed to **coexist** with launchers and automation tools such
as BetterTouchTool, Raycast, and Alfred — call it as a single action from your
launcher of choice.

## Supported commands (`v1`)

| URL                                      | Action                                                                                                |
| ---------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `tomari://v1/snap/<preset>`              | Snap the focused window to the given preset (`left-half`, `right-half`, `maximize`, … — 15 in total). |
| `tomari://v1/move-display/next` (`prev`) | Move the window to the neighboring display.                                                           |
| `tomari://v1/undo`                       | Undo the most recent window move.                                                                     |
| `tomari://v1/toggle-panel`               | Show / hide the Tomari window.                                                                        |

`<preset>` is kebab-case (`left-half`, `top-left-quarter`, `left-two-thirds`,
`center`, `maximize`, …). Snapping is **idempotent**: unlike hotkeys, calling
the same URL twice does not cycle 1/2 → 1/3 — the window always lands on the
preset you asked for.

## How to call it

```bash
open -g 'tomari://v1/snap/left-half'
```

The `-g` flag (open in the background) matters. Without it, Tomari may come to
the foreground and mistake _itself_ for the "frontmost window."

From Raycast / Alfred, run the command above via a **Script Command / Run
Script**, **not** an Open URL / Quicklink action. Open-URL actions tend to go
through the browser, which can end up acting on the browser's or the launcher's
own window instead.

## Security & limitations

- The public commands cover **window placement and showing/hiding the Tomari
  window only** (snap / move-display / undo / toggle-panel). Launching apps,
  sending keys, and switching the IME **cannot** be invoked from a URL.
- Window placement (snap / move-display / undo) is **off by default**. Turn on
  the **"URL scheme control"** toggle under **External control** in the General
  tab to let launchers move your windows. Until then those URLs are logged
  and ignored.
- **`toggle-panel` always works**, regardless of that toggle. It only shows or
  hides Tomari's own window, so it stays available as a recovery route when the
  menu bar icon is hidden.
- Calls are one-way (fire-and-forget). There is no success/failure response;
  invalid or disabled URLs are logged and ignored.
