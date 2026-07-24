# Permissions & privacy

Tomari asks for macOS permissions only for the features that genuinely need
them. This page explains what each one is for and how to grant it.

## The setup checklist

While either permission below is missing, the settings window offers a
one-screen **setup checklist**: each permission as a row with what it is for,
a **Grant Access** button, and a green **Granted** mark once it is on. On a
first launch the window opens on this checklist automatically; **Set up
later** switches to the normal tabs, leaving a thin reminder bar under the
tab bar until everything is granted. The per-tab permission banners stay, and
their **Open Setup** button leads back to the checklist.

The administrator password for Prevent Sleep is deliberately not on the
checklist — it is asked for each time rather than granted once (see below).

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
