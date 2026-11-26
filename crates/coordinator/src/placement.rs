//! The placement task: a team with a new live submission plays its first ranked series back to back, each
//! as soon as the last one ends, instead of one per scheduler round. Its rating is near its place within
//! minutes of the upload rather than after an hour of rounds (`crates/rating/README.md`, Placement).

use crate::error::ApiResult;
use crate::{db, patch, util, App};
use ibc_rating::{MatchmakingConfig, TeamState};

impl App {
    /// One pass of the placement task. Every placing team (`db::placement`) with no ranked series running,
    /// not left out of pairing by a patch, and under `placement_daily_series` gets one ranked series
    /// against the opponent `ibc_rating::plan_placement` picks. Returns the ids of the series created;
    /// none while a tournament lock pauses ranked matchmaking.
    pub fn placement_round(&self) -> ApiResult<Vec<String>> {
        if self.cfg.placement_series <= 0 {
            return Ok(Vec::new());
        }
        let now = util::now_ms();
        let created = self.db.tx(|tx| {
            if db::tournaments::ranked_paused(tx, now)?.is_some() {
                return Ok(Vec::new());
            }
            let placing = db::placement::placing_teams(tx, self.cfg.placement_series)?;
            if placing.is_empty() {
                return Ok(Vec::new());
            }
            let started = db::placement::started_since(tx, now - 86_400_000)?;
            let pool = db::league::matchmaking_pool(tx, self.rating.as_ref(), now)?;
            let excluded = patch::excluded_teams(self, tx, now)?;
            // The pool marks a team with a ranked series running as not live. An opponent may be busy: a
            // placement series does not wait for the other side's series to end.
            let states: Vec<TeamState> = pool
                .iter()
                .map(|(t, s)| TeamState {
                    live: !excluded.contains(&t.id),
                    ..s.clone()
                })
                .collect();
            let mut created = Vec::new();
            for (i, (team, state)) in pool.iter().enumerate() {
                let due = state.live
                    && states[i].live
                    && placing.contains(&team.id)
                    && started.get(&team.id).copied().unwrap_or(0)
                        < self.cfg.placement_daily_series;
                if !due {
                    continue;
                }
                let seed = u64::from_str_radix(&util::random_hex(8), 16).expect("16 hex digits");
                let cfg = MatchmakingConfig::default();
                let Some(o) = ibc_rating::plan_placement(
                    self.rating.system(),
                    &states[i],
                    &states,
                    &cfg,
                    seed,
                ) else {
                    continue;
                };
                let opponent = &pool[o as usize].0;
                let id = self.new_series(
                    tx,
                    "ranked",
                    team,
                    opponent,
                    self.cfg.ranked_games,
                    None,
                    now,
                )?;
                db::placement::mark(tx, &id, &team.id)?;
                created.push(id);
            }
            Ok(created)
        })?;
        if !created.is_empty() {
            self.queue_changed.notify_waiters();
        }
        Ok(created)
    }
}
