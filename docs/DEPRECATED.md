# Deprecated

Features we decided to drop. One line each: what it was and why it went. We do not keep archived copies of old
docs; git has them. When a feature is dropped, add a line here in the same change that removes it from the code,
the rules and `docs/GLOSSARY.md`. Entries marked "still in code" are decided but not yet removed.

- **Sideways kick** (still in code): the pellet fan was shifted `(spread - base) / 2` to alternating sides after
  movement and recoil. A bot can compute it and aim the other way, so it penalised nothing. Replaced by startup
  time from unsteadiness (`docs/informal/vision-feedback.md`, 3.1).
- **`move_spread`** (still in code, `rules/weapons.toml`, `WeaponRules`): extra spread per unit of speed. Movement
  is paid for in startup time only; the field is to be removed, not left at zero.
- **Seed coin flip tie-break** (still in code, `economy::tiebreak`): a game level on rounds, kills and damage
  after the last overtime pair was decided by `seed & 1`. It becomes a drawn game.
- **Per-unit vision** (still in code, `Unit.seen`): each unit saw only through its own eyes, so teammates relayed
  sightings through team memory. Vision and heard sounds become shared by the team.
