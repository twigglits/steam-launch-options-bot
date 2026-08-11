# steamtrain

[![CI](https://github.com/twigglits/steamtrain/actions/workflows/ci.yml/badge.svg)](https://github.com/twigglits/steamtrain/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.74%2B-orange.svg)](https://www.rust-lang.org/)
[![Release](https://img.shields.io/github/v/release/twigglits/steamtrain?sort=semver)](https://github.com/twigglits/steamtrain/releases/latest)

<p align="center">
  <img src="assets/mascot.svg" width="760" height="380" alt="Project mascot: a little steam locomotive whose boiler carries a brass steam gate-valve with a spoked handwheel, steam puffing from the smokestack and hissing from the valve as the driving wheels turn — a visual pun on Steam launch options.">
</p>

A systemd (user) service for Linux that scans your installed Steam games (only
game folders that actually exist on disk) and sets launch options appropriate
for **your** OS, desktop environment, and hardware.

Works across Ubuntu/Debian, Arch, and Fedora (and their derivatives) — anything
with a Linux kernel and, for automatic scheduling, a systemd user session.
Without a systemd user session the CLI still works; you just run
`steamtrain apply` yourself. A single statically linked binary with **no runtime
dependencies at all**. Fully offline.

## Why not just copy options from ProtonDB?

[ProtonDB](https://www.protondb.com) is the best community source for
per-game launch options, but every report comes from *someone else's*
hardware — an option that helps on a Steam Deck or an AMD APU can do nothing
(or harm) on an NVIDIA desktop. This tool inverts that: it detects your GPU
vendor/driver, session type (Wayland/X11), desktop, and installed helpers
(gamemode, MangoHud), and generates a conservative baseline per game. When
you find a game-specific tip on ProtonDB that you trust for your hardware,
put it in the config `overrides` (see below) and it takes precedence.

## What it sets (examples for an NVIDIA + Wayland + gamemode system)

| Game type | Generated launch options |
|---|---|
| Proton game | `PROTON_ENABLE_NVAPI=1 __GL_SHADER_DISK_CACHE_SKIP_CLEANUP=1 gamemoderun %command%` |
| Native game | `__GL_SHADER_DISK_CACHE_SKIP_CLEANUP=1 gamemoderun %command%` |

On AMD/Intel (Mesa) systems native GL games get `mesa_glthread=true` instead
of the NVIDIA variables. Rules are deliberately conservative: nothing that is
known to break games (e.g. it never forces `SDL_VIDEODRIVER=wayland`).

## Safety guarantees

- **Nothing runs on a schedule unless you switch it on.** No install path —
  package, `install.sh`, or the desktop interface — enables the timer for you.
  Until you opt in, steamtrain only ever runs when you run it, and the settings
  window states whether the timer is actually running at the top of the window.
- **Never overwrites options a human set.** It only writes when the current
  value is empty or byte-identical to what it wrote previously (tracked in
  `~/.local/state/steamtrain/state.json`). Your manual tweaks
  always win.
- **Never writes while Steam is running** (Steam would silently discard the
  change on exit). The timer just retries later.
- **Backs up** `localconfig.vdf` before every write (last 10 kept in the
  state dir) and replaces it atomically, preserving permissions.
- **Only touches games that exist on disk**: a game counts as installed only
  if its `appmanifest_*.acf` is present in a *mounted* library and
  `steamapps/common/<installdir>/` exists.
- `steamtrain revert` restores everything it manages back to empty.

## Install

### From a package (recommended)

Download the artifacts for your distribution from the
[latest release](https://github.com/twigglits/steamtrain/releases/latest). The
core package is a compiled binary and so is built per architecture — `amd64`
and `arm64` for deb, `x86_64` and `aarch64` for rpm:

```sh
sudo apt install ./steamtrain_*_amd64.deb ./steamtrain-gui_*_all.deb       # Debian, Ubuntu
sudo dnf install ./steamtrain-*.x86_64.rpm ./steamtrain-gui-*.noarch.rpm   # Fedora
```

On arm64, substitute `_arm64.deb` or `.aarch64.rpm` for the core package;
`steamtrain-gui` is still Python and ships a single artifact for every
architecture, hence `all`/`noarch`.

`steamtrain-gui` is the desktop interface — one settings window in your
application menu, no tray icon and no background process. It is
optional: drop that argument for a CLI-only install. It pins the core's exact
version, so install the pair in one command rather than one after the other.

The packages are statically linked, so there is no glibc floor and no
runtime dependency to satisfy. On Arch and other distributions without a
package, install from source (below).

**Installing a package does nothing on its own.** It writes no files into your
home directory and schedules nothing. Turn the timer on yourself, either in the
settings window or with:

```sh
systemctl --user enable --now steamtrain.timer
```

Restart Steam to see applied options take effect in the UI.

> Ubuntu 22.04 can install the CLI package but not `steamtrain-gui`: jammy has
> no `python3-pyqt6`.

### Already installed with `install.sh`?

An older `~/.local` install **silently wins** over a packaged one — `~/.local/bin`
comes before `/usr/bin` in `PATH`, and a user unit overrides the packaged one — so
the package you just installed would not actually be the code running. steamtrain
detects this and tells you. To clear it out:

```sh
steamtrain doctor          # report what is conflicting
steamtrain doctor --fix    # remove the old install
```

Your config and state are preserved; only the old executables and unit files
are removed.

### From source, or on other distributions

```sh
./install.sh
```

This builds with `cargo build --release`, installs the binary to
`~/.local/bin/steamtrain`, and writes the systemd **user** units without
enabling them. Like the packages, it schedules nothing: run `steamtrain apply`
yourself, or opt in to the timer (see below). Restart Steam to see applied
options take effect in the UI.

Building from source needs a Rust toolchain. If you would rather not install
one, every release also ships a prebuilt binary tarball alongside the
distribution packages.

`./install.sh --migrate` removes a previous `~/.local` install and exits
without installing anything, for switching to a distribution package.

The installer checks for `cargo` first (printing a per-distro install hint if
it is missing — `pacman`/`dnf`/`apt`, never run for you). If there is no
systemd user session it warns and skips the units, leaving a working CLI. When
run in a terminal it finishes by launching the hardware setup wizard (below);
piped/non-interactive installs skip it and print a reminder to run
`steamtrain setup`.

### Scheduling (opt-in)

Nothing runs on its own until you say so. Turn it on with the **Run
automatically** switch in the settings window, or:

```sh
systemctl --user enable --now steamtrain.timer   # 2 min after boot, then every 30 min
systemctl --user list-timers steamtrain.timer    # confirm it is really running
systemctl --user disable --now steamtrain.timer  # stop it
journalctl --user -u steamtrain.service -e       # what it did
```

The settings window reads the same state back from systemd rather than
remembering what it asked for, so a timer that was enabled but never started —
ticked box, no run — is reported as exactly that.

### Supported distributions

Ubuntu/Debian, Arch, and Fedora and their derivatives are all supported. The
packaged binary is statically linked against musl, so it carries no glibc
version requirement and runs the same on all of them; installing from source
needs only a Rust toolchain and a systemd user session for the timer.

```sh
./uninstall.sh        # run `steamtrain revert` first if you want options cleared
```

## CLI

```sh
steamtrain setup            # confirm detected hardware, or pick/clear the GPU vendor
steamtrain scan             # detected system profile + per-game proposals
steamtrain apply --dry-run  # what would change, writing nothing
steamtrain apply            # write (skipped safely if Steam is running)
steamtrain status           # what the tool currently manages
steamtrain revert           # restore managed options to empty
steamtrain doctor           # report install problems (exits 2 if any are unfixed)
steamtrain doctor --fix     # remove a conflicting old ~/.local install
```

`scan`, `apply`, `status` and `revert` also accept `--json`, which emits
newline-delimited JSON instead of text — one object per line, the last one
always a `result` record. That is how the desktop interface talks to the CLI,
and it is stable enough to script against:

```sh
steamtrain apply --dry-run --json | jq -c 'select(.kind == "change" and .action == "set")'
```

A run blocked because Steam is open still exits `0` — that is the expected
case, not a failure — and reports `"outcome": "blocked"` in the result record.

`steamtrain setup` (also run automatically at the end of an interactive
install) prints the autodetected hardware profile and asks you to confirm it
(`[Y/n]`, Enter accepts). Confirm and nothing is written. Disagree — e.g. a
hybrid-graphics laptop where the wrong GPU was detected — and it shows a
numbered menu: 1) NVIDIA 2) AMD 3) Intel 4) Autodetect (clear override)
5) Skip. Picks 1–3 save `gpu_vendor`; 4 clears it back to `""` so autodetection
governs again; 5 changes nothing. When autodetection *fails*, the confirm step
is skipped and the menu appears directly. Piped/non-interactive runs never
write: they accept the detection at the confirm prompt, and if detection failed
the menu treats end-of-input as Skip.

## Configuration

`~/.config/steamtrain/config.json` (created on first run):

```json
{
  "gpu_vendor": "",
  "enable_gamemode": true,
  "enable_mangohud": false,
  "enable_nvapi": true,
  "enable_shader_cache_skip_cleanup": true,
  "enable_mesa_glthread": true,
  "enable_proton_wayland": false,
  "overrides": {
    "292030": "{auto} -dx11"
  },
  "exclude": ["3744430"]
}
```

- `gpu_vendor` — force the GPU vendor (`nvidia` / `amd` / `intel`) when
  autodetection fails or picks the wrong one; `""` means autodetect (the
  default). Set it with `steamtrain setup`; an override wins over detection, an
  unrecognized value is ignored (with a warning) and autodetection is used.
  Existing config files without this key keep autodetecting.
- `enable_*` — toggle individual built-in rules.
- `overrides` — appid → launch options used verbatim; `{auto}` expands to the
  generated baseline. This is where ProtonDB-sourced, hardware-vetted tips go.
- `exclude` — appids the tool must never touch.

## Running as a root system service instead

A user unit is the right default (all Steam data is user-owned), but a
system-level variant works too — create
`/etc/systemd/system/steamtrain.service` with `User=<you>` and
`Environment=HOME=/home/<you>`, plus a matching timer, and point `ExecStart`
at `/home/<you>/.local/bin/steamtrain apply`.

## Development

```sh
cargo test                                  # the Core
cargo build                                 # the interface tests execute it
python3 -m unittest discover -s tests -v    # the desktop interface
```

The Core is Rust; the desktop interface is Python and reaches the Core by
executing it, never by importing it. `scripts/check-boundaries.py` enforces
both halves of that and runs in CI.
