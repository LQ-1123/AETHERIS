import { describe, expect, it } from 'vitest';
import { importConflictMessage, importSummary } from './transfer-report';
import type { ImportTransferResponse } from './types';

describe('import transfer report', () => {
  it('keeps identical retransmissions in the summary without reporting an error', () => {
    const response: ImportTransferResponse = {
      job: { result: { created: 1, duplicates: 2, conflicts: 0 } },
      items: [{
        item_key: 'upload:image.dcm', status: 'succeeded', input: { name: 'image.dcm' },
        result: { disposition: 'duplicate', sop_instance_uid: '1.2.3' },
      }],
    };
    expect(importSummary(response)).toBe('新增 1 · 重复 2 · 冲突 0');
    expect(importConflictMessage(response)).toBeNull();
  });

  it('reports conflicting filename and SOP Instance UID', () => {
    const response: ImportTransferResponse = {
      job: { result: { created: 0, duplicates: 0, conflicts: 1 } },
      items: [{
        item_key: 'upload:changed.dcm', status: 'conflict', input: { name: 'changed.dcm' },
        result: { disposition: 'conflict', sop_instance_uid: '1.2.840.1', error: '内容不同' },
      }],
    };
    expect(importConflictMessage(response)).toContain('changed.dcm（SOPInstanceUID: 1.2.840.1）');
    expect(importConflictMessage(response)).toContain('已拒绝覆盖原始影像');
  });
});
