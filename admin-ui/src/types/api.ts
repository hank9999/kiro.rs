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
  apiKeyHash?: string
  maskedApiKey?: string
  successCount: number
  lastUsedAt: string | null
  hasProxy: boolean
  proxyUrl?: string
  refreshFailureCount: number
  disabledReason?: string
  endpoint: string
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
  refreshToken?: string
  authMethod?: 'social' | 'idc' | 'api_key'
  clientId?: string
  clientSecret?: string
  priority?: number
  authRegion?: string
  apiRegion?: string
  machineId?: string
  proxyUrl?: string
  proxyUsername?: string
  proxyPassword?: string
  kiroApiKey?: string
  endpoint?: string
  email?: string
}

// 添加凭据响应
export interface AddCredentialResponse {
  success: boolean
  message: string
  credentialId: number
  email?: string
}

// 更新凭据请求（字段为 undefined 表示不修改，空字符串表示清除）
export interface UpdateCredentialRequest {
  email?: string
  proxyUrl?: string
  proxyUsername?: string
  proxyPassword?: string
}

// 更新 refreshToken 请求
export interface UpdateRefreshTokenRequest {
  refreshToken: string
  accessToken?: string
  expiresAt?: string
}

// 代理池条目
export interface ProxyPoolEntry {
  id: number
  url: string
  label?: string
  enabled: boolean
  credentialCount: number
}

// 代理池列表响应
export interface ProxyPoolResponse {
  total: number
  proxies: ProxyPoolEntry[]
}

// 添加代理请求
export interface AddProxyRequest {
  url: string
  label?: string
}

// 批量添加代理请求
export interface BatchAddProxyRequest {
  urls: string[]
}

// 分配代理给凭据请求
export interface AssignProxyRequest {
  proxyId?: number | null
}

// 批量添加代理响应
export interface BatchAddProxyResponse {
  added: number
  errors: number
  proxies: ProxyPoolEntry[]
  errorMessages: string[]
}

// 全局代理配置
export interface GlobalProxyResponse {
  proxyUrl: string | null
}

export interface SetGlobalProxyRequest {
  proxyUrl: string | null
}

// Admin Key 修改
export interface UpdateAdminKeyRequest {
  newKey: string
}

// IdC 设备授权登录
export interface StartIdcLoginRequest {
  region: string
  startUrl?: string
  priority?: number
  email?: string
  proxyUrl?: string
}

export interface StartIdcLoginResponse {
  sessionId: string
  userCode: string
  verificationUri: string
  verificationUriComplete?: string
  expiresAt: string
  pollInterval: number
}

export type PollIdcLoginResponse =
  | { status: 'pending' }
  | { status: 'success'; credentialId: number }
  | { status: 'expired' }

// Social 登录（Portal PKCE OAuth）
export interface StartSocialLoginRequest {
  priority?: number
  email?: string
  proxyUrl?: string
  authEndpoint?: string
}

/** 远程访问时手动完成 Social 登录：从浏览器地址栏粘贴的回调 URL 中提取参数 */
export interface CompleteSocialLoginRequest {
  code: string
  state: string
  loginOption?: string
  path?: string
}

export interface StartSocialLoginResponse {
  sessionId: string
  portalUrl: string
  expiresAt: string
}

export type PollSocialLoginResponse = PollIdcLoginResponse
