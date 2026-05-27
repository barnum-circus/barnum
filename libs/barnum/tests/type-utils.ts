// ---------------------------------------------------------------------------
// Type assertion helpers (compile-time only)
// ---------------------------------------------------------------------------

export type IsAny<T> = 0 extends 1 & T ? true : false;

export type IsExact<T, U> =
  IsAny<T> extends IsAny<U>
    ? [T] extends [U]
      ? [U] extends [T]
        ? true
        : false
      : false
    : false;

// eslint-disable-next-line @typescript-eslint/no-unused-vars
export function assertExact<_T extends true>(): void {}
