import { describe, expect, it } from 'vitest';
import { payloadFromTemplate, renderReportText, validatePayload } from './report-render';
import type { ReportTemplateStructure, StructuredPayload } from './types';

const structure: ReportTemplateStructure = {
  schema_version: 1,
  sections: [
    {
      id: 'findings',
      title: '影像所见',
      fields: [
        { id: 'f1', kind: 'text', label: '整体描述', required: true },
        {
          id: 'f2',
          kind: 'choice',
          label: '肺实质',
          options: [
            { id: 'normal', label: '未见明显异常' },
            { id: 'abnormal', label: '异常（展开描述）', expands: true },
          ],
        },
        { id: 'f3', kind: 'number', label: '最大结节径', unit: 'mm', min: 0, max: 300 },
      ],
    },
    {
      id: 'impression',
      title: '诊断意见',
      fields: [{ id: 'i1', kind: 'text', label: '诊断意见', required: true }],
    },
    {
      id: 'recommendation',
      title: '建议',
      fields: [{ id: 'r1', kind: 'text', label: '建议' }],
    },
  ],
};

function payload(values: Record<string, unknown>): StructuredPayload {
  return {
    template_id: 'tpl-1',
    template_version: 1,
    structure,
    values,
  };
}

describe('payloadFromTemplate', () => {
  it('creates an empty payload carrying template identity and structure', () => {
    const empty = payloadFromTemplate({ id: 'tpl-1', version: 3, structure });
    expect(empty.template_id).toBe('tpl-1');
    expect(empty.template_version).toBe(3);
    expect(empty.structure).toEqual(structure);
    expect(empty.values).toEqual({});
  });
});

describe('renderReportText', () => {
  it('renders text, choice and number fields with labels and units', () => {
    const text = renderReportText(payload({
      'findings.f1': '双肺纹理清晰',
      'findings.f2': { choice: 'normal' },
      'findings.f3': { value: 6.5 },
      'impression.i1': '右肺上叶小结节',
    }));
    expect(text.findings).toContain('整体描述：双肺纹理清晰');
    expect(text.findings).toContain('肺实质：未见明显异常');
    expect(text.findings).toContain('最大结节径：6.5 mm');
    expect(text.impression).toContain('诊断意见：右肺上叶小结节');
  });

  it('appends the description of an expanded choice option', () => {
    const text = renderReportText(payload({
      'findings.f1': '双肺纹理清晰',
      'findings.f2': { choice: 'abnormal', description: '右肺上叶小结节' },
      'impression.i1': '小结节，建议随访',
    }));
    expect(text.findings).toContain('肺实质：异常（展开描述）（右肺上叶小结节）');
  });

  it('skips empty optional fields and omits empty sections entirely', () => {
    const text = renderReportText(payload({ 'impression.i1': '未见异常' }));
    expect(text.findings).toBe('');
    expect(text.recommendation).toBe('');
    expect(text.impression).toContain('诊断意见：未见异常');
  });

  it('renders exclusively from the payload structure (I1: never from a current template)', () => {
    const differentStructure: ReportTemplateStructure = {
      schema_version: 1,
      sections: [
        {
          id: 'findings',
          title: '其他标题',
          fields: [{ id: 'other', kind: 'text', label: '其他字段' }],
        },
      ],
    };
    const sameValues = { 'findings.other': '只有新结构能渲染出这个值' };
    const before = renderReportText(payload(sameValues));
    const after = renderReportText({
      template_id: 'tpl-1',
      template_version: 9,
      structure: differentStructure,
      values: sameValues,
    });
    expect(before.findings).toBe('');
    expect(after.findings).toContain('其他字段：只有新结构能渲染出这个值');
  });
});

describe('validatePayload', () => {
  it('accepts a fully filled payload', () => {
    const result = validatePayload(payload({
      'findings.f1': '双肺纹理清晰',
      'findings.f2': { choice: 'normal' },
      'impression.i1': '未见异常',
    }));
    expect(result.ok).toBe(true);
    expect(result.errors).toEqual([]);
  });

  it('rejects a non-1 schema version', () => {
    const result = validatePayload({
      template_id: 'tpl-1',
      template_version: 1,
      structure: { schema_version: 2, sections: [] },
      values: {},
    } as unknown as StructuredPayload);
    expect(result.ok).toBe(false);
    expect(result.errors.join()).toContain('schema_version');
  });

  it('rejects unknown section ids and duplicate field ids (I5/I4)', () => {
    const badSection = {
      schema_version: 1,
      sections: [{ id: 'summary', title: 'X', fields: [] }],
    } as unknown as ReportTemplateStructure;
    expect(validatePayload(payload({}).structure !== badSection
      ? { template_id: 'x', template_version: 1, structure: badSection, values: {} }
      : payload({}))).toHaveProperty('ok', false);

    const duplicateFields: ReportTemplateStructure = {
      schema_version: 1,
      sections: [
        { id: 'findings', title: 'A', fields: [{ id: 'dup', kind: 'text', label: '1' }] },
        { id: 'impression', title: 'B', fields: [{ id: 'dup', kind: 'text', label: '2' }] },
      ],
    };
    const dupResult = validatePayload({
      template_id: 'x',
      template_version: 1,
      structure: duplicateFields,
      values: {},
    });
    expect(dupResult.ok).toBe(false);
    expect(dupResult.errors.join()).toContain('dup');
  });

  it('rejects unknown value keys, missing required values, bad choice ids and out-of-range numbers', () => {
    const result = validatePayload(payload({
      'findings.f2': { choice: 'not-an-option' },
      'findings.f3': { value: 9999 },
      'findings.unknown': 'orphan',
    }));
    expect(result.ok).toBe(false);
    const joined = result.errors.join();
    expect(joined).toContain('findings.f1');
    expect(joined).toContain('impression.i1');
    expect(joined).toContain('findings.f2');
    expect(joined).toContain('findings.f3');
    expect(joined).toContain('findings.unknown');
  });
});
