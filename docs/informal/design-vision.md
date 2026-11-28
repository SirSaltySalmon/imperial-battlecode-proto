# Design vision: the website and the replay viewer

Informal notes on how the site and the viewer should look and be built, written 2026-11-27 from a discussion.
Nothing here is implemented: `web/` is still plain TypeScript with placeholder styling, and the viewer still draws
discs and rings. Sections 2 and 3 are decided; the rest is the direction we talked through, with the few questions
that are still open collected in section 9.

## 1. The reference, and what we take from it

[Cambridge Battlecode 2026](https://2026a.battlecode.co.uk/) is the site we admired. Looked at on 2026-11-27:

- It is a React single-page app (styled-components) plus a separate landing-page bundle.
- What makes it look good is mostly not the framework: commissioned character art, three typefaces used with
  discipline (a grotesque for headings, an italic serif for accents, a mono for labels), and one consistent dark
  shell with grouped sidebar navigation (Info / Compete / Build).
- Its character page is hero art, a role tag, stat tiles and kit cards, with a carousel across characters and a
  compare-all table with filters. Its handbook has its own sidebar and search.
- Its match list is mirrored rows: team, rating, rating change, score in the centre, a mark on the winner, a
  "colour by" selector and "show my team on left".
- Its ladder has search, category filters and per-team language, region and members.

We do not copy the look. What we take is the lesson (art, type and consistency carry it) and the list of things
a competitor expects to find. Our own identity should come from the game: real geometry, maps, vision cones,
swept shots. We have something Cambridge does not: the engine compiled to wasm and a combat lab, so a kit page
can show the ability running instead of describing it.

## 2. Decided: the stack

- **React**, with **Tailwind** and **shadcn/ui** for components and **Motion** for animation.
- **Static build only.** The coordinator keeps serving `dist/` and answering `index.html` for client routes. No
  Node server, no second always-on service. This rules out Next.js: its static export cannot serve a route such
  as `/teams/:id` for an id unknown at build time, and its better features want a server.
- **The viewer's canvas stays plain TypeScript.** The world (map, units, bullets, effects, fog) is drawn by the
  code that draws it today. Only the HUD around it (kill feed, replay bar, panels, headline) moves to React.
- **shadcn components are copied into the repo and restyled** from our tokens (type, colour, radius, motion), so
  the site does not look like every other shadcn site.

Why React over Svelte, which was the other candidate: the largest choice of libraries (the stated priority),
MDX for handbook pages that embed live widgets, and it is what agents and most students write most reliably.
Svelte would have meant less code and built-in transitions; the margin was small either way.

Likely companions, to be confirmed when work starts rather than now: React Router with a static build and
prerendered handbook pages, TanStack Query around the existing `Api` class, shadcn's chart component for graphs,
MDX for the handbook, Pagefind for search, Lucide for interface icons, our own SVG set for game things.

## 3. Decided: scope

- **Sprites, maps and effects get new SVG art** in the viewer. Who draws it and in what style is decided later.
- **Maps get themes**: skins for floor, walls, low cover and sites, plus decoration. A theme has one look; only
  the interface around the map follows light or dark.
- **Sound will exist** in the viewer: sound effects only, no announcer.
- **Avatars will exist, without free upload.** A Discord avatar is fetched once Discord sign-in is added; an
  email account cannot upload one. Otherwise an avatar is chosen from a gallery, probably the kit portraits once
  we have them.
- **The match summary may carry a per-round digest.** This is a deliberate exception to "do not store what can be
  recomputed from a replay": the match list cannot run the engine once per row.
- **The round strip on a match row is on for ladder games and off for tournaments.** People usually know a ladder
  result already; a tournament game is watched for its result.
- **The viewer has two presets, debug and broadcast**, over the same panels.
- **Round highlights are ACE, CLUTCH and THRIFTY**, in that order of precedence, else the normal headline.
- **Real paths, not hash routes.**
- **One site, with a landing page in front.**
- **Dark and light both.**
- **Phones are supported**, and the viewer is in scope for phones.

## 4. How it fits together

Today `web/` and `viewer/` share nothing: two palettes (`web/src/style.css`, `viewer/src/theme.ts`), two DOM
helpers, two sets of formatting code. The plan is one workspace with shared packages:

| Piece | Holds | Comes from |
|---|---|---|
| tokens | Colour, type, spacing, motion: CSS variables for Tailwind and shadcn, the same values in TS for the canvas | `viewer/src/theme.ts`, `web/src/style.css` |
| game data | Frame types, replay parsing, the timeline, the rules parsers. Pure TS, no DOM | `viewer/src/types.ts`, `timeline.ts`, `ibcr.ts`, `vision.ts`, `panels/shared.ts` |
| assets | Kit sprites, weapon and ability icons, map themes, sounds | new |
| components | Team tag, avatar, kit portrait, kill row, result pill, rating change, tier badge | new, used by the site and the HUD |
| `web/` | The site | its pages, ported one at a time; `api.ts`, `format.ts`, `session.ts` carry over, `dom.ts` and `router.ts` go |
| `viewer/` | Canvas as now; HUD as React components fed from the player | its panels, moved one at a time |

Rules for keeping it clean:

- **Numbers on the site come from the ruleset.** Kit, weapon and ability pages are generated from `rules/*.toml`
  at build time and show the ruleset hash. Nobody types a damage number into a page. Patch notes can then be the
  difference between two rulesets.
- **One asset set.** The sprite a kit has on the map is the portrait in the kill feed, the buy menu and the
  handbook.
- **The viewer still works offline** from `ibc view` with relative asset URLs: fonts, sprites and sounds are
  shipped with it, nothing loads from a CDN.
- **Derived facts are computed in one place.** See the digest in section 7.

## 5. The site

**Handbook and kit showcase.** One page per kit: the sprite, role, base numbers, both abilities, and a live demo
of each ability run by the engine. A roster page with filters and a compare table. Search across the handbook.
Three kinds of diagram, each with its own source:

1. Flow diagrams (a tick, a round, how a job is dispatched): authored, rendered at build time.
2. Geometry diagrams (vision cone, line of sight, a swept hit): drawn by the viewer's own drawing code from a
   small scene, so they cannot disagree with the game.
3. Live demos: engine scenes embedded in the page, the way the combat lab already plays its recorded duels.

The participant handbook is a different audience from `docs/` (which is for developers and agents); it gets its
own pages and takes its numbers from the same generated sources.

**Ladder.** Tier bands (Iron to Radiant) as the visual structure, avatar and name, rating, movement since the
last look, a small sparkline per row, search and filters.

**Match history.** Two meanings of colour are kept apart: team identity (the blue and orange pair, which is the
slot in a game) and outcome (won, lost, drawn). Outcome never rides on red against green alone: it comes with an
arrow or icon, as the viewer already pairs colour with shape. From a signed-in team's point of view rows read as
won or lost with the rating change; with no point of view the winner is marked by weight, not colour. With the
round digest a ladder row shows the game as a strip of rounds. Tournament rows do not, so the page does not give
away a game someone is about to watch.

**Landing page and routes.** One site: a landing page in front (what the competition is, schedule, sponsors) and
the competitor's pages behind it, in one build. Pages have real paths (`/ladder`, `/teams/:id`), which the
coordinator's `index.html` fallback already allows, and a replay link can name a moment: this game, round 7,
this tick.

**Rating graph.** On a team's page: rating over time, the uncertainty of the estimate as a band, tier bands
behind it, and a mark at each new live submission and each balance patch. The question a team brings to this
graph is "did my new bot help", so the submission marks matter more than the line.

**Avatars.** No free upload, so there is no upload queue to moderate. An avatar comes from one of three places:
the Discord account, once Discord sign-in exists; a pick from a gallery, probably the kit portraits once the art
exists; or, until either applies, one generated from the id. Shown on the ladder, in match rows and on the
viewer's scoreboard.

## 6. The viewer

What exists already, so we restyle rather than rebuild:

- The kill feed (`viewer/src/panels/killfeed.ts`): killer, what killed, victim, fading over replay ticks so
  that pausing freezes it and scrubbing rebuilds it. That behaviour is right and stays; Motion handles rows
  entering and leaving, the fade stays tied to replay time.
- Round starts and plants on the scrub bar (`buildMarkers` in `viewer/src/ui.ts`).
- The round headline, ATTACKERS WIN or DEFENDERS WIN with the reason and score (`viewer/src/ending.ts`).
- Buy menu and team tab panels; perspectives and fog.

**Replay bar.** Kills on a whole-match bar would be clutter, so the bar splits in two: a match strip (one
segment per round, coloured by who won it) and a round strip for the current round, which carries kills, plant,
defuse and detonation. Each kind of mark can be switched on and off; the choice is remembered per browser.

**Presets.** Two, over the same panels. **Debug** is for a competitor reading their own bot: debug drawings,
fuel, memory, every panel within reach. **Broadcast** is for someone watching: the map, the kill feed, the
headline and little else. The phone layout is an arrangement of either, not a third preset.

**Round highlights.** Beyond who won, a round can earn one highlight. When it earns more than one, the first in
this order is shown; a round that earns none keeps the normal headline.

| Order | Highlight | Proposed meaning |
|---|---|---|
| 1 | ACE | One unit made every kill on the losing team, and all of it died |
| 2 | CLUTCH | The winning team was down to its last unit against two or more, and won |
| 3 | THRIFTY | The winning team's equipment was worth much less than the loser's |

The order is decided; the exact meanings and the THRIFTY threshold are still proposals. The highlight replaces
the headline's title; side, reason and score stay in the line under it. The thresholds are presentation, not
rules: they do not go in `rules/*.toml`, where they would change the ruleset hash. TEAM ACE and FLAWLESS were
talked about and are not in the list.

**Sprites.** Aim is a free angle, so sprites are top-down and rotate; side-view art with eight directions would
lie about where a unit faces. A sprite's drawn size is the unit's real radius. Teams must stay apart by shape as
well as colour, as the disc and ring do today, so sprites take a team tint and a team outline. SVGs are
rasterised once per zoom level and drawn as bitmaps; vision cones, aim lines and bullets stay drawn by code.

**Map themes.** A theme is a set of colours and tile art for floor, wall, low cover and site, which is the map
half of today's `Palette`. A theme has one look: it does not come in a light and a dark version, and only the
interface around the map follows the reader's light or dark setting. Constraints:

- A theme is presentation only. It is not part of the map hash and not in a replay, so reskinning a map never
  invalidates a replay or a golden. Which theme a map wears, and where its decoration sits, is stored outside
  the hashed content.
- Decoration must never read as geometry. Something that looks like cover and is not misleads everyone watching.
- Walls, low cover and sites are distinguishable in every theme by shape and lightness, not hue alone, and both
  team colours stay readable on every theme's floor.
- Theme data is a data file, so the map renders in `maps/renders/` and the viewer read the same one.

**Sound.** Sound effects only: steps, shots, abilities, plant, defuse, detonation, a round ending. No announcer
and no voice lines. Playback only, derived from frames (the per-unit `sounds` lists and the events); replays
carry none of it. Off until the viewer turns it on, since browsers block audio before a gesture. It follows
replay time: quiet while scrubbing and at high speeds. In a team's perspective you hear what that team heard,
and a decoy sounds like the real thing; with no perspective you hear everything. Every sound file has its
licence recorded.

**Phones.** Landscape first. Pinch to zoom, drag to pan, tap a unit to follow it, a scrub bar a thumb can hit,
and a tap for everything that is hover today. Panels become sheets; the kill feed shortens. Loading a replay
re-simulates the match in the engine, so how long that takes on a phone is to be measured before we promise it.
The map editor, the combat lab and `ibc play` stay desktop tools.

**Reduced motion.** Banners, slides and the blast respect the system setting.

## 7. What the platform has to provide

- **Rating history.** `rating_events` is already the history every rating is folded from
  (`crates/coordinator/src/db/ratings.rs`), so the graph needs an endpoint, not new storage.
- **Per-round digest in the summary.** A few numbers per round: winner, attackers, reason, kills by team,
  whether and where the charge was planted, the highlight and its unit. `Summary.game` is the free-form place for
  it. The worker writes it once when the game ends.
- **One definition of a highlight.** The digest and the viewer must not classify rounds with two copies of the
  logic. The classification is written once on the Rust side and reaches the viewer through the engine build it
  already loads.
- **Avatars.** No upload endpoint. The stored value is a gallery choice or, once Discord sign-in exists, a copy
  of the Discord avatar taken at sign-in and kept at one small size, so that pages and the offline viewer do not
  load images from Discord. A Discord avatar is still a picture a person chose, so the moderation terminal can
  reset one to the generated avatar.

## 8. Order of work

Each step is its own change, so that one thing moves at a time.

1. Design direction: type, colour, motion, navigation, both themes. Before any code.
2. The workspace and the tokens and game-data packages. Mechanical.
3. `web/` ported to React page by page, with the new shell.
4. Platform additions: rating history, the digest, avatars.
5. Ladder, match history, team page with the graph.
6. Handbook and kit pages generated from the rules.
7. Viewer HUD: kill feed restyle, the two-strip replay bar, highlights.
8. Sprites, map themes, sound, the phone layout.

`web/` is about 1,900 lines today, so step 3 is cheap now and dearer with every page added before it.

## 9. Open questions

1. **Who draws the art, and in what style?** Decided later. It decides the look more than any other choice here,
   and the gallery of avatars waits on it.
2. **Where does a map's theme and decoration live?** A sidecar file beside the `.ibcmap`, or a part of the map
   file that the hash skips. And does the map editor get a decoration layer.
3. **The exact meaning of each highlight**, and the THRIFTY threshold. Their order is decided.
4. **Whose avatar is it?** A Discord avatar belongs to a person; the ladder and match rows show teams of 1 to 4
   people. Leaning towards: a person shows their Discord avatar, a team shows its gallery pick.
5. **Do tournament rows hide more than the round strip?** The score gives a game away as much as the strip does.
6. **Where the sound effects come from**, with a licence recorded for each.
