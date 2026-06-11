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
| `tomari://v1/toggle-panel`               | Show / hide the panel.                                                                                |

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

- The public commands cover **window placement only** (snap / move-display /
  undo / toggle-panel). Launching apps, sending keys, and switching the IME
  **cannot** be invoked from a URL.
- You can disable it with the **"External window control (URL scheme)"** toggle
  in settings (enabled by default in the canary release).
- Calls are one-way (fire-and-forget). There is no success/failure response;
  invalid or disabled URLs are logged and ignored.
