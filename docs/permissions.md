# Permissions & privacy

Tomari asks for macOS permissions only for the features that genuinely need
them. This page explains what each one is for and how to grant it.

## Accessibility

Required for **moving windows**, **switching the IME**, and **sending keys**.
macOS prompts you the first time one of these is used. Grant it under
**System Settings → Privacy & Security → Accessibility**.

Global shortcuts work regardless of this permission.

## Input Monitoring

Required for **modifier tap/hold, remapping, the hyper key, and drag-to-snap**.
These rely on a resident `CGEventTap` connected to real keyboard and mouse
events, which macOS gates behind **Input Monitoring**.

If you start Tomari without granting it, creating the event tap fails and
Tomari is added to the Input Monitoring list. Enable it under **System Settings
→ Privacy & Security → Input Monitoring** (Tomari's tray menu also guides you
there). After granting it, toggling the keyboard-customization switch restarts
the listener.

## Administrator password (Keep Awake)

To keep working with the **display closed**, Keep Awake uses
`pmset disablesleep`, which requires your **administrator password**. This is
separate from Accessibility and Input Monitoring.

This lid-close layer is part of Keep Awake, so the password prompt appears both
when you **enable** it (declining cancels Keep Awake entirely — there is no
display-open-only fallback) and when you **disable** it (clearing the override
needs the same authorization; declining leaves Keep Awake on, since sleep is
still prevented until it is cleared).

## Trying things without permissions

The pure decision logic is implemented and unit-tested independently of the OS
hooks, so behavior can be confirmed without granting any permission. Global
shortcuts also work without Accessibility or Input Monitoring.
