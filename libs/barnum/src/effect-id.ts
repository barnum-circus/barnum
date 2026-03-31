// ---------------------------------------------------------------------------
// Shared effect ID counter for gensym'd effect identifiers
// ---------------------------------------------------------------------------

let nextEffectId = 0;

/** Allocate a fresh, unique effect ID. */
export function allocateEffectId(): number {
  return nextEffectId++;
}

/** Reset the effect ID counter. For test isolation only. */
export function resetEffectIdCounter(): void {
  nextEffectId = 0;
}
