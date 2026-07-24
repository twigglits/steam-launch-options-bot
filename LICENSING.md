# Licensing

steamtrain ships as two packages under two different licences. This is
deliberate, and worth understanding before you vendor, fork, or repackage it.

| Package | Source | Licence |
| --- | --- | --- |
| `steamtrain` (Core: CLI, rule engine, systemd units) | `steamtrain/`, `systemd/`, `install.sh`, `uninstall.sh`, `scripts/` | **MIT** |
| `steamtrain-gui` (settings window, tray applet) | `steamtrain_gui/` | **GPL-3.0-or-later** |

## Why they differ

The GUI links PyQt6, which is offered under GPLv3 or a commercial licence.
Distributing a GPL-linked application means the combined work is distributed
under the GPL, so `steamtrain-gui` is GPL-3.0-or-later.

PySide6 would have been preferable — it is LGPL, and would have let the whole
project stay permissive. It was rejected on availability: PySide6 is not in
Ubuntu 24.04 LTS, which is the single largest desktop Linux target and is
supported until 2029. PyQt6 is present in the stock repositories of every
distribution steamtrain targets.

## What this means in practice

- **The Core stays MIT and dependency-free.** It imports only the Python
  standard library, and CI fails the build if that ever stops being true. You
  can vendor, embed, or relicense it exactly as before.
- **No GPL code may be imported into the Core.** The GUI reaches the Core by
  executing `/usr/bin/steamtrain`, never by importing it. CI enforces that too.
  The boundary is a licensing boundary as much as an architectural one.
- **Installing the CLI does not pull in any GPL code**, or any Qt. That is the
  main reason the two packages are separate rather than one.

MIT-licensed code is GPL-compatible, so combining the Core into the GPL-licensed
GUI package is lawful; the combination is what carries the GPL, not the Core
itself.

## If PySide6 becomes universally available

Migrating the GUI to PySide6 and relicensing it as LGPL is a desirable
follow-up, and the architecture keeps that door open: the GUI is a separate
package that talks to the Core over a process boundary, so it can be replaced
wholesale without touching anything else.

Full texts: [`LICENSE`](LICENSE) (MIT) and
[`packaging/LICENSE.gui`](packaging/LICENSE.gui) (GPL-3.0).
