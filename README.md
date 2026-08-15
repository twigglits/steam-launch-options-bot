# steamtrain

[![CI](https://github.com/twigglits/steamtrain/actions/workflows/ci.yml/badge.svg)](https://github.com/twigglits/steamtrain/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.74%2B-orange.svg)](https://www.rust-lang.org/)
[![Release](https://img.shields.io/github/v/release/twigglits/steamtrain?sort=semver)](https://github.com/twigglits/steamtrain/releases/latest)

<p align="center">
  <img src="assets/mascot.svg" width="760" height="380" alt="Project mascot: a little steam locomotive whose boiler carries a brass steam gate-valve with a spoked handwheel, steam puffing from the smokestack and hissing from the valve as the driving wheels turn — a visual pun on Steam launch options.">
</p>

A Linux tool that scans your installed Steam games (only game folders that
actually exist on disk) and sets launch options appropriate for **your** OS,
desktop environment, and hardware.

Works across Ubuntu/Debian, Arch, and Fedora (and their derivatives) — anything
with a Linux kernel. A single statically linked binary with **no runtime
dependencies at all**, plus an optional settings window. Fully offline, and it
only ever runs when you run it or while that window is open.

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

- **Nothing runs unless steamtrain is open.** Scheduled runs live in the
  settings window: while it is open, options are applied every 30 minutes for
  newly installed games; close the window and steamtrain is not running, full
  stop. There is no switch to get that wrong, no background service, no tray
  icon and no systemd timer — "is it running?" is answered by looking at your
  screen.
- **Never overwrites options a human set.** It only writes when the current
  value is empty or byte-identical to what it wrote previously (tracked in
  `~/.local/state/steamtrain/state.json`). Your manual tweaks
  always win.
- **Never writes while Steam is running** (Steam would silently discard the
  change on exit). A scheduled run just retries half an hour later.
- **Backs up** `localconfig.vdf` before every write (last 10 kept in the
  state dir) and replaces it atomically, preserving permissions.
- **Only touches games that exist on disk**: a game counts as installed only
  if its `appmanifest_*.acf` is present in a *mounted* library and
  `steamapps/common/<installdir>/` exists.
- `steamtrain revert` restores everything it manages back to empty.

## Install

### From a package (recommended)

Download the artifacts for your distribution from the
[latest release](https://github.com/twigglits/steamtrain/releases/latest):

```sh
sudo apt install ./steamtrain_*_amd64.deb ./steamtrain-gui_*_all.deb       # Debian, Ubuntu
sudo dnf install ./steamtrain-*.x86_64.rpm ./steamtrain-gui-*.noarch.rpm   # Fedora
```

x86_64 only, because Steam's Linux client is. `steamtrain-gui` is Python and
ships a single artifact for every architecture, hence `all`/`noarch`.

`steamtrain-gui` is the desktop interface — one settings window in your
application menu, no tray icon and no background process. It is
optional: drop that argument for a CLI-only install. It pins the core's exact
version, so install the pair in one command rather than one after the other.

The packages are statically linked, so there is no glibc floor and no
runtime dependency to satisfy. On Arch and other distributions without a
package, install from source (below).

**Installing a package does nothing on its own.** It writes no files into your
home directory, installs no service or timer, and schedules nothing. Open the
settings window and press **Apply now**, or run `steamtrain apply`. Restart
Steam to see applied options take effect in the UI.

> Ubuntu 22.04 can install the CLI package but not `steamtrain-gui`: jammy has
> no `python3-pyqt6`.

### Already installed with `install.sh`?

An older `~/.local` install **silently wins** over a packaged one — `~/.local/bin`
comes before `/usr/bin` in `PATH` — so the package you just installed would not
actually be the code running, and any systemd units that install left behind
would go on running the old binary. steamtrain detects this and tells you. To
clear it out:

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

This builds with `cargo build --release` and installs one binary to
`~/.local/bin/steamtrain`. Like the packages it schedules nothing: run
`steamtrain apply` yourself, or open the settings window (see below). Restart
Steam to see applied options take effect in the UI. If an earlier release of
this script left systemd user units behind, they are removed.

Building from source needs a Rust toolchain. If you would rather not install
one, every release also ships a prebuilt binary tarball alongside the
distribution packages.

`./install.sh --migrate` removes a previous `~/.local` install and exits
without installing anything, for switching to a distribution package.

The installer checks for `cargo` first (printing a per-distro install hint if
it is missing — `pacman`/`dnf`/`apt`, never run for you). When run in a
terminal it finishes by launching the hardware setup wizard (below);
piped/non-interactive installs skip it and print a reminder to run
`steamtrain setup`.

### Scheduling: the window is the switch

Open the settings window (`steamtrain-gui`, or *steamtrain* in your application
menu) and it applies options every 30 minutes for newly installed games, saying
so in the **Scheduled runs** row. Close the window and that stops — there is
nothing left behind to run.

There is deliberately no checkbox. steamtrain writes to your Steam
configuration, and a tool that does that should not be able to run at a moment
when nothing of it is visible; a switch would be a second answer to "is it
running?", and the kind that can be on while nothing happens. Open means
running, closed means not.

If you *want* runs without the window — a headless box, a machine you rarely
log into — schedule the CLI yourself. It is one line of crontab:

```sh
*/30 * * * * /usr/bin/steamtrain apply
```

That is your unit and your decision, which is the point: steamtrain does not
install one for you.

### Supported distributions

Ubuntu/Debian, Arch, and Fedora and their derivatives are all supported. The
packaged binary is statically linked against musl, so it carries no glibc
version requirement and runs the same on all of them; installing from source
needs only a Rust toolchain.

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

## Running it from a unit of your own

steamtrain ships no units, but the CLI is an ordinary one-shot program, so
writing one is easy if you want runs without the window. A **user** unit is the
right shape (all Steam data is user-owned): `ExecStart=%h/.local/bin/steamtrain
apply`, `Type=oneshot`, plus a `.timer` with `OnCalendar=*:0/30` and
`Persistent=true`. Use a calendar rather than a monotonic timer: a monotonic
pair enabled long after boot has both elapse points in the past and lands
straight in `active (elapsed)` — switched on, never firing.

A system-level variant works too: `/etc/systemd/system/steamtrain.service` with
`User=<you>` and `Environment=HOME=/home/<you>`.

## Development

```sh
cargo test                                  # the Core
cargo build                                 # the interface tests execute it
python3 -m unittest discover -s tests -v    # the desktop interface
```

The Core is Rust; the desktop interface is Python and reaches the Core by
executing it, never by importing it. `scripts/check-boundaries.py` enforces
both halves of that and runs in CI.
