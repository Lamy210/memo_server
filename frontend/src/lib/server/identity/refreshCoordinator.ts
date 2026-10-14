import {
  createRefreshResultEnvelope,
  deriveCoordinationId,
  verifyRefreshResultEnvelope
} from './refreshCoordinationCrypto';
import type {
  RefreshCoordinationStore,
  StoreAcquireResult
} from './valkeyRefreshCoordinationStore';

const LEASE_TTL_MS = 30_000;
const RESULT_TTL_MS = 120_000;
const FOLLOWER_DEADLINE_MS = 25_000;
const POLL_DELAY_BASE_MS = 100;
const POLL_DELAY_JITTER_VALUES = 101;

export type RefreshCoordinationDecision =
  | { kind: 'leader'; owner: string }
  | { kind: 'result'; sealedSession: string }
  | { kind: 'indeterminate'; reason: 'leader_lost' | 'wait_timeout' | 'invalid_result' }
  | { kind: 'unavailable' };

export interface RefreshCoordinator {
  acquire(input: {
    sealedSession: string;
    replayGuardMs: number;
  }): Promise<RefreshCoordinationDecision>;
  publishSuccess(input: {
    sealedSession: string;
    owner: string;
    refreshedSealedSession: string;
  }): Promise<boolean>;
  completeFailure(input: { sealedSession: string; owner: string }): Promise<void>;
  close(): Promise<void>;
}

type CreateRefreshCoordinatorInput = {
  store: RefreshCoordinationStore;
  sessionKey: Uint8Array;
  sleep?: (ms: number) => Promise<void>;
  random?: () => number;
  now?: () => number;
};

function ownerToken(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function defaultSleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export function createRefreshCoordinator({
  store,
  sessionKey,
  sleep = defaultSleep,
  random = Math.random,
  now = Date.now
}: CreateRefreshCoordinatorInput): RefreshCoordinator {
  async function interpretResult(
    coordinationId: string,
    result: StoreAcquireResult
  ): Promise<RefreshCoordinationDecision | { kind: 'wait' }> {
    switch (result.kind) {
      case 'leader':
        throw new Error('leader result requires caller owner');
      case 'wait':
        return { kind: 'wait' };
      case 'indeterminate':
        return { kind: 'indeterminate', reason: 'leader_lost' };
      case 'result': {
        try {
          const sealedSession = await verifyRefreshResultEnvelope(
            coordinationId,
            result.envelope,
            sessionKey
          );
          return sealedSession
            ? { kind: 'result', sealedSession }
            : { kind: 'indeterminate', reason: 'invalid_result' };
        } catch {
          return { kind: 'indeterminate', reason: 'invalid_result' };
        }
      }
    }
  }

  return {
    async acquire({ sealedSession, replayGuardMs }) {
      const coordinationId = await deriveCoordinationId(sealedSession, sessionKey);
      const owner = ownerToken();
      let current: StoreAcquireResult;
      try {
        current = await store.acquire({
          coordinationId,
          owner,
          attemptTtlMs: replayGuardMs,
          leaseTtlMs: LEASE_TTL_MS
        });
      } catch {
        return { kind: 'unavailable' };
      }

      if (current.kind === 'leader') return { kind: 'leader', owner };

      const startedAt = now();
      while (current.kind === 'wait') {
        const elapsed = Math.max(0, now() - startedAt);
        if (elapsed >= FOLLOWER_DEADLINE_MS) {
          return { kind: 'indeterminate', reason: 'wait_timeout' };
        }

        const jitter = Math.floor(random() * POLL_DELAY_JITTER_VALUES);
        const delay = Math.min(
          POLL_DELAY_BASE_MS + jitter,
          FOLLOWER_DEADLINE_MS - elapsed
        );
        await sleep(delay);

        if (Math.max(0, now() - startedAt) >= FOLLOWER_DEADLINE_MS) {
          return { kind: 'indeterminate', reason: 'wait_timeout' };
        }

        try {
          current = await store.acquire({
            coordinationId,
            owner,
            attemptTtlMs: replayGuardMs,
            leaseTtlMs: LEASE_TTL_MS
          });
        } catch {
          return { kind: 'unavailable' };
        }

        if (current.kind === 'leader') {
          return { kind: 'indeterminate', reason: 'leader_lost' };
        }
      }

      const interpreted = await interpretResult(coordinationId, current);
      return interpreted.kind === 'wait'
        ? { kind: 'indeterminate', reason: 'wait_timeout' }
        : interpreted;
    },

    async publishSuccess({ sealedSession, owner, refreshedSealedSession }) {
      try {
        const coordinationId = await deriveCoordinationId(sealedSession, sessionKey);
        const envelope = await createRefreshResultEnvelope(
          coordinationId,
          refreshedSealedSession,
          sessionKey
        );
        return await store.publishSuccess({
          coordinationId,
          owner,
          envelope,
          resultTtlMs: RESULT_TTL_MS
        });
      } catch {
        return false;
      }
    },

    async completeFailure({ sealedSession, owner }) {
      try {
        const coordinationId = await deriveCoordinationId(sealedSession, sessionKey);
        await store.completeFailure({ coordinationId, owner });
      } catch {
        // The persistent attempt marker is intentionally not cleared on cleanup failure.
      }
    },

    async close() {
      await store.close();
    }
  };
}
