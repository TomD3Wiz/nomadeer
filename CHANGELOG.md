# Changelog

Everything that changes in Nomadeer from one release to the next. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and Nomadeer
uses [semantic versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-10-18

The first release of Nomadeer, a companion that rides alongside your EVE
Online session, maps your wormhole chain as you jump and briefs you on the
systems you pass through.

### Added

- Sign in your characters with EVE single sign-on, using Nomadeer's own EVE
  application, and see where each of them is and whether they are online.
- A chain map that builds itself as your characters jump. Zoom and pan around
  it, fit the chain to the window, drag systems where you want them, drag from
  one system to another to link them, and resize the map against the tabs
  below it. Save and restore chains as snapshots.
- Wormhole and system artwork on the map: wormholes swirl in their class
  colour (C1 cool blue through C6 red, with their own looks for Thera,
  shattered and Drifter systems), known space shows its security band and a
  badge marks the NPC faction that holds it. The **Legend** explains every
  look.
- Wormhole types and state on every connection: set its code (B274, H296, ...)
  with suggestions for where it leads, mark it end of life, mass reduced or
  critical, and see its size, mass, lifetime and roughly when it closes.
- Probe scans pasted straight from the game: copy the probe scanner's rows and
  press **Ctrl+V** to add and update the selected system's signatures.
  Signatures that dropped off the scanner are offered for removal, never
  removed on their own.
- Local and d-scan pasted the same way as live intel. Local shows who is in
  system by alliance and corporation and flags pilots who got kills there
  recently; the d-scan is summed up by ship class and calls out interdictors,
  recons, covert ships, strategic cruisers, capitals, combat probes and
  bubbles. Each paste counts towards the system's assessment for 15 minutes.
- Threat, activity and confidence assessments for every system, built from
  recent kills, live jump traffic and the system's usual traffic at this time
  of day, with the killboard history and full kill reports in the app.
- Automatic intel: when a recon pilot arrives somewhere new, Nomadeer gathers
  intel on the system by itself. Choose up to three recon pilots; arrivals
  only count while one of them is logged in.
- Umbra, an optional on-device intel companion. Install her core once and she
  briefs you on each new system as soon as its intel is in, without any AI
  service or account. Her panel sits beside the dashboard tabs with her
  portrait and a feed of every briefing she has written.
- Downloads for Windows (installer or portable zip) and Linux (AppImage,
  Flatpak or plain tarballs). Each comes in an optimized edition for newer
  processors, where Umbra briefs two to three times faster, and a compatible
  edition for any 64-bit PC; the installer, AppImage and Flatpak pick the
  right one for you.
- Update checks: Nomadeer looks on GitHub once a day for a new release and
  tells you when one is out. Nothing is installed until you press **Update**,
  and the check can be turned off in settings. The Windows installer and the
  AppImage then update themselves: the download is checked against its
  published checksum, installed and Nomadeer restarts, keeping your data.

[Unreleased]: https://github.com/TomD3Wiz/nomadeer/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/TomD3Wiz/nomadeer/releases/tag/v0.1.0
