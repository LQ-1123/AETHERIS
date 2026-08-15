import type { UserWindowPreset, ViewState, WindowPreset } from './types';

export type WindowPresetSelection =
  | { source: 'dicom'; id: number }
  | { source: 'user'; id: number };

export function parseWindowPresetSelection(value: string): WindowPresetSelection | null {
  const [source, rawId, extra] = value.split(':');
  const id = Number(rawId);
  if (extra !== undefined || !Number.isSafeInteger(id) || id < 0) return null;
  if (source === 'dicom' || source === 'user') return { source, id };
  return null;
}

export function normalizedModality(value: string | null | undefined): string | null {
  const modality = value?.trim().toUpperCase() ?? '';
  return /^[A-Z0-9]{1,16}$/.test(modality) ? modality : null;
}

export function userPresetsForModality(
  presets: readonly UserWindowPreset[],
  modality: string | null,
): UserWindowPreset[] {
  return modality ? presets.filter((preset) => preset.modality === modality) : [];
}

export function windowPresetMatchesState(preset: WindowPreset, state: ViewState): boolean {
  return Math.abs(preset.center - state.windowCenter) < 0.001
    && Math.abs(preset.width - state.windowWidth) < 0.001
    && preset.function === state.voiFunction;
}
