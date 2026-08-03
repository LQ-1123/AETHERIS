import { describe, expect, it } from 'vitest';
import { RequestVersion } from './request-version';

describe('RequestVersion', () => {
  it('accepts only the latest asynchronous response', () => {
    const requests = new RequestVersion();
    const first = requests.next();
    const second = requests.next();
    expect(requests.isCurrent(first)).toBe(false);
    expect(requests.isCurrent(second)).toBe(true);
  });

  it('invalidates an in-flight response without starting another request', () => {
    const requests = new RequestVersion();
    const pending = requests.next();
    requests.invalidate();
    expect(requests.isCurrent(pending)).toBe(false);
  });
});
