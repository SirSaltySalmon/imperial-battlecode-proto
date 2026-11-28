# Weapons

Informal vision for the six weapons: what each is for, what it gives up, and what to measure before tuning it, so
that balance changes move towards something. Written 2026-11-27 against `rules/weapons.toml`, the generated tables
in `docs/rules/combat.md` and the eight-bot pool, then updated the same day with the decisions in section 4. The
pistol is described at its decided 10-tile range; the TOML and TTK tables show 20 until that lands. Research is used
for philosophy only: our numbers and names are our own.

## 1. Principles

What Valorant and Counter-Strike do:

- **A gun for every situation, and no best gun.** Riot cut guns that duplicated others and wanted power and price
  to trade off so that nothing is simply best; every gun needs a clear use
  ([Riot: how the arsenal was built](https://playvalorant.com/en-gb/news/dev/how-the-valorant-arsenal-was-built/)).
- **The rifle is the baseline.** Everything else is cheaper and narrower, or dearer and a role. Vandal and Phantom
  cost the same and split by range profile (a one-tap at any range against fire rate, falloff and an easier spray);
  Riot finds both viable almost everywhere, nearly half of Phantom kills inside 15 m, Vandal kills spread across
  ranges ([Riot data drop](https://playvalorant.com/en-us/news/dev/valorant-data-drop-phantom-vs-vandal/)).
- **Price buys power; kill reward pays back a handicap.** CS pays shotguns $900, SMGs $600, rifles and pistols
  $300, the AWP $100: harder kills pay more ([Refrag](https://refrag.gg/blog/cs2-economy-crash-course-what-are-kill-rewards-and-loss-bonus)).
  Valve still moves it (XM1014 $900 to $600 in April 2024, [patch notes](https://ggrecon.com/guides/counter-strike-2-april-2024-patch-notes/)).
  Valorant pays a flat 200 and puts everything into price. We use the CS model, so kill reward is a lever.
- **Contain the sniper with price, movement and reward, not by breaking its shot.** The AWP costs about 1.75 rifles
  and pays $100 ([GuidingTech](https://www.guidingtech.com/the-decline-of-the-awp-is-reshaping-defensive-setups/)).
  Riot answered a too-strong Operator with price (against round-3 Operator-and-armour snowballs), scoped movement,
  equip time and a narrower still-accurate window, curbing dry peeks while keeping it the best angle-holding gun
  ([patch 1.09](https://playvalorant.com/en-us/news/game-updates/valorant-patch-notes-1-09)). Nerf the peek, keep the hold.
- **Moving costs; stopping is the skill.** Riot raised moving inaccuracy again and again to reward stopping before
  the shot ([2.02](https://playvalorant.com/en-gb/news/game-updates/valorant-patch-notes-2-02/),
  [3.0](https://playvalorant.com/en-gb/news/game-updates/valorant-patch-notes-3-0)); Valve banned keyboards that
  automate counter-strafing ([Notebookcheck](https://www.notebookcheck.net/Valve-prohibits-new-keyboard-features-Razer-Snap-Tap-and-Wooting-Snappy-Tappy-in-Counter-Strike-2.877368.0.html)).
  The exception is deliberate: 9.10 spared SMGs because "their mobile accuracy is a core part of how the weapons
  are unique" ([patch 9.10](https://playvalorant.com/en-us/news/game-updates/valorant-patch-notes-9-10)).
- **Fix what a gun does outside its band, not its identity.** The Stinger overperformed at medium and long range for
  its price: price and falloff ([6.02](https://playvalorant.com/en-us/news/game-updates/valorant-patch-notes-6-02/)).
  The Judge did too well at range (3.0), then was made weaker on the move to sharpen it as a hold-in-place shotgun
  ([7.09](https://playvalorant.com/en-us/news/game-updates/valorant-patch-notes-7-09/)).
- **Armour sets breakpoints.** CS separates eco guns from full-buy guns with per-weapon armour penetration: armour
  soaks most of a low-penetration pistol or SMG hit ([Dignitas](https://dignitas.gg/articles/armor-penetration-and-the-utility-of-helmets-how-does-csgo-s-armor-system-work)),
  and Valve raised SMG penetration by about a fifth in 2014, which weakened armour-and-pistol second rounds
  ([GosuGamers](https://www.gosugamers.net/counterstrike/news/28727-major-changes-in-this-week-s-update)).
  We do not copy this: it is bloat for a game meant to be tactical and leave room for coding creativity.
  Armour is plain extra health, as in Valorant, and a weapon's damage per hit decides how it fares against it.
- **Small, single changes, above all to the guns everyone uses.** Valve rolled back a winter-2015 rifle change meant
  to favour tapping over spraying: it made every way of firing worse and players sprayed anyway
  ([GosuGamers](https://www.gosugamers.net/counterstrike/news/33706-cs-go-update-spraying-nerf-reverted)).
- **Data with bounds, then judgement.** Riot sets upper and lower bounds on win and pick rates, investigates what
  leaves them, does not chase 50/50 and says data alone does not show a weapon's impact
  ([how we balance](https://playvalorant.com/en-us/news/dev/how-we-balance-valorant/)). It uses non-mirror win rates,
  weapon popularity by round and weapon-against-weapon duels
  ([trust the balance process](https://playvalorant.com/en-gb/news/dev/trust-the-balance-process-data-and-insights/)).

What changes in our model:

- **Exact bullets, no kick, ever.** A one-pellet weapon (pistol, marksman) goes exactly where it is aimed, so it is
  balanced by startup, cooldown, falloff, bullet speed and price. A fan (rifle, SMG, shotgun, LMG) is balanced by
  pellet damage, fan width, falloff and how fast recoil opens it.
- **Bots react for free, so the skill gate is planning,** and weapons differ in what must be planned: unsteadiness
  that builds and decays over ticks, a startup the enemy can see, bullets that take time to arrive.
- **The startup is the tell.** Enemies who see a unit start up get `S_WINDUP` and its aim angle, fixed at the pull.
  A startup of two ticks or more leaves the target a whole tick to step off the line (0.68 tiles at a run against a
  0.29-tile radius), so a seen, free target can always dodge it. A one-tick startup and every braced shot can only
  be dodged in flight. Startup is the dodge lever at any range, bullet speed at range. Dodging is not shooting: it
  breaks your own brace and builds unsteadiness, so it buys time or cover but never trades.
- **Peeks.** A holder that sees a peeker needs only its windup; the peeker pays windup plus unsteadiness. A peek
  wins on information (sound, shared vision), on a walk (rifle +1 tick), with the SMG (+0), or against a holder that
  is reloading, braced on someone else or facing away.
- **Armour is extra health.** Light armour makes a unit 125 to kill and heavy 150: a hit takes armour first, then
  health, point for point. No penetration and no per-weapon armour value. Breakpoints are plain arithmetic: a
  weapon that deals 63 or more per pull kills light armour in two, 75 or more kills heavy in two.
- **Pulls to kill are the identity.** With no headshots, "how many pulls against no, light and heavy armour at the
  home range" plays the part of the Vandal's one-tap. Decide those first; ticks follow from cooldown and startup.
- **Fire rate is a main lever,** alongside damage, range and startup: it separates the rifle from the LMG and the
  SMG. It stays on whole ticks (a pull every `1 + cooldown` ticks), so only intervals of 1, 2, 3 ticks and so on
  exist. Where two weapons share an interval, damage per shot and pellets separate them. Today the pistol, rifle
  and SMG all fire every 2 ticks.
- **Falloff makes up close hit harder than far,** for every weapon. Full damage up to `falloff_start`, then a
  straight line down to `falloff_min_pct` at `range`. How steep it is decides whether the hits to kill change
  across a weapon's band.

Pulls to kill at full damage (100 health; light and heavy armour add 25 and 50, taken before health), and how
close a sprinting target must be for the shot to land anyway (back-of-envelope; check in the combat lab). The
current 50% armour split already adds up to these totals, give or take rounding, so the switch changes no column:

| | pistol | rifle | smg | shotgun | marksman | lmg |
|---|---:|---:|---:|---:|---:|---:|
| pulls: none / light / heavy | 4 / 5 / 6 | 2 / 2 / 3 | 3 / 4 / 5 | 1 / 2 / 2 | 2 / 2 / 2 (decided: 2 / 2 / 3) | 2 / 2 / 2 |
| every pellet lands, tiles (seen one-tick startup) | 3.4 | 4.0 | 3.4 | 1.8 | 8.6 (braced) | 2.6 |
| some pellet lands, tiles | 3.4 | 12 | 4.4 | whole range | 8.6 (braced) | whole range |

The marksman's first shot always starts up for two ticks, so a target that sees it can always step aside; its
figure is for braced follow-ups against a target that predicts the cadence. Recoil widens fans under sustained fire.

Today heavy armour changes nothing against the marksman (75 damage meets the 150 breakpoint exactly) or the LMG.
So in the heavy-armour TTK table the rifle trails the LMG at 5 tiles (10 against 6) and the marksman at 5, 10 and
20 tiles (10, 10 and 17 against 8): in a full-buy round the rifle is the best weapon nowhere. That, more than
range, is the first suspect for the marksman taking 23% of the kills from about a fifth of the units. Decided: heavy
armour takes three marksman hits (section 4).

## 2. The weapons

| weapon | role | home band (tiles) | price (x rifle) | kill reward | fire rate (intended order) | bought in |
|---|---|---|---:|---:|---|---|
| pistol | free sidearm, last resort | 0 to 6, nothing past 10 | 0 | 300 | as now; tune damage instead | pistol rounds, ecos, finishing a fight |
| smg | run-and-peek, anti-eco | 0 to 6 | 1500 (0.56) | 400 | the fastest | forces; full buys against an eco |
| shotgun | close hold | 0 to 3, nothing past 6 | 1800 (0.67) | 500 | slow pump | forces, mostly on defence |
| lmg | team fights, zone control | 5 to 15, set up | 2300 (0.85) | 150 | fast once braced | defence; the cheaper full buy |
| rifle | all-rounder, the baseline | 3 to 20, best 6 to 15 | 2700 (1.00) | 200 | the middle | every full buy by default |
| marksman | long angles, one per team | 12 to 30, set up | 3200 (1.19) | 100 | slow, kept as now | full buys with credits to spare |
| hunter (planned) | eco poke and hold | 10 to 24, set up | cheap (tbd) | high (tbd) | the slowest | ecos and forces |

Today's whole-tick intervals are pistol, rifle and SMG every 2 ticks, the LMG every 3, the shotgun every 4 and the
marksman every 5. The order above is where they should end up; the numbers come from measurement.

Round types as in `docs/rules/economy.md`: pistol round (800), eco (save, sidearm), force (cheap primary and light
armour, about 1900 to 3600), full buy (rifle or better and heavy armour, 3700 up). After a lost pistol round a unit
holds about 1800. TTK figures are the generated table: standing, still target; "after a run" is its second column.

### Rifle

- **Role.** The default full buy, our Vandal-or-Phantom: good at every range a fight happens, best in the middle,
  never the best specialist. The Vandal's promise (the same pulls to kill across the fighting range) with the
  Phantom's forgiveness (a fan that still lands a pellet on a dodger out to about 12 tiles).
- **Beats** SMG and pistol past 8 tiles, the marksman inside 10, the LMG when either has moved. **Loses to** the
  shotgun inside 3, the SMG on a running peek inside 5, a set marksman past 15, a set LMG against armour up close.
- **Economy.** Full buy (3700 with heavy armour), affordable in round 2 for the pistol-round winner; reward 200.
- **Keep:** 2 pulls against light armour to about 18 tiles and bare units to 20, the tight 3-bullet fan, a one-tick
  steady startup, a fast bullet. **Tune with** cooldown (fire rate) or unsteadiness (startup after moving), as decided;
  then falloff end and price. Its fire rate sits below the SMG's and the braced LMG's; damage per shot, not rate,
  separates it from the pistol. **Never** kick, the SMG's free running startup, or a one-pull kill.
- **Targets.** Within 2 ticks of the fastest weapon in every column from 5 to 15 tiles, unarmoured (now first or
  tied at 5 and 10); first or tied at 10 tiles against heavy armour too (now 10 against the marksman's 8). After a
  run +2 to +3 (now +3). Kill index 1.0 in full-buy rounds by definition: the reference. On the ladder, the
  most-bought primary in full-buy rounds.

### SMG

- **Role.** The peeker's gun and the anti-eco gun: running costs it nothing, so it starts as fast off a sprint as a
  holder does standing, and it carries at full speed. The fastest fire rate, small hits, short reach. Its low
  damage per bullet is what makes it weak against armour: more health to chew through, no penetration rule.
- **Beats** the rifle on a running peek inside 5 tiles, the pistol everywhere, anything unarmoured up close.
  **Loses to** a standing rifle past 6, heavy armour, the shotgun inside 3, everything past 10.
- **Economy.** Forces, and full buys facing an eco. Reward 400, two rifles' worth as in CS: a force must fund itself.
- **Keep:** no running surcharge (`unsteady_gain`, `unsteady_max`), `move_pct` 100, cheap. **Tune with** pellet
  damage, falloff start, range, cooldown, magazine, price, reward. **Never** reach: past about 6 tiles it is a cheap
  rifle, the Stinger's problem.
- **Targets.** At 5 tiles after a run, at least 2 ticks faster than the rifle after a run (now 6 against 9).
  Standing at 5: within 1 of the rifle unarmoured (now tied at 6), 2 or more slower against heavy armour (now tied
  at 10; see section 4). At 10 tiles 3 or more behind (now 9 against 6). Kill index at least the rifle's against
  bare or light-armoured victims, at most 0.8 of it against heavy. Kill-reward income per carrier-round above the
  rifle's. On the ladder, under ~15% of full-buy primaries.
- A lead for the `forcer` diagnosis, no more: half of all kills happen inside 5 tiles, where the SMG ties the rifle.

### Shotgun

- **Role.** The close hold: a door, a corner, a site entrance. Inside 3 tiles one pull kills a bare unit and the fan
  is too wide to sidestep; past 6 it tickles. Slow pump, so a miss is punished.
- **Beats** everything inside 3 tiles, dodgers up close. **Loses to** anything past 6, armour (two pulls against
  light or heavy), a second enemy while it pumps.
- **Economy.** Force buy, mostly on defence. Reward 500, the highest (CS pays three rifles), for getting that close.
- **Keep:** one pull inside 3 tiles against no armour, the wide fan, the slow cycle. **Tune with** falloff start,
  range, pellet damage (the light-armour breakpoint), price, reward, startup after a run. **Never** reach past ~6.
- **Targets.** Fastest of all at 2 to 3 tiles (needs that column; now one pull). No kill past 8 (now none at 10).
  At least two thirds of its kills inside 5 tiles. Kill index higher on defence than attack, and 0.8 to 1.0 of the
  rifle's overall (about 0.7 in the old report, section 3).

### LMG

- **Role.** The team-fight and zone-control gun; it earns its place (decided). Set on a choke, its wide fan damages
  several enemies at once and its recoil nets out (it never blooms). Twenty pulls a magazine and a fast fire rate
  once braced let it deny an area to a group, but a focused weapon can trade with it one on one and win. Slowest to
  carry, to start after moving and to reload.
- **Beats** groups pushing through a choke, armour up close, dodgers (a braced fan cannot be stepped out of).
  **Loses to** a focused weapon one on one, anything after it moved (6-tick startup), flanks, a peek timed on its
  7-tick reload, range past 15.
- **Economy.** 2300: with heavy armour 3300, a cheaper full buy for a defender. Reward 150, below the rifle because
  it farms kills from safety; raise it if the LMG stays under par.
- **Keep:** the constant wide fan, the deep magazine, slow to move and to start. **Tune with** pellet damage and
  count (breakpoints), falloff, fan width, price, reload. **Never** make it quick off a run.
- **Targets.** The largest share of pulls that hit two or more enemies of any weapon (the bench does not count this
  yet). Standing at 10 tiles within 2 ticks of the rifle (now 9 against 6). May be the fastest against heavy
  armour at 5 tiles (now 6): that is its hold. After moving, +4 like the marksman. Kill index 0.9 to 1.1 of the
  rifle on defence; lower on attack is fine. On the ladder a niche, 3 to 10% of primaries.

### Marksman

- **Role.** Long angles, about one per team. One exact, fast bullet. Two hits kill a bare or light-armoured unit,
  three a heavy-armoured one, anywhere in its band (decided). That means 63 to 74 damage everywhere from 0 to 30
  tiles, with a shallow falloff: more damage up close, never enough to change the hits to kill. Range 30 tiles
  (decided, down from 40): six tiles past its own 24-tile vision, so a teammate's spot still extends it a little.
  It punishes the committed (planting, defusing, reloading, braced on someone else) and the unaware. A seen
  marksman's first shot is always dodgeable because its startup is two ticks: that tell is its counterplay.
- **Beats** the rifle past 15 tiles, light armour, anyone who does not see it. **Loses to** everything inside 10,
  heavy armour up close, any peek right after it moved (6-tick startup), aware targets that step off a visible windup.
- **Economy.** A role buy in full-buy rounds. Reward 100, half a rifle (CS: a third), the tax on reach and on
  ignoring armour. Today a pistol-round winner can buy it with light armour in round 2 (400 left, 3000 win and one
  300 kill make 3700; it costs 3600): the early snowball Riot priced the Operator out of.
- **Keep:** one exact bullet, the 2 / 2 / 3 hits to kill, 20 tiles a tick, a two-tick steady startup, the slowest
  start after moving, its fire rate (faster than the hunter's). **Tune with** price first, then magazine,
  `move_pct`, reward. **Never** a one-tick startup (it removes the tell), or falloff steep enough to change its
  hits to kill inside its band (that makes it a rifle).
- **Targets.** Slower than the rifle inside 10 tiles against heavy armour (now 8 against 10; three hits fix it) and
  bare (now 8 against 6); faster past 15 against bare and light (now 8 against 11 bare at 20). At least a third of its kills past 10 tiles. Kill
  index 0.9 to 1.15 of the rifle in full-buy rounds (about 1.1 to 1.2 now). Out of reach in round 2 without two
  kills. On the ladder at most ~25% of full-buy primaries.

### Pistol

- **Role.** The free sidearm and last resort, for pistol rounds, ecos and an empty primary: exact single shots, a
  short startup even off a run, and short range (10 tiles decided, 8 or 5 if still too strong).
- **Kills** bare units inside 5 tiles. **Loses to** every bought primary in its own band, and to armour.
- **Economy.** Free. Reward 300, above the rifle: an eco kill is an upset against armour, and paying for it is the
  catch-up. Pistol rounds alone give it roughly a sixth of all kills (2 rounds in 12): kill share is no target.
- **Keep:** price 0, one exact bullet, a quick start off a run, short reach. **Tune with** range and falloff (now),
  then damage per shot (hits to kill; decided as the next lever after range, not fire rate), magazine. **Never** a
  price, or kick.
- **Targets.** No kill past 10 tiles. At 5 tiles at least 2 ticks slower than every bought primary (now 8: tied with
  the marksman, 2 behind the rifle), and slower than all of them against heavy armour (now 12, so it is). Kill index
  against heavy-armoured victims with a primary at most half the rifle's. A pistol team beats a full buy in 10 to 20%
  of rounds (our pick: an eco is a gamble, not a plan).

### Hunter (planned seventh weapon)

- **Role.** The eco long-range gun, for poking and holding. It is the marksman's numbers with a slower fire rate, a
  24-tile range, a cheap price and a good kill reward (decided). It gives a saving team a way to contest long
  angles, since once the pistol is cut nothing under 2700 reaches past 10 tiles. A miss costs a long wait, so it
  rewards patience and set-up angles over trades.
- **Same as the marksman:** damage and falloff (2 / 2 / 3 hits to kill), one exact bullet, bullet speed, the
  two-tick startup tell, unsteadiness, `move_pct`. **Different:** a longer fire interval, 24 tiles of range
  (its own vision range, so spotting does not extend it), the price and the reward.
- **Beats** a peeker who walks into its held angle, anyone it can poke from range. **Loses to** anything that
  closes the distance, a second enemy after it fires, the marksman (more reach, faster follow-up), anyone who
  sees its startup and steps aside.
- **Economy.** Eco price, affordable with light armour after a lost pistol round. A kill reward well above the
  marksman's, on the same principle as the other cheap guns: a handicapped gun pays more per kill.
- **Levers.** Fire interval first, then price and reward. **Never** the marksman's fire rate or reach: then
  nobody buys the marksman.
- **Targets.** Set once the six are tuned: it must not displace the rifle or the marksman in full-buy rounds
  (under ~10% of full-buy primaries on the ladder), and an eco team holding it should win more long-range duels
  than one with pistols, not more rounds than a full buy.

## 3. Measuring without being fooled

- **Kill share is not strength.** It is carriers times kills per carrier. Report the **kill index**: kills per
  carrier-round over all kills per unit-round, a carrier-round being a unit holding the weapon when the action
  phase starts (`ibc-bench balance` records that weapon, by side). From the last full report (ruleset
  `0304dfc448350cca`, older than today's rules): marksman 21.7% of kills from 19.4% of units (index 1.12), rifle
  0.97, SMG 0.90, LMG 0.89, pistol 0.82, shotgun 0.68. Re-measure before acting.
- **Compare like with like.** Split by round type and by the victim's armour: an SMG farming ecos and a marksman
  dueling full buys do not belong in one number. The rifle in full-buy rounds is the 1.0.
- **Separate the bot from the weapon.** The pool buys by bot and unit slot: `rusher`, `holder` and `utility` buy
  the dearest weapon they can afford (the marksman, whole teams of it); `sapper` and `lurker` give slot 3 the
  marksman, slot 2 the LMG and slot 4 the shotgun (defence) or SMG (attack); `retaker` and `saver` buy rifles; only
  `forcer` buys SMG and shotgun every round. So the marksman is mostly held by three bots, two of them the weakest,
  and every SMG and shotgun number carries the strongest. Split by bot and slot; compare within a slot (slot 3 with
  the marksman against slot 3 when it can only afford a rifle). Pick rate in the pool is code, not choice: pick-rate
  bands apply to ladder replays only.
- **Better: a loadout A/B.** Games are deterministic, so we can run what Riot cannot: the same bot with one slot's
  weapon swapped (a small `bots/squad` style option), paired against the whole pool with `ibc-bench ab`. The change
  in round win rate is that weapon's worth to that bot, with map, seed and side cancelled out.
- **The pool does not dodge.** No reference bot steps off a visible startup or times a peek on a reload; the SDK
  leads its targets and never dodges (`sdk/rust/src/tactics.rs`). Weapons whose counterplay is the dodge, the
  marksman above all, measure stronger than they should. Teach one bot to dodge, or report against it separately.
- **Both sides, whole pool, no mirrors.** LMG and shotgun are defence weapons; a mirror measures a bot against itself.
- **Sample size.** Give n (distinct games) and the 95% interval, clustered by match; do not act inside it. Cells
  shrink fast split by bot, slot, side and round type: say so when one is under ~100 games.
- **Bot-free first.** A change must move the generated TTK table the intended way before a league is worth running.
  The table wants a 2 to 3 tile column (shotgun), a 15-tile column (rifle against marksman) and light armour. The
  bench lacks, per weapon: kills, damage and kill-reward income per carrier-round, kill distance, victim armour, and
  duels by weapon pair and distance band. Today it has overall kill share, shots per kill and one distance histogram.
- **Read the replays.** A weapon that loses might be a bot holding it wrong.

## 4. Decisions and open questions

Decided 2026-11-27:

- **No armour penetration.** Armour is plain extra health, as in Valorant: 125 to kill with light armour, 150 with
  heavy, armour taken first, point for point. A weapon's damage per hit decides its viability against armour. In
  the ruleset this is `armor_absorb_pct = 50` becoming 100 (a data change; the totals barely move, the breakpoints
  become plain arithmetic).
- **Marksman: 2 / 2 / 3.** Two hits kill no or light armour and three kill heavy, across its band: 63 to 74 damage,
  a shallow falloff so up close hits harder without changing the hits to kill.
- **Marksman range 30 tiles** (from 40). Spotting for it with teammates stays part of the game, six tiles' worth.
- **Fire rate is a main lever** that separates the rifle, LMG and SMG, and it stays on whole ticks. Where two
  weapons share an interval, damage per shot separates them. The marksman keeps its fire rate.
- **The pistol is tuned by damage,** not fire rate, once its range is cut.
- **The LMG earns its place** as the team-fight and zone-control gun: its fan damages several enemies at once and
  holds an area, while a focused weapon can beat it one on one.
- **No tagging.** Being hit does not slow you.
- **Per-weapon kill rewards stay,** and stay a lever: cheap and handicapped guns pay more per kill.
- **Hunter,** the eco long gun (section 2): the marksman's numbers with a slower fire rate, 24 tiles of range, a
  cheap price and a good kill reward.
- **Fights happen close because the maps are built that way:** 48% of kills land inside 5 tiles and 86% inside 10
  (mean 5.8, last full report). New maps should be more varied, with open spaces, so reach has a use.
- **No side-specific prices.** Side balance is not done through prices.

Why no tagging: CS made it cumulative in 2014, and Riot cut it in 3.0 because slowed targets made running shooters
accurate. Here a slowed unit builds less unsteadiness, so tagging would shorten the victim's next startup and cut the
dodging we want viable.

Still open (numbers, to be set by measurement rather than decided up front):

1. **The hunter's fire interval, price and reward.** Valorant's Marshal and Outlaw
   ([Riot](https://playvalorant.com/en-us/news/game-updates/a-new-addition-to-the-arsenal-outlaw-insights/)) and the
   CS Scout are the reference points for an eco long gun's price against a rifle's; ours is its own gun.
2. **The pistol's damage** at 10 tiles of range: how many hits it needs against no armour at close range, where
   about half of all kills happen and the free pistol is now two ticks behind a 2700 rifle.
