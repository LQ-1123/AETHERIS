interface CacheEntry {
  value: ArrayBuffer;
  expiresAt: number;
}

export class ByteLruCache {
  private entries = new Map<number, CacheEntry>();
  private totalBytes = 0;
  private cleanupTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly maxBytes: number,
    private readonly ttlMs = Number.POSITIVE_INFINITY,
  ) {}

  get(key: number): ArrayBuffer | undefined {
    const now = Date.now();
    this.removeExpired(now);
    const entry = this.entries.get(key);
    if (!entry) return undefined;
    this.entries.delete(key);
    entry.expiresAt = this.expiryFrom(now);
    this.entries.set(key, entry);
    this.scheduleCleanup(now);
    return entry.value;
  }

  set(key: number, value: ArrayBuffer): void {
    const now = Date.now();
    this.removeExpired(now);
    const existing = this.entries.get(key);
    if (existing) this.totalBytes -= existing.value.byteLength;
    this.entries.delete(key);
    this.entries.set(key, { value, expiresAt: this.expiryFrom(now) });
    this.totalBytes += value.byteLength;
    while (this.totalBytes > this.maxBytes && this.entries.size > 1) {
      const oldest = this.entries.keys().next().value as number | undefined;
      if (oldest == null) break;
      const removed = this.entries.get(oldest);
      this.entries.delete(oldest);
      this.totalBytes -= removed?.value.byteLength ?? 0;
    }
    this.scheduleCleanup(now);
  }

  clear(): void {
    if (this.cleanupTimer != null) clearTimeout(this.cleanupTimer);
    this.cleanupTimer = null;
    this.entries.clear();
    this.totalBytes = 0;
  }

  get size(): number {
    return this.entries.size;
  }

  private expiryFrom(now: number): number {
    return Number.isFinite(this.ttlMs) ? now + Math.max(0, this.ttlMs) : Number.POSITIVE_INFINITY;
  }

  private removeExpired(now: number): void {
    for (const [key, entry] of this.entries) {
      if (entry.expiresAt > now) continue;
      this.entries.delete(key);
      this.totalBytes -= entry.value.byteLength;
    }
  }

  private scheduleCleanup(now: number): void {
    if (this.cleanupTimer != null) clearTimeout(this.cleanupTimer);
    this.cleanupTimer = null;
    if (!Number.isFinite(this.ttlMs) || this.entries.size === 0) return;

    let nextExpiry = Number.POSITIVE_INFINITY;
    for (const entry of this.entries.values()) {
      nextExpiry = Math.min(nextExpiry, entry.expiresAt);
    }
    this.cleanupTimer = setTimeout(() => {
      this.cleanupTimer = null;
      const cleanupTime = Date.now();
      this.removeExpired(cleanupTime);
      this.scheduleCleanup(cleanupTime);
    }, Math.max(0, nextExpiry - now));
  }
}
