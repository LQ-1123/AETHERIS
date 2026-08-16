import type {
  ChoiceValue,
  NumberValue,
  ReportTemplateStructure,
  StructuredPayload,
  TemplateField,
} from './types';

/**
 * 结构化报告纯逻辑：payload 校验、空 payload 生成、三列文本渲染。
 *
 * 不变量（设计文档 §3）：
 *   I1 渲染/校验只读 payload.structure，任何路径不得查模板表；
 *   I2 findings/impression/recommendation 是 payload 的派生缓存；
 *   I5 section.id 固定枚举。
 */

const SECTION_IDS = ['findings', 'impression', 'recommendation'] as const;

export interface PayloadValidation {
  ok: boolean;
  errors: string[];
}

/** 由模板生成空 payload：结构原样携带，值为空。 */
export function payloadFromTemplate(template: {
  id: string;
  version: number;
  structure: ReportTemplateStructure;
}): StructuredPayload {
  return {
    template_id: template.id,
    template_version: template.version,
    structure: template.structure,
    values: {},
  };
}

/** 校验 payload：结构版本、章节枚举、字段 ID 唯一、值键存在、必填与取值范围。 */
export function validatePayload(payload: StructuredPayload): PayloadValidation {
  const errors: string[] = [];
  const structure = payload.structure;
  if (!structure || structure.schema_version !== 1) {
    errors.push('schema_version 必须为 1');
    return { ok: false, errors };
  }
  if (!Array.isArray(structure.sections) || structure.sections.length === 0) {
    errors.push('structure.sections 不能为空');
    return { ok: false, errors };
  }

  const seenSections = new Set<string>();
  const seenFields = new Set<string>();
  const fieldsByKey = new Map<string, TemplateField>();
  for (const section of structure.sections) {
    if (!(SECTION_IDS as readonly string[]).includes(section.id)) {
      errors.push(`非法 section.id: ${section.id}`);
      continue;
    }
    if (seenSections.has(section.id)) {
      errors.push(`重复 section.id: ${section.id}`);
      continue;
    }
    seenSections.add(section.id);
    for (const field of section.fields ?? []) {
      if (seenFields.has(field.id)) {
        // I4：字段 ID 全局唯一、不复用
        errors.push(`字段 ID 重复: ${field.id}`);
        continue;
      }
      seenFields.add(field.id);
      fieldsByKey.set(`${section.id}.${field.id}`, field);
    }
  }

  const valueKeys = new Set(Object.keys(payload.values ?? {}));
  for (const key of valueKeys) {
    if (!fieldsByKey.has(key)) errors.push(`未知值键: ${key}`);
  }
  for (const [key, field] of fieldsByKey) {
    if (field.required && !hasValue(payload.values?.[key])) {
      errors.push(`必填字段未填写: ${key}`);
      continue;
    }
    const value = payload.values?.[key];
    if (value === undefined || value === null) continue;
    if (field.kind === 'choice') {
      const choice = value as ChoiceValue;
      const optionIds = new Set((field.options ?? []).map((option) => option.id));
      if (!optionIds.has(choice?.choice)) errors.push(`非法选项值: ${key}`);
    } else if (field.kind === 'number') {
      const number = (value as NumberValue).value;
      if (!Number.isFinite(number)) {
        errors.push(`数值非法: ${key}`);
      } else if (field.min != null && number < field.min) {
        errors.push(`数值超出范围: ${key} < ${field.min}`);
      } else if (field.max != null && number > field.max) {
        errors.push(`数值超出范围: ${key} > ${field.max}`);
      }
    }
  }
  return { ok: errors.length === 0, errors };
}

function hasValue(value: unknown): boolean {
  if (value === undefined || value === null) return false;
  if (typeof value === 'string') return value.trim().length > 0;
  if (typeof value === 'object') {
    const choice = value as ChoiceValue;
    if ('choice' in choice) return choice.choice != null;
    const number = value as NumberValue;
    if ('value' in number) return Number.isFinite(number.value);
  }
  return true;
}

/** 由 payload 渲染三列文本（I2 派生缓存的唯一产生处）。 */
export function renderReportText(payload: StructuredPayload): {
  findings: string;
  impression: string;
  recommendation: string;
} {
  const rendered = { findings: '', impression: '', recommendation: '' };
  const structure = payload.structure;
  if (!structure || !Array.isArray(structure.sections)) return rendered;
  for (const section of structure.sections) {
    const lines: string[] = [];
    for (const field of section.fields ?? []) {
      const line = renderFieldLine(field, payload.values?.[`${section.id}.${field.id}`]);
      if (line) lines.push(line);
    }
    if (lines.length === 0) continue;
    const block = [section.title, ...lines].join('\n');
    if (section.id === 'findings') rendered.findings = appendBlock(rendered.findings, block);
    else if (section.id === 'impression') rendered.impression = appendBlock(rendered.impression, block);
    else if (section.id === 'recommendation') rendered.recommendation = appendBlock(rendered.recommendation, block);
  }
  return rendered;
}

function appendBlock(current: string, block: string): string {
  return current ? `${current}\n\n${block}` : block;
}

function renderFieldLine(field: TemplateField, value: unknown): string | null {
  if (field.kind === 'text') {
    if (typeof value !== 'string' || value.trim().length === 0) return null;
    return `${field.label}：${value.trim()}`;
  }
  if (field.kind === 'choice') {
    const choice = value as ChoiceValue;
    if (!choice || choice.choice == null) return null;
    const option = (field.options ?? []).find((candidate) => candidate.id === choice.choice);
    if (!option) return null;
    const description = choice.description?.trim();
    return `${field.label}：${option.label}${description ? `（${description}）` : ''}`;
  }
  // number
  const numberValue = value as NumberValue | undefined;
  const number = numberValue?.value;
  if (number == null || !Number.isFinite(number)) return null;
  return `${field.label}：${number}${field.unit ? ` ${field.unit}` : ''}`;
}
