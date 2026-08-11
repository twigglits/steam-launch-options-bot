# Licensing

steamtrain ships as two packages under two different licences. This is
deliberate, and worth understanding before you vendor, fork, or repackage it.

| Package | Source | Licence |
| --- | --- | --- |
| `steamtrain` (Core: CLI, rule engine, systemd units) | `src/`, `Cargo.toml`, `systemd/`, `install.sh`, `uninstall.sh`, `scripts/` | **MIT** |
| `steamtrain-gui` (settings window) | `steamtrain_gui/` | **GPL-3.0-or-later** |

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

- **The Core stays MIT and dependency-light.** It depends on five crates from
  a committed allowlist — `clap`, `serde`, `serde_json`, `shlex` and `ureq`,
  each `MIT OR Apache-2.0` — and CI fails the build if a sixth appears. You can
  vendor, embed, or relicense it exactly as before.
- **Static linking makes the transitive tree a licensing question, not just a
  supply-chain one.** The shipped binary contains its dependencies rather than
  loading them, so their terms travel with it. Every crate in the tree is
  permissive (MIT, Apache-2.0, ISC, BSD-3-Clause, Unicode-3.0,
  CDLA-Permissive-2.0); none is copyleft. Check with `cargo metadata` before
  adding anything, and keep it that way — a single GPL crate anywhere in the
  graph would relicense the Core.
- **No GPL code may be linked into the Core.** The GUI reaches the Core by
  executing `/usr/bin/steamtrain`, never by importing it — and now cannot, the
  two being different languages. CI enforces that too. The boundary is a
  licensing boundary as much as an architectural one.
- **Installing the CLI does not pull in any GPL code**, or any Qt. That is the
  main reason the two packages are separate rather than one.

MIT-licensed code is GPL-compatible, so combining the Core into the GPL-licensed
GUI package is lawful; the combination is what carries the GPL, not the Core
itself.

## Replacing the GUI

Migrating the GUI off PyQt6 and relicensing it — to PySide6 if that ever
becomes universally available, or to a non-Qt toolkit — would let the whole
project be permissively licensed. The architecture keeps that door open, and
the Core being Rust does not narrow it: the GUI is a separate package that
talks to the Core over a versioned process protocol, so it can be replaced
wholesale, in any language, without touching anything else.

Full texts: [`LICENSE`](LICENSE) (MIT) and
[`packaging/LICENSE.gui`](packaging/LICENSE.gui) (GPL-3.0).
