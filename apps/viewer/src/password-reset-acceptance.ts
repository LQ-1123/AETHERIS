import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';

export interface PasswordResetAcceptanceState {
  calls: Array<{ command: string; args: Record<string, unknown> }>;
  approved: boolean;
}

declare global {
  interface Window {
    __passwordResetAcceptance?: PasswordResetAcceptanceState;
  }
}

export function installPasswordResetAcceptanceMock(): void {
  const state: PasswordResetAcceptanceState = { calls: [], approved: false };
  window.__passwordResetAcceptance = state;
  mockWindows('main');
  const invokeHandler = async (command: string, payload?: Record<string, unknown>): Promise<unknown> => {
    const args = (payload ?? {}) as Record<string, unknown>;
    state.calls.push({ command, args });
    if (command === 'local_stack_info') return null;
    if (command === 'plugin:event|listen' || command === 'plugin:event|unlisten') return null;
    if (command === 'remote_login') {
      return {
        id: 1,
        username: 'admin.wang',
        display_name: '王管理员',
        role: 'admin',
        institution_id: 1,
        institution_name: '中心医院',
      };
    }
    if (command === 'list_window_presets' || command === 'list_queue_studies') return [];
    if (command === 'transform_schema') return { manual_tags: [] };
    if (command === 'request_password_reset') return null;
    if (command === 'list_devices') return [];
    if (command === 'list_users') {
      return [
        {
          id: 1,
          username: 'admin.wang',
          display_name: '王管理员',
          role: 'admin',
          is_active: true,
          must_change_password: false,
          last_login_at: '2026-08-18T11:30:00Z',
          created_at: '2026-01-01T08:00:00Z',
        },
        {
          id: 8,
          username: 'doctor.li',
          display_name: '李医生',
          role: 'radiologist',
          is_active: true,
          must_change_password: false,
          last_login_at: '2026-08-18T10:12:00Z',
          created_at: '2026-03-01T08:00:00Z',
        },
      ];
    }
    if (command === 'list_user_permissions') return [];
    if (command === 'list_password_reset_requests') {
      return state.approved ? [] : [{
        id: 17,
        user_id: 8,
        username: 'doctor.li',
        display_name: '李医生',
        status: 'pending',
        requested_at: '2026-08-18T12:05:00+08:00',
        reviewed_by: null,
        reviewer_name: null,
        reviewed_at: null,
      }];
    }
    if (command === 'review_password_reset_request') {
      state.approved = Boolean(args.approve);
      return {
        id: Number(args.requestId),
        user_id: 8,
        username: 'doctor.li',
        display_name: '李医生',
        status: state.approved ? 'approved' : 'rejected',
        requested_at: '2026-08-18T12:05:00+08:00',
        reviewed_by: 1,
        reviewer_name: 'admin.wang',
        reviewed_at: '2026-08-18T12:08:00+08:00',
      };
    }
    throw new Error(`密码重置验收未模拟命令: ${command}`);
  };
  mockIPC(invokeHandler as Parameters<typeof mockIPC>[0]);
}
