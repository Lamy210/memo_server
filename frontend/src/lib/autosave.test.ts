import { describe, expect, it } from 'vitest';

import { SaveRevisionTracker } from './autosave';

describe('SaveRevisionTracker', () => {
  it('detects edits that happen while an older revision is being saved', () => {
    const tracker = new SaveRevisionTracker();

    tracker.markDirty();
    const revisionBeingSaved = tracker.snapshot();
    tracker.markDirty();

    expect(tracker.isCurrent(revisionBeingSaved)).toBe(false);
    expect(tracker.isCurrent(tracker.snapshot())).toBe(true);
  });

  it('treats a completed save as current when no later edit exists', () => {
    const tracker = new SaveRevisionTracker();

    tracker.markDirty();
    const revisionBeingSaved = tracker.snapshot();

    expect(tracker.isCurrent(revisionBeingSaved)).toBe(true);
  });
});
