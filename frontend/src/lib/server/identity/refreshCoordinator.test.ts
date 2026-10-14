import { describe, expect, it } from 'vitest';

import {
  createRefreshResultEnvelope,
  deriveCoordinationId
} from './refreshCoordinationCrypto';
import {
  createRefreshCoordinator,
  type RefreshCoordinator
} from './refreshCoordinator';
import type {
  RefreshCoordinationStore,
  StoreAcquireResult
} from './valkeyRefreshCoordinationStore';

const SESSION_KEY = new Uint8Array(32).fill(59);

type AcquireInput = Parameters<RefreshCoordinationStore['acquire']>[0];
type PublishInput = Parameters<RefreshCoordinationStore['publishSuccess']>[0];
type FailureInput = Parameters<RefreshCoordinationStore['completeFailure']>[0];

class FakeStore implements RefreshCoordinationStore {
  acquireCalls: AcquireInput[] = [];
  publishCalls: PublishInput[] = [];
  failureCalls: FailureInput[] = [];
  acquireImpl: (input: AcquireInput) => Promise<StoreAcquireResult> = async () => ({ kind: 'leader' });
  publishResult = true;

  async acquire(input: AcquireInput): Promise<StoreAcquireResult> {
    this.acquireCalls.push(input);
    return this.acquireImpl(input);
  }

  async publishSuccess(input: PublishInput): Promise<boolean> {
    this.publishCalls.push(input);
    return this.publishResult;
  }

  async completeFailure(input: FailureInput): Promise<void> {
    this.failureCalls.push(input);
  }

  async close(): Promise<void> {}
}

function harness(store: FakeStore, start = 0): { coordinator: RefreshCoordinator; now: () => number } {
  let current = start;
  return {
    coordinator: createRefreshCoordinator({
      store,
      sessionKey: SESSION_KEY,
      random: () => 0,
      now: () => current,
      sleep: async (ms) => {
        current += ms;
      }
    }),
    now: () => current
  };
}

describe('distributed refresh coordinator', () => {
  it('elects the first caller as leader with a 30 second lease', async () => {
    const store = new FakeStore();
    const { coordinator } = harness(store);

    const decision = await coordinator.acquire({ sealedSession: 'S0', replayGuardMs: 90_000 });

    expect(decision.kind).toBe('leader');
    if (decision.kind !== 'leader') throw new Error('expected leader');
    expect(decision.owner).toMatch(/^[0-9a-f]{32}$/);
    expect(store.acquireCalls).toHaveLength(1);
    expect(store.acquireCalls[0]).toMatchObject({ attemptTtlMs: 90_000, leaseTtlMs: 30_000 });
    expect(store.acquireCalls[0].coordinationId).toMatch(/^[0-9a-f]{64}$/);
  });

  it('waits for and authenticates a follower result', async () => {
    const store = new FakeStore();
    const coordinationId = await deriveCoordinationId('S0', SESSION_KEY);
    const envelope = await createRefreshResultEnvelope(coordinationId, 'S1', SESSION_KEY);
    let calls = 0;
    store.acquireImpl = async () => (++calls === 1 ? { kind: 'wait' } : { kind: 'result', envelope });
    const { coordinator } = harness(store);

    await expect(coordinator.acquire({ sealedSession: 'S0', replayGuardMs: 90_000 })).resolves.toEqual({
      kind: 'result',
      sealedSession: 'S1'
    });
  });

  it('maps lease loss while an attempt remains to leader_lost', async () => {
    const store = new FakeStore();
    let calls = 0;
    store.acquireImpl = async () => (++calls === 1 ? { kind: 'wait' } : { kind: 'indeterminate' });
    const { coordinator } = harness(store);

    await expect(coordinator.acquire({ sealedSession: 'S0', replayGuardMs: 90_000 })).resolves.toEqual({
      kind: 'indeterminate',
      reason: 'leader_lost'
    });
  });

  it('rejects a result envelope copied from another coordination generation', async () => {
    const store = new FakeStore();
    const otherId = await deriveCoordinationId('other-S0', SESSION_KEY);
    const envelope = await createRefreshResultEnvelope(otherId, 'attacker-S1', SESSION_KEY);
    store.acquireImpl = async () => ({ kind: 'result', envelope });
    const { coordinator } = harness(store);

    await expect(coordinator.acquire({ sealedSession: 'S0', replayGuardMs: 90_000 })).resolves.toEqual({
      kind: 'indeterminate',
      reason: 'invalid_result'
    });
  });

  it('normalizes store acquisition failures to unavailable', async () => {
    const store = new FakeStore();
    store.acquireImpl = async () => {
      throw new Error('secret connection detail');
    };
    const { coordinator } = harness(store);

    await expect(coordinator.acquire({ sealedSession: 'S0', replayGuardMs: 90_000 })).resolves.toEqual({
      kind: 'unavailable'
    });
  });

  it('publishes an authenticated handoff with exactly 120 seconds result TTL', async () => {
    const store = new FakeStore();
    const { coordinator } = harness(store);

    await expect(
      coordinator.publishSuccess({ sealedSession: 'S0', owner: 'owner-a', refreshedSealedSession: 'S1' })
    ).resolves.toBe(true);

    expect(store.publishCalls).toHaveLength(1);
    expect(store.publishCalls[0]).toMatchObject({ owner: 'owner-a', resultTtlMs: 120_000 });
    expect(store.publishCalls[0].envelope).not.toContain('"mac":""');
  });

  it('continues waiting beyond the superseded 15 second lease assumption', async () => {
    const store = new FakeStore();
    const coordinationId = await deriveCoordinationId('S0', SESSION_KEY);
    const envelope = await createRefreshResultEnvelope(coordinationId, 'S1', SESSION_KEY);
    const { coordinator, now } = harness(store);
    store.acquireImpl = async () =>
      now() < 15_100 ? { kind: 'wait' } : { kind: 'result', envelope };

    await expect(coordinator.acquire({ sealedSession: 'S0', replayGuardMs: 90_000 })).resolves.toEqual({
      kind: 'result',
      sealedSession: 'S1'
    });
    expect(now()).toBeGreaterThan(15_000);
  });

  it('accepts a follower result after 20 seconds while still below the 25 second deadline', async () => {
    const store = new FakeStore();
    const coordinationId = await deriveCoordinationId('S0', SESSION_KEY);
    const envelope = await createRefreshResultEnvelope(coordinationId, 'S1', SESSION_KEY);
    const { coordinator, now } = harness(store);
    store.acquireImpl = async () =>
      now() < 20_100 ? { kind: 'wait' } : { kind: 'result', envelope };

    await expect(coordinator.acquire({ sealedSession: 'S0', replayGuardMs: 90_000 })).resolves.toEqual({
      kind: 'result',
      sealedSession: 'S1'
    });
    expect(now()).toBeGreaterThan(20_000);
    expect(now()).toBeLessThan(25_000);
  });

  it('times out at 25 seconds without ever becoming a leader', async () => {
    const store = new FakeStore();
    store.acquireImpl = async () => ({ kind: 'wait' });
    const { coordinator, now } = harness(store);

    await expect(coordinator.acquire({ sealedSession: 'S0', replayGuardMs: 90_000 })).resolves.toEqual({
      kind: 'indeterminate',
      reason: 'wait_timeout'
    });
    expect(now()).toBe(25_000);
    expect(store.acquireCalls.every((call) => call.leaseTtlMs === 30_000)).toBe(true);
  });
});
