# flokshell

A Wayland shell for Hyprland, built on Quickshell. A hard fork of
[Ambxst](https://github.com/Axenide/Ambxst).

Named after my cat.

---

## Credit

**Ambxst is [Axenide](https://github.com/Axenide)'s work, and it is most of what runs here.**
The architecture, the reactive JSON configuration, the panel system, the lockscreen, the
widgets — his. I did not have to reinvent this wheel because he had already made it, and made
it well.

| | |
|---|---|
| Original project | **[Axenide/Ambxst](https://github.com/Axenide/Ambxst)** |
| Author | **Adriano Tisera (Axenide)** — [GitHub](https://github.com/Axenide), [socials](https://zaap.bio/Axenide) |
| Support him | **[Ko-fi](https://ko-fi.com/Axenide)** |
| Community | **[Discord](https://axeni.de/discord)** |
| Licence | **AGPL-3.0**, upstream copyright intact in [LICENSE](LICENSE) |

**For what this shell is, read [Ambxst's README](https://github.com/Axenide/Ambxst).** It is the
better document and nothing about it is repeated here. Only the NixOS path is written up below,
because that one is mine and his does not cover it.

---

## Why the fork

Ambxst is built to be extremely customisable. That is the right goal for it and it is not mine.

I wanted one opinionated configuration instead of a preset library — so a hard fork was more
honest than a pile of local overrides losing a fight with upstream on every pull. Gone so far:
the AI integration and its sidebar, the bundled theme presets, the bar, the duplicate media
players, and the config surface that existed to carry all of it. The design language is being
replaced rather than re-skinned.

---

## Weight

It does less, so it holds less.

| | Ambxst 1.1.5 | flokshell |
|---|---|---|
| QML files | 216 | 181 |
| Lines of QML | 78,160 | 66,397 |

Three inefficiencies upstream still carries are fixed here. None of them change behaviour:

- The notch built its dashboard, launcher, powermenu and tools views on first open and never
  released them — per monitor. They are released now once closed.
- Seventeen images decoded at full source resolution to fill much smaller items. A 2560×1440
  image costs 14.7 MB of memory whether or not you can see it at that size.
- Every config property change rewrote a whole JSON file atomically. Dragging one slider was a
  file rewrite per frame. Writes coalesce now.

Idle footprint went from 601 MB resident to 510 MB, and 411 MB private dirty to 326 MB, on two
monitors with nothing open.

I have not benchmarked Ambxst, so there is no honest speed figure to quote against it. Those
numbers are this shell before and after, one sample each, on my machine. The three fixes are
things I read in upstream's source, not things I measured it doing.

---

## Install

NixOS only. There is no general installer here — for anything else, Ambxst's is the one to use.

### Flake

```nix
{
  inputs.flokshell.url = "github:OrynVail/flokshell";

  # in your nixosSystem modules list
  flokshell.nixosModules.default
}
```

That brings the package, the fonts, and `flokd` as a systemd user service. Then start the shell
from your compositor's autostart — the command is `flok`.

### Profile

```bash
nix profile install github:OrynVail/flokshell
```

Enough to look at it, and easy to undo.

> [!IMPORTANT]
> The profile install is the package alone. `flokd` gets no service, so whatever it owns —
> currently the weather — stays empty and nothing restarts it. The recommended system services
> (`upower`, `power-profiles-daemon`, NetworkManager) are not enabled either; most setups
> already have them. Fonts are fine: the `flok` wrapper points fontconfig at its own bundle.

No config file is needed either way. The shipped defaults in
`assets/presets/Flokshell Default/` are the setup I actually run.

---

## Status

In development, continuously, with no end state in view.

The install notes above cover NixOS and nothing else, deliberately — that is the path I actually
run and can vouch for. Everything else still points at Ambxst.
