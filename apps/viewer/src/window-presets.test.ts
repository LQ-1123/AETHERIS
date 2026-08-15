import { describe, expect, it } from 'vitest';

import type { UserWindowPreset, ViewState } from './types';
import {
  normalizedModality,
  parseWindowPresetSelection,
  userPresetsForModality,
  windowPresetMatchesState,
} from './window-presets';

const presets: UserWindowPreset[] = [
  {
    id: 1,
    modality: 'CT',
    name: '肺窗',
    center: -600,
    width: 1500,
    function: 'LINEAR',
    explanation: null,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  },
  {
    id: 2,
    modality: 'MR',
    name: 'MR 窗',
    center: 80,
    width: 160,
    function: 'LINEAR_EXACT',
    explanation: null,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  },
];

describe('personal window presets', () => {
  it('normalizes and filters by modality', () => {
    expect(normalizedModality(' ct ')).toBe('CT');
    expect(normalizedModality('CT/MR')).toBeNull();
    expect(userPresetsForModality(presets, 'CT')).toEqual([presets[0]]);
    expect(userPresetsForModality(presets, null)).toEqual([]);
  });

  it('parses only DICOM and personal selection values', () => {
    expect(parseWindowPresetSelection('dicom:0')).toEqual({ source: 'dicom', id: 0 });
    expect(parseWindowPresetSelection('user:42')).toEqual({ source: 'user', id: 42 });
    expect(parseWindowPresetSelection('ct:0')).toBeNull();
    expect(parseWindowPresetSelection('user:-1')).toBeNull();
  });

  it('matches the complete window state', () => {
    const state = {
      windowCenter: -600,
      windowWidth: 1500,
      voiFunction: 'LINEAR',
    } as ViewState;
    expect(windowPresetMatchesState(presets[0], state)).toBe(true);
    expect(windowPresetMatchesState({ ...presets[0], width: 1499 }, state)).toBe(false);
  });
});
