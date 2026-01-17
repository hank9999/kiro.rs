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

// ============ 批量导入 token.json ============

// 官方 token.json 格式
export interface TokenJsonItem {
  provider?: string
  refreshToken?: string
  clientId?: string
  clientSecret?: string
  authMethod?: string
  priority?: number
}

// 批量导入请求
export interface ImportTokenJsonRequest {
  dryRun: boolean
  items: TokenJsonItem | TokenJsonItem[]
}

// 批量导入响应
export interface ImportTokenJsonResponse {
  summary: ImportSummary
  items: ImportItemResult[]
}

// 导入汇总
export interface ImportSummary {
  parsed: number
  added: number
  skipped: number
  invalid: number
}

// 单项导入结果
export interface ImportItemResult {
  index: number
  fingerprint: string
  action: 'added' | 'skipped' | 'invalid'
  reason?: string
  credentialId?: number
}
