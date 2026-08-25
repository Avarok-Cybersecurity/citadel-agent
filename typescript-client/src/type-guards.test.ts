/**
 * Tests for the response/request narrowing helpers.
 *
 * These replace `"test": "echo 'No tests configured yet' && exit 0"`, which made
 * the typescript-integration CI job green whatever the state of this package.
 *
 * Node's built-in runner on the output `tsc` already emits — no test framework
 * is added, so this works the same standalone as it does inside the workspace,
 * which matters because the package is built on its own in Docker.
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { isResponseType, isRequestType, isVariant } from './type-guards.js';
import type { InternalServiceResponse, InternalServiceRequest } from './types/index.js';

describe('isResponseType', () => {
  it('identifies the variant a response carries', () => {
    const response = { GetSessionsResponse: { sessions: [] } } as unknown as InternalServiceResponse;
    assert.equal(isResponseType(response, 'GetSessionsResponse'), true);
  });

  it('rejects a variant the response does not carry', () => {
    const response = { GetSessionsResponse: { sessions: [] } } as unknown as InternalServiceResponse;
    assert.equal(isResponseType(response, 'MessageNotification'), false);
  });

  it('matches on the key alone, even when its value is undefined', () => {
    // `in` is a key test, not a truthiness test. Worth pinning: a variant whose
    // payload is absent is still that variant, and narrowing has to say so.
    const response = { MessageNotification: undefined } as unknown as InternalServiceResponse;
    assert.equal(isResponseType(response, 'MessageNotification'), true);
  });
});

describe('isRequestType', () => {
  it('identifies the variant a request carries', () => {
    const request = { Connect: { username: 'alice' } } as unknown as InternalServiceRequest;
    assert.equal(isRequestType(request, 'Connect'), true);
  });

  it('rejects a variant the request does not carry', () => {
    const request = { Connect: { username: 'alice' } } as unknown as InternalServiceRequest;
    assert.equal(isRequestType(request, 'Disconnect'), false);
  });
});

describe('isVariant', () => {
  it('narrows an object variant', () => {
    const union = { Workspace: { id: '1' } };
    assert.equal(isVariant(union, 'Workspace'), true);
  });

  it('returns false for a string-literal member of a mixed union', () => {
    // The case the runtime guard exists for. WorkspaceProtocolResponse mixes
    // object variants with bare strings like "WorkspaceNotInitialized"; `key in
    // union` throws on a string operand, so the typeof check is load-bearing
    // rather than defensive padding.
    const union = 'WorkspaceNotInitialized' as string | { Workspace: unknown };
    assert.equal(isVariant(union as never, 'Workspace' as never), false);
  });

  it('returns false for null rather than throwing', () => {
    // `typeof null === 'object'`, so without the explicit null check this would
    // reach `key in null` and throw a TypeError.
    assert.equal(isVariant(null as never, 'Workspace' as never), false);
  });

  it('rejects a discriminator the union does not carry', () => {
    const union = { Workspace: { id: '1' } };
    assert.equal(isVariant(union, 'Workspace'), true);
    assert.equal(isVariant(union as never, 'Offices' as never), false);
  });
});
