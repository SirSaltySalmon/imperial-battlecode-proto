# Deprecated

Features we decided to drop. One line each: what it was and why it went. We do not keep archived copies of old
docs; git has them. When a feature is dropped, add a line here in the same change that removes it from the code,
the rules and `docs/GLOSSARY.md`. Entries marked "still in code" are decided but not yet removed.

- **Sideways kick**: the pellet fan was shifted `(spread - base) / 2` to alternating sides after
  movement and recoil. A bot can compute it and aim the other way, so it penalised nothing. Replaced by startup
  time from unsteadiness (`docs/informal/vision-feedback.md`, 3.1).
- **`move_spread`** (`rules/weapons.toml`, `WeaponRules`): extra spread per unit of speed. Movement is paid for in
  startup time only. Removed from the rules; its ABI slot is `WeaponRules._reserved0`, always 0.
- **Seed coin flip tie-break** (`economy::tiebreak`): a game level on rounds, kills and damage
  after the last overtime pair was decided by `seed & 1`. It is a drawn game now.
- **Per-unit vision** (`Unit.seen`): each unit saw only through its own eyes, so teammates relayed sightings
  through team memory. Vision and heard sounds are shared by the team (`World.views`).
- **Effect arming** (`AbilityRules.delay` as arming time): a device lay in the world for `delay` ticks before it
  acted. `delay` is now the user's startup (`S_CASTING`, hands busy, `A_CANCEL` abandons it) and an effect acts
  from the tick it appears.
