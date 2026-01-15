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
  authMethod?: 'social' | 'idc' | 'builder-id'
  clientId?: string
  clientSecret?: string
  priority?: number
}

// 添加凭据响应
export interface AddCredentialResponse {
  success: boolean
  message: string
  credentialId: number
}

// ============ 凭据验证 ============

// 验证请求
export interface ValidateCredentialsRequest {
  credentialIds: number[]
  model?: 'sonnet' | 'opus' | 'haiku'
  timeoutMs?: number
  maxConcurrency?: number
}

// 验证响应
export interface ValidateCredentialsResponse {
  results: CredentialValidationResult[]
  summary: ValidationSummary
}

// 单个凭据验证结果
export interface CredentialValidationResult {
  id: number
  status: ValidationStatus
  message?: string
  latencyMs?: number
}

// 验证状态
export type ValidationStatus = 'ok' | 'denied' | 'invalid' | 'transient' | 'not_found'

// 验证汇总
export interface ValidationSummary {
  total: number
  ok: number
  denied: number
  invalid: number
  transient: number
  notFound: number
}

