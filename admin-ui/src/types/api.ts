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
  hasProxy: boolean
  proxyUrl?: string
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
  authRegion?: string
  apiRegion?: string
  machineId?: string
  proxyUrl?: string
  proxyUsername?: string
  proxyPassword?: string
}

// 添加凭据响应
export interface AddCredentialResponse {
  success: boolean
  message: string
  credentialId: number
  email?: string
}

// 邮件配置响应（密码脱敏）
export interface EmailConfigResponse {
  enabled: boolean
  smtpHost: string
  smtpPort: number
  smtpUsername: string
  smtpPasswordSet: boolean
  smtpTls: boolean
  fromAddress: string
  toAddresses: string[]
}

// 保存邮件配置请求
export interface SaveEmailConfigRequest {
  enabled: boolean
  smtpHost: string
  smtpPort: number
  smtpUsername: string
  smtpPassword: string
  smtpTls: boolean
  fromAddress: string
  toAddresses: string[]
}

// 测试邮件请求
export interface TestEmailRequest {
  smtpHost: string
  smtpPort: number
  smtpUsername: string
  smtpPassword: string
  smtpTls: boolean
  fromAddress: string
  toAddresses: string[]
}

// 测试 Webhook 请求
export interface TestWebhookRequest {
  url: string
  body?: string | null
}
