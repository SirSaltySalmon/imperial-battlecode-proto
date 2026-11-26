# Economy and match flow

The exact rules behind the "Economy" and "Match flow" sections of [`docs/RULES.md`](../RULES.md). Every number
quoted here is the current value in [`rules/economy.toml`](../../rules/economy.toml) (or `rules/core.toml` /
`rules/weapons.toml` where noted); the TOML is the source of truth. Bots see prices through `T_ITEM`, `T_WEAPON`
and `T_ABILITY`, and their own balance in `UnitInfo.credits`.

## Credits

- Credits belong to a **unit**, not to a team. There is no pooling and no gifting.
- A unit never holds more than `max_credits` (8000). Income above the cap is lost.
- Everyone starts each half with `start_credits` (800).

## Income

| Event | Who is paid | Amount |
|---|---|---|
| Round won | every unit of the winning team | `win_reward` = 3000 |
| Round lost | every unit of the losing team | `loss_reward` + `loss_streak_bonus` x streak = 1400 / 1900 / 2400 / 2900 |
| Kill with a weapon | the killer | that weapon's `kill_reward` (pistol 300, rifle 200) |
| Kill without a weapon (ability damage) | the killer | `ability_kill_reward` = 200 |
| Plant | every attacker, at the moment of the plant, win or lose | `plant_reward` = 300 |
| Defuse | every defender, on top of the win reward | `defuse_reward` = 300 |

- **Loss streak.** "Streak" is the number of rounds the team had already lost in a row before this one, capped at
  `loss_streak_cap` (3). A first loss pays 1400, the fourth and later consecutive losses pay 2900. Winning a
  round sets the streak back to zero. Streaks are also cleared at half-time.
- Round income is paid when the round ends, to living and dead units alike. Kill and plant income is paid at
  once, but it can only be spent in the next buy phase.

## The shop

`A_BUY(item)` works in the buy phase only and applies immediately. Items are the rows of `T_ITEM`; ids are
positions in the `[[item]]` list and are append-only.

| id | Item | Kind | Price | Effect |
|---|---|---|---|---|
| 0 | `rifle` | weapon | 2700 | primary weapon, full magazine |
| 1 | `light_armor` | armour | 400 | armour set to 25 |
| 2 | `heavy_armor` | armour | 1000 | armour set to 50 |
| 3 | `defuse_kit` | defuse kit | 400 | defuse in `defuse_kit_ticks`; defenders only |
| 4 | `pistol` | weapon | 0 | the default sidearm (only useful to trade back to it) |
| 5 | `ability_charge_0` | ability charge | 0 + ability price | one charge of the ability in kit slot 0 |
| 6 | `ability_charge_1` | ability charge | 0 + ability price | one charge of the ability in kit slot 1 |

Checks run in this order, and a refused purchase costs nothing:

1. `ERR_BAD_ARG`: no such item (or the item names a weapon that does not exist).
2. `ERR_NOT_ALLOWED`: you cannot own it. You already hold that weapon; your armour is already at or above the
   item's value; you already have a kit or you are attacking; your kit has no ability in that slot.
3. `ERR_INSUFFICIENT_FUNDS`: you cannot pay for it (after any trade-in, below).
4. For ability charges only, and only once the price is covered: whatever the abilities module answers (for
   example `ERR_NOT_ALLOWED` at `max_charges`). A unit that is both short of credits and at its charge limit
   is told `ERR_INSUFFICIENT_FUNDS`.

Details:

- **Weapons** go into the weapon's own slot (0 primary, 1 sidearm) and replace what was there. The replaced
  weapon is gone; it is not dropped. The unit switches to its primary if it has one.
- **Armour** items set the armour value; they do not add. Damaged armour can be bought again at full price.
- **Ability charges.** There is one item per kit slot, not per ability. A charge costs the item's price (0) plus
  the `price` of the ability your kit has in that slot (`T_ABILITY`).
- **Trade-in.** Replacing a weapon or armour that you bought *earlier in the same buy phase* returns
  `refund_pct` (100%) of what you paid for it. Rifle then a 1500-credit primary leaves you 1500 down, not 4200;
  light then heavy armour costs 1000 in total; buying the free `pistol` item returns the price of a sidearm
  upgrade bought this phase. Gear carried over from an earlier round has no trade-in value, and there is no way
  to sell a primary back to an empty slot. The refund is forgotten when the buy phase ends.

## Carry-over

At the start of a round every unit is revived at full health with full magazines.

- A unit that was **alive when the previous round ended** (on either team) keeps its weapons, whatever is left
  of its armour, and its defuse kit.
- A unit that died starts with the default sidearm (`default_sidearm`), no armour and no kit.
- Credits always carry over, except at the resets below.

## Match flow

- **Regulation** is two halves of `rounds_per_half` (6) rounds. Team A attacks in the first half. The first team
  to `rounds_to_win` (7) wins.
- **Pistol round.** In the first round of each half nobody has more than 800 credits, so no primary weapon is
  affordable: the choice is light armour, a kit, a sidearm upgrade or ability charges, or saving. The winner
  can buy a rifle in round 2; the loser cannot until round 3 unless it wins round 2 with pistols.
- **Half-time reset.** Before round `rounds_per_half + 1` everyone is set to `start_credits`, every loadout is
  reset to the default sidearm (survivors included) and loss streaks are cleared.
- **Overtime.** If regulation ends level (6-6), overtime is played in **pairs** of rounds. Team A attacks in the
  first round of each pair and team B in the second. Every overtime round starts fresh: everyone has exactly
  `overtime_credits` (5000), the default sidearm, no armour and no kit, whatever happened before; income earned
  during an overtime round is therefore never spent. Both rounds of a pair are always played. A team that wins
  both rounds of a pair wins the match (for example 8-6). If the pair is split, another pair is played, up to
  `overtime_max_pairs` (3).
- **Tie-break.** If every overtime pair is split, the winner is the team with more rounds, then more kills, then
  more damage dealt. If all three are level the match is a **draw** (`winner: null` in the summary and the
  replay). Detonation-blast deaths have no killer and are not kills here.
- **Out of ticks.** A match that reaches `max_ticks` is a draw, whatever the score.
- If regulation ends without a level score and without anyone reaching `rounds_to_win` (only possible with a
  non-default `rounds_to_win`), the team ahead wins without overtime.
- **Tick bound.** A round lasts at most `buy_ticks + round_ticks + fuse_ticks + round_end_ticks` = 208 ticks, so
  the longest match (12 regulation and 6 overtime rounds, plus the draft tick) is 3745 ticks, inside
  `max_ticks` = 4000. Raising `overtime_max_pairs` or any phase length needs `max_ticks` raised with it; an
  engine test checks this.

## What this makes you decide

A full buy (rifle and heavy armour) is 3700; a defender adds a 400 kit.

- **Eco.** A first loss pays 1400. After losing the pistol round you hold about 1800: not enough for a rifle.
  Saving this round guarantees a full buy next round (1800 + 1900 = 3700).
- **Force.** With 3100 to 3700 you can take a rifle and light armour now. If you lose and die you are back to a
  sidearm with one round of loss income.
- **Saving a weapon.** Staying alive in a lost round keeps 2700 to 3700 of gear, which is worth more than a kill.
- **The cap.** A team that keeps winning banks credits it cannot use: at 8000 further income is lost, so the
  loser's streak bonus closes the gap.

## Worked example: twelve rounds

One unit from each team is followed: **a** (team A, attacks rounds 1 to 6) and **b** (team B). Both follow the same
policy: full buy (rifle and heavy armour, 3700) when affordable; light armour in a pistol round; a force buy
(rifle and light armour, 3100) from 3100 credits; otherwise save. In every round the winner's followed unit
kills the loser's, so the loser always starts over and the winner carries its gear. In the detonation rounds
the followed attacker has run clear of the 12-tile blast; one caught inside it would die and start the next
round with the sidearm like the loser. This table is pinned by the
engine test `worked_example_matches_the_docs`.

| Round | Result | a: credits, buy, left | b: credits, buy, left | a income | b income |
|---|---|---|---|---|---|
| 1 (pistol) | A wins, detonation | 800, light armour, 400 | 800, light armour, 400 | 300 kill + 300 plant + 3000 win | 1400 loss |
| 2 | A wins, elimination | 4000, rifle + heavy, 300 | 1800, save (eco), 1800 | 200 kill + 3000 win | 1900 loss (2nd in a row) |
| 3 | B wins, elimination | 3500, carried rifle and armour, 3500 | 3700, rifle + heavy, 0 | 1400 loss | 200 kill + 3000 win |
| 4 | B wins, defuse | 4900, rifle + heavy, 1200 | 3200, carried, 3200 | 300 plant + 1900 loss | 200 kill + 3000 win + 300 defuse |
| 5 | B wins, timeout | 3400, rifle + light (force), 300 | 6700, carried, 6700 | 2400 loss (3rd in a row) | 200 kill + 3000 win, capped at 8000 |
| 6 | A wins, detonation | 2700, save, 2700 | 8000, carried, 8000 | 300 kill + 300 plant + 3000 win | 1400 loss |
| | **Half-time: 3-3** | reset to 800, sidearm only | reset to 800, sidearm only | | |
| 7 (pistol) | B wins, detonation | 800, light armour, 400 | 800, light armour, 400 | 1400 loss | 300 kill + 300 plant + 3000 win |
| 8 | A wins, elimination | 1800, save (eco), 1800 | 4000, rifle + heavy, 300 | 300 kill + 3000 win | 1400 loss |
| 9 | A wins, defuse | 5100, rifle + heavy, 1400 | 1700, save, 1700 | 200 kill + 3000 win + 300 defuse | 300 plant + 1900 loss |
| 10 | B wins, elimination | 4900, carried, 4900 | 3900, rifle + heavy, 200 | 1400 loss | 200 kill + 3000 win |
| 11 | A wins, elimination | 6300, rifle + heavy, 2600 | 3400, carried, 3400 | 200 kill + 3000 win | 1400 loss |
| 12 | A wins, timeout | 5800, carried, 5800 | 4800, rifle + heavy, 1100 | 200 kill + 3000 win, capped at 8000 | 1900 loss |

Team A wins 7-5. Points to notice:

- Round 2: b saves after the lost pistol round and can full-buy in round 3, which it wins against a's carried
  rifle. In round 8 the eco itself wins: a takes the round with pistols.
- Round 5: a forces at 3400 after two losses, loses, and has to save in round 6 (which it wins with a pistol).
- Round 5: b's income hits the 8000 cap; from then on its winnings are wasted.
- Round 6: a's win ends its loss streak, and half-time wipes b's 8000 bank and carried rifle anyway.
