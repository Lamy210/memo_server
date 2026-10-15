export type RefreshCoordinationOutcome =
  | 'leader_success'
  | 'follower_success'
  | 'unavailable_continue'
  | 'unavailable_fail_closed'
  | 'leader_lost'
  | 'wait_timeout'
  | 'invalid_result'
  | 'session_expired'
  | 'upstream_failure'
  | 'publish_failure';

type RefreshCoordinationCounters = Record<RefreshCoordinationOutcome, number> & {
  total: number;
};

const counters: RefreshCoordinationCounters = {
  total: 0,
  leader_success: 0,
  follower_success: 0,
  unavailable_continue: 0,
  unavailable_fail_closed: 0,
  leader_lost: 0,
  wait_timeout: 0,
  invalid_result: 0,
  session_expired: 0,
  upstream_failure: 0,
  publish_failure: 0
};

const warningOutcomes = new Set<RefreshCoordinationOutcome>([
  'unavailable_continue',
  'unavailable_fail_closed',
  'leader_lost',
  'wait_timeout',
  'invalid_result',
  'upstream_failure',
  'publish_failure'
]);

function emitWarning(outcome: RefreshCoordinationOutcome): void {
  try {
    console.warn(`identity refresh coordination warning: outcome=${outcome}`);
  } catch {
    // Telemetry must remain passive if logging fails.
  }
}

function emitAggregate(): void {
  try {
    console.info(
      `identity refresh coordination aggregate: total=${counters.total} leader_success=${counters.leader_success} follower_success=${counters.follower_success} unavailable_continue=${counters.unavailable_continue} unavailable_fail_closed=${counters.unavailable_fail_closed} leader_lost=${counters.leader_lost} wait_timeout=${counters.wait_timeout} invalid_result=${counters.invalid_result} session_expired=${counters.session_expired} upstream_failure=${counters.upstream_failure} publish_failure=${counters.publish_failure}`
    );
  } catch {
    // Telemetry must remain passive if logging fails.
  }
}

export function recordRefreshCoordinationOutcome(outcome: RefreshCoordinationOutcome): void {
  counters.total += 1;
  counters[outcome] += 1;

  if (warningOutcomes.has(outcome)) {
    emitWarning(outcome);
  }

  if (counters.total % 50 === 0) {
    emitAggregate();
  }
}
