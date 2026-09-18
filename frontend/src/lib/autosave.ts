export class SaveRevisionTracker {
  private revision = 0;

  markDirty(): number {
    this.revision += 1;
    return this.revision;
  }

  snapshot(): number {
    return this.revision;
  }

  isCurrent(revision: number): boolean {
    return revision === this.revision;
  }
}
