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
  getCredentialDisplayName,
  getCredentialSecondaryText,
} from '@/lib/credential-format'
import type { BalanceResponse, CredentialStatusItem } from '@/types/api'

interface CredentialCompactCardProps {
  credential: CredentialStatusItem
  selected: boolean
  balance: BalanceResponse | null
  loadingBalance: boolean
  onToggleSelect: () => void
  onOpenDetails: (id: number) => void
  onViewBalance: (id: number) => void
}

export function CredentialCompactCard({
  credential,
  selected,
  balance,
  loadingBalance,
  onToggleSelect,
  onOpenDetails,
  onViewBalance,
}: CredentialCompactCardProps) {
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
      <div className="rounded-lg border bg-card p-4 shadow-sm">
        <div className="flex items-start gap-3">
          <Checkbox checked={selected} onCheckedChange={onToggleSelect} />
          <div className="min-w-0 flex-1 space-y-3">
            <div className="flex min-w-0 items-start justify-between gap-3">
              <div className="min-w-0">
                <div className="truncate text-sm font-semibold" title={getCredentialDisplayName(credential)}>
                  {getCredentialDisplayName(credential)}
                </div>
                <div className="truncate text-xs text-muted-foreground" title={getCredentialSecondaryText(credential)}>
                  {getCredentialSecondaryText(credential)}
                </div>
              </div>
              <StatusBadge health={health} />
            </div>

            <div className="grid grid-cols-2 gap-3 text-sm">
              <div className="min-w-0">
                <div className="text-xs text-muted-foreground">认证</div>
                <div className="truncate font-medium">{getAuthMethodLabel(credential.authMethod)}</div>
              </div>
              <div>
                <div className="text-xs text-muted-foreground">优先级</div>
                <div className="font-medium">{credential.priority}</div>
              </div>
              <div className="min-w-0">
                <div className="text-xs text-muted-foreground">余额</div>
                <div className="truncate font-medium">
                  {loadingBalance ? (
                    <span className="inline-flex items-center gap-1">
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      查询中
                    </span>
                  ) : (
                    formatBalanceSummary(balance)
                  )}
                </div>
              </div>
              <div>
                <div className="text-xs text-muted-foreground">最近调用</div>
                <div className="font-medium">{formatRelativeTime(credential.lastUsedAt)}</div>
              </div>
            </div>

            <div className="flex items-center justify-between gap-3 border-t pt-3">
              <div className="flex items-center gap-2">
                <Switch
                  checked={!credential.disabled}
                  onCheckedChange={handleToggleDisabled}
                  disabled={setDisabled.isPending || !canToggleDisabled}
                  title={disabledToggleTitle}
                />
                <span className="text-xs text-muted-foreground">
                  {credential.disabled ? '禁用' : '启用'}
                </span>
              </div>
              <div className="flex items-center gap-1">
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-8 px-2"
                  onClick={() => onOpenDetails(credential.id)}
                >
                  <Info className="h-4 w-4" />
                  <span className="sr-only">详情</span>
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-8 px-2"
                  onClick={() => onViewBalance(credential.id)}
                >
                  <Wallet className="h-4 w-4" />
                  <span className="sr-only">余额</span>
                </Button>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button size="sm" variant="ghost" className="h-8 w-8 p-0">
                      <MoreHorizontal className="h-4 w-4" />
                      <span className="sr-only">更多操作</span>
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem
                      onClick={handleReset}
                      disabled={!canResetCredentialFailure(credential)}
                    >
                      <RotateCcw className="h-4 w-4" />
                      重置失败
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      onClick={handleForceRefresh}
                      disabled={!canRefreshCredentialToken(credential)}
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
            </div>
          </div>
        </div>
      </div>

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
