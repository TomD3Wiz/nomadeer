# Nomadeer Agent Notes

## Product/UI direction

- Nomadeer is a gameplay companion first, not a data-entry app.
- Prefer game-facing language over implementation details. Do not surface storage/auth internals like "tokens are stored in SQLite" in normal UI copy.
- The app should complement EVE gameplay by reacting to character state, movement, selected systems, and imported/pulled intel.
- Manual forms should not dominate the primary UI. Keep manual entry secondary or absent unless specifically requested.
- The sidebar + tabbed layout is preferred over a top-down dashboard-only layout.
- Use clear user-facing labels: "Dashboard", "Selected System", "Characters", "Movement", "Connections".
- Avoid architecture-focused labels like "Reactive dashboard"; players do not care whether a panel is static/reactive.
- Chain map should be interactive: selecting a system in the map should drive the selected-system sidebar.
- On launch, default the selected system to something sensible, preferably the first character's current system.
- Character names and system names should use standardized theme colors from config.
