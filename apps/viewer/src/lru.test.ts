import { afterEach, describe, expect, it, vi } from 'vitest';
import { ByteLruCache } from './lru';

afterEach(() => vi.useRealTimers());

describe('ByteLruCache', () => {
  it('evicts the least recently used frame by byte budget', () => {
    const cache = new ByteLruCache(8);
    cache.set(1, new ArrayBuffer(4));
    cache.set(2, new ArrayBuffer(4));
    expect(cache.get(1)).toBeDefined();
    cache.set(3, new ArrayBuffer(4));

    expect(cache.get(1)).toBeDefined();
    expect(cache.get(2)).toBeUndefined();
    expect(cache.get(3)).toBeDefined();
  });

  it('keeps one oversized current frame and resets cleanly', () => {
    const cache = new ByteLruCache(2);
    cache.set(7, new ArrayBuffer(5));
    expect(cache.get(7)?.byteLength).toBe(5);
    cache.clear();
    expect(cache.size).toBe(0);
  });

  it('removes an idle frame after its sliding ttl', () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const cache = new ByteLruCache(8, 180_000);
    cache.set(1, new ArrayBuffer(4));

    vi.advanceTimersByTime(179_000);
    expect(cache.get(1)).toBeDefined();
    vi.advanceTimersByTime(179_000);
    expect(cache.size).toBe(1);
    vi.advanceTimersByTime(1_000);

    expect(cache.size).toBe(0);
    expect(cache.get(1)).toBeUndefined();
  });
});
