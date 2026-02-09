// 凭据状态响应
export interface CredentialsStatusResponse {
  total: number
  available: number
  currentId: number
  credentials: CredentialStatusItem[]
}

// 单个凭据状态
export interface CredentialStatusItem {
  id: number
  priority: number
  disabled: boolean
  failureCount: number
  isCurrent: boolean
  expiresAt: string | null
  authMethod: string | null
  hasProfileArn: boolean
  email?: string
  refreshTokenHash?: string
  successCount: number
  lastUsedAt: string | null
}

// 余额响应
export interface BalanceResponse {
  id: number
  subscriptionTitle: string | null
  currentUsage: number
  usageLimit: number
  remaining: number
  usagePercentage: number
  nextResetAt: number | null
}

// 成功响应
export interface SuccessResponse {
  success: boolean
  message: string
}

// 错误响应
export interface AdminErrorResponse {
  error: {
    type: string
    message: string
  }
}

// 请求类型
export interface SetDisabledRequest {
  disabled: boolean
}

export interface SetPriorityRequest {
  priority: number
}

// 添加凭据请求
export interface AddCredentialRequest {
  refreshToken: string
  authMethod?: 'social' | 'idc'
  clientId?: string
  clientSecret?: string
  priority?: number
  region?: string
  machineId?: string
}

// 添加凭据响应
export interface AddCredentialResponse {
  success: boolean
  message: string
  credentialId: number
  email?: string
}

// ===== Flow Monitor 类型 =====

export interface FlowRecord {
  id: number
  requestId: string
  timestamp: string
  path: string
  model: string
  stream: boolean
  inputTokens: number | null
  outputTokens: number | null
  totalTokens: number | null
  durationMs: number
  statusCode: number
  error: string | null
  userId: string | null
}

export interface FlowListResponse {
  total: number
  page: number
  pageSize: number
  records: FlowRecord[]
}

export interface FlowStatsResponse {
  totalRequests: number
  totalInputTokens: number
  totalOutputTokens: number
  totalTokens: number
  avgDurationMs: number
  errorCount: number
  errorRate: number
  models: ModelStats[]
}

export interface ModelStats {
  model: string
  count: number
  totalInputTokens: number
  totalOutputTokens: number
  avgDurationMs: number
}

export interface FlowQuery {
  page?: number
  pageSize?: number
  model?: string
  status?: 'success' | 'error'
  startTime?: string
  endTime?: string
}
