import { describe, expect, it } from 'vitest';
import { ByteLruCache } from './lru';

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
});
