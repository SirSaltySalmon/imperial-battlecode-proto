// {{name}}: a small bot to grow from.
//
// Buy:     the best affordable weapon, then armour, then (defenders) a defuse kit.
// Attack:  everyone runs to the first site; the carrier plants there.
// Defence: hold the first site; once the charge is planted, run to it and defuse.
// Always:  shoot the nearest visible enemy.
//
// Build and play:  ibc build bots/{{name}}  &&  ibc run bots/{{name}}
// The rules are in docs/RULES.md, the API in docs/SDK-CPP.md and docs/ABI.md.
#include "ibc.h"

using namespace ibc;

namespace {

/// Interact from well inside the allowed reach, so rounding never puts us outside it.
constexpr int32_t REACH_PCT = 70;

class {{type}} {
  public:
    void turn() {
        const UnitInfo me = ibc::me();  // fetch once per tick: every host call costs fuel
        if (!alive(me)) return;
        const MatchInfo info = match_info();
        if (info.phase == PHASE_BUY) {
            shop(me);
        } else if (info.phase == PHASE_ACTION || info.phase == PHASE_PLANTED) {
            fight(me, info);
        }
    }

  private:
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

    /// Face the nearest visible enemy and shoot. Returns false when no enemy is in sight.
    bool engage(const UnitInfo& me) {
        const Point from = pos(me);
        int64_t nearest = INT64_MAX;
        Point target{};
        for (const UnitInfo& e : enemies()) {
            if (alive(e) && dist2(from, pos(e)) < nearest) {
                nearest = dist2(from, pos(e));
                target = pos(e);
            }
        }
        if (nearest == INT64_MAX) return false;
        const int32_t aim = angle_to(from, target);
        face(aim);  // bullets leave along the facing
        if (!has(me, S_WINDUP | S_RELOADING)) {
            if (me.ammo <= 0) {
                reload();
            } else {
                fire(aim);
            }
        }
        return true;
    }

    void fight(const UnitInfo& me, const MatchInfo& info) {
        if (engage(me)) return;  // an enemy is in sight: stand and shoot
        const auto site = sites().at(0);
        if (!site) return;
        const Point goal{site->centre_x, site->centre_y};
        const Point charge{info.charge_x, info.charge_y};
        const bool attacking = info.my_side == SIDE_ATTACK;
        if (!attacking && info.phase == PHASE_PLANTED) {
            if (dist(pos(me), charge) <= core_.interact_reach * REACH_PCT / 100) {
                defuse();
            } else {
                travel(me, charge);
            }
            return;
        }
        if (travel(me, goal) && attacking && has(me, S_HAS_CHARGE) && info.phase == PHASE_ACTION) plant();
    }

    /// Walk to `goal` around walls, looking where we go (you only see inside your view cone). True on arrival.
    bool travel(const UnitInfo& me, Point goal) {
        const bool arrived = nav_.go(me, goal, core_.run_speed);
        if (!arrived) face(nav_.heading);
        return arrived;
    }

    // Rules and the map never change: read them once. Never hard-code a number the rules give you.
    CoreRules core_ = core_rules();
    Navigator nav_;
};

}  // namespace

IBC_BOT({{type}})
