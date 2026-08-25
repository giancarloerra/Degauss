# Contributing

## Before writing code

Open an issue first for anything more than a small fix. Degauss is
deliberately small, and the fastest way to have work turned down is to write
it before agreeing what it should do.

## The gates

Every change has to pass all three, and CI runs the same ones:

```bash
cargo fmt --check
cargo test
cargo clippy --target armv7-unknown-linux-musleabihf --all-targets -- -D warnings
```

Clippy is run **for the ARM target**, not the host. The device is what the
program runs on, and a host run reports dead code that only looks dead off
the device.

Anything that changes what is drawn should be looked at on a CRT at 352x240,
which is what Degauss is built for. `--render` writes a frame to an image
without taking the screen, so this does not need a machine in front of you.

## Tests

A test should say why the behaviour matters, not only what it does. A test
that cannot fail when the logic changes is not doing anything.

## Licence

Degauss is under the [PolyForm Noncommercial License 1.0.0](LICENSE) and
contributions are accepted under that same licence.

`MiSTer_Degauss` is a **separate GPLv3 program** with its own repository,
[Degauss-Main](https://github.com/giancarloerra/Degauss-Main). Changes to it
go there, under GPLv3. The two are kept apart on purpose: they are separate
programs that talk through files, which is what lets each keep its own
licence.
