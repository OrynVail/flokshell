# PROJECT KNOWLEDGE BASE

**Framework:** QtQuick / Quickshell · **Language:** QML / JavaScript

## OVERVIEW
A Wayland shell for Hyprland, hard-forked from [Ambxst](https://github.com/Axenide/Ambxst).
There is no bar and no dock in use — the notch is the shell. Everything is driven by a
reactive JSON config and rendered per-screen via `Variants` on `Quickshell.screens`.

This is a hand-rolled config. Keep it that way: minimal comments, no generated-looking
scaffolding, no explanatory prose in the source.

## RUNNING IT
```bash
./cli.sh                      # from the repo root — execs qs -p ./shell.qml
```
Quickshell hot-reloads on file save, so edits apply without a restart. Reloading resets
`Visibilities.currentActiveModule`, which closes whatever panel was open.

For a full restart, the socket must be cleared or the shell comes up crippled:
```bash
pkill -f 'quickshell -p .*flokshell'; pkill -x axctl
rm -f /tmp/axctl-1000.sock
./cli.sh
```
`AxctlService` spawns the `axctl` daemon but never restarts it after a failed start, while
`axctlSubscribe` retries forever — so a stale socket means a permanent error loop, no
compositor state, and a shell that otherwise looks fine. Verify with `pgrep -af axctl`:
you want both a `daemon` and a `subscribe`.

Driving it without touching the UI:
```bash
echo dashboard > /tmp/flokshell_ipc.pipe    # toggles; see GlobalShortcuts.run()
grim -o <output> shot.png                # multi-monitor: pass the output name
```

## STRUCTURE
```
./
├── config/               # Config singleton + JSON defaults (see config/AGENTS.md)
│   └── defaults/*.js     # Blueprint per config domain — MUST match Config.qml
├── modules/
│   ├── bar/              # Notch widgets: clock, systray, workspaces, battery, layout
│   ├── components/       # UI primitives + GLSL shaders
│   ├── corners/          # Rounded screen corners overlay
│   ├── desktop/          # Desktop background + icon grid
│   ├── dock/             # App dock (present, disabled by default)
│   ├── frame/            # Screen border/glow effect
│   ├── globals/          # GlobalStates.qml — transient runtime state
│   ├── lockscreen/       # WlSessionLock + PAM
│   ├── notch/            # The shell surface: StackView, reveal, notifications
│   ├── notifications/    # Popup system + history
│   ├── services/         # Backend singletons (39)
│   ├── shell/            # UnifiedShellPanel + ReservationWindows + OSD
│   ├── theme/            # Colors, Icons, Styling singletons
│   ├── tools/            # Screenshot, recording, mirror, colour picker
│   └── widgets/          # dashboard, launcher, overview, powermenu, tools, defaultview
├── assets/               # Wallpapers, colour presets, sounds
├── scripts/              # Python/Bash backends
├── flokd/                # Rust service daemon (see flokd/AGENTS.md)
├── nix/                  # Flake packages and module definitions
├── shell.qml             # Entry point
└── cli.sh                # Launch wrapper and IPC controller
```

`modules/bar/` is a misnomer kept for import stability — the bar was removed, but its
widgets live on inside the notch and everything imports `qs.modules.bar.*`.

The service layer is being moved into `flokd`, a Rust daemon that owns fetching, parsing
and system integration so QML is left with the rendering. A ported service keeps its
singleton in `modules/services/` with the same property names and becomes a socket client,
so nothing downstream changes. Ported so far: `WeatherService`, `NetworkService`, `BluetoothService`, `SystemResources`, `Brightness`.

## WHERE TO LOOK
| Task | Location |
|------|----------|
| Entry point | `shell.qml` |
| Config logic | `config/Config.qml` (3469 lines) |
| Transient state | `modules/globals/GlobalStates.qml` |
| Theme / colours | `modules/theme/Colors.qml`, watches `~/.cache/flokshell/colors.json` |
| Layout tokens | `modules/theme/Styling.qml` — `control`, `glyph`, `gutter`, `tight` |
| UI primitives | `modules/components/StyledRect.qml` |
| Notch | `modules/notch/Notch.qml`, idle content in `modules/widgets/defaultview/` |
| Dashboard | `modules/widgets/dashboard/` |
| Dashboard sizing | `modules/widgets/dashboard/DashboardView.qml` — every dimension derives here |
| Compositor | `modules/services/AxctlService.qml` |
| IPC commands | `modules/services/GlobalShortcuts.qml` — `run()` |
| Adding config | `config/defaults/*.js` **and** `Config.qml` — both, always |

## CODE MAP
| Symbol | Location | Role |
|--------|----------|------|
| `Config` | `config/Config.qml` | Central config store, reactive to disk |
| `GlobalStates` | `modules/globals/GlobalStates.qml` | Shared runtime state, non-persistent |
| `Visibilities` | `modules/services/Visibilities.qml` | Active module / layering per screen |
| `Colors` | `modules/theme/Colors.qml` | Dynamic palette |
| `Styling` | `modules/theme/Styling.qml` | `radius()`, layout tokens, variants |
| `Icons` | `modules/theme/Icons.qml` | Phosphor-Bold character map |
| `StyledRect` | `modules/components/StyledRect.qml` | Base themed container |
| `AxctlService` | `modules/services/AxctlService.qml` | Compositor abstraction |
| `UnifiedShellPanel` | `modules/shell/UnifiedShellPanel.qml` | Full-screen `PanelWindow` |

## CONVENTIONS
- **Singletons**: `pragma Singleton` + `Singleton { id: root }`.
- **Imports**: `import qs.modules.*` — a Quickshell VFS construct, not a real directory.
- **Persistence**: `FileView` watches JSON; `JsonAdapter` binds it bidirectionally.
- **Formatting**: 4-space indent.
- **Multi-monitor**: `Variants { model: Quickshell.screens }`.
- **StyledRect variants**: `"pane"`, `"popup"`, `"common"`, `"internalbg"`, `"focus"`,
  `"transparent"`. The deboxed look uses `"transparent"` at rest with dimmed idle glyphs.
- **Null safety**: null-check nested properties; config reads can land before load.
- **Bulk config**: `root.pauseAutoSave` when writing several keys at once.
- **Async safety**: `Qt.callLater()` when modifying lists inside process handlers.
- **Launching apps**: always `DesktopService.runInActiveWorkspace()`; a second copy of that
  spawn line is a second answer.

## ANTI-PATTERNS
- Hardcoding colours or sizes instead of `Config.*`, `Colors.*`, `Styling.*`.
- Adding a config key without a matching default in `config/defaults/*.js`.
- Modifying `Config` properties outside the `JsonAdapter` binding system.
- Raw `Rectangle` containers — use `StyledRect` with a variant.
- Using `JSON.parse()` results in `Connections`; they carry no QML signals.
- Adding properties to `root` in `shell.qml` — use `GlobalStates`.

## NOTES
- Large files worth care: `ClipboardTab` (3615), `NotesTab` (3505), `TmuxTab` (2250),
  `BindsPanel` (2067), `ShellPanel` (1695), `ThemePanel` (1564), `Wallpaper` (1452).
- `axctl` is an external Go binary from upstream, not built here.
- Components that take a `bar` property want an object with `orientation` and
  `barPosition`. The notch and dashboard pass small `QtObject` stubs.
