# Display management

Tomari's Displays page controls connected screens independently of the keyboard
and window-management features. It does not need Accessibility, Input Monitoring,
Screen Recording, an administrator password, or an additional helper process.
Choose **Display Settings…** from the menu bar to open this page directly.
The direct menu-bar route skips the unrelated first-run permission prompt.
On a normal first launch, choose **Set up later** to use Displays without granting
window or keyboard permissions; the permission checklist remains in the sidebar.
In **Keyboard**, a global shortcut or modifier tap can be assigned to
**Open Display Settings** or **Reset All Display Dimming**. No keys are assigned
automatically. The reset action removes software shades even during a mode preview;
it does not alter hardware brightness or the pending preview.
**Open macOS Display settings…** at the bottom opens the system's Displays pane
for mirroring and other controls managed by macOS. It remains available when
Tomari cannot read the display inventory and does not change a setting by itself.
The screen list and arrangement diagram use matching numbers. **Identify**
shows those numbers on the physical screens for three seconds, making identical
display names easier to distinguish. The labels ignore clicks and disappear on
display reconfiguration or shutdown. Identification is unavailable for mirrored
desktops, where the same image appears on multiple screens.
If macOS reports a shared identity for multiple screens, Tomari explains the
ambiguity and disables controls for those screens and whole-configuration
changes. It never selects the first matching physical screen for a write.
Other uniquely identified screens can still use brightness controls, and
**Reset all dimming** remains available.

## Resolution and refresh rate

Choose a display in the display list, then open Resolution, Brightness, or
Arrangement. **Profiles** saves the complete connected configuration. The arrangement diagram can also select a display. Resolution and
refresh rate are separate choices. HiDPI modes are shown by default for a display
currently using HiDPI; enable **Show non-HiDPI resolutions** for the other modes.
The rendering resolution is shown separately from the logical desktop size.
A zero refresh rate reported by macOS is presented as **Automatic / variable**,
not as a guessed fixed rate.

Tomari enumerates the modes macOS provides, including duplicate low-resolution
variants. It does not manufacture unsupported HiDPI modes, change EDID data,
create virtual displays, or override the display driver.

## Arrangement and the main display

With more than one display connected, position the selected display to the left,
right, above, or below another display. Adjacent edges are centered; macOS may
adjust the final positions to keep the desktop connected. **Use as main display**
translates the arrangement so the selected display anchors the desktop.

Mirrored configurations are managed in macOS System Settings. Tomari does not
change a mirrored display independently because that can also change the other
members of the mirror set.

If macOS cannot report a current mode for every screen, refresh the inventory
before changing or confirming the configuration. Tomari needs a complete original
configuration to restore a preview. Independent brightness, dimming, and screen
identification remain available when supported.

## Saved configurations

Under **Profiles**, give the current confirmed configuration a name and choose
**Save current configuration**. A profile includes every connected display's
resolution, rendering size, refresh rate, and position; the display at the origin
is the main display. Brightness and extra dimming are deliberately excluded.
Up to 32 profiles can be stored, with names of 1–80 characters.
**Rename profile** changes only the saved name, including when its displays are
disconnected or all 32 profile slots are occupied.
Selecting a profile shows each saved display's resolution, refresh rate, and
main-display status before previewing. Recall is unavailable when the current
inventory no longer offers a saved mode; the backend checks again when applying.

A profile applies only when exactly the same display identities are connected.
Tomari resolves the saved mode descriptions against the current inventory, so
reconnecting a display can change macOS mode numbers without breaking the saved
configuration. An unavailable mode or changed display set prevents application;
Tomari does not substitute a different refresh rate or low-DPI mode.
**Preview configuration** uses the same 20-second confirmation and rollback as
an individual change. Saving is unavailable while a preview is pending.

Profiles are part of settings backup format 2. Import replaces saved profiles
without changing the currently active display configuration. Deletion asks for
confirmation and does not affect the live display. If saved profile data cannot
be read, retry or explicitly reset the saved profiles from this page.

## Preview, confirmation, and recovery

Resolution, refresh rate, arrangement, and main-display changes use one preview
transaction. Tomari records all connected displays' modes and positions before
applying the change. Choose **Keep changes** within 20 seconds to ask macOS to
save it. Choose **Revert now**, press Escape while on the Displays page, or wait
for the timeout to restore the previous arrangement.

The recovery timer runs in the Rust backend, so leaving the page or closing the
settings window does not disable it. The preview also uses Quartz's app-only
configuration scope: WindowServer restores the session/permanent configuration
when the process exits, including a crash. Confirmation uses the permanent
scope; macOS can limit configurations its own UI cannot represent to the current
login session.

If a display disconnects or the configuration changes elsewhere, Tomari cancels
its confirmation rather than overwriting the newer desktop. A failed rollback
keeps its restore point and presents a retry action. It cannot be confirmed as a
new permanent configuration. Use macOS Display settings if the hardware continues
to reject restoration.

## Brightness

Hardware brightness is available only when the optional native DisplayServices
API successfully reads a valid value for that display. This usually covers the
built-in screen and compatible Apple displays. Tomari loads this API at runtime
and leaves the control unavailable if the framework, symbols, or display support
are absent. Brightness changes are explicit and are read back after application.

For displays without native brightness support, **Extra dimming** places a
click-through software shade over the screen. It changes the displayed image,
not the backlight power. Dimming is limited to 80%, stays active only for the
current Tomari session, and is removed on disconnect, mirroring, or app exit.
**Reset all dimming** is available in Displays and in the menu bar while any
screen is dimmed. It remains available during a configuration preview. The
software shade may appear in screenshots or screen sharing.

DDC/CI control of third-party external monitors, HDR/XDR boosts, rotation, and
virtual displays are not implemented. Brightness changes are not
part of a resolution preview and are not automatically restored on quit.

## Validation

The controller and arrangement tests use a fake display backend; ordinary tests
never change the host's screen configuration. To inspect a real macOS graphical
session without changing it:

```sh
mise exec -- cargo test -p tomari-app \
  display_management::native::tests::read_only_native_inventory \
  -- --ignored --nocapture
```

For manual testing, use an expendable graphical session and verify the scenarios
in [runtime validation](runtime-validation.md). Browser development preview data
is simulated and cannot verify a physical monitor's mode switching or brightness.

## API references

- [Apple Quartz Display Services](https://developer.apple.com/documentation/coregraphics/quartz-display-services)
- [Apple display configuration transactions](https://developer.apple.com/documentation/coregraphics/cgcompletedisplayconfiguration(_:_:))
- [BetterDisplay feature overview](https://github.com/waydabber/BetterDisplay)
