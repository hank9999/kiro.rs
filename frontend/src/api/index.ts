// API 服务
// Input: fetch API
// Output: API 请求函数
// Pos: 前端 API 层

const API_BASE = '/api/admin';

// 通用请求函数
async function request<T>(url: string, options?: RequestInit): Promise<T> {
  const response = await fetch(`${API_BASE}${url}`, {
    ...options,
    headers: {
      'Content-Type': 'application/json',
      ...options?.headers,
    },
    credentials: 'include',
  });

  if (!response.ok) {
    const error = await response.json().catch(() => ({ message: '请求失败' }));
    throw new Error(error.message || error.error?.message || '请求失败');
  }

  return response.json();
}

// 认证相关
export interface LoginRequest {
  username: string;
  password: string;
}

export interface LoginResponse {
  success: boolean;
  message: string;
  username?: string;
}

export interface UserInfo {
  id: number;
  username: string;
}

export interface ChangePasswordRequest {
  oldPassword: string;
  newPassword: string;
}

export const authApi = {
  login: (data: LoginRequest) =>
    request<LoginResponse>('/login', {
      method: 'POST',
      body: JSON.stringify(data),
    }),

  logout: () =>
    request<{ success: boolean; message: string }>('/logout', {
      method: 'POST',
    }),

  me: () => request<UserInfo>('/me'),

  changePassword: (data: ChangePasswordRequest) =>
    request<{ success: boolean; message: string }>('/change-password', {
      method: 'POST',
      body: JSON.stringify(data),
    }),
};

// 凭据相关
export interface CredentialItem {
  id: number;
  priority: number;
  disabled: boolean;
  disabledReason?: string; // suspended: 账号暂停, quota: 额度用尽, manual: 手动禁用, failures: 连续失败
  failureCount: number;
  isCurrent: boolean;
  expiresAt?: string;
  authMethod?: string;
  region?: string;
  machineId?: string;
  hasProfileArn: boolean;
  email?: string;
  subscriptionTitle?: string;
  currentUsage: number;
  usageLimit: number;
  proxyUrl?: string;
}

export interface CredentialsResponse {
  total: number;
  available: number;
  currentId: number;
  credentials: CredentialItem[];
}

export interface AddCredentialRequest {
  refreshToken: string;
  authMethod?: string;
  clientId?: string;
  clientSecret?: string;
  priority?: number;
  region?: string;
  machineId?: string;
  proxyUrl?: string;
}

export interface BalanceResponse {
  id: number;
  subscriptionTitle?: string;
  currentUsage: number;
  usageLimit: number;
  remaining: number;
  usagePercentage: number;
  nextResetAt?: number;
}

export interface UpdateCredentialRequest {
  priority?: number;
  region?: string;
  machineId?: string;
  refreshToken?: string;
  clientId?: string;
  clientSecret?: string;
  proxyUrl?: string;
}

export interface RefreshAllResponse {
  success: boolean;
  message: string;
  successCount: number;
  failures: { id: number; error: string }[];
}

export const credentialsApi = {
  getAll: () => request<CredentialsResponse>('/credentials'),

  add: (data: AddCredentialRequest) =>
    request<{ success: boolean; message: string; credentialId: number }>('/credentials', {
      method: 'POST',
      body: JSON.stringify(data),
    }),

  update: (id: number, data: UpdateCredentialRequest) =>
    request<{ success: boolean; message: string }>(`/credentials/${id}`, {
      method: 'PUT',
      body: JSON.stringify(data),
    }),

  delete: (id: number) =>
    request<{ success: boolean; message: string }>(`/credentials/${id}`, {
      method: 'DELETE',
    }),

  setDisabled: (id: number, disabled: boolean) =>
    request<{ success: boolean; message: string }>(`/credentials/${id}/disabled`, {
      method: 'POST',
      body: JSON.stringify({ disabled }),
    }),

  setPriority: (id: number, priority: number) =>
    request<{ success: boolean; message: string }>(`/credentials/${id}/priority`, {
      method: 'POST',
      body: JSON.stringify({ priority }),
    }),

  reset: (id: number) =>
    request<{ success: boolean; message: string }>(`/credentials/${id}/reset`, {
      method: 'POST',
    }),

  getBalance: (id: number) => request<BalanceResponse>(`/credentials/${id}/balance`),

  refreshAll: () =>
    request<RefreshAllResponse>('/credentials/refresh', {
      method: 'POST',
    }),
};

// 系统设置相关
export interface SettingsResponse {
  kiroVersion: string;
  systemVersion: string;
  nodeVersion: string;
  minUsageThreshold: number;
  countTokensApiUrl?: string;
  countTokensApiKey?: string;
  countTokensAuthType: string;
}

export interface UpdateSettingsRequest {
  kiroVersion?: string;
  systemVersion?: string;
  nodeVersion?: string;
  minUsageThreshold?: number;
  countTokensApiUrl?: string;
  countTokensApiKey?: string;
  countTokensAuthType?: string;
}

export const settingsApi = {
  get: () => request<SettingsResponse>('/settings'),

  update: (data: UpdateSettingsRequest) =>
    request<{ success: boolean; message: string }>('/settings', {
      method: 'POST',
      body: JSON.stringify(data),
    }),
};

// 导入导出相关
export interface ExportCredentialItem {
  refreshToken: string;
  clientId?: string;
  clientSecret?: string;
  region?: string;
  proxyUrl?: string;
}

export interface ExportCredentialsRequest {
  ids?: number[];
}

export interface ExportCredentialsResponse {
  success: boolean;
  message: string;
  count: number;
  credentials: ExportCredentialItem[];
}

export interface ImportCredentialItem {
  refreshToken: string;
  clientId?: string;
  clientSecret?: string;
  region?: string;
  proxyUrl?: string;
}

export interface ImportCredentialsResponse {
  success: boolean;
  message: string;
  successCount: number;
  failures: { index: number; error: string }[];
}

export const importExportApi = {
  // 导出凭据
  export: (data: ExportCredentialsRequest) =>
    request<ExportCredentialsResponse>('/credentials/export', {
      method: 'POST',
      body: JSON.stringify(data),
    }),

  // 批量导入凭据
  import: (data: ImportCredentialItem | ImportCredentialItem[]) =>
    request<ImportCredentialsResponse>('/credentials/import', {
      method: 'POST',
      body: JSON.stringify(data),
    }),

  // 使用此账号
  useCurrent: (id: number) =>
    request<{ success: boolean; message: string }>(`/credentials/${id}/use`, {
      method: 'POST',
    }),
};
