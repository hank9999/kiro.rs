// 凭据状态响应
export interface CredentialsStatusResponse {
  total: number;
  available: number;
  currentId: number;
  credentials: CredentialStatusItem[];
}

// 单个凭据状态
export interface CredentialStatusItem {
  id: number;
  priority: number;
  disabled: boolean;
  failureCount: number;
  isCurrent: boolean;
  expiresAt: string | null;
  authMethod: string | null;
  hasProfileArn: boolean;
  email?: string;
  refreshTokenHash?: string;
  successCount: number;
  lastUsedAt: string | null;
  hasProxy: boolean;
  proxyUrl?: string;
  refreshFailureCount: number;
  disabledReason?: string;
}

// 余额响应
export interface BalanceResponse {
  id: number;
  subscriptionTitle: string | null;
  currentUsage: number;
  usageLimit: number;
  remaining: number;
  usagePercentage: number;
  nextResetAt: number | null;
}

// 成功响应
export interface SuccessResponse {
  success: boolean;
  message: string;
}

// 错误响应
export interface AdminErrorResponse {
  error: {
    type: string;
    message: string;
  };
}

// 请求类型
export interface SetDisabledRequest {
  disabled: boolean;
}

export interface SetPriorityRequest {
  priority: number;
}

// 添加凭据请求
export interface AddCredentialRequest {
  refreshToken: string;
  authMethod?: "social" | "idc";
  clientId?: string;
  clientSecret?: string;
  priority?: number;
  authRegion?: string;
  apiRegion?: string;
  machineId?: string;
  proxyUrl?: string;
  proxyUsername?: string;
  proxyPassword?: string;
}

// 添加凭据响应
export interface AddCredentialResponse {
  success: boolean;
  message: string;
  credentialId: number;
  email?: string;
}

// 请求活动统计
export interface RequestActivitySummary {
  totalRequests: number;
  successRequests: number;
  failedRequests: number;
  inFlight: number;
  successRate: number;
  lastUpdatedAt: string | null;
}

export interface RequestActivityRecord {
  id: number;
  requestId: string;
  method: string;
  path: string;
  endpoint: string;
  model?: string;
  messageCount?: number;
  stream: boolean;
  statusCode: number;
  success: boolean;
  durationMs: number;
  startedAt: string;
  finishedAt: string;
  error?: string;
  clientIp?: string;
  forwardedFor?: string;
  realIp?: string;
  forwardedProto?: string;
  userAgent?: string;
  referer?: string;
  origin?: string;
  transferEncoding?: string;
  contentLength?: number;
  clientRequestId?: string;
}

export interface RequestActivityResponse {
  summary: RequestActivitySummary;
  records: RequestActivityRecord[];
}

export interface AvailableModel {
  id: string;
  object: string;
  created: number;
  ownedBy: string;
  displayName: string;
  type: string;
  maxTokens: number;
}

export interface AvailableModelsResponse {
  object: string;
  data: AvailableModel[];
}

// 日志响应
export interface LogsResponse {
  path: string;
  available: boolean;
  fetchedAt: string;
  truncated: boolean;
  lines: string[];
  error?: string;
}

// ============ API Key 管理类型 ============

// API Key 信息
export interface ApiKeyInfo {
  id: string;
  key: string;
  name: string;
  enabled: boolean;
  createdAt: string;
  lastUsedAt?: string;
  isPrimary: boolean;
}

// API Keys 列表响应
export interface ApiKeysListResponse {
  apiKeys: ApiKeyInfo[];
  primaryKey?: ApiKeyInfo;
}

// 添加 API Key 请求
export interface AddApiKeyRequest {
  key: string;
  name: string;
}

// 生成 API Key 请求
export interface GenerateApiKeyRequest {
  name: string;
  length?: number;
}

// 生成 API Key 响应
export interface GenerateApiKeyResponse {
  key: string;
  id: string;
}

// 更新 API Key 请求
export interface UpdateApiKeyRequest {
  name?: string;
  enabled?: boolean;
}
