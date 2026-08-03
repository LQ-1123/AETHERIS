export class ByteLruCache {
  private entries = new Map<number, ArrayBuffer>();
  private totalBytes = 0;

  constructor(private readonly maxBytes: number) {}

  get(key: number): ArrayBuffer | undefined {
    const value = this.entries.get(key);
    if (!value) return undefined;
    this.entries.delete(key);
    this.entries.set(key, value);
    return value;
  }

  set(key: number, value: ArrayBuffer): void {
    const existing = this.entries.get(key);
    if (existing) this.totalBytes -= existing.byteLength;
    this.entries.delete(key);
    this.entries.set(key, value);
    this.totalBytes += value.byteLength;
    while (this.totalBytes > this.maxBytes && this.entries.size > 1) {
      const oldest = this.entries.keys().next().value as number | undefined;
      if (oldest == null) break;
      const removed = this.entries.get(oldest);
      this.entries.delete(oldest);
      this.totalBytes -= removed?.byteLength ?? 0;
    }
  }

  clear(): void {
    this.entries.clear();
    this.totalBytes = 0;
  }

  get size(): number {
    return this.entries.size;
  }
}
