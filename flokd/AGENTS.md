# FLOKD

**Language:** Rust · **Runtime:** tokio (current_thread)

The service daemon. Owns everything between the system and the pixels — fetching, D-Bus,
procfs, Wayland protocols — so the QML is left rendering. Runs as a systemd user unit
(`nix/modules/default.nix`), never spawned by the shell.

## STRUCTURE
```
src/
├── main.rs              # socket server, client fan-out, command dispatch
├── bus.rs               # latest-state-per-service + broadcast to clients
├── config.rs            # reads ~/.config/flokshell/config/*.json, watches for changes
├── dbus.rs              # shared system-bus helpers: calls, property reads, owner watch
├── ddc.rs               # DDC/CI over i2c-dev: EDID, VCP get/set
├── http.rs              # shared client builder, size-capped GET
├── notify.rs            # desktop notifications and their action clicks, session bus
├── wayland.rs           # one compositor connection: outputs, gamma, idle, screencopy; reconnects
└── services/
    ├── bluetooth.rs     # BlueZ: adapter, devices, discovery, connects
    ├── brightness.rs    # backlight through logind, external displays over DDC/CI
    ├── capture.rs       # frozen frames in memory: save, OCR, QR, Lens, colour pick
    ├── coverart.rs      # MPRIS art: URL, tags, sidecar, online; content-addressed cache
    ├── idle.rs          # idle listeners, caffeine, logind lock and sleep hooks with a delay lock
    ├── linkpreview.rs   # Open Graph scrape, YouTube and X oEmbed
    ├── network.rs       # NetworkManager over the system bus: state, scans, connects
    ├── nightlight.rs    # wlr-gamma-control at a fixed 4500 K
    ├── resources.rs     # CPU, RAM, GPU (NVML, amdgpu sysfs), disks; sampled under a lease
    ├── thumbs.rs        # wallpaper/desktop thumbnails, lockscreen frame; ffmpeg for video only
    └── weather.rs       # open-meteo + geocoding + geoip fallback chain
```

## PROTOCOL
Unix socket at `$XDG_RUNTIME_DIR/flokd.sock`, newline-delimited JSON.

Daemon to client:
```json
{"service":"weather","event":"state","data":{…}}
```
Client to daemon:
```json
{"service":"weather","cmd":"refresh"}
{"service":"network","cmd":"connect","args":{"ssid":"home"}}
```
`args` is optional and belongs to the service: `main.rs` routes on `service` alone and each
service parses its own commands, so adding one never touches dispatch.

Every event is a **full-state replace for one service**, so receiving the same state twice is
harmless. That is what lets a connecting client subscribe first and take the snapshot second
with no gap, and lets a lagging client be resynced instead of replayed.

The QML side reads it with `SplitParser { splitMarker: "\n" }` on a `Socket` and writes
commands with `Socket.write`. New clients use `modules/services/FlokdSocket.qml`.

## ADDING A SERVICE
1. `src/services/<name>.rs` with a `State` that is `Serialize + Deserialize + Default`.
2. Publish through `Bus::publish("<name>", &value)` on every change.
3. Register a command channel in `main.rs` if the shell needs to poke it.
4. Rewrite the QML singleton as a socket client **keeping every property name**, so the
   consumers never learn anything changed.
5. Delete the shell/Python script it replaces. A service still spawning one is not ported.

## CONVENTIONS
- Canonical units on the wire (Celsius, km/h, bytes). Display units are the shell's problem.
- Push, don't poll. A `Timer` here means no signal was found, and gets a comment saying so.
- Never spawn a process to ask what a bus will answer.
- Anything that parses or marshals gets tests. This is where tests live; the shell has none.
- Comments state a policy and when to revisit it, or they do not exist.

## ANTI-PATTERNS
- Watching a config **file** instead of its directory — the shell saves atomically
  (temp + rename), so a file watch survives one save and then goes silent forever.
- Rebuilding part of a cached state from another part. Cache what you will serve.
- Reading config the shell could have sent, or sending what the shell could have computed.

## RUNNING IT
```bash
cargo run                       # inside `nix develop`
FLOKD_LOG=debug cargo run        # per-module filtering via tracing EnvFilter
cargo test
```
