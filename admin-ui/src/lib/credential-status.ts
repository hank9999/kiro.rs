import type { CredentialStatusItem } from '@/types/api'

export type CredentialHealthKind =
  | 'available'
  | 'current'
  | 'call_unstable'
  | 'refresh_unstable'
  | 'manual_disabled'
  | 'too_many_failures'
  | 'too_many_refresh_failures'
  | 'quota_exceeded'
  | 'invalid_refresh_token'
  | 'invalid_config'

export type CredentialHealthTone =
  | 'green'
  | 'blue'
  | 'yellow'
  | 'orange'
  | 'red'
  | 'gray'

export interface CredentialHealth {
  kind: CredentialHealthKind
  label: string
  description: string
  tone: CredentialHealthTone
  isAvailable: boolean
  isProblem: boolean
}

export type CredentialStatusFilter =
  | 'all'
  | 'available'
  | 'current'
  | 'problem'
  | 'disabled'

export type AuthMethodFilter = 'all' | 'social' | 'idc' | 'api_key'

export function normalizeAuthMethod(
  authMethod: string | null | undefined
): string {
  if (!authMethod) return 'unknown'
  const value = authMethod.toLowerCase()
  if (value === 'builder-id' || value === 'iam') return 'idc'
  if (value === 'apikey') return 'api_key'
  return value
}

export function getAuthMethodLabel(
  authMethod: string | null | undefined
): string {
  const normalized = normalizeAuthMethod(authMethod)
  if (normalized === 'api_key') return 'API Key'
  if (normalized === 'idc') return 'IdC'
  if (normalized === 'social') return 'Social'
  return authMethod || '未知'
}

export function isApiKeyCredential(credential: CredentialStatusItem): boolean {
  return (
    normalizeAuthMethod(credential.authMethod) === 'api_key' ||
    Boolean(credential.apiKeyHash || credential.maskedApiKey)
  )
}

export function getCredentialHealth(
  credential: CredentialStatusItem
): CredentialHealth {
  if (credential.disabled) {
    switch (credential.disabledReason) {
      case 'InvalidConfig':
        return {
          kind: 'invalid_config',
          label: '配置无效',
          description: '该账号配置无效，需要修正配置后再恢复。',
          tone: 'red',
          isAvailable: false,
          isProblem: true,
        }
      case 'InvalidRefreshToken':
        return {
          kind: 'invalid_refresh_token',
          label: 'Token 失效',
          description: 'Refresh Token 已失效，需要重新导入或更新凭据。',
          tone: 'red',
          isAvailable: false,
          isProblem: true,
        }
      case 'QuotaExceeded':
        return {
          kind: 'quota_exceeded',
          label: '额度耗尽',
          description: '账号额度已耗尽，当前不会参与调度。',
          tone: 'red',
          isAvailable: false,
          isProblem: true,
        }
      case 'TooManyRefreshFailures':
        return {
          kind: 'too_many_refresh_failures',
          label: '刷新异常',
          description: 'Token 刷新连续失败，已被自动禁用。',
          tone: 'orange',
          isAvailable: false,
          isProblem: true,
        }
      case 'TooManyFailures':
        return {
          kind: 'too_many_failures',
          label: '调用异常',
          description: 'API 调用连续失败，已被自动禁用。',
          tone: 'orange',
          isAvailable: false,
          isProblem: true,
        }
      case 'Manual':
      case undefined:
        return {
          kind: 'manual_disabled',
          label: '已禁用',
          description: '账号已手动禁用，不参与调度。',
          tone: 'gray',
          isAvailable: false,
          isProblem: false,
        }
      default:
        return {
          kind: 'manual_disabled',
          label: '已禁用',
          description: `账号已禁用：${credential.disabledReason}`,
          tone: 'gray',
          isAvailable: false,
          isProblem: false,
        }
    }
  }

  if (credential.refreshFailureCount > 0) {
    return {
      kind: 'refresh_unstable',
      label: '刷新不稳',
      description: 'Token 刷新最近出现失败，但账号仍处于启用状态。',
      tone: 'yellow',
      isAvailable: true,
      isProblem: true,
    }
  }

  if (credential.failureCount > 0) {
    return {
      kind: 'call_unstable',
      label: '调用不稳',
      description: 'API 调用最近出现失败，但账号仍处于启用状态。',
      tone: 'yellow',
      isAvailable: true,
      isProblem: true,
    }
  }

  if (credential.isCurrent) {
    return {
      kind: 'current',
      label: '当前',
      description: '管理器最近选中的凭据。',
      tone: 'blue',
      isAvailable: true,
      isProblem: false,
    }
  }

  return {
    kind: 'available',
    label: '可用',
    description: '账号已启用，可参与调度。',
    tone: 'green',
    isAvailable: true,
    isProblem: false,
  }
}

export function matchesCredentialStatusFilter(
  credential: CredentialStatusItem,
  filter: CredentialStatusFilter
): boolean {
  if (filter === 'all') return true
  const health = getCredentialHealth(credential)
  if (filter === 'available') return health.isAvailable && !health.isProblem
  if (filter === 'current') return credential.isCurrent
  if (filter === 'problem') return health.isProblem
  if (filter === 'disabled') return credential.disabled
  return true
}

export function matchesAuthMethodFilter(
  credential: CredentialStatusItem,
  filter: AuthMethodFilter
): boolean {
  if (filter === 'all') return true
  return normalizeAuthMethod(credential.authMethod) === filter
}

export function canRefreshCredentialToken(
  credential: CredentialStatusItem
): boolean {
  return !credential.disabled && !isApiKeyCredential(credential)
}

export function canResetCredentialFailure(
  credential: CredentialStatusItem
): boolean {
  if (credential.disabledReason === 'InvalidConfig') return false
  return (
    credential.failureCount > 0 ||
    credential.refreshFailureCount > 0 ||
    (credential.disabled && credential.disabledReason !== 'Manual')
  )
}

export function canDeleteCredential(credential: CredentialStatusItem): boolean {
  return credential.disabled
}

export function canVerifyCredential(credential: CredentialStatusItem): boolean {
  return !credential.disabled
}

export function canQueryCredentialBalance(
  credential: CredentialStatusItem
): boolean {
  return !credential.disabled
}

export function canEnableCredential(credential: CredentialStatusItem): boolean {
  return credential.disabled && credential.disabledReason !== 'InvalidConfig'
}

export function canDisableCredential(credential: CredentialStatusItem): boolean {
  return !credential.disabled
}

export function canToggleCredentialDisabled(
  credential: CredentialStatusItem
): boolean {
  return credential.disabled
    ? canEnableCredential(credential)
    : canDisableCredential(credential)
}
