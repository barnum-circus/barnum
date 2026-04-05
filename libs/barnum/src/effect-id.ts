// ---------------------------------------------------------------------------
// Shared effect ID counter for gensym'd effect identifiers
// ---------------------------------------------------------------------------

/** Branded ID for resume-style effect handlers. */
export type ResumeHandlerId = number & {
  readonly __resumeHandlerBrand: unique symbol;
};

/** Branded ID for restart-style effect handlers. */
export type RestartHandlerId = number & {
  readonly __restartHandlerBrand: unique symbol;
};

let nextId = 0;

/** Allocate a fresh, unique resume handler ID. */
export function allocateResumeHandlerId(): ResumeHandlerId {
  return nextId++ as ResumeHandlerId;
}

/** Allocate a fresh, unique restart handler ID. */
export function allocateRestartHandlerId(): RestartHandlerId {
  return nextId++ as RestartHandlerId;
}

/** Reset the ID counter. For test isolation only. */
export function resetEffectIdCounter(): void {
  nextId = 0;
}
