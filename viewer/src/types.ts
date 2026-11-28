// Shapes and ids the viewer reads. Ids mirror `crates/abi/src/lib.rs`; the frame shape mirrors
// `frame_json` in `crates/engine/src/host.rs`; `Header` mirrors `crates/replay/src/lib.rs`.

/** Fixed-point units per tile. */
export const TILE = 1024;
/** Angle units per full turn. 0 = +x, growing towards +y (clockwise on screen). */
export const FULL_TURN = 65536;

export const TILE_FLOOR = 0;
export const TILE_WALL = 1;
export const TILE_LOW = 2;

export const PHASE_DRAFT = 0;
export const PHASE_BUY = 1;
export const PHASE_ACTION = 2;
export const PHASE_PLANTED = 3;
export const PHASE_ROUND_END = 4;
export const PHASE_MATCH_END = 5;

export const S_ALIVE = 1 << 0;
export const S_WINDUP = 1 << 1;
export const S_RELOADING = 1 << 2;
export const S_PLANTING = 1 << 3;
export const S_DEFUSING = 1 << 4;
export const S_BLINDED = 1 << 5;
export const S_HAS_CHARGE = 1 << 6;
export const S_RUNNING = 1 << 7;
export const S_HAS_KIT = 1 << 8;
export const S_SLOWED = 1 << 9;
export const S_REVEALED = 1 << 10;
export const S_CASTING = 1 << 12;

export const SOUND_STEP = 1;
export const SOUND_SHOT = 2;
export const SOUND_PLANT = 3;
export const SOUND_DEFUSE = 4;
export const SOUND_ABILITY = 5;
export const SOUND_DECOY = 6;

export const EFFECT_SMOKE = 1;
export const EFFECT_FIRE = 2;
export const EFFECT_BARRIER = 3;
export const EFFECT_FLASH = 4;
export const EFFECT_RECON = 5;
export const EFFECT_HEAL = 6;
export const EFFECT_DECOY = 7;
/** 8 is the dash, which never appears in the world. */
export const EFFECT_TRAP = 9;
export const EFFECT_SONAR = 10;
export const EFFECT_CONCUSSION = 11;

export const DROP_CHARGE = 1;
export const DROP_WEAPON = 2;

export const CHARGE_CARRIED = 0;
export const CHARGE_DROPPED = 1;
export const CHARGE_PLANTED = 2;

export const EVENT_KILL = 1;
export const EVENT_PLANT = 2;
export const EVENT_DEFUSE = 3;
export const EVENT_DETONATE = 4;
export const EVENT_ROUND_END = 5;
export const EVENT_DAMAGE = 6;
/** An effect gave a unit a status (blinded, slowed, revealed); `value` is the `EFFECT_*` kind. */
export const EVENT_EFFECT = 7;

export const ROUND_WIN_REASONS: Record<number, string> = {
  1: "elimination",
  2: "detonation",
  3: "defuse",
  4: "time ran out",
};

export const DEBUG_LINE = 1;
export const DEBUG_DOT = 2;

export const SIDE_ATTACK = 0;

export interface UnitFrame {
  id: number;
  team: number;
  side: number;
  kit: number;
  alive: boolean;
  state: number;
  x: number;
  y: number;
  facing: number;
  vx: number;
  vy: number;
  hp: number;
  armor: number;
  weapon: number;
  ammo: number;
  /** Ticks left in the current channel (wind-up, reload, plant, defuse). */
  channel: number;
  aim: number;
  credits: number;
  /** Bitmask of unit ids this unit sees, a u64 as a decimal string. */
  seen: string;
  kills: number;
  deaths: number;
  /** `[kind, x, y]` per sound heard. */
  sounds: [number, number, number][];
}

export interface ProjectileFrame {
  owner: number;
  team: number;
  weapon: number;
  kind: number;
  x: number;
  y: number;
  vx: number;
  vy: number;
}

/** A bullet that ended during the tick into this frame: where it was when the tick began, and where it ended. */
export interface TrailFrame {
  owner: number;
  team: number;
  weapon: number;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  vx: number;
  vy: number;
}

export interface EffectFrame {
  kind: number;
  team: number;
  x: number;
  y: number;
  radius: number;
  ticks_left: number;
  x2: number;
  y2: number;
}

export interface DropFrame {
  kind: number;
  x: number;
  y: number;
  item: number;
  /** A weapon's magazine on the ground. */
  ammo: number;
}

export interface EventFrame {
  kind: number;
  /** The true actor: frames are not fogged, so it is set even where bots read -1. */
  actor: number;
  target: number;
  x: number;
  y: number;
  value: number;
  /**
   * What the target alone learned of where a hit came from (`EVENT_DAMAGE`, `EVENT_EFFECT`): the angle from it
   * back along a bullet or towards an effect's centre, else -1. Absent from older engine builds.
   */
  dir?: number;
  /** The effect's centre for an effect's hit, else -2147483648 (a bullet never tells where its shooter is). */
  src_x?: number;
  src_y?: number;
}

/**
 * What a unit carries, added to frames by `crates/engine-wasm/src/view.rs` (absent from older engine builds).
 * Indexed like the abilities and weapon slots of the rules: slot 0 primary / signature, slot 1 sidearm / bought.
 */
export interface LoadoutFrame {
  id: number;
  /** Weapon id per slot, -1 = empty. */
  weapons: [number, number];
  ammo: [number, number];
  /** Slot in hand. */
  slot: number;
  charges: [number, number];
  cooldown: [number, number];
  has_kit: boolean;
  damage: number;
  faulted: boolean;
}

/** `[unit, kind, x0, y0, x1, y1, colour 0xRRGGBB]`. */
export type DebugDraw = [number, number, number, number, number, number, number];

export interface Frame {
  tick: number;
  round: number;
  phase: number;
  phase_left: number;
  score: [number, number];
  /** Team id that attacks this round. */
  attackers: number;
  charge: { state: number; x: number; y: number };
  units: UnitFrame[];
  projectiles: ProjectileFrame[];
  /** Absent from frames of engines before trails. */
  trails?: TrailFrame[];
  effects: EffectFrame[];
  drops: DropFrame[];
  events: EventFrame[];
  debug: DebugDraw[];
  winner: number | null;
  loadout?: LoadoutFrame[];
}

export interface BotInfo {
  name: string;
  sha256: string;
  fuel_multiplier: number;
}

export interface Header {
  engine_version: string;
  ruleset_hash: string;
  map_hash: string;
  map_text: string;
  /** u64; kept as the decimal text of the JSON so no precision is lost. */
  seed: string;
  wasmtime_version: string;
  abi_version: number;
  bots: [BotInfo, BotInfo];
  result: { winner: number | null; score: [number, number]; ticks: number; finished: boolean };
  /** Units per team: ids below this are team A. */
  team_size: number;
  /** Teams whose memory writes, logs, fuel and debug drawings were removed from this file. */
  redacted: [boolean, boolean];
  /** The rules TOML texts; the engine turns them into `Engine.rules()`. Absent in some replays. */
  rules?: { core: string; weapons: string; abilities: string; economy: string };
}

export type Vec2 = [number, number];
