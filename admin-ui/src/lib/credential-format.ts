import type { BalanceResponse, CredentialStatusItem } from '@/types/api'
import { getAuthMethodLabel } from '@/lib/credential-status'

export function formatRelativeTime(value: string | null): string {
  if (!value) return '从未使用'

  const date = new Date(value)
  const time = date.getTime()
  if (Number.isNaN(time)) return '未知'

  const diff = Date.now() - time
  if (diff < 0) return '刚刚'

  const seconds = Math.floor(diff / 1000)
  if (seconds < 60) return `${seconds} 秒前`

  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes} 分钟前`

  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} 小时前`

  const days = Math.floor(hours / 24)
  if (days < 30) return `${days} 天前`

  return date.toLocaleDateString('zh-CN')
}

export function formatDateTime(value: string | number | null): string {
  if (value === null) return '未知'

  const date =
    typeof value === 'number' ? new Date(value * 1000) : new Date(value)
  if (Number.isNaN(date.getTime())) return '未知'

  return date.toLocaleString('zh-CN')
}

export function formatNumber(value: number): string {
  return value.toLocaleString('zh-CN', {
    minimumFractionDigits: 0,
    maximumFractionDigits: 2,
  })
}

export function formatCurrencyLikeNumber(value: number): string {
  return value.toLocaleString('zh-CN', {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })
}

export function formatPercent(value: number): string {
  return `${value.toFixed(1)}%`
}

export function getCredentialDisplayName(
  credential: CredentialStatusItem
): string {
  if (credential.email) return credential.email
  if (credential.maskedApiKey) return credential.maskedApiKey
  return `凭据 #${credential.id}`
}

export function getCredentialSecondaryText(
  credential: CredentialStatusItem
): string {
  const parts = [`#${credential.id}`]

  if (credential.hasProfileArn) parts.push('Profile ARN')
  if (credential.hasProxy) parts.push('代理')
  if (credential.endpoint) parts.push(credential.endpoint)
  parts.push(getAuthMethodLabel(credential.authMethod))

  return parts.join(' · ')
}

export function formatBalanceSummary(balance: BalanceResponse | null): string {
  if (!balance) return '未查询'
  return `${formatCurrencyLikeNumber(balance.remaining)} / ${formatCurrencyLikeNumber(
    balance.usageLimit
  )}`
}

export function formatSubscriptionTitle(
  balance: BalanceResponse | null
): string {
  return balance?.subscriptionTitle || '未知'
}

export function formatUsageRatio(balance: BalanceResponse | null): string {
  if (!balance) return '未查询'
  return `${formatPercent(balance.usagePercentage)} 已使用`
}

export function getCredentialSearchText(
  credential: CredentialStatusItem
): string {
  return [
    credential.id,
    credential.email,
    credential.endpoint,
    credential.authMethod,
    credential.disabledReason,
    credential.maskedApiKey,
    credential.refreshTokenHash,
    credential.apiKeyHash,
    credential.proxyUrl,
  ]
    .filter(Boolean)
    .join(' ')
    .toLowerCase()
}
