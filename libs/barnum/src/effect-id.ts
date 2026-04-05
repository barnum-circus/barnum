// ---------------------------------------------------------------------------
// Shared effect ID counter for gensym'd effect identifiers
// ---------------------------------------------------------------------------

/** Branded numeric ID preventing accidental interchange with other number types. */
export type EffectId = number & { readonly __brand: unique symbol };

/** Branded ID for resume-style effect handlers. */
export type ResumeHandlerId = number & {
  readonly __resumeHandlerBrand: unique symbol;
};

let nextEffectId = 0;

/** Allocate a fresh, unique effect ID. */
export function allocateEffectId(): EffectId {
  return nextEffectId++ as EffectId;
}

/** Allocate a fresh, unique resume handler ID. Shares the global counter. */
export function allocateResumeHandlerId(): ResumeHandlerId {
  return nextEffectId++ as ResumeHandlerId;
}

/** Reset the effect ID counter. For test isolation only. */
export function resetEffectIdCounter(): void {
  nextEffectId = 0;
}
