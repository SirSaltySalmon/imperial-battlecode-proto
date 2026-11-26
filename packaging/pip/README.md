# imperial-battlecode

The Imperial Battlecode command line, `ibc`, as a pip package.

```
pip install imperial-battlecode
ibc version
ibc run my-bot other-bot --map maps/crossfire.ibcmap --seed 1 -o out/run
ibc replay verify out/run/replay.ibcr
ibc view out/run/replay.ibcr          # the replay viewer, served on localhost
ibc update                            # is this the version the server runs? (needs IBC_SERVER)
```

The wheel contains one native binary plus data: the replay viewer, the map pool and the game docs, under
`<prefix>/share/imperial-battlecode/` (`ibc view` finds the viewer there). No Python code runs, and Python is
only the installer. It is the same binary as in the release archives, and the same one the match workers
run, so a match you run locally gives the result the ladder would give for the same patch, map, bots and
seed.

`ibc version` prints the patch identity (engine version, ruleset hash, Wasmtime version, ABI version). A
replay can only be re-verified by a build with the same patch identity; install an older version of this
package to check an older replay: `pip install imperial-battlecode==<engine version>`.

Staying current: with `IBC_SERVER` set, `ibc` asks the server once a day which version it runs and warns
when this one is older; `ibc update` prints the exact upgrade command. It never downloads anything by
itself. A workspace can pin the engine in `ibc.toml` (`[engine] version = "x.y.z"`); a different binary is
warned about on every command.

Building bots also needs a compiler for the bot's language; for Rust: `rustup target add wasm32-wasip1`.

## For maintainers

Wheels are built by `.github/workflows/release.yml` for the six release targets with
[maturin](https://www.maturin.rs) (`bindings = "bin"`), and attached to the draft GitHub Release. Nothing
is uploaded to PyPI by CI; publishing is a manual step in `docs/release.md`.

```
cargo xtask dist pip-data                 # stage viewer/dist, maps and docs into wheel-data/ (gitignored)
cd packaging/pip
maturin build --release --locked          # wheel for this machine, in target/wheels
```

`pip-data` needs a built viewer (`viewer/dist` with `engine/*.wasm`; see `viewer/README.md`), or
`--no-viewer` for a wheel without one.
