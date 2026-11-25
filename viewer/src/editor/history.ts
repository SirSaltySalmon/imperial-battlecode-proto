// Undo and redo over whole-map snapshots. A 128x128 map is 16 KiB of text, so a snapshot per step is cheap
// and every edit (a stroke, a transform, a resize, a header change) undoes the same way.

import { cloneDoc, sameDoc, type MapDoc } from "./doc";

export class History {
  private readonly past: MapDoc[] = [];
  private readonly future: MapDoc[] = [];

  constructor(private readonly limit = 200) {}

  /** Record `before` as the state an edit started from. No-op if the edit changed nothing. */
  commit(before: MapDoc, after: MapDoc): void {
    if (sameDoc(before, after)) return;
    this.past.push(cloneDoc(before));
    if (this.past.length > this.limit) this.past.shift();
    this.future.length = 0;
  }

  /** The state before the last edit, or null; `current` becomes redoable. */
  undo(current: MapDoc): MapDoc | null {
    const prev = this.past.pop();
    if (!prev) return null;
    this.future.push(cloneDoc(current));
    return prev;
  }

  redo(current: MapDoc): MapDoc | null {
    const next = this.future.pop();
    if (!next) return null;
    this.past.push(cloneDoc(current));
    return next;
  }

  clear(): void {
    this.past.length = 0;
    this.future.length = 0;
  }

  get canUndo(): boolean {
    return this.past.length > 0;
  }

  get canRedo(): boolean {
    return this.future.length > 0;
  }
}
