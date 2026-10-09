import { useState } from 'react'
import {
  ChevronDown,
  ChevronUp,
  Info,
  Loader2,
  MoreHorizontal,
  RefreshCw,
  RotateCcw,
  Trash2,
  Wallet,
} from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Switch } from '@/components/ui/switch'
import { StatusBadge } from '@/components/account-pool/status-badge'
import {
  useDeleteCredential,
  useForceRefreshToken,
  useResetFailure,
  useSetDisabled,
  useSetPriority,
} from '@/hooks/use-credentials'
import {
  canDeleteCredential,
  canRefreshCredentialToken,
  canResetCredentialFailure,
  canToggleCredentialDisabled,
  getAuthMethodLabel,
  getCredentialHealth,
} from '@/lib/credential-status'
import {
  formatBalanceSummary,
  formatRelativeTime,
  formatSubscriptionTitle,
  getCredentialDisplayName,
  getCredentialSecondaryText,
} from '@/lib/credential-format'
import { cn } from '@/lib/utils'
import type { BalanceResponse, CredentialStatusItem } from '@/types/api'

interface CredentialPoolTableProps {
  credentials: CredentialStatusItem[]
  selectedIds: Set<number>
  balanceMap: Map<number, BalanceResponse>
  loadingBalanceIds: Set<number>
  onToggleSelect: (id: number) => void
  onTogglePageSelection: (checked: boolean) => void
  onOpenDetails: (id: number) => void
  onViewBalance: (id: number) => void
}

interface CredentialRowProps {
  credential: CredentialStatusItem
  selected: boolean
  balance: BalanceResponse | null
  loadingBalance: boolean
  onToggleSelect: () => void
  onOpenDetails: (id: number) => void
  onViewBalance: (id: number) => void
}

function CredentialRow({
  credential,
  selected,
  balance,
  loadingBalance,
  onToggleSelect,
  onOpenDetails,
  onViewBalance,
}: CredentialRowProps) {
  const [showDeleteDialog, setShowDeleteDialog] = useState(false)
  const setDisabled = useSetDisabled()
  const setPriority = useSetPriority()
  const resetFailure = useResetFailure()
  const deleteCredential = useDeleteCredential()
  const forceRefresh = useForceRefreshToken()
  const health = getCredentialHealth(credential)
  const canToggleDisabled = canToggleCredentialDisabled(credential)
  const disabledToggleTitle = !canToggleDisabled && credential.disabledReason === 'InvalidConfig'
    ? '配置无效，需要修正配置后重启服务'
    : undefined

  const pending =
    setDisabled.isPending ||
    setPriority.isPending ||
    resetFailure.isPending ||
    deleteCredential.isPending ||
    forceRefresh.isPending

  const handleToggleDisabled = () => {
    if (!canToggleDisabled) {
      toast.error('配置无效，需要修正配置后重启服务')
      return
    }

    setDisabled.mutate(
      { id: credential.id, disabled: !credential.disabled },
      {
        onSuccess: (res) => toast.success(res.message),
        onError: (err) => toast.error('操作失败: ' + (err as Error).message),
      }
    )
  }

  const handlePriorityStep = (delta: number) => {
    const nextPriority = Math.max(0, credential.priority + delta)
    if (nextPriority === credential.priority) return

    setPriority.mutate(
      { id: credential.id, priority: nextPriority },
      {
        onSuccess: (res) => toast.success(res.message),
        onError: (err) => toast.error('操作失败: ' + (err as Error).message),
      }
    )
  }

  const handleReset = () => {
    resetFailure.mutate(credential.id, {
      onSuccess: (res) => toast.success(res.message),
      onError: (err) => toast.error('操作失败: ' + (err as Error).message),
    })
  }

  const handleForceRefresh = () => {
    forceRefresh.mutate(credential.id, {
      onSuccess: (res) => toast.success(res.message),
      onError: (err) => toast.error('刷新失败: ' + (err as Error).message),
    })
  }

  const handleDelete = () => {
    if (!credential.disabled) {
      toast.error('请先禁用凭据再删除')
      setShowDeleteDialog(false)
      return
    }

    deleteCredential.mutate(credential.id, {
      onSuccess: (res) => {
        toast.success(res.message)
        setShowDeleteDialog(false)
      },
      onError: (err) => toast.error('删除失败: ' + (err as Error).message),
    })
  }

  return (
    <>
      <tr
        className={cn(
          'border-b transition-colors hover:bg-muted/40',
          selected && 'bg-muted/60',
          credential.isCurrent && !credential.disabled && 'bg-sky-50/70 dark:bg-sky-950/20'
        )}
      >
        <td className="w-10 px-4 py-3 align-middle">
          <Checkbox checked={selected} onCheckedChange={onToggleSelect} />
        </td>
        <td className="px-3 py-3 align-middle">
          <div className="flex min-w-0 flex-col gap-1">
            <div
              className="truncate text-sm font-medium text-foreground"
              title={getCredentialDisplayName(credential)}
            >
              {getCredentialDisplayName(credential)}
            </div>
            <div
              className="truncate text-xs text-muted-foreground"
              title={getCredentialSecondaryText(credential)}
            >
              {getCredentialSecondaryText(credential)}
            </div>
          </div>
        </td>
        <td className="w-28 px-3 py-3 align-middle">
          <StatusBadge health={health} />
        </td>
        <td className="w-36 px-3 py-3 align-middle">
          <div className="flex min-w-0 flex-col gap-1 text-sm">
            <span className="truncate">{getAuthMethodLabel(credential.authMethod)}</span>
            <span className="truncate text-xs text-muted-foreground" title={credential.endpoint}>
              {credential.endpoint}
            </span>
          </div>
        </td>
        <td className="w-24 px-3 py-3 align-middle text-sm font-medium">
          {credential.priority}
        </td>
        <td className="w-32 px-3 py-3 align-middle text-sm">
          <div className="flex flex-col gap-1">
            <span>{credential.successCount} 成功</span>
            <span
              className={cn(
                'text-xs text-muted-foreground',
                (credential.failureCount > 0 || credential.refreshFailureCount > 0) &&
                  'font-medium text-red-600 dark:text-red-400'
              )}
            >
              {credential.failureCount} 失败 / {credential.refreshFailureCount} 刷新
            </span>
          </div>
        </td>
        <td className="w-44 px-3 py-3 align-middle text-sm">
          {loadingBalance ? (
            <span className="inline-flex items-center gap-1 text-muted-foreground">
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              查询中
            </span>
          ) : (
            <div className="flex min-w-0 flex-col gap-1">
              <span className="truncate font-medium" title={formatBalanceSummary(balance)}>
                {formatBalanceSummary(balance)}
              </span>
              <span className="truncate text-xs text-muted-foreground">
                {formatSubscriptionTitle(balance)}
              </span>
            </div>
          )}
        </td>
        <td className="w-28 px-3 py-3 align-middle text-sm text-muted-foreground">
          {formatRelativeTime(credential.lastUsedAt)}
        </td>
        <td className="w-24 px-3 py-3 align-middle">
          <div className="flex items-center gap-2">
            <Switch
              checked={!credential.disabled}
              onCheckedChange={handleToggleDisabled}
              disabled={setDisabled.isPending || !canToggleDisabled}
              title={disabledToggleTitle}
            />
          </div>
        </td>
        <td className="w-24 px-4 py-3 align-middle">
          <div className="flex items-center justify-end gap-1">
            <Button
              size="sm"
              variant="ghost"
              className="h-8 px-2"
              onClick={() => onOpenDetails(credential.id)}
            >
              <Info className="h-4 w-4" />
              <span className="sr-only">查看详情</span>
            </Button>
            <Button
              size="sm"
              variant="ghost"
              className="h-8 px-2"
              onClick={() => onViewBalance(credential.id)}
            >
              <Wallet className="h-4 w-4" />
              <span className="sr-only">查看余额</span>
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button size="sm" variant="ghost" className="h-8 w-8 p-0" disabled={pending}>
                  <MoreHorizontal className="h-4 w-4" />
                  <span className="sr-only">更多操作</span>
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem
                  onClick={handleReset}
                  disabled={!canResetCredentialFailure(credential) || resetFailure.isPending}
                >
                  <RotateCcw className="h-4 w-4" />
                  重置失败
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={handleForceRefresh}
                  disabled={!canRefreshCredentialToken(credential) || forceRefresh.isPending}
                >
                  <RefreshCw className="h-4 w-4" />
                  刷新 Token
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  onClick={() => handlePriorityStep(-1)}
                  disabled={setPriority.isPending || credential.priority === 0}
                >
                  <ChevronUp className="h-4 w-4" />
                  提高优先级
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={() => handlePriorityStep(1)}
                  disabled={setPriority.isPending}
                >
                  <ChevronDown className="h-4 w-4" />
                  降低优先级
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  onClick={() => setShowDeleteDialog(true)}
                  disabled={!canDeleteCredential(credential)}
                  className="text-destructive focus:text-destructive"
                >
                  <Trash2 className="h-4 w-4" />
                  删除
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </td>
      </tr>

      <Dialog open={showDeleteDialog} onOpenChange={setShowDeleteDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>确认删除凭据</DialogTitle>
            <DialogDescription>
              确定要删除凭据 #{credential.id} 吗？此操作无法撤销。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setShowDeleteDialog(false)}
              disabled={deleteCredential.isPending}
            >
              取消
            </Button>
            <Button
              variant="destructive"
              onClick={handleDelete}
              disabled={deleteCredential.isPending || !credential.disabled}
            >
              确认删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}

export function CredentialPoolTable({
  credentials,
  selectedIds,
  balanceMap,
  loadingBalanceIds,
  onToggleSelect,
  onTogglePageSelection,
  onOpenDetails,
  onViewBalance,
}: CredentialPoolTableProps) {
  const allSelected =
    credentials.length > 0 &&
    credentials.every(credential => selectedIds.has(credential.id))

  return (
    <div className="h-full min-h-0 min-w-0 max-w-full overflow-hidden rounded-lg border bg-card">
      <div className="h-full max-w-full overflow-auto pb-2">
        <table className="w-full min-w-[1200px] table-fixed border-collapse text-left">
          <colgroup>
            <col className="w-11" />
            <col className="w-72" />
            <col className="w-24" />
            <col className="w-32" />
            <col className="w-20" />
            <col className="w-32" />
            <col className="w-32" />
            <col className="w-28" />
            <col className="w-20" />
            <col className="w-28" />
          </colgroup>
          <thead className="sticky top-0 z-10 bg-muted text-xs uppercase text-muted-foreground shadow-sm">
            <tr className="border-b">
              <th className="w-10 px-4 py-3">
                <Checkbox
                  checked={allSelected}
                  onCheckedChange={(checked) => onTogglePageSelection(Boolean(checked))}
                  disabled={credentials.length === 0}
                />
              </th>
              <th className="px-3 py-3 font-medium">账号</th>
              <th className="px-3 py-3 font-medium">状态</th>
              <th className="px-3 py-3 font-medium">认证/端点</th>
              <th className="px-3 py-3 font-medium">优先级</th>
              <th className="px-3 py-3 font-medium">调用</th>
              <th className="px-3 py-3 font-medium">余额</th>
              <th className="px-3 py-3 font-medium">最近调用</th>
              <th className="px-3 py-3 font-medium">启用</th>
              <th className="px-4 py-3 text-right font-medium">操作</th>
            </tr>
          </thead>
          <tbody>
            {credentials.map((credential) => (
              <CredentialRow
                key={credential.id}
                credential={credential}
                selected={selectedIds.has(credential.id)}
                balance={balanceMap.get(credential.id) || null}
                loadingBalance={loadingBalanceIds.has(credential.id)}
                onToggleSelect={() => onToggleSelect(credential.id)}
                onOpenDetails={onOpenDetails}
                onViewBalance={onViewBalance}
              />
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}
