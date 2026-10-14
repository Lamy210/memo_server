import { afterEach, describe, expect, it } from 'vitest';

import {
  createValkeyRefreshCoordinationStore,
  type RefreshCoordinationStore
} from './valkeyRefreshCoordinationStore';

const TEST_URL = process.env.VALKEY_TEST_URL;
const describeWithValkey = TEST_URL ? describe : describe.skip;
const stores: RefreshCoordinationStore[] = [];

function store(): RefreshCoordinationStore {
  const created = createValkeyRefreshCoordinationStore(new URL(TEST_URL!));
  stores.push(created);
  return created;
}

function uniqueId(label: string): string {
  return `${label}-${crypto.randomUUID().replaceAll('-', '')}`;
}

afterEach(async () => {
  await Promise.all(stores.splice(0).map((current) => current.close()));
});

describeWithValkey('Valkey refresh coordination store', () => {
  it('elects one leader and makes concurrent acquisition wait while its lease exists', async () => {
    const first = store();
    const second = store();
    const coordinationId = uniqueId('leader-wait');

    await expect(
      first.acquire({ coordinationId, owner: 'owner-a', attemptTtlMs: 5_000, leaseTtlMs: 500 })
    ).resolves.toEqual({ kind: 'leader' });
    await expect(
      second.acquire({ coordinationId, owner: 'owner-b', attemptTtlMs: 5_000, leaseTtlMs: 500 })
    ).resolves.toEqual({ kind: 'wait' });
  });

  it('returns the published result to followers and rejects publication by a different owner', async () => {
    const first = store();
    const second = store();
    const coordinationId = uniqueId('publish');

    await first.acquire({ coordinationId, owner: 'owner-a', attemptTtlMs: 5_000, leaseTtlMs: 500 });
    await expect(
      second.publishSuccess({
        coordinationId,
        owner: 'owner-b',
        envelope: 'wrong-owner-result',
        resultTtlMs: 5_000
      })
    ).resolves.toBe(false);
    await expect(
      first.publishSuccess({
        coordinationId,
        owner: 'owner-a',
        envelope: 'authenticated-result-envelope',
        resultTtlMs: 5_000
      })
    ).resolves.toBe(true);
    await expect(
      second.acquire({ coordinationId, owner: 'owner-c', attemptTtlMs: 5_000, leaseTtlMs: 500 })
    ).resolves.toEqual({ kind: 'result', envelope: 'authenticated-result-envelope' });
  });

  it('becomes indeterminate after the leader lease expires while the attempt marker remains', async () => {
    const first = store();
    const second = store();
    const coordinationId = uniqueId('lease-expired');

    await first.acquire({ coordinationId, owner: 'owner-a', attemptTtlMs: 5_000, leaseTtlMs: 25 });
    await new Promise((resolve) => setTimeout(resolve, 60));

    await expect(
      second.acquire({ coordinationId, owner: 'owner-b', attemptTtlMs: 5_000, leaseTtlMs: 500 })
    ).resolves.toEqual({ kind: 'indeterminate' });
  });

  it('completeFailure removes only the matching lease and retains the attempt marker', async () => {
    const first = store();
    const second = store();
    const coordinationId = uniqueId('failure');

    await first.acquire({ coordinationId, owner: 'owner-a', attemptTtlMs: 5_000, leaseTtlMs: 5_000 });
    await first.completeFailure({ coordinationId, owner: 'owner-a' });

    await expect(
      second.acquire({ coordinationId, owner: 'owner-b', attemptTtlMs: 5_000, leaseTtlMs: 500 })
    ).resolves.toEqual({ kind: 'indeterminate' });
  });

  it('completeFailure from a non-owner cannot clear the active leader lease', async () => {
    const first = store();
    const second = store();
    const coordinationId = uniqueId('foreign-failure');

    await first.acquire({ coordinationId, owner: 'owner-a', attemptTtlMs: 5_000, leaseTtlMs: 5_000 });
    await second.completeFailure({ coordinationId, owner: 'owner-b' });

    await expect(
      second.acquire({ coordinationId, owner: 'owner-c', attemptTtlMs: 5_000, leaseTtlMs: 500 })
    ).resolves.toEqual({ kind: 'wait' });
  });
});
