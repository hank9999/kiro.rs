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
  accountEmail: string | null

  // ===== 统计（可持久化） =====
  callsTotal: number
  callsOk: number
  callsErr: number
  inputTokensTotal: number
  outputTokensTotal: number
  lastCallAt: string | null
  lastSuccessAt: string | null
  lastErrorAt: string | null
  lastError: string | null
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

// ===== 统计（可持久化） =====

export interface StatsBucket {
  // 按日：YYYY-MM-DD；按模型：model id
  key: string
  callsTotal: number
  callsOk: number
  callsErr: number
  inputTokensTotal: number
  outputTokensTotal: number
  lastCallAt: string | null
  lastSuccessAt: string | null
  lastErrorAt: string | null
  lastError: string | null
}

export interface CredentialStatsResponse {
  id: number
  callsTotal: number
  callsOk: number
  callsErr: number
  inputTokensTotal: number
  outputTokensTotal: number
  lastCallAt: string | null
  lastSuccessAt: string | null
  lastErrorAt: string | null
  lastError: string | null
  byDay: StatsBucket[]
  byModel: StatsBucket[]
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

// ===== 账号信息（套餐/用量/邮箱等） =====

export interface CreditBonus {
  code: string
  name: string
  current: number
  limit: number
  expiresAt: string | null
}

export interface CreditsResourceDetail {
  displayName: string | null
  displayNamePlural: string | null
  resourceType: string | null
  currency: string | null
  unit: string | null
  overageRate: number | null
  overageCap: number | null
}

export interface CreditsUsageSummary {
  current: number
  limit: number
  baseCurrent: number
  baseLimit: number
  freeTrialCurrent: number
  freeTrialLimit: number
  freeTrialExpiry: string | null
  bonuses: CreditBonus[]
  nextResetDate: string | null
  overageEnabled: boolean | null
  resourceDetail: CreditsResourceDetail | null
}

export interface AccountSubscriptionDetails {
  rawType: string | null
  managementTarget: string | null
  upgradeCapability: string | null
  overageCapability: string | null
}

export interface ResourceUsageSummary {
  resourceType: string | null
  displayName: string | null
  unit: string | null
  currency: string | null
  current: number
  limit: number
}

export interface UsageAndLimitsResponse {
  userInfo: { email: string | null; userId: string | null } | null
  subscriptionInfo:
    | {
        type: string | null
        subscriptionTitle: string | null
        upgradeCapability: string | null
        overageCapability: string | null
        subscriptionManagementTarget: string | null
      }
    | null
  usageBreakdownList:
    | Array<{
        resourceType: string | null
        currentUsage: number | null
        currentUsageWithPrecision: number | null
        usageLimit: number | null
        usageLimitWithPrecision: number | null
        displayName: string | null
        displayNamePlural: string | null
        currency: string | null
        unit: string | null
        overageRate: number | null
        overageCap: number | null
        freeTrialInfo:
          | {
              usageLimit: number | null
              usageLimitWithPrecision: number | null
              currentUsage: number | null
              currentUsageWithPrecision: number | null
              freeTrialExpiry: string | null
              freeTrialStatus: string | null
            }
          | null
        bonuses:
          | Array<{
              bonusCode: string | null
              displayName: string | null
              usageLimit: number | null
              usageLimitWithPrecision: number | null
              currentUsage: number | null
              currentUsageWithPrecision: number | null
              status: string | null
              expiresAt: string | null
            }>
          | null
      }>
    | null
  nextDateReset: string | null
  overageConfiguration: { overageEnabled: boolean | null } | null
}

export interface AccountAggregateInfo {
  email: string | null
  userId: string | null
  idp: string | null
  status: string | null
  featureFlags: string[] | null
  subscriptionTitle: string | null
  subscriptionType: string
  subscription: AccountSubscriptionDetails
  usage: CreditsUsageSummary
  resources: ResourceUsageSummary[]
  rawUsage: UsageAndLimitsResponse
}

export interface CredentialAccountInfoResponse {
  id: number
  account: AccountAggregateInfo
}
