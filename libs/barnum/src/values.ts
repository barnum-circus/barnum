import type { Result, Option } from "./ast.js";

export function ok<TValue, TError = unknown>(
  value: TValue,
): Result<TValue, TError> {
  return { kind: "Result.Ok", value } as Result<TValue, TError>;
}

export function err<TValue = unknown, TError = never>(
  error: TError,
): Result<TValue, TError> {
  return { kind: "Result.Err", value: error } as Result<TValue, TError>;
}

export function some<T>(value: T): Option<T> {
  return { kind: "Option.Some", value } as Option<T>;
}

export function none<T = unknown>(): Option<T> {
  return { kind: "Option.None", value: null } as Option<T>;
}
