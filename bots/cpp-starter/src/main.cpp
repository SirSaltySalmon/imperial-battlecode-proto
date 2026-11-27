// cpp-starter: a small but complete C++ bot. Copy this directory and start changing it.
//
// Buy:     the best affordable weapon, then armour, then (defenders) a defuse kit.
// Attack:  everyone runs to the site nearest their spawn; the carrier plants, the rest guard the charge.
// Defence: run at the last enemy sound, else sweep the attackers' spawn and the sites; defuse a planted charge.
// Always:  shoot the nearest visible enemy, leading it; stand still when moving would spoil the shot.
//
// Build and run:  ibc build bots/cpp-starter && ibc run bots/cpp-starter bots/rusher --map maps/tunnels.ibcmap
#include "ibc.h"

using namespace ibc;

namespace {

/// A heard enemy position is chased for this many ticks.
constexpr int32_t SOUND_MEMORY_TICKS = 12;
/// Keep channelling a plant or defuse under fire when this few ticks remain.
constexpr int32_t FINISH_CHANNEL_TICKS = 2;
/// Interact from well inside the allowed reach, so rounding never puts us outside it.
constexpr int32_t REACH_PCT = 70;
/// `MatchInfo::charge_state` value: the charge lies on the ground.
constexpr int32_t CHARGE_DROPPED = 1;

class Starter {
  public:
    void turn() {
        const UnitInfo me = ibc::me();  // fetch once per tick
        if (!alive(me)) return;
        const MatchInfo info = match_info();
        if (info.phase == PHASE_DRAFT) return;  // sides and spawns are not set yet
        // Lands on the first buy tick of a round, where the fuel budget is larger.
        if (plan_round_ != info.round || plan_side_ != info.my_side) plan(info);
        if (info.phase == PHASE_BUY) {
            shop(me);
        } else if (info.phase == PHASE_ACTION || info.phase == PHASE_PLANTED) {
            fight(me, info);
        }
    }

  private:
    /// Once per round: pick the site to rush (nearest to our spawn by path length).
    void plan(const MatchInfo& info) {
        plan_round_ = info.round;
        plan_side_ = info.my_side;
        map_ = map_info();  // spawns swap with the sides
        heard_tick_ = -1;
        sweep_ = 0;
        const Point spawn{map_.spawn_x, map_.spawn_y};
        int32_t best = INT32_MAX;
        for (const SiteInfo& s : sites()) {
            const Point point = plant_point(s);  // a site tile, unlike the box centre
            const int32_t d = nav_.path_len(spawn, point).value_or(INT32_MAX - 1);
            if (d < best) {
                best = d;
                site_ = point;
            }
        }
    }

    /// Buy one thing per buy tick, most valuable first.
    void shop(const UnitInfo& me) {
        const int32_t held_price = weapon(me.weapon).value_or(WeaponRules{}).price;
        int32_t best_priority = 0, best_item = -1;
        for (const ItemRules& it : items()) {
            if (it.price > me.credits) continue;
            int32_t priority = 0;
            if (it.kind == ITEM_KIND_WEAPON && it.value != me.weapon && it.price > held_price) {
                priority = 2'000'000 + it.price;
            } else if (it.kind == ITEM_KIND_ARMOR && it.value > me.armor) {
                priority = 1'000'000 + it.value;
            } else if (it.kind == ITEM_KIND_DEFUSE_KIT && me.side == SIDE_DEFEND && !has(me, S_HAS_KIT)) {
                priority = 1;
            }
            if (priority > best_priority) {
                best_priority = priority;
                best_item = it.id;
            }
        }
        if (best_item >= 0) buy(best_item);
    }

    /// Rules of the weapon in hand (one host call when it changes).
    const WeaponRules& gun(const UnitInfo& me) {
        if (gun_.id != me.weapon) {
            gun_ = weapon(me.weapon).value_or(WeaponRules{});
            gun_.id = me.weapon;
        }
        return gun_;
    }

    struct Engage {
        bool fighting;  // an enemy is in sight
        bool hold;      // shooting: moving this tick would cancel the startup or break the brace
    };

    /// Face the nearest enemy with no wall in between and shoot it whenever the weapon is ready.
    /// `enemies()` is the team's sight: an enemy only a teammate sees behind a wall is skipped. Shooting
    /// roots the unit: a move cancels the startup and breaks the brace, so the caller stands still while
    /// `hold` is set.
    Engage engage(const UnitInfo& me) {
        const Point from = pos(me);
        UnitInfo target{};
        int64_t nearest = INT64_MAX;
        for (const UnitInfo& e : enemies()) {
            if (alive(e) && dist2(from, pos(e)) < nearest && los(from, pos(e))) {
                nearest = dist2(from, pos(e));
                target = e;
            }
        }
        if (nearest == INT64_MAX) return {false, false};
        const WeaponRules& w = gun(me);
        // Braced, bullets leave on the pull; otherwise after the startup (windup plus unsteadiness).
        const int32_t startup = has(me, S_BRACED)
            ? 0
            : w.windup + static_cast<int32_t>((int64_t{me.unsteadiness > 0 ? me.unsteadiness : 0} * w.startup_per_point + 50) / 100);
        const int32_t aim = angle_to(from, lead(from, pos(target), vel(target), w.bullet_speed, startup));
        const int32_t distance = dist(from, pos(target));
        face(aim);  // bullets leave along the facing, so keep turning during the startup
        bool shooting = has(me, S_WINDUP | S_BRACED);
        if (!has(me, S_WINDUP | S_RELOADING)) {
            if (me.ammo <= 0) {
                reload();
            } else if (distance <= w.range && ok(fire(aim))) {
                shooting = true;
            }
        }
        return {true, shooting};
    }

    void fight(const UnitInfo& me, const MatchInfo& info) {
        // A plant or defuse that is nearly done is worth more than one more shot.
        if (has(me, S_PLANTING | S_DEFUSING) && me.channel <= FINISH_CHANNEL_TICKS) {
            has(me, S_PLANTING) ? plant() : defuse();
            return;
        }
        const Engage e = engage(me);
        const bool used_hands = info.my_side == SIDE_ATTACK ? attack(me, info, e) : defend(me, info, e);
        const WeaponRules& w = gun(me);
        if (!e.fighting && !used_hands && !busy(me) && me.ammo * 2 <= w.magazine && me.ammo < w.magazine) reload();
    }

    /// Returns true if the tick's one "hands" action (plant, pickup) was used.
    bool attack(const UnitInfo& me, const MatchInfo& info, Engage e) {
        const Point charge{info.charge_x, info.charge_y};
        Point goal = site_;
        if (info.phase == PHASE_PLANTED) {
            goal = charge;  // guard it until it goes off
        } else if (has(me, S_HAS_CHARGE)) {
            // Standing on the site: plant unless someone is shooting at us.
            if (dist(pos(me), site_) <= TILE / 4) return !e.fighting && ok(plant());
        } else if (info.charge_state == CHARGE_DROPPED && nearest_ally_to(me, charge)) {
            if (dist(pos(me), charge) <= core_.pickup_reach * REACH_PCT / 100) return !e.fighting && ok(pickup());
            goal = charge;
        }
        travel(me, goal, e);
        return false;
    }

    /// Returns true if the tick's one "hands" action (defuse) was used.
    bool defend(const UnitInfo& me, const MatchInfo& info, Engage e) {
        for (const SoundInfo& s : sounds()) {
            if (s.team != me.team && s.kind != SOUND_DECOY) {
                heard_ = {s.x, s.y};
                heard_tick_ = info.tick;
            }
        }
        const bool chasing = heard_tick_ >= 0 && info.tick - heard_tick_ <= SOUND_MEMORY_TICKS;
        const Point charge{info.charge_x, info.charge_y};
        Point goal = chasing ? heard_ : sweep_target();
        if (info.phase == PHASE_PLANTED) {
            if (dist(pos(me), charge) <= core_.interact_reach * REACH_PCT / 100) return !e.fighting && ok(defuse());
            goal = charge;
        }
        if (travel(me, goal, e) && info.phase != PHASE_PLANTED) {
            // Nothing here: forget the sound, or move on to the next sweep point.
            if (chasing) {
                heard_tick_ = -1;
            } else {
                ++sweep_;
            }
        }
        return false;
    }

    /// Run to `goal` (unless the gun wants us still), looking where we walk. True on arrival.
    bool travel(const UnitInfo& me, Point goal, Engage e) {
        if (e.hold) return false;
        const bool arrived = nav_.go(me, goal, core_.run_speed);
        if (!e.fighting && !arrived) face(nav_.heading);
        return arrived;
    }

    /// Am I the living teammate closest to `p`? (Ties go to the lower id.)
    bool nearest_ally_to(const UnitInfo& me, Point p) const {
        const int64_t mine = dist2(pos(me), p);
        for (const UnitInfo& a : allies()) {
            const int64_t theirs = dist2(pos(a), p);
            if (alive(a) && (theirs < mine || (theirs == mine && a.id < me.id))) return false;
        }
        return true;
    }

    /// Defence sweep route: the attackers' spawn first, then every site in turn.
    Point sweep_target() const {
        const uint32_t stops = static_cast<uint32_t>(map_.sites > 0 ? map_.sites : 0) + 1;
        const uint32_t stop = sweep_ % stops;
        if (stop > 0) {
            if (const auto s = sites().at(stop - 1)) return plant_point(*s);
        }
        return {map_.enemy_spawn_x, map_.enemy_spawn_y};
    }

    CoreRules core_ = core_rules();
    MapInfo map_ = map_info();
    Navigator nav_;
    WeaponRules gun_{.id = -1};
    /// (round, side) the plan below was made for.
    int32_t plan_round_ = -1, plan_side_ = -1;
    /// Attack: plant point of the site we rush.
    Point site_{};
    /// Defence: last heard enemy position and the tick it was heard (-1 = none).
    Point heard_{};
    int32_t heard_tick_ = -1;
    /// Defence: index into the sweep route.
    uint32_t sweep_ = 0;
};

}  // namespace

IBC_BOT(Starter)
