# Linux session and setup updates

This branch ports the reviewed session planner and setup to Linux 5.0.3. It
preserves the existing GNOME/Wayland portal capture, native window decorations,
local model paths, pinned llama.cpp runtime and task-provider integrations.

## Behavior

- Session requests use literal task/topic anchors and explicit user estimates.
  The host schedules every requested topic into available local calendar time,
  inserts breaks of 5–30 minutes and reports work that cannot fit. The ADDA
  regression uses four topics from 10:35–16:35, and a revision puts PLE first
  with 15-minute rests. Generic warm-ups cannot replace the requested work.
- A draft expires after 30 minutes. Only confirmation saves local events;
  conflicts and elapsed start times are revalidated. Failed persistence keeps
  the draft pending for a retry. No cloud calendar is written by this feature.
- Local planner state uses AES-256-GCM, a random nonce per write and a 32-byte
  key held in the desktop Secret Service. SQLite contains ciphertext. A locked
  or unavailable keyring produces a visible error; plaintext fallback is absent.
- Setup offers four optional steps supported by the Linux host. The planner
  illustration contains the Break label inside its block. The reminder example
  uses shipped notification copy with a fictional task and is labelled Example.
  Rendering it requests no permission and sends no notification.
- Reminder and contextual-notification consent are separate. Tracking start,
  pause and stop activate/deactivate the native detector. The selected task is
  transient; recorded foreground identity is filtered against exclusions.
- Optional weekly PDF scheduling saves existing report output to a chosen local
  folder, at most once per ISO week while the app runs, with a bounded retry.
- The Linux UI has no Notion feature. Existing auth/task integrations remain.
- Dark button hovers slightly increase brightness; no pale hover fills are added.

## Verification

The `Linux session and UI verification` workflow runs on Ubuntu 22.04. It checks
native Rust targets, the planner/persistence module format, Clippy, library tests,
an isolated real Secret Service encryption/decryption test, renderer tests, Vite
production build, native desktop binary linkage and MCP STDIO startup.

Chromium checks the actual renderer with synthetic native responses at 370×700,
340×400 and 900×800. It verifies label containment, generic/contextual previews,
independent consent, four-step completion, dark hover brightness, and no implicit
tracking or calendar writes. The ADDA UI replays a saved local Qwen result for
four exercises and three rests, including PLE-first/15-minute revision and
unfitted-work messages. The fixture is evidence of that specific local response,
not a new Linux model run.

This validation does not exercise live Wayland screen capture or real Qwen
inference on a Linux desktop, and does not build or publish a release installer.
Compile-only model placeholders cannot be used as a distributable package.

PR: https://github.com/Mancasvel/FlowSight_linux/pull/6
