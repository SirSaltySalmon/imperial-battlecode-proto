//! Elimination brackets: the shape of a single or double elimination bracket for `n` seeded entrants, who
//! reaches each match given the results so far, and how a pairing that played several maps is decided.
//!
//! A bracket is a pure function of (format, number of entrants), so the coordinator stores only the results
//! and rebuilds the shape when it needs it.
//!
//! - **Seeding** is the standard one: the bracket is padded to the next power of two `P`, and the first round
//!   puts seed 1 against seed `P`, 2 against `P - 1`, ... in the line order that keeps 1 and 2 apart until the
//!   final ([`seed_order`]). Seeds past `n` are byes: their opponent advances without playing.
//! - **Double elimination** adds a losers bracket of `2 (k - 1)` rounds for `P = 2^k`: round 1 pairs the
//!   losers of winners round 1, then every even round takes the losers dropping from the next winners round
//!   (in reversed order every other time, so a team does not meet the one that just beat it), and every odd
//!   round halves the field. The grand final is winners champion against losers champion, one series, no
//!   bracket reset: every pairing already plays every tournament map, and a reset is a conditional match a
//!   static bracket cannot express. With two entrants a double elimination bracket is a single match.
//! - **Byes cascade**: a seat whose feeder had nobody (a bye, or the loser of a bye) is empty, and a match
//!   with one empty seat sends its other seat through. One rule covers byes in both brackets.

use crate::Outcome;

/// Single or double elimination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Format {
    Single,
    Double,
}

/// Which part of the bracket a match belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Stage {
    Winners,
    Losers,
    /// The grand final of a double elimination bracket.
    Final,
}

/// Where one seat of a match comes from. `Winner(m)` / `Loser(m)` name a match by its index in the bracket,
/// which is always smaller than the index of the match it feeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Source {
    /// Seed `s` (0-based: 0 is the top seed).
    Seed(u32),
    /// Nobody: the bracket was padded to a power of two.
    Bye,
    Winner(u32),
    Loser(u32),
}

/// One match of the bracket. `round` counts from 1 within its stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BracketMatch {
    pub stage: Stage,
    pub round: u32,
    pub sources: [Source; 2],
}

/// Who sits in a seat, or who won or lost a match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Seat {
    /// Not known yet: a feeding match has not been decided.
    Pending,
    /// Nobody will sit here (a bye, or the loser of a match that had a bye).
    Empty,
    /// The entrant with this 0-based seed.
    Team(u32),
}

/// A side of a match: the seat fed by `sources[0]` (A) or `sources[1]` (B).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Side {
    A,
    B,
}

/// A match given the results so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MatchState {
    pub seats: [Seat; 2],
    pub winner: Seat,
    pub loser: Seat,
}

impl MatchState {
    /// Both seats are teams and no result is in: this match should be played now.
    pub fn playable(&self) -> bool {
        matches!(self.seats, [Seat::Team(_), Seat::Team(_)]) && self.winner == Seat::Pending
    }
}

/// Bracket line order of the 0-based seeds for a bracket of `size` (a power of two): consecutive pairs meet
/// in round 1. For 8: 1v8, 4v5, 2v7, 3v6 (as 0-based seeds `[0, 7, 3, 4, 1, 6, 2, 5]`).
pub fn seed_order(size: u32) -> Vec<u32> {
    assert!(
        size.is_power_of_two(),
        "bracket size must be a power of two"
    );
    let mut order = vec![0u32];
    while (order.len() as u32) < size {
        let n = order.len() as u32 * 2;
        order = order.iter().flat_map(|&s| [s, n - 1 - s]).collect();
    }
    order
}

/// The matches of a bracket for `entrants` seeded teams, in an order where every match comes after the
/// matches that feed it: winners rounds, then losers rounds, then the grand final. Empty below two entrants.
pub fn build(format: Format, entrants: u32) -> Vec<BracketMatch> {
    if entrants < 2 {
        return Vec::new();
    }
    let size = entrants.next_power_of_two();
    let mut out = Vec::new();
    let winners = winners_rounds(&mut out, entrants, size);
    if format == Format::Double && entrants > 2 {
        let losers_champion = losers_rounds(&mut out, &winners);
        let winners_champion = *winners.last().and_then(|r| r.first()).expect("a final");
        out.push(BracketMatch {
            stage: Stage::Final,
            round: 1,
            sources: [
                Source::Winner(winners_champion),
                Source::Winner(losers_champion),
            ],
        });
    }
    out
}

/// Push the winners bracket; returns the match indices of each round.
fn winners_rounds(out: &mut Vec<BracketMatch>, entrants: u32, size: u32) -> Vec<Vec<u32>> {
    let seat = |s: u32| {
        if s < entrants {
            Source::Seed(s)
        } else {
            Source::Bye
        }
    };
    let order = seed_order(size);
    let mut rounds: Vec<Vec<u32>> = Vec::new();
    let first = order
        .chunks(2)
        .map(|pair| push(out, Stage::Winners, 1, [seat(pair[0]), seat(pair[1])]))
        .collect();
    rounds.push(first);
    while rounds.last().map_or(0, Vec::len) > 1 {
        let round = rounds.len() as u32 + 1;
        let prev = rounds.last().expect("a round").clone();
        let next = prev
            .chunks(2)
            .map(|p| {
                push(
                    out,
                    Stage::Winners,
                    round,
                    [Source::Winner(p[0]), Source::Winner(p[1])],
                )
            })
            .collect();
        rounds.push(next);
    }
    rounds
}

/// Push the losers bracket fed by `winners` (at least two rounds); returns the losers final's index.
fn losers_rounds(out: &mut Vec<BracketMatch>, winners: &[Vec<u32>]) -> u32 {
    let mut round = 1;
    let mut current: Vec<u32> = winners[0]
        .chunks(2)
        .map(|p| {
            push(
                out,
                Stage::Losers,
                round,
                [Source::Loser(p[0]), Source::Loser(p[1])],
            )
        })
        .collect();
    for (r, dropping) in winners.iter().enumerate().skip(1) {
        // Drop-in round: the survivors meet this winners round's losers, reversed every other round.
        round += 1;
        let reversed = r % 2 == 1;
        let len = dropping.len();
        current = (0..len)
            .map(|j| {
                let from = if reversed {
                    dropping[len - 1 - j]
                } else {
                    dropping[j]
                };
                push(
                    out,
                    Stage::Losers,
                    round,
                    [Source::Winner(current[j]), Source::Loser(from)],
                )
            })
            .collect();
        if r + 1 < winners.len() {
            // Consolidation round: the survivors play each other.
            round += 1;
            current = current
                .chunks(2)
                .map(|p| {
                    push(
                        out,
                        Stage::Losers,
                        round,
                        [Source::Winner(p[0]), Source::Winner(p[1])],
                    )
                })
                .collect();
        }
    }
    current[0]
}

fn push(out: &mut Vec<BracketMatch>, stage: Stage, round: u32, sources: [Source; 2]) -> u32 {
    out.push(BracketMatch {
        stage,
        round,
        sources,
    });
    out.len() as u32 - 1
}

/// Who sits where and who won, given `results[m]` for each match `m` (`None`: not decided). A result for a
/// match that does not have two teams in it is ignored: byes decide themselves.
pub fn resolve(bracket: &[BracketMatch], results: &[Option<Side>]) -> Vec<MatchState> {
    let mut states: Vec<MatchState> = Vec::with_capacity(bracket.len());
    for (i, m) in bracket.iter().enumerate() {
        let seat = |s: Source| match s {
            Source::Seed(t) => Seat::Team(t),
            Source::Bye => Seat::Empty,
            Source::Winner(j) => states[j as usize].winner,
            Source::Loser(j) => states[j as usize].loser,
        };
        let seats = [seat(m.sources[0]), seat(m.sources[1])];
        let (winner, loser) = match seats {
            [Seat::Pending, _] | [_, Seat::Pending] => (Seat::Pending, Seat::Pending),
            [Seat::Empty, Seat::Empty] => (Seat::Empty, Seat::Empty),
            [team @ Seat::Team(_), Seat::Empty] | [Seat::Empty, team @ Seat::Team(_)] => {
                (team, Seat::Empty)
            }
            [a, b] => match results.get(i).copied().flatten() {
                Some(Side::A) => (a, b),
                Some(Side::B) => (b, a),
                None => (Seat::Pending, Seat::Pending),
            },
        };
        states.push(MatchState {
            seats,
            winner,
            loser,
        });
    }
    states
}

/// The bracket's champion: the winner of its last match (`Pending` until it is decided).
pub fn champion(states: &[MatchState]) -> Seat {
    states.last().map_or(Seat::Pending, |s| s.winner)
}

/// One map of a pairing, from the pairing's side A: the map's outcome (`None`: it has no result, e.g. it
/// could not be run or ended unfinished) and the rounds each side won on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MapResult {
    pub outcome: Option<Outcome>,
    pub rounds: [u32; 2],
}

/// What decided a pairing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DecidedBy {
    /// More maps won.
    Maps,
    /// Level on maps; more rounds won across all maps (regulation and overtime alike).
    Rounds,
    /// Level on both; the higher seed advanced. Organisers should look at it.
    Seed,
}

impl DecidedBy {
    pub fn name(self) -> &'static str {
        match self {
            DecidedBy::Maps => "maps",
            DecidedBy::Rounds => "rounds",
            DecidedBy::Seed => "seed",
        }
    }
}

/// A decided pairing. Maps are counted in halves so that a drawn map is half a map to each side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PairingResult {
    pub map_halves: [u32; 2],
    pub rounds_won: [u32; 2],
    pub winner: Side,
    pub decided_by: DecidedBy,
}

/// Decide a pairing: more maps won (a draw is half to each); if level, more rounds won across the maps; if
/// still level, the higher seed (smaller 0-based `seeds` entry) advances.
pub fn decide(maps: &[MapResult], seeds: [u32; 2]) -> PairingResult {
    let mut halves = [0u32; 2];
    let mut rounds = [0u32; 2];
    for m in maps {
        match m.outcome {
            Some(Outcome::AWins) => halves[0] = halves[0].saturating_add(2),
            Some(Outcome::BWins) => halves[1] = halves[1].saturating_add(2),
            Some(Outcome::Draw) => {
                halves[0] = halves[0].saturating_add(1);
                halves[1] = halves[1].saturating_add(1);
            }
            None => {}
        }
        // Scores come from workers: saturate rather than overflow on nonsense.
        rounds[0] = rounds[0].saturating_add(m.rounds[0]);
        rounds[1] = rounds[1].saturating_add(m.rounds[1]);
    }
    let by = |x: [u32; 2]| (x[0] != x[1]).then_some(if x[0] > x[1] { Side::A } else { Side::B });
    let (winner, decided_by) = if let Some(side) = by(halves) {
        (side, DecidedBy::Maps)
    } else if let Some(side) = by(rounds) {
        (side, DecidedBy::Rounds)
    } else if seeds[0] <= seeds[1] {
        (Side::A, DecidedBy::Seed)
    } else {
        (Side::B, DecidedBy::Seed)
    };
    PairingResult {
        map_halves: halves,
        rounds_won: rounds,
        winner,
        decided_by,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Play every playable match until the bracket is decided; `pick` chooses the winner of each.
    fn play(bracket: &[BracketMatch], pick: impl Fn(u32, u32) -> u32) -> (Vec<MatchState>, usize) {
        let mut results = vec![None; bracket.len()];
        let mut played = 0;
        loop {
            let states = resolve(bracket, &results);
            let ready: Vec<usize> = (0..states.len())
                .filter(|&i| states[i].playable())
                .collect();
            if ready.is_empty() {
                return (states, played);
            }
            for i in ready {
                let [Seat::Team(a), Seat::Team(b)] = states[i].seats else {
                    unreachable!()
                };
                results[i] = Some(if pick(a, b) == a { Side::A } else { Side::B });
                played += 1;
            }
        }
    }

    fn better(a: u32, b: u32) -> u32 {
        a.min(b)
    }

    #[test]
    fn seed_order_is_standard() {
        assert_eq!(seed_order(1), vec![0]);
        assert_eq!(seed_order(2), vec![0, 1]);
        assert_eq!(seed_order(4), vec![0, 3, 1, 2]);
        assert_eq!(seed_order(8), vec![0, 7, 3, 4, 1, 6, 2, 5]);
        // Every first-round pair sums to size - 1 (1 v N, 2 v N-1, ...).
        for size in [16, 32, 64] {
            let order = seed_order(size);
            assert!(order.chunks(2).all(|p| p[0] + p[1] == size - 1));
            let mut sorted = order.clone();
            sorted.sort();
            assert_eq!(sorted, (0..size).collect::<Vec<_>>());
        }
    }

    #[test]
    fn single_elimination_with_byes() {
        let b = build(Format::Single, 5);
        // 8-slot bracket: 4 + 2 + 1 matches.
        assert_eq!(b.len(), 7);
        assert_eq!(b[0].sources, [Source::Seed(0), Source::Bye]);
        assert_eq!(b[1].sources, [Source::Seed(3), Source::Seed(4)]);
        assert_eq!(b[2].sources, [Source::Seed(1), Source::Bye]);
        assert_eq!(b[3].sources, [Source::Seed(2), Source::Bye]);
        let states = resolve(&b, &vec![None; b.len()]);
        // Byes decide themselves; only 4v5 is playable at first, then 2v3 once its feeders are known.
        assert_eq!(states[0].winner, Seat::Team(0));
        assert!(states[1].playable());
        assert!(states[5].playable());
        assert_eq!(states[5].seats, [Seat::Team(1), Seat::Team(2)]);
        assert_eq!(states[4].seats, [Seat::Team(0), Seat::Pending]);
        let (states, played) = play(&b, better);
        // n - 1 series in single elimination.
        assert_eq!(played, 4);
        assert_eq!(champion(&states), Seat::Team(0));
    }

    #[test]
    fn single_elimination_sizes() {
        for n in 2..=40 {
            let b = build(Format::Single, n);
            let (states, played) = play(&b, better);
            assert_eq!(played as u32, n - 1, "n = {n}");
            assert_eq!(champion(&states), Seat::Team(0));
            // The underdog always winning crowns the lowest seed.
            let (states, _) = play(&b, |a, b| a.max(b));
            assert_eq!(champion(&states), Seat::Team(n - 1), "n = {n}");
        }
        assert!(build(Format::Single, 1).is_empty());
        assert_eq!(champion(&[]), Seat::Pending);
    }

    #[test]
    fn double_elimination_shape() {
        let b = build(Format::Double, 8);
        let count = |s: Stage| b.iter().filter(|m| m.stage == s).count();
        assert_eq!(count(Stage::Winners), 7);
        // 2 (k - 1) = 4 losers rounds of 2, 2, 1, 1 matches.
        assert_eq!(count(Stage::Losers), 6);
        assert_eq!(count(Stage::Final), 1);
        let rounds: Vec<u32> = b
            .iter()
            .filter(|m| m.stage == Stage::Losers)
            .map(|m| m.round)
            .collect();
        assert_eq!(rounds, vec![1, 1, 2, 2, 3, 4]);
        // Every feeder comes before the match it feeds.
        for (i, m) in b.iter().enumerate() {
            for s in m.sources {
                if let Source::Winner(j) | Source::Loser(j) = s {
                    assert!((j as usize) < i);
                }
            }
        }
        // Every match's loser goes somewhere exactly once, except the final's.
        for j in 0..b.len() as u32 - 1 {
            let fed = b
                .iter()
                .flat_map(|m| m.sources)
                .filter(|s| *s == Source::Loser(j))
                .count();
            let expected = usize::from(b[j as usize].stage == Stage::Winners);
            assert_eq!(fed, expected, "match {j}");
        }
    }

    #[test]
    fn double_elimination_everyone_loses_twice() {
        for n in 3..=33 {
            let b = build(Format::Double, n);
            let (states, played) = play(&b, better);
            assert_eq!(champion(&states), Seat::Team(0), "n = {n}");
            // Without a bracket reset: every team but the champion loses twice, except that the grand
            // final's loser may have lost only once. 2n - 3 or 2n - 2 series.
            assert!(
                played as u32 == 2 * n - 2 || played as u32 == 2 * n - 3,
                "n = {n}: {played}"
            );
            let mut losses = vec![0; n as usize];
            for s in &states {
                if let (Seat::Team(l), true) =
                    (s.loser, matches!(s.seats, [Seat::Team(_), Seat::Team(_)]))
                {
                    losses[l as usize] += 1;
                }
            }
            assert_eq!(losses[0], 0);
            let finalist_loser = match states.last().unwrap().loser {
                Seat::Team(t) => t,
                _ => panic!(),
            };
            for (t, l) in losses.iter().enumerate().skip(1) {
                if t as u32 == finalist_loser {
                    assert!(*l == 1 || *l == 2);
                } else {
                    assert_eq!(*l, 2, "n = {n}, team {t}");
                }
            }
        }
    }

    #[test]
    fn double_elimination_upset_path() {
        // Seed 2 (index 1) loses early in the winners bracket and wins the losers bracket and the final.
        let b = build(Format::Double, 4);
        let met_three = std::cell::Cell::new(false);
        let (states, played) = play(&b, |a, b| {
            if (a, b) == (1, 2) && !met_three.replace(true) {
                return 2;
            }
            if a == 1 || b == 1 {
                return 1;
            }
            a.min(b)
        });
        assert_eq!(champion(&states), Seat::Team(1));
        // Two winners round 1 matches, losers 1, winners final, losers final, grand final.
        assert_eq!(played, 6);
        assert_eq!(states.last().unwrap().seats, [Seat::Team(0), Seat::Team(1)]);
    }

    #[test]
    fn two_entrants_double_is_one_match() {
        assert_eq!(build(Format::Double, 2), build(Format::Single, 2));
        assert_eq!(build(Format::Double, 2).len(), 1);
    }

    fn map(outcome: Option<Outcome>, a: u32, b: u32) -> MapResult {
        MapResult {
            outcome,
            rounds: [a, b],
        }
    }

    #[test]
    fn tie_break_chain() {
        use Outcome::*;
        // Maps first, whatever the rounds say.
        let r = decide(
            &[
                map(Some(AWins), 7, 6),
                map(Some(BWins), 0, 7),
                map(Some(AWins), 7, 5),
            ],
            [1, 0],
        );
        assert_eq!(
            (r.winner, r.decided_by, r.map_halves),
            (Side::A, DecidedBy::Maps, [4, 2])
        );
        assert_eq!(r.rounds_won, [14, 18]);
        // A drawn map is half each: 1.5 - 1.5, then rounds.
        let r = decide(
            &[
                map(Some(AWins), 7, 3),
                map(Some(BWins), 5, 7),
                map(Some(Draw), 6, 6),
            ],
            [0, 1],
        );
        assert_eq!(
            (r.winner, r.decided_by, r.map_halves),
            (Side::A, DecidedBy::Rounds, [3, 3])
        );
        // Level on maps and rounds: the higher seed (smaller index) advances, from either side.
        let level = [
            map(Some(AWins), 7, 5),
            map(Some(BWins), 5, 7),
            map(Some(Draw), 6, 6),
        ];
        let r = decide(&level, [3, 2]);
        assert_eq!((r.winner, r.decided_by), (Side::B, DecidedBy::Seed));
        let r = decide(&level, [2, 3]);
        assert_eq!((r.winner, r.decided_by), (Side::A, DecidedBy::Seed));
        // Maps without a result count no map but keep their rounds.
        let r = decide(&[map(None, 4, 2), map(Some(Draw), 6, 6)], [1, 0]);
        assert_eq!(
            (r.winner, r.decided_by, r.map_halves),
            (Side::A, DecidedBy::Rounds, [1, 1])
        );
        // Nothing played at all: seed.
        let r = decide(&[], [1, 0]);
        assert_eq!((r.winner, r.decided_by), (Side::B, DecidedBy::Seed));
    }
}
