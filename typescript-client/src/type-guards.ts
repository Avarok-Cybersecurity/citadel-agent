/**
 * Type-safe discriminated union narrowing utilities.
 *
 * These helpers replace raw `'Key' in obj` checks with compile-time checked
 * narrowing. If a Rust enum variant is renamed and ts-rs regenerates the
 * TypeScript union, all call sites using these helpers will produce
 * compile-time errors instead of silently failing at runtime.
 */

import type { InternalServiceRequest } from './types/InternalServiceRequest';
import type { InternalServiceResponse } from './types/InternalServiceResponse';

/**
 * Extract all possible discriminator keys from a tagged/discriminated union.
 *
 * Given a union like `{ A: ... } | { B: ... } | { C: ... }`, this produces
 * the union type `"A" | "B" | "C"`.
 */
export type DiscriminatorOf<T> = T extends Record<infer K extends string, unknown> ? K : never;

/** All valid discriminator keys for InternalServiceResponse */
export type ResponseType = DiscriminatorOf<InternalServiceResponse>;

/** All valid discriminator keys for InternalServiceRequest */
export type RequestType = DiscriminatorOf<InternalServiceRequest>;

/**
 * Type-safe check for InternalServiceResponse variant.
 *
 * Compile-time error if `key` is not a valid variant name.
 * Narrows the response to the specific variant on success.
 *
 * @example
 * ```typescript
 * if (isResponseType(response, 'GetSessionsResponse')) {
 *   response.GetSessionsResponse.sessions // correctly narrowed
 * }
 *
 * // Compile ERROR: 'NonExistent' is not in ResponseType
 * if (isResponseType(response, 'NonExistent')) { }
 * ```
 */
export function isResponseType<K extends ResponseType>(
  response: InternalServiceResponse,
  key: K
): response is Extract<InternalServiceResponse, Record<K, unknown>> {
  return key in response;
}

/**
 * Type-safe check for InternalServiceRequest variant.
 *
 * Compile-time error if `key` is not a valid variant name.
 * Narrows the request to the specific variant on success.
 */
export function isRequestType<K extends RequestType>(
  request: InternalServiceRequest,
  key: K
): request is Extract<InternalServiceRequest, Record<K, unknown>> {
  return key in request;
}

/**
 * Generic type-safe discriminated union narrowing.
 *
 * Works with any tagged union type (InternalServiceResponse, WorkspaceProtocolResponse, etc.).
 * Compile-time error if `key` is not a valid discriminator for the union.
 *
 * Handles mixed unions that include both object variants and string literal variants
 * (e.g., `{ Workspace: ... } | "WorkspaceNotInitialized"`). String literal members
 * are excluded from the key constraint via `Exclude<T, string>`, and the runtime
 * check guards against calling `in` on a non-object.
 *
 * @example
 * ```typescript
 * // Works with any discriminated union
 * if (isVariant(workspaceResponse, 'Workspace')) {
 *   workspaceResponse.Workspace // correctly narrowed
 * }
 * ```
 */
export function isVariant<
  T extends Record<string, unknown> | string,
  K extends DiscriminatorOf<Exclude<T, string>>
>(
  union: T,
  key: K
): union is Extract<T, Record<K, unknown>> {
  return typeof union === 'object' && union !== null && key in union;
}
