import type { Result, Option } from "./ast.js";

export function ok<TValue, TError = unknown>(value: TValue): Result<TValue, TError> {
  return { kind: "Ok", value } as Result<TValue, TError>;
}

export function err<TValue = unknown, TError = never>(error: TError): Result<TValue, TError> {
  return { kind: "Err", value: error } as Result<TValue, TError>;
}

export function some<T>(value: T): Option<T> {
  return { kind: "Some", value } as Option<T>;
}

export function none<T = unknown>(): Option<T> {
  return { kind: "None", value: null } as Option<T>;
}
