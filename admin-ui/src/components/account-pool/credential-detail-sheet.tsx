import { useEffect, useState, type ReactNode } from 'react'
import {
  Loader2,
  RefreshCw,
  RotateCcw,
  Save,
  Trash2,
  Wallet,
} from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Progress } from '@/components/ui/progress'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
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
  canQueryCredentialBalance,
  canRefreshCredentialToken,
  canResetCredentialFailure,
  canToggleCredentialDisabled,
  getAuthMethodLabel,
  getCredentialHealth,
} from '@/lib/credential-status'
import {
  formatBalanceSummary,
  formatCurrencyLikeNumber,
  formatDateTime,
  formatRelativeTime,
  formatSubscriptionTitle,
  formatUsageRatio,
  getCredentialDisplayName,
} from '@/lib/credential-format'
import type { BalanceResponse, CredentialStatusItem } from '@/types/api'

interface CredentialDetailSheetProps {
  credential: CredentialStatusItem | null
  open: boolean
  balance: BalanceResponse | null
  loadingBalance: boolean
  balanceError: string | null
  onOpenChange: (open: boolean) => void
  onQueryBalance: (id: number) => void
}

function DetailItem({
  label,
  value,
  mono = false,
}: {
  label: string
  value: string | number | null | undefined
  mono?: boolean
}) {
  return (
    <div className="min-w-0 space-y-1">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className={mono ? 'break-words font-mono text-sm' : 'break-words text-sm font-medium'}>
        {value === null || value === undefined || value === '' ? '未知' : value}
      </div>
    </div>
  )
}

function DetailSection({
  title,
  children,
}: {
  title: string
  children: ReactNode
}) {
  return (
    <section className="space-y-3">
      <h3 className="text-sm font-semibold">{title}</h3>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">{children}</div>
    </section>
  )
}

export function CredentialDetailSheet({
  credential,
  open,
  balance,
  loadingBalance,
  balanceError,
  onOpenChange,
  onQueryBalance,
}: CredentialDetailSheetProps) {
  const [priorityValue, setPriorityValue] = useState('')
  const [showDeleteDialog, setShowDeleteDialog] = useState(false)
  const setDisabled = useSetDisabled()
  const setPriority = useSetPriority()
  const resetFailure = useResetFailure()
  const deleteCredential = useDeleteCredential()
  const forceRefresh = useForceRefreshToken()

  useEffect(() => {
    setPriorityValue(credential ? String(credential.priority) : '')
  }, [credential])

  if (!credential) {
    return (
      <Sheet open={open} onOpenChange={onOpenChange}>
        <SheetContent className="w-full overflow-y-auto sm:max-w-md">
          <SheetHeader>
            <SheetTitle>凭据详情</SheetTitle>
            <SheetDescription>未选择凭据</SheetDescription>
          </SheetHeader>
        </SheetContent>
      </Sheet>
    )
  }

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

  const handleSavePriority = () => {
    const priority = Number.parseInt(priorityValue, 10)
    if (Number.isNaN(priority) || priority < 0) {
      toast.error('优先级必须是非负整数')
      return
    }

    setPriority.mutate(
      { id: credential.id, priority },
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
        onOpenChange(false)
      },
      onError: (err) => toast.error('删除失败: ' + (err as Error).message),
    })
  }

  return (
    <>
      <Sheet open={open} onOpenChange={onOpenChange}>
        <SheetContent className="flex w-full flex-col overflow-y-auto p-0 sm:max-w-xl">
          <div className="border-b p-5 pr-12">
            <SheetHeader>
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <SheetTitle className="truncate" title={getCredentialDisplayName(credential)}>
                    {getCredentialDisplayName(credential)}
                  </SheetTitle>
                  <SheetDescription>凭据 #{credential.id}</SheetDescription>
                </div>
                <StatusBadge health={health} />
              </div>
            </SheetHeader>
          </div>

          <div className="flex-1 space-y-6 overflow-y-auto p-5">
            <DetailSection title="身份信息">
              <DetailItem label="邮箱" value={credential.email} />
              <DetailItem label="认证方式" value={getAuthMethodLabel(credential.authMethod)} />
              <DetailItem label="端点" value={credential.endpoint} />
              <DetailItem
                label="API Key"
                value={credential.maskedApiKey}
                mono
              />
              <DetailItem
                label="Profile ARN"
                value={credential.hasProfileArn ? '存在' : '无'}
              />
              <DetailItem
                label="代理"
                value={credential.hasProxy ? credential.proxyUrl || '已配置' : '未配置'}
              />
            </DetailSection>

            <DetailSection title="调度信息">
              <DetailItem label="状态" value={health.label} />
              <div className="space-y-1">
                <div className="text-xs text-muted-foreground">启用状态</div>
                <div className="flex items-center gap-2">
                  <Switch
                    checked={!credential.disabled}
                    onCheckedChange={handleToggleDisabled}
                    disabled={setDisabled.isPending || !canToggleDisabled}
                    title={disabledToggleTitle}
                  />
                  <span className="text-sm font-medium">
                    {credential.disabled ? '已禁用' : '已启用'}
                  </span>
                </div>
              </div>
              <div className="col-span-2 space-y-2">
                <div className="text-xs text-muted-foreground">优先级</div>
                <div className="flex gap-2">
                  <Input
                    type="number"
                    min="0"
                    value={priorityValue}
                    onChange={(event) => setPriorityValue(event.target.value)}
                    className="h-9"
                  />
                  <Button
                    size="sm"
                    onClick={handleSavePriority}
                    disabled={setPriority.isPending}
                  >
                    <Save className="h-4 w-4" />
                    保存
                  </Button>
                </div>
              </div>
            </DetailSection>

            <DetailSection title="健康统计">
              <DetailItem label="成功次数" value={credential.successCount} />
              <DetailItem label="连续失败" value={credential.failureCount} />
              <DetailItem label="刷新失败" value={credential.refreshFailureCount} />
              <DetailItem label="最后调用" value={formatRelativeTime(credential.lastUsedAt)} />
              <DetailItem label="Token 过期" value={formatDateTime(credential.expiresAt)} />
              <DetailItem label="禁用原因" value={credential.disabledReason} />
            </DetailSection>

            <section className="space-y-3">
              <div className="flex items-center justify-between gap-3">
                <h3 className="text-sm font-semibold">余额/订阅</h3>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => onQueryBalance(credential.id)}
                  disabled={loadingBalance || !canQueryCredentialBalance(credential)}
                >
                  {loadingBalance ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : (
                    <Wallet className="h-4 w-4" />
                  )}
                  查询余额
                </Button>
              </div>

              {loadingBalance ? (
                <div className="rounded-md border p-4 text-sm text-muted-foreground">
                  正在查询余额...
                </div>
              ) : balanceError ? (
                <div className="rounded-md border border-red-200 bg-red-50 p-4 text-sm text-red-700 dark:border-red-950 dark:bg-red-950/30 dark:text-red-300">
                  {balanceError}
                </div>
              ) : balance ? (
                <div className="space-y-3 rounded-md border p-4">
                  <div className="flex items-center justify-between gap-3 text-sm">
                    <span className="font-medium">{formatSubscriptionTitle(balance)}</span>
                    <span className="text-muted-foreground">{formatUsageRatio(balance)}</span>
                  </div>
                  <Progress value={balance.usagePercentage} />
                  <div className="grid grid-cols-2 gap-3 text-sm">
                    <DetailItem label="剩余额度" value={formatBalanceSummary(balance)} />
                    <DetailItem
                      label="已使用"
                      value={formatCurrencyLikeNumber(balance.currentUsage)}
                    />
                    <DetailItem
                      label="限额"
                      value={formatCurrencyLikeNumber(balance.usageLimit)}
                    />
                    <DetailItem
                      label="下次重置"
                      value={formatDateTime(balance.nextResetAt)}
                    />
                  </div>
                </div>
              ) : (
                <div className="rounded-md border p-4 text-sm text-muted-foreground">
                  未查询
                </div>
              )}
            </section>
          </div>

          <div className="border-t p-5">
            <div className="flex flex-wrap justify-end gap-2">
              <Button
                size="sm"
                variant="outline"
                onClick={handleReset}
                disabled={!canResetCredentialFailure(credential) || resetFailure.isPending}
              >
                <RotateCcw className="h-4 w-4" />
                重置失败
              </Button>
              <Button
                size="sm"
                variant="outline"
                onClick={handleForceRefresh}
                disabled={!canRefreshCredentialToken(credential) || forceRefresh.isPending}
              >
                <RefreshCw className={forceRefresh.isPending ? 'h-4 w-4 animate-spin' : 'h-4 w-4'} />
                刷新 Token
              </Button>
              <Button
                size="sm"
                variant="destructive"
                onClick={() => setShowDeleteDialog(true)}
                disabled={!canDeleteCredential(credential)}
              >
                <Trash2 className="h-4 w-4" />
                删除
              </Button>
            </div>
          </div>
        </SheetContent>
      </Sheet>

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
