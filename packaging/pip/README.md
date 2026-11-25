# imperial-battlecode

The Imperial Battlecode command line, `ibc`, as a pip package.

```
pip install imperial-battlecode
ibc version
ibc run my-bot other-bot --map maps/crossfire.ibcmap --seed 1 -o out/run
ibc replay verify out/run/replay.ibcr
```

The wheel contains one native binary and nothing else: no Python code runs, and Python is only the
installer. It is the same binary as in the release archives, and the same one the match workers run, so a
match you run locally gives the result the ladder would give for the same patch, map, bots and seed.

`ibc version` prints the patch identity (engine version, ruleset hash, Wasmtime version, ABI version). A
replay can only be re-verified by a build with the same patch identity; install an older version of this
package to check an older replay: `pip install imperial-battlecode==<engine version>`.

Building bots also needs a compiler for the bot's language; for Rust: `rustup target add wasm32-wasip1`.

## For maintainers

Wheels are built by `.github/workflows/release.yml` for the six release targets with
[maturin](https://www.maturin.rs) (`bindings = "bin"`), and attached to the draft GitHub Release. Nothing
is uploaded to PyPI by CI; publishing is a manual step in `docs/release.md`.

```
cd packaging/pip
maturin build --release --locked          # wheel for this machine, in target/wheels
```
