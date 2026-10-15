import { afterEach, describe, expect, expectTypeOf, it, vi } from 'vitest';

import type { RefreshCoordinationOutcome } from './refreshObservability';

const OUTCOMES: RefreshCoordinationOutcome[] = [
  'leader_success',
  'follower_success',
  'unavailable_continue',
  'unavailable_fail_closed',
  'leader_lost',
  'wait_timeout',
  'invalid_result',
  'session_expired',
  'upstream_failure',
  'publish_failure'
];

const WARN_OUTCOMES: RefreshCoordinationOutcome[] = [
  'unavailable_continue',
  'unavailable_fail_closed',
  'leader_lost',
  'wait_timeout',
  'invalid_result',
  'upstream_failure',
  'publish_failure'
];

const NON_WARN_OUTCOMES: RefreshCoordinationOutcome[] = [
  'leader_success',
  'follower_success',
  'session_expired'
];

async function freshRecorder() {
  vi.resetModules();
  return import('./refreshObservability');
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('recordRefreshCoordinationOutcome', () => {
  it('exposes only the fixed outcome argument', async () => {
    const { recordRefreshCoordinationOutcome } = await freshRecorder();

    expectTypeOf(recordRefreshCoordinationOutcome).toEqualTypeOf<
      (outcome: RefreshCoordinationOutcome) => void
    >();
  });

  it('keeps cumulative per-outcome counts and emits one aggregate at total 50', async () => {
    const info = vi.spyOn(console, 'info').mockImplementation(() => undefined);
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const { recordRefreshCoordinationOutcome } = await freshRecorder();

    for (const outcome of OUTCOMES) {
      for (let index = 0; index < 5; index += 1) {
        recordRefreshCoordinationOutcome(outcome);
      }
    }

    expect(info).toHaveBeenCalledTimes(1);
    expect(info).toHaveBeenCalledWith(
      'identity refresh coordination aggregate: total=50 leader_success=5 follower_success=5 unavailable_continue=5 unavailable_fail_closed=5 leader_lost=5 wait_timeout=5 invalid_result=5 session_expired=5 upstream_failure=5 publish_failure=5'
    );
  });

  it('does not reset counters after an aggregate emission', async () => {
    const info = vi.spyOn(console, 'info').mockImplementation(() => undefined);
    const { recordRefreshCoordinationOutcome } = await freshRecorder();

    for (let index = 0; index < 100; index += 1) {
      recordRefreshCoordinationOutcome('leader_success');
    }

    expect(info).toHaveBeenCalledTimes(2);
    expect(info).toHaveBeenNthCalledWith(
      2,
      'identity refresh coordination aggregate: total=100 leader_success=100 follower_success=0 unavailable_continue=0 unavailable_fail_closed=0 leader_lost=0 wait_timeout=0 invalid_result=0 session_expired=0 upstream_failure=0 publish_failure=0'
    );
  });

  it('does not emit an aggregate before total 50', async () => {
    const info = vi.spyOn(console, 'info').mockImplementation(() => undefined);
    const { recordRefreshCoordinationOutcome } = await freshRecorder();

    for (let index = 0; index < 49; index += 1) {
      recordRefreshCoordinationOutcome('leader_success');
    }

    expect(info).not.toHaveBeenCalled();
  });

  it('emits one fixed warning for each abnormal outcome only', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const { recordRefreshCoordinationOutcome } = await freshRecorder();

    for (const outcome of WARN_OUTCOMES) {
      recordRefreshCoordinationOutcome(outcome);
    }
    for (const outcome of NON_WARN_OUTCOMES) {
      recordRefreshCoordinationOutcome(outcome);
    }

    expect(warn).toHaveBeenCalledTimes(WARN_OUTCOMES.length);
    for (const outcome of WARN_OUTCOMES) {
      expect(warn).toHaveBeenCalledWith(
        `identity refresh coordination warning: outcome=${outcome}`
      );
    }
  });

  it('contains a throwing warning logger and still attempts the aggregate at total 50', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {
      throw new Error('logger failed');
    });
    const info = vi.spyOn(console, 'info').mockImplementation(() => undefined);
    const { recordRefreshCoordinationOutcome } = await freshRecorder();

    for (let index = 0; index < 49; index += 1) {
      recordRefreshCoordinationOutcome('leader_success');
    }

    expect(() => recordRefreshCoordinationOutcome('unavailable_continue')).not.toThrow();
    expect(warn).toHaveBeenCalledTimes(1);
    expect(info).toHaveBeenCalledTimes(1);
  });

  it('contains a throwing aggregate logger', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const info = vi.spyOn(console, 'info').mockImplementation(() => {
      throw new Error('logger failed');
    });
    const { recordRefreshCoordinationOutcome } = await freshRecorder();

    for (let index = 0; index < 49; index += 1) {
      recordRefreshCoordinationOutcome('leader_success');
    }

    expect(() => recordRefreshCoordinationOutcome('leader_success')).not.toThrow();
    expect(info).toHaveBeenCalledTimes(1);
  });
});
