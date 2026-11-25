// Colours, in one place for the canvas and the page (the page gets them as CSS variables).
//
// Team colours are a blue / orange pair that stays apart under the common colour-vision deficiencies, and the
// teams also differ in shape on the map (solid disc versus ring), so colour is never the only cue.

export interface Palette {
  bg: string;
  panel: string;
  panel2: string;
  border: string;
  text: string;
  dim: string;
  floor: string;
  grid: string;
  wall: string;
  wallEdge: string;
  low: string;
  lowEdge: string;
  site: string;
  /** Team A, team B. */
  team: [string, string];
  /** Text drawn on a team-coloured fill. */
  onTeam: string;
  charge: string;
  warn: string;
  hp: string;
  hpLow: string;
  armor: string;
  barBack: string;
  smoke: string;
  fire: string;
  heal: string;
  flash: string;
  sound: string;
  fog: string;
}

export type ThemeName = "dark" | "light";

export const PALETTES: Record<ThemeName, Palette> = {
  dark: {
    bg: "#0d1014",
    panel: "#151a21",
    panel2: "#1c232c",
    border: "#2a333f",
    text: "#e6ebf2",
    dim: "#8b97a8",
    floor: "#1a2029",
    grid: "#222a35",
    wall: "#56637a",
    wallEdge: "#6f7d96",
    low: "#2c3644",
    lowEdge: "#7c8aa3",
    site: "#c77dba",
    team: ["#4fb0ff", "#ff9a3d"],
    onTeam: "#0b0e12",
    charge: "#f25fd0",
    warn: "#ffe45c",
    hp: "#dfe6ee",
    hpLow: "#ff5d5d",
    armor: "#8fa3c2",
    barBack: "#05070a",
    smoke: "#aab4c2",
    fire: "#ff5a36",
    heal: "#5fd6a4",
    flash: "#ffffff",
    sound: "#c9d3e0",
    fog: "rgba(4, 6, 9, 0.62)",
  },
  light: {
    bg: "#eef0f3",
    panel: "#ffffff",
    panel2: "#f3f5f8",
    border: "#d3d9e1",
    text: "#18202b",
    dim: "#5d6978",
    floor: "#fbfbf9",
    grid: "#eceef1",
    wall: "#3f4a5c",
    wallEdge: "#2b3442",
    low: "#cfd6df",
    lowEdge: "#5d6978",
    site: "#a2428f",
    team: ["#0a6fcf", "#d06000"],
    onTeam: "#ffffff",
    charge: "#c21fa2",
    warn: "#a67c00",
    hp: "#2b3442",
    hpLow: "#d12d2d",
    armor: "#6b7f9e",
    barBack: "#c9d0d9",
    smoke: "#7f8a99",
    fire: "#e0401c",
    heal: "#1f9e6b",
    flash: "#8a6d00",
    sound: "#3f4a5c",
    fog: "rgba(70, 80, 96, 0.38)",
  },
};

export function applyTheme(name: ThemeName): Palette {
  const p = PALETTES[name];
  const root = document.documentElement;
  root.dataset.theme = name;
  const vars: Record<string, string> = {
    "--bg": p.bg,
    "--panel": p.panel,
    "--panel2": p.panel2,
    "--border": p.border,
    "--text": p.text,
    "--dim": p.dim,
    "--team-a": p.team[0],
    "--team-b": p.team[1],
    "--on-team": p.onTeam,
    "--charge": p.charge,
    "--hp": p.hp,
    "--hp-low": p.hpLow,
    "--armor": p.armor,
    "--bar-back": p.barBack,
    "--warn": p.warn,
  };
  for (const [k, v] of Object.entries(vars)) root.style.setProperty(k, v);
  return p;
}

/** `#rrggbb` with an alpha, as a canvas colour. */
export function alpha(hex: string, a: number): string {
  const n = Number.parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
}

/** A bot's `0xRRGGBB` debug colour as a canvas colour. */
export function debugColor(rgb: number): string {
  return `#${(rgb & 0xffffff).toString(16).padStart(6, "0")}`;
}
