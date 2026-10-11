# Runtime validation and release evidence

Tomari targets Apple Silicon Macs running **macOS 27 or later**. Unit tests
and CI builds establish logic and packaging coverage; they do not establish
that input interception, Accessibility, authorization, or cleanup work in a
logged-in desktop session. This document defines that separate release gate.

**Status: the release matrix and performance baselines remain unexecuted.**
Adding this procedure or passing the measurement tool's tests does not mark a
Tomari build as validated. Record actual results for each candidate. Do not
describe a synthetic process measurement as an application baseline.

### Development checks on 2026-10-02

The following targeted checks used the local ad-hoc debug bundle, version 0.0.2,
on Apple Silicon with macOS 27.0.1 (26A434). This was the existing development
account, not a clean release-test account, so these observations do not mark
the full matrix below as passed.

The final bundled executable, rebuilt after tightening warning-action routing,
passed signature verification and a native UI smoke check. Its SHA-256 was
`f7c3dd51bfa7d042c7c702c5a595594e7b085d582c11b64ece44de9aea3e97a1`.

- The modifier editor created and deleted a disabled Shift/Hyper rule and
  rejected a duplicate Caps Lock slot. Real remapping and ANSI/JIS typing remain
  blocked by stale Accessibility registration and missing Input Monitoring.
- A native export had mode `0600`. File selection, preview, cancellation,
  re-selection, import, relaunch, and restoration from the surviving pre-import
  snapshot succeeded. The import used the original configuration; all four
  portable tables matched their pre-test contents afterward. This does not
  cover every changed-data, conflict, or failure scenario in R17–R19.
- macOS classified the exported JSON as plain text and disabled Open under the
  file-type filter. Removing that filter made it selectable; the size limit and
  strict content validation remain. The three backup controls measure 208 × 34
  CSS pixels at the minimum 860-pixel window width; the Japanese heading stays
  on one line. Layout was also checked in the native 940-pixel window.
- Both permission buttons opened the matching System Settings pane, including
  Input Monitoring after prior denial. TCC logs confirmed an obsolete code
  requirement despite the old Accessibility entry appearing enabled. Re-adding
  the final bundle and testing actual input remains a user-controlled step.
- The previous bundle failed strict signature verification. Explicit whole-app
  ad-hoc signing passed `codesign --verify --deep --strict` with the correct
  bundle identifier and resource seal. Developer ID and notarization remain
  deferred; changed ad-hoc binaries still need permission re-registration.
- A read-only `hidutil` check returned four keyboard-capable services. Their
  macOS 27 table output is now parsed consistently; differing per-service
  mappings are refused. Mapping writes and custom-mapping restoration remain
  untested in this desktop session.
- Lid-open Prevent Sleep started without an authorization prompt, appeared as
  a real `PreventUserIdleSystemSleep` assertion, and left `disablesleep`
  unchanged. Diagnostics reported it healthy. Manual stop and an explicit
  restart both removed the assertion; restart reopened Settings with the
  session off. The separately invoked native IOKit create/release test passed.
- Direct executable relaunch could not be reacquired by the native UI tool.
  The explicit restart action now launches the packaged app through macOS
  LaunchServices; two subsequent restarts returned to the visible Settings
  window. No automatic permission-triggered restart is used.

Administrator prompts, lid closure, battery guards, sleep/wake, display changes,
updates, and the eight-hour performance session remain outside these checks.

## Record the candidate and environment

Use a dedicated test account with non-sensitive windows and documents. Record:

- candidate version, commit, build mode, and artifact checksum;
- the prior release used for upgrade tests and whether signing is ad-hoc or
  Developer ID;
- macOS version and build, Apple Silicon model and RAM, without serial numbers
  or hostnames;
- display count, resolutions, scaling, arrangement, and keyboard layout
  (ANSI/US and JIS/Japanese);
- AC or battery, starting permission state, enabled Tomari features, language,
  and whether the settings window is open.

Test the **release bundle** that will be distributed, installed in Applications,
with no development server attached. Use the oldest supported macOS 27 release
and the latest supported macOS release; when they are the same, record that
explicitly. Record the exact OS build even when using a preview runner. A CI
runner compiling the bundle does not substitute for a desktop-session check.

## Desktop regression matrix

Run these cases manually on the test machine. The measurement tool below never
launches or terminates Tomari, injects input, changes permissions or settings,
or performs an update. Destructive or interrupting scenarios belong to this
explicit test session, not ordinary development checks.

For each row, record `Pass`, `Fail`, `Blocked`, or `Not run`, the environment,
observed result, and an issue link for a failure. For a feature deliberately
disabled on the target OS, verify its unavailable behavior; do not silently
mark its case passed because the control is absent.

| ID | Scenario and steps | Required observation | Initial result |
| --- | --- | --- | --- |
| R01 | Install the candidate into a fresh test account; launch it, defer Setup, then grant each required permission. | Setup represents actual grants; deferral leaves Settings usable; enabled features begin working after grants. Record any Gatekeeper workaround required by the current signing model. | Not run |
| R02 | Configure an ordinary remap and tap action, then type with left/right modifiers, repeated taps, held keys, clicks, and scrolling on ANSI and JIS keyboards. | Remaps and solo taps fire only as configured; no stuck modifier, duplicate key, or swallowed ordinary input remains after releasing keys. IME actions switch as configured. | Not run |
| R03 | Enable a Caps Lock rule; disable it, re-enable it, and quit normally. Repeat with a pre-existing custom HID mapping in the test account. | Tomari removes only its owned mapping and restores the prior Caps Lock mapping. Other mappings remain intact. Diagnostics agrees with effective ownership. | Not run |
| R04 | With remapping active, revoke and restore Accessibility and Input Monitoring separately. Repeat after sleep/wake and lock/unlock. | Permission status converges; affected automation stops safely and recovers after the documented grant flow. No stuck key or unbounded restart loop occurs. | Not run |
| R05 | Force-quit Tomari while a Caps Lock rule is active, then relaunch. | Startup reconciles owned mappings, exposes any unresolved ownership, and leaves unrelated mappings intact. Repeat ordinary typing after recovery. | Not run |
| R06 | Save positions A/B, restore repeatedly, move to another display, undo, and redo using Settings and shortcuts. | Every action addresses the intended external app/window; A/B alternation and history remain coherent. Opening Settings does not make Tomari the target. | Not run |
| R07 | Disconnect/reconnect the external display, change scaling/arrangement, then restore and undo. | The window stays within an available usable display; an unavailable or changed target produces a recoverable result rather than moving a different window. | Not run |
| R08 | Exercise edge snapping and modifier-drag move/resize; release the modifier or button during a drag and disable the feature between gestures. | The preview and final geometry agree, ordinary pointer input resumes, and a completed or canceled gesture cannot move a later window. | Not run |
| R09 | Start and stop Standard Prevent Sleep, then select lid-closed mode while stopped and accept/decline its administrator prompts; cancel and retry. Attempt a mode change while active, pending, or in recovery. | Standard creates/releases its assertion without authorization or changes to `disablesleep`; Diagnostics reports it healthy. Lid-closed state stays pending until authorization settles; denial does not switch modes. Mode changes are refused while active or unresolved, and retry follows the failed direction. | Not run |
| R10 | In each mode, test a deadline, AC removal, and the low-battery policy in a controlled session; test lid closure while monitoring locally. | Each condition initiates the selected mode's stop once. Standard stops without authorization and does not claim lid-close protection. Declined lid-closed authorization remains visible and retryable. Record untested battery/lid conditions as Blocked. | Not run |
| R11 | Quit normally and force-quit in separate sessions of each mode; relaunch after forced exit. Decline recovery of a prior lid-closed override, then try Standard. | Standard's assertion is released on exit. Lid-closed normal exit completes cleanup and its crash path offers explicit recovery. Relaunch defaults to Standard but refuses to bypass an unresolved owned override; foreign overrides remain untouched. | Not run |
| R12 | On macOS 27+, verify Menu Bar is absent from the sidebar and Diagnostics, and try any saved menu-bar shortcut. | Unsupported tidying stays inactive: no divider/handle is inserted and an unavailable action does not reserve its shortcut. Other features remain usable. | Not run |
| R13 | Hide the tray icon, relaunch Tomari, and invoke the panel URL; launch a second instance while Settings is open. | The existing Settings window remains recoverable and only one instance owns automation. External window actions still honor their opt-in control. | Not run |
| R14 | Upgrade the prior release with configured positions/remaps and a granted test profile. Repeat with a failed download and then retry. | Settings persist; failed delivery leaves a usable installed app. A successful restart cleans up the old runtime and starts one new instance. Record actual permission retention or re-grant behavior for the signing model used. | Not run |
| R15 | Switch English/Japanese, navigate Setup, feature tabs, and Diagnostics by keyboard, and export the support bundle. | Text and action state remain correct; focus can reach and leave controls and dialogs. The exported bundle excludes input, window labels, process details, and paths. | Not run |
| R16 | Run an eight-hour ordinary-work session, including ten lock/unlock and sleep/wake cycles plus display reconnection. Capture Diagnostics before and after. | No persistent input fault, crash, runaway tap restart, unbounded memory growth, or unresolved owned system state remains. Record the actual duration and cycle counts. | Not run |
| R17 | Export configured general settings, hotkeys, modifier rules, and positions; reveal the file in Finder. Change each group, select the export, inspect the preview, cancel once, then preview and confirm replacement. | The export is an owner-only file. Cancel changes nothing; confirmation restores all four groups, including deletions, without moving open windows or starting Prevent Sleep. Settings and active shortcuts agree with the saved result. | Not run |
| R18 | Preview a malformed, unsupported-version, incomplete, or conflicting backup. Also preview a valid backup, change the current configuration, then try the old confirmation. | Invalid files and stale previews are refused without replacing any configuration or earlier restore point. Quarantined current rows must be repaired before import. Old autosaves cannot land over an accepted import. | Not run |
| R19 | Import while a requested OS effect is unavailable, such as a shortcut conflict or missing Input Monitoring. Inspect the apply warning, quit/relaunch, preview the pre-import restore point, and confirm restoration. Repeat after making a post-import edit. | Saved data and live-apply failure are distinguished. The restore point survives relaunch; its preview clearly replaces later edits. Restoration recovers all four configuration groups, and any remaining OS mismatch stays visible. An old position-edit undo cannot rewrite the imported or restored snapshot. | Not run |

Authentication and permission changes must be checked through macOS and the
running bundle. Mocks are still valuable for race conditions, database recovery,
and failure paths; they do not satisfy these rows. Preserve the existing unit
coverage and add regressions when an observed failure can be reproduced there.

For configuration imports, also exercise fault injection in automated tests:
fail a write after an earlier table has been deleted, fail the restore-point
write or consumption, race a saved-position edit with the expected-snapshot
check, and fail live application after the database commit. The first group
must preserve every pre-existing row and restore point; the last must report
saved data plus an apply failure and retain the documented recovery path.
Repeat preview/apply after an error to prove a failed attempt cannot silently
consume the user's recovery option. Do not simulate database failures by
damaging the user's live database.

## CPU and resident-memory measurement

`tools/measure_runtime.py` uses Python 3's standard library and `/bin/ps` to read
only a selected PID's start time, cumulative CPU time, RSS, and process state.
It emits one JSON object to standard output. It does not request permissions or
collect names, command arguments, input, window titles, or filesystem paths.
The process start time is used only to detect a changed process and is not
exported. Start-time precision is one second; a PID reused within that same
second may not be distinguishable if its CPU counter has also caught up.

1. Close the development server and unrelated heavy workloads. Keep the same
   hardware, OS, power source, display arrangement, and enabled features for
   baseline and candidate measurements. Let the release app settle for two
   minutes after startup or a scenario change.
2. Select Tomari in Activity Monitor and read its PID. Supply that PID explicitly;
   the tool does not discover processes or record their command lines.
3. Capture three runs of each scenario, using a fresh output filename per run:

   ```sh
   mise exec -- python3 tools/measure_runtime.py --pid 12345 --duration 60 --interval 1 > /tmp/tomari-idle-candidate-1.json
   ```

   Replace `12345` with the current PID. Check the command's exit status and
   `status` in the JSON before using the result. Never compare a partial result
   to a completed baseline.

4. Repeat the same measurements with the prior accepted release on that machine.
   Keep filenames, build metadata, and scenario notes in the validation record;
   they are not inferred by the measurement script.

| Scenario | Repeatable workload | Result status |
| --- | --- | --- |
| Idle, Settings closed | Use the same enabled features; leave the pointer and keyboard idle for each 60-second run. | Not run; no baseline |
| Idle, Settings open | Keep the same feature page visible for each 60-second run. | Not run; no baseline |
| Active interaction | For 60 seconds, perform the same written sequence of taps, shortcuts, ten position restores, and ten drag gestures. Keep the repetition count with the result. | Not run; no baseline |
| Prevent Sleep | Complete authorization, close Settings, and measure the stable session for 60 seconds on the same power source. Stop it and verify cleanup afterward. | Not run; no baseline |
| Long session | Sample an eight-hour ordinary-work session with `--duration 28800 --interval 60`; annotate sleep/wake, display changes, and workloads in separate notes. | Not run; no baseline |

The requested duration is finite (1–28800 seconds); intervals are 0.25–60 seconds
and cannot exceed duration. Each `ps` call has a two-second timeout. The measured
interval begins after the first read and finishes after the final read, so wall
time can exceed the requested duration by sampling overhead or OS scheduling.
Suspend/resume and a stalled machine also affect elapsed-time interpretation;
do not compare a sleeping long-run interval with a continuously awake baseline.

The JSON contains raw numerical samples and a summary:

- `cpu_percent`: CPU-time increase divided by actual elapsed wall time, times
  100. It is **not** the historical/smoothed `ps %cpu` reading. One fully occupied
  core is 100%, so a multi-threaded process can exceed 100%. Short intervals are
  quantized by `ps` CPU-time precision; use the complete 60-second measurement
  for comparisons.
- `rss_initial_bytes`, `rss_final_bytes`, `rss_peak_bytes`, and
  `rss_change_bytes`: resident process memory in bytes. A negative change is
  valid; a rise on its own is not proof of a leak. RSS is neither private memory
  nor the whole application's memory footprint.
- `samples`: elapsed seconds, cumulative CPU seconds, and RSS for each read.
  These permit inspection of sustained growth instead of relying on one peak.

Scope is deliberately **one process**. Separate WKWebView, WindowServer, and
other helper processes are excluded. For application-wide investigation, use
Activity Monitor or Instruments and record that measurement's scope separately.
Do not sum RSS across processes and call it private application memory. The
sampler's own cost is outside the selected PID's CPU accounting.

An unavailable or zombie process, changed start time, decreasing CPU counter,
malformed sample, command timeout, invalid options, unsupported OS, or Ctrl-C
returns `status: "error"` and exits nonzero. Failures during measurement retain
already collected samples with `summary: null`; they are **incomplete**. The tool
never reconnects to a replacement process. After an intentional restart, start
a new capture with the new PID. Raw command error output is not exported.

## Performance acceptance

There is no measured baseline or claimed performance result yet. Establish one
from the prior accepted release on a named test configuration before interpreting
a candidate's numbers. If no prior release is usable on macOS 27, explicitly
record the first validated build as the baseline and do not claim an improvement.

Use the median CPU percentage and median peak RSS of the three runs for each
scenario. The following initial **review thresholds** define when a repeat and
investigation are required; they are policy choices, not measured guarantees:

- CPU increase greater than the larger of **0.25 percentage points or 20%** of
  the baseline CPU percentage;
- peak RSS increase greater than the larger of **20 MiB or 20%** of baseline
  peak RSS;
- a continuing rise across comparable idle portions of the long session, or
  unexpected tap restarts, regardless of the short-run averages.

Repeat a threshold breach under the same conditions. A persistent regression
needs an identified cause and an explicit release decision; do not silently
replace the baseline. Keep an accepted baseline fixed until a deliberate update
records why the resource change is acceptable. The script validates sampling,
not these performance thresholds, and exits zero even if a reviewer later finds
the measured performance unacceptable.

For input or window-action latency, profile a fixed action sequence with
Instruments and record median and p95 plus the capture method. This CPU/RSS tool
does not measure event latency, energy consumption, or wakeups; do not label its
results as those metrics. If investigating those properties, collect them
separately without logging raw user input.

## Release decision record

Keep a record alongside the candidate's release notes using this template:

```text
Candidate version / commit / artifact checksum:
Prior release / baseline artifact:
macOS version and build / hardware / display / keyboard / power:
Signing model:
Validation date and reviewer:
Automated fmt / lint / typecheck / tests / dependency audit / bundle build:
R01–R19 results, observations, and failure issue links:
Measurement files and scenario repetition counts:
CPU / RSS baseline and candidate comparisons:
Long-session actual duration / cycles / diagnostic changes:
Known limitations and blocked cases:
Decision: Not assessed | Hold | Accept
Reason and explicit approvals for any accepted limitation:
```

The initial decision is **Not assessed**. Hold a candidate for a failed required
check, data loss, persistent input interference, incorrect system-state cleanup,
or an unexplained repeatable resource regression. A required `Not run` or
`Blocked` row is incomplete evidence, not a pass. Stable release acceptance
requires completing the matrix and reviewing performance; if a canary is shipped
with missing evidence, its release notes must list that gap and the reviewer
must explicitly accept that limited validation.

Run the sampler's deterministic tests without observing the host:

```sh
mise exec -- python3 -m unittest discover -s tools -p 'test_*.py'
```

These tests cover sampling arithmetic, finite timing, process replacement and
exit, malformed data, failures, and argument bounds. They do not execute the
desktop matrix or establish a performance baseline.


## Display management

Perform configuration-changing checks only in a graphical session where a
brief resolution change is acceptable. The normal automated suite uses fake
hardware. The opt-in inventory test in [the display guide](displays.md) is
read-only.

- Open Displays with no input permissions granted. Verify names, main-display
  marker, proportional arrangement, logical/pixel sizes, and current rate.
- Preview another supported resolution/rate. Confirm it, then restore the
  original explicitly. Separately let a preview expire with Settings closed.
- Quit while a preview is pending; verify the previous/session configuration
  returns. In a disposable session, also verify process termination recovery.
- With two screens, preview each relative placement and a main-display change.
  Revert and verify every screen's original mode and origin, not just the target.
- Disconnect a screen or change the configuration in System Settings during a
  preview. An old confirmation must not save or overwrite that external change.
- Verify native brightness reads and writes on a supported display, and the
  unavailable explanation on an unsupported one. Restore the test brightness.
- Verify mirrored displays are explained and configuration controls do not
  independently change a member of the mirror set.
- Verify Escape reverts the current preview and keyboard focus moves to its
  confirmation heading. Check the smallest settings window in English/Japanese.

- Apply software dimming to one screen, then change resolution and switch Spaces.
  The shade must track only that display, leave mouse/keyboard input untouched,
  and remain visible over full-screen apps. Reset from the tray and Settings.
  Verify unplugging, switching to mirroring, and quitting remove the shade.
- With a preview pending, verify **Reset all dimming** still works. Reject
  non-finite, negative, and greater-than-80% amounts at the command boundary.
- Assign **Open Display Settings** and **Reset All Display Dimming** to hotkeys
  or modifier taps. Verify opening works while another settings section is active,
  and reset removes shades without confirming or reverting a pending mode preview.
- Leave Displays closed with no preview pending. The display watchdog must have
  no recurring timer wakeups, and no inventory polling should be active.

### Saved display configurations

- Save a confirmed multi-display layout, change resolution and arrangement, then
  recall it. Verify every saved mode and origin and exercise both Keep and timeout.
- Reconnect the same screens and recall again; mode numbers may differ, but
  resolution, rendering dimensions, and refresh rate must match.
- Disconnect one screen or attach an extra screen. Recall must be unavailable.
- Rename a profile while its displays are disconnected and while all 32 slots
  are occupied. Its identity and saved modes/origins must remain unchanged.
  Cancel an edit and simulate a storage failure; failure must retain the draft.
- Export, delete a profile, then import. The profile must return without any live
  display changes; one-step import undo must restore the prior saved-profile set.
- Switch to another application with extra dimming enabled. The shade must stay
  visible and click-through until explicitly reset or Tomari exits.

These physical configuration and panel tests have not been run on the host.

The Japanese browser preview was visually checked at the minimum 860 × 620
window size with a long unbroken display name and a 76-character profile name.
Display cards and saved-mode summaries wrap within the content area, the saved
profile selector stays bounded, and the preview/delete/system-settings actions
remain reachable by vertical scrolling. This uses simulated display data.

- Choose **Open macOS Display settings…** while inventory loading is failing;
  verify the system Displays pane opens and no display setting changes. The
  command uses the installed Displays extension identifier. Native pane launch
  has not been exercised on the host; the failure-state UI route is tested.

- Identify two displays with identical model names. The numbers on physical
  screens must match the cards, arrangement diagram, placement reference selector,
  and saved-profile summaries. Identification panels must ignore input,
  remain above extra dimming, and disappear after three seconds. Repeat the
  action before expiry, open a native menu, unplug a screen, and quit during
  identification; no stale panel should remain. Not run on the host.

### Read-only inventory timing

On 2026-10-11, 20 consecutive inventory reads with the host's built-in display
measured approximately 2.04 seconds at the slowest (initial) read and 0.39 ms at
the median. This is a local observation, not a bound for external display setups.
The UI coalesces concurrent reads and the backend performs them off the main
thread. Reproduce without changing display settings using:

```sh
mise exec -- cargo test -p tomari-app \
  display_management::native::tests::read_only_inventory_latency \
  -- --ignored --nocapture
```

For longer resource checks, the read-only soak test samples the inventory once
per second and reports successful reads, read errors, maximum read latency, and
peak resident-memory growth after warm-up. Its default duration is one minute;
the environment variable accepts 10–21,600 seconds. For a three-hour run:

```sh
TOMARI_DISPLAY_SOAK_SECONDS=10800 mise exec -- cargo test -p tomari-app \
  display_management::native::tests::read_only_inventory_soak \
  -- --ignored --nocapture
```

Run this in a graphical macOS session. The test never changes display settings.
It requires at least one successful sample and limits peak resident-memory
growth to 64 MiB; this detects substantial retained allocations, not every leak.
