# Permissions & privacy

Tomari asks for macOS permissions only for the features that genuinely need
them. This page explains what each one is for and how to grant it.

## The setup checklist

Setup is a focused dialog over the settings page. Each permission appears with
what it enables, an **Open System Settings** button, and a green **Granted** mark
once it is on. The dialog opens automatically on first launch when a permission
is missing, and after an update when Tomari detects that a previously granted
permission was invalidated. **Set up later** closes the dialog without hiding
or disabling navigation.

Outside that automatic setup flow, permission health is kept in one place: the
settings sidebar footer. It reads **Permissions: Ready** when both grants are
present. If either is missing it becomes **Needs attention**; selecting it
reopens Setup. Individual Windows, Keyboard, and Menu Bar pages do not repeat
permission banners. Their settings remain visible, and the backend still
reports a permission error if an action requiring a missing grant is attempted.

The **Diagnostics** section provides the deeper read-only view: it shows both
permission bits beside the health of the event taps and other macOS integrations.
When a grant is missing, its action opens Setup, where the corresponding macOS
System Settings pane can be opened directly.
Its exported support bundle contains only status, version, and aggregate-count
fields. Raw keyboard/pointer input, menu bar Accessibility labels, process
details, configured shortcuts and actions, database rows, error text, and
filesystem paths are excluded by construction.

The administrator password for Prevent Sleep is deliberately not on the
checklist — it is asked for each time rather than granted once (see below).

## Accessibility

Required for **moving windows**, **switching the IME**, **sending keys**, and
**reading or rearranging menu bar items**. macOS prompts you the first time one
of these is used. Grant it under
**System Settings → Privacy & Security → Accessibility**.

It is also a precondition for the two event taps that *modify* input —
**keyboard customization** (modifier tap/hold, remapping, the hyper key) and
**drag to move**. Tomari does not start either of these taps until
Accessibility is granted, and stops them within a few seconds of the grant
being revoked, then starts them again once it is back. This is deliberate:
macOS has been observed to stop delivering input system-wide — persisting even
after the app quits, until you log out or restart — when Accessibility is taken
away from an app whose event tap is still active. Keeping the taps strictly
inside the window in which the grant exists avoids that state. Drag to snap
only *listens* and is unaffected; it needs Input Monitoring alone.

Global shortcuts work regardless of this permission.

## Input Monitoring

Required for **modifier tap/hold, remapping, the hyper key, drag-to-snap, and
drag to move**. These rely on a resident `CGEventTap` connected to real
keyboard and mouse events, which macOS gates behind **Input Monitoring**.
Keyboard customization and drag to move additionally wait for Accessibility
(see above).

If you start Tomari without granting it, creating the event tap fails and
Tomari is added to the Input Monitoring list. Enable it from the setup
checklist, or under **System Settings → Privacy & Security → Input
Monitoring** (Tomari's tray menu also guides you there). Tomari notices the
grant on its own and restarts the listener.

## Administrator password (Prevent Sleep)

To keep working with the **display closed**, Prevent Sleep (keep awake) uses
`pmset disablesleep`, which requires your **administrator password**. This is
separate from Accessibility and Input Monitoring.

This lid-close layer is part of Prevent Sleep, so the password prompt appears both
when you **enable** it (declining cancels Prevent Sleep entirely — there is no
display-open-only fallback) and when you **disable** it (clearing the override
needs the same authorization; declining leaves Prevent Sleep on, since sleep is
still prevented until it is cleared).

The switch remains unavailable for the entire authorization prompt. You can
cancel that prompt from Settings, and retry a declined or unconfirmed change.
Automatic deadlines, AC disconnects, and the optional low-battery auto-off
initiate this same administrator-approved clear, so macOS can show the password
prompt when a safety guard fires.

## Trying things without permissions

The pure decision logic is implemented and unit-tested independently of the OS
hooks, so behavior can be confirmed without granting any permission. Global
shortcuts also work without Accessibility or Input Monitoring.

## Re-granting permissions after an update

Tomari is currently signed ad-hoc rather than with a Developer ID certificate
(see the [README](../README.md#installation)). macOS ties Accessibility and
Input Monitoring grants to the binary's code signature (CDHash), and an
ad-hoc signature is regenerated on every build. As a result, **updating
Tomari currently invalidates both grants**, and macOS silently drops the app
from both permission lists — you will need to re-add and re-enable
Accessibility and Input Monitoring after each update, the same way you did on
first install. This is a known limitation until proper Developer ID signing
and notarization are in place; it is not something you can work around from
inside the app.

Tomari does detect it, though: each run stores which permissions were granted,
and a launch that finds a previously granted permission missing *and* a changed
app version opens the settings window with an update-specific Setup dialog. If
the version has not changed — you revoked a permission yourself — nothing opens
automatically; the tray items and the settings sidebar's **Needs attention**
status point it out. (Detection needs a stored snapshot to compare against, so
it starts working from the first update *after* the release that introduced
it.)
