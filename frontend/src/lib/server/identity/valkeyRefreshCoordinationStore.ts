import { Valkey } from 'iovalkey';

export type StoreAcquireResult =
  | { kind: 'leader' }
  | { kind: 'wait' }
  | { kind: 'result'; envelope: string }
  | { kind: 'indeterminate' };

export interface RefreshCoordinationStore {
  acquire(input: {
    coordinationId: string;
    owner: string;
    attemptTtlMs: number;
    leaseTtlMs: number;
  }): Promise<StoreAcquireResult>;
  publishSuccess(input: {
    coordinationId: string;
    owner: string;
    envelope: string;
    resultTtlMs: number;
  }): Promise<boolean>;
  completeFailure(input: { coordinationId: string; owner: string }): Promise<void>;
  close(): Promise<void>;
}

const ACQUIRE_SCRIPT = `
local result = redis.call('GET', KEYS[3])
if result then
  return {'RESULT', result}
end

local attempt = redis.call('GET', KEYS[1])
if attempt then
  if redis.call('EXISTS', KEYS[2]) == 1 then
    return {'WAIT'}
  end
  return {'INDETERMINATE'}
end

redis.call('SET', KEYS[1], ARGV[1], 'PX', ARGV[2])
redis.call('SET', KEYS[2], ARGV[1], 'PX', ARGV[3])
return {'LEADER'}
`;

const PUBLISH_SUCCESS_SCRIPT = `
if redis.call('GET', KEYS[1]) ~= ARGV[1] then
  return 0
end

redis.call('SET', KEYS[3], ARGV[2], 'PX', ARGV[3])
if redis.call('GET', KEYS[2]) == ARGV[1] then
  redis.call('DEL', KEYS[2])
end
return 1
`;

const COMPLETE_FAILURE_SCRIPT = `
if redis.call('GET', KEYS[2]) == ARGV[1] then
  redis.call('DEL', KEYS[2])
end
return 1
`;

function keys(coordinationId: string): [string, string, string] {
  const prefix = `memo:identity:refresh:v1:{${coordinationId}}`;
  return [`${prefix}:attempt`, `${prefix}:lease`, `${prefix}:result`];
}

function positiveInteger(value: number): string {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error('Invalid refresh coordination TTL');
  }
  return String(value);
}

function parseAcquireResult(result: unknown): StoreAcquireResult {
  if (!Array.isArray(result) || typeof result[0] !== 'string') {
    throw new Error('Invalid refresh coordination response');
  }

  switch (result[0]) {
    case 'LEADER':
      return { kind: 'leader' };
    case 'WAIT':
      return { kind: 'wait' };
    case 'INDETERMINATE':
      return { kind: 'indeterminate' };
    case 'RESULT':
      if (typeof result[1] !== 'string') throw new Error('Invalid refresh coordination response');
      return { kind: 'result', envelope: result[1] };
    default:
      throw new Error('Invalid refresh coordination response');
  }
}

export function createValkeyRefreshCoordinationStore(url: URL): RefreshCoordinationStore {
  const client = new Valkey(url.toString(), {
    connectTimeout: 1_000,
    commandTimeout: 1_000,
    maxRetriesPerRequest: 1,
    enableOfflineQueue: false,
    lazyConnect: true
  });
  let connectPromise: Promise<void> | undefined;

  async function ensureConnected(): Promise<void> {
    if (client.status === 'ready') return;
    if (!connectPromise) {
      connectPromise = client.connect().finally(() => {
        connectPromise = undefined;
      });
    }
    await connectPromise;
  }

  return {
    async acquire({ coordinationId, owner, attemptTtlMs, leaseTtlMs }) {
      await ensureConnected();
      const [attemptKey, leaseKey, resultKey] = keys(coordinationId);
      const result = await client.eval(
        ACQUIRE_SCRIPT,
        3,
        attemptKey,
        leaseKey,
        resultKey,
        owner,
        positiveInteger(attemptTtlMs),
        positiveInteger(leaseTtlMs)
      );
      return parseAcquireResult(result);
    },

    async publishSuccess({ coordinationId, owner, envelope, resultTtlMs }) {
      await ensureConnected();
      const [attemptKey, leaseKey, resultKey] = keys(coordinationId);
      const result = await client.eval(
        PUBLISH_SUCCESS_SCRIPT,
        3,
        attemptKey,
        leaseKey,
        resultKey,
        owner,
        envelope,
        positiveInteger(resultTtlMs)
      );
      return result === 1;
    },

    async completeFailure({ coordinationId, owner }) {
      await ensureConnected();
      const [attemptKey, leaseKey, resultKey] = keys(coordinationId);
      await client.eval(
        COMPLETE_FAILURE_SCRIPT,
        3,
        attemptKey,
        leaseKey,
        resultKey,
        owner
      );
    },

    async close() {
      client.disconnect();
    }
  };
}
