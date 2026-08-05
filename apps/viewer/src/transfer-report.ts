import type { ImportTransferResponse } from './types';

export function importSummary(response: ImportTransferResponse): string {
  const result = response.job.result;
  if (!result) return '导入完成';
  return `新增 ${result.created ?? 0} · 重复 ${result.duplicates ?? 0} · 冲突 ${result.conflicts ?? 0}`;
}

export function importConflictMessage(response: ImportTransferResponse): string | null {
  const conflicts = (response.items ?? []).filter(
    (item) => item.status === 'conflict' || item.result.disposition === 'conflict',
  );
  if (!conflicts.length) return null;

  const details = conflicts.slice(0, 3).map((item) => {
    const name = item.input.name?.trim() || item.item_key;
    const uid = item.result.sop_instance_uid?.trim();
    return uid ? `${name}（SOPInstanceUID: ${uid}）` : name;
  });
  const remaining = conflicts.length - details.length;
  const suffix = remaining > 0 ? `；另有 ${remaining} 个冲突` : '';
  return `导入发现 ${conflicts.length} 个 UID 内容冲突，已拒绝覆盖原始影像：${details.join('；')}${suffix}`;
}
