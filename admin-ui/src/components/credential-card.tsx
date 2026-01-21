import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { RefreshCw, ChevronUp, ChevronDown, Wallet, BarChart3, Trash2, Trash } from 'lucide-react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { Input } from '@/components/ui/input'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import type { CredentialStatusItem } from '@/types/api'
import {
  useDeleteCredential,
  useSetDisabled,
  useSetPriority,
  useSetEnabledModels,
  useResetFailure,
  useResetCredentialStats,
  useCredentialBalance,
} from '@/hooks/use-credentials'
import { StatsDialog } from '@/components/stats-dialog'
import { formatExpiry, formatTokensPair } from '@/lib/format'
import { ALL_MODEL_IDS, MODEL_OPTIONS, normalizeEnabledModels, type SupportedModelId } from '@/lib/models'

interface CredentialCardProps {
  credential: CredentialStatusItem
  onViewBalance: (id: number) => void
}

function formatAuthMethodLabel(authMethod: string | null): string {
  if (!authMethod) return '未知'
  if (authMethod.toLowerCase() === 'idc') return 'IdC/Builder-ID/IAM'
  return authMethod
}

export function CredentialCard({ credential, onViewBalance }: CredentialCardProps) {
  const [editingPriority, setEditingPriority] = useState(false)
  const [priorityValue, setPriorityValue] = useState(String(credential.priority))
  const [statsDialogOpen, setStatsDialogOpen] = useState(false)
  const [showDeleteDialog, setShowDeleteDialog] = useState(false)
  const [enabledModelsDraft, setEnabledModelsDraft] = useState<SupportedModelId[]>(
    normalizeEnabledModels(credential.enabledModels)
  )

  const deleteCredential = useDeleteCredential()
  const setDisabled = useSetDisabled()
  const setPriority = useSetPriority()
  const setEnabledModels = useSetEnabledModels()
  const resetFailure = useResetFailure()
  const resetStats = useResetCredentialStats()
  const balanceQuery = useCredentialBalance(credential.id, {
    refetchInterval: 10 * 60 * 1000, // 每 10 分钟刷新一次
  })

  useEffect(() => {
    setEnabledModelsDraft(normalizeEnabledModels(credential.enabledModels))
  }, [credential.id, credential.enabledModels])

  const saveEnabledModels = () => {
    setEnabledModels.mutate(
      { id: credential.id, enabledModels: enabledModelsDraft },
      {
        onSuccess: (res) => toast.success(res.message),
        onError: (err) => toast.error('操作失败: ' + (err as Error).message),
      }
    )
  }

  const handleToggleDisabled = () => {
    setDisabled.mutate(
      { id: credential.id, disabled: !credential.disabled },
      {
        onSuccess: (res) => {
          toast.success(res.message)
        },
        onError: (err) => {
          toast.error('操作失败: ' + (err as Error).message)
        },
      }
    )
  }

  const handlePriorityChange = () => {
    const newPriority = parseInt(priorityValue, 10)
    if (isNaN(newPriority) || newPriority < 0) {
      toast.error('优先级必须是非负整数')
      return
    }
    setPriority.mutate(
      { id: credential.id, priority: newPriority },
      {
        onSuccess: (res) => {
          toast.success(res.message)
          setEditingPriority(false)
        },
        onError: (err) => {
          toast.error('操作失败: ' + (err as Error).message)
        },
      }
    )
  }

  const handleReset = () => {
    resetFailure.mutate(credential.id, {
      onSuccess: (res) => {
        toast.success(res.message)
      },
      onError: (err) => {
        toast.error('操作失败: ' + (err as Error).message)
      },
    })
  }

  const handleDelete = () => {
    deleteCredential.mutate(credential.id, {
      onSuccess: (res) => {
        toast.success(res.message)
        setShowDeleteDialog(false)
      },
      onError: (err) => {
        toast.error('删除失败: ' + (err as Error).message)
      },
    })
  }

  const formatMoney = (num: number | null | undefined) => {
    if (num === null || num === undefined) return '-'
    if (!Number.isFinite(num)) return String(num)
    return num.toLocaleString('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
  }

  const formatTime = (t: string | null) => {
    if (!t) return '从未'
    const d = new Date(t)
    if (isNaN(d.getTime())) return t
    return d.toLocaleString()
  }

  return (
    <>
      <Card className={credential.isCurrent ? 'ring-2 ring-primary' : ''}>
        <CardHeader className="pb-2">
          <div className="flex items-center justify-between">
            <CardTitle className="text-lg flex items-center gap-2">
              凭据 #{credential.id}
              {credential.isCurrent && (
                <Badge variant="success">当前</Badge>
              )}
              {credential.disabled && (
                <Badge variant="destructive">已禁用</Badge>
              )}
            </CardTitle>
            <div className="flex items-center gap-2">
              <span className="text-sm text-muted-foreground">启用</span>
              <Switch
                checked={!credential.disabled}
                onCheckedChange={handleToggleDisabled}
                disabled={setDisabled.isPending}
              />
            </div>
          </div>
          {/* 账户信息：邮箱和用户ID */}
          {(credential.accountEmail || credential.userId) && (
            <div className="text-sm text-muted-foreground mt-1">
              {credential.accountEmail && (
                <span className="mr-3">{credential.accountEmail}</span>
              )}
              {credential.userId && (
                <span className="text-xs opacity-70">ID: {credential.userId}</span>
              )}
            </div>
          )}
        </CardHeader>
        <CardContent className="space-y-4">
          {/* 信息网格 */}
          <div className="grid grid-cols-2 gap-4 text-sm">
            <div>
              <span className="text-muted-foreground">优先级：</span>
              {editingPriority ? (
                <div className="inline-flex items-center gap-1 ml-1">
                  <Input
                    type="number"
                    value={priorityValue}
                    onChange={(e) => setPriorityValue(e.target.value)}
                    className="w-16 h-7 text-sm"
                    min="0"
                  />
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-7 w-7 p-0"
                    onClick={handlePriorityChange}
                    disabled={setPriority.isPending}
                  >
                    ✓
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-7 w-7 p-0"
                    onClick={() => {
                      setEditingPriority(false)
                      setPriorityValue(String(credential.priority))
                    }}
                  >
                    ✕
                  </Button>
                </div>
              ) : (
                <span
                  className="font-medium cursor-pointer hover:underline ml-1"
                  onClick={() => setEditingPriority(true)}
                >
                  {credential.priority}
                  <span className="text-xs text-muted-foreground ml-1">(点击编辑)</span>
                </span>
              )}
            </div>
            <div>
              <span className="text-muted-foreground">失败次数：</span>
              <span className={credential.failureCount > 0 ? 'text-red-500 font-medium' : ''}>
                {credential.failureCount}
              </span>
            </div>
            <div>
              <span className="text-muted-foreground">认证方式：</span>
              <span className="font-medium">{formatAuthMethodLabel(credential.authMethod)}</span>
            </div>
            <div>
              <span className="text-muted-foreground">Token 有效期：</span>
              <span className="font-medium">{formatExpiry(credential.expiresAt)}</span>
            </div>
            <div className="col-span-2">
              <span className="text-muted-foreground">模型开关：</span>
              <div className="mt-2 flex flex-wrap items-center gap-4">
                {MODEL_OPTIONS.map((m) => {
                  const checked = enabledModelsDraft.includes(m.id)
                  return (
                    <div key={m.id} className="flex items-center gap-2">
                      <Switch
                        checked={checked}
                        onCheckedChange={(v) => {
                          setEnabledModelsDraft((prev) => {
                            if (v) return Array.from(new Set([...prev, m.id]))
                            return prev.filter((x) => x !== m.id)
                          })
                        }}
                        disabled={setEnabledModels.isPending}
                      />
                      <span className="text-sm">{m.label}</span>
                    </div>
                  )
                })}

                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => setEnabledModelsDraft([...ALL_MODEL_IDS])}
                  disabled={setEnabledModels.isPending}
                >
                  全选
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => setEnabledModelsDraft([])}
                  disabled={setEnabledModels.isPending}
                >
                  全不选
                </Button>
                <Button
                  size="sm"
                  variant="default"
                  onClick={saveEnabledModels}
                  disabled={setEnabledModels.isPending}
                >
                  保存
                </Button>
              </div>
              <p className="text-xs text-muted-foreground mt-1">
                未配置时默认全开；保存会写回 credentials.json（多凭据数组格式才会回写）。
              </p>
            </div>
            <div className="col-span-2">
              <span className="text-muted-foreground">调用次数：</span>
              <span className="font-medium ml-1">总 {credential.callsTotal}</span>
              <span className="text-green-600 font-medium ml-3">成功 {credential.callsOk}</span>
              <span className={credential.callsErr > 0 ? 'text-red-500 font-medium ml-3' : 'font-medium ml-3'}>
                失败 {credential.callsErr}
              </span>
            </div>
            <div>
              <span className="text-muted-foreground">累计 Tokens：</span>
              <span className="font-medium">
                {formatTokensPair(credential.inputTokensTotal, credential.outputTokensTotal)}
              </span>
            </div>
            <div>
              <span className="text-muted-foreground">用量：</span>
              {balanceQuery.isLoading ? (
                <span className="text-muted-foreground">加载中...</span>
              ) : balanceQuery.error ? (
                <span className="text-red-500 font-medium">获取失败</span>
              ) : balanceQuery.data ? (
                <span className="font-medium">
                  ${formatMoney(balanceQuery.data.currentUsage)} / ${formatMoney(balanceQuery.data.usageLimit)}
                  <span className="text-xs text-muted-foreground ml-2">
                    ({balanceQuery.data.usagePercentage.toFixed(1)}%)
                  </span>
                </span>
              ) : (
                <span className="text-muted-foreground">-</span>
              )}
            </div>
            <div className="col-span-2">
              <span className="text-muted-foreground">最后调用：</span>
              <span className="font-medium">{formatTime(credential.lastCallAt)}</span>
            </div>
            {credential.lastError && (
              <div className="col-span-2">
                <span className="text-muted-foreground">最后错误：</span>
                <span className="text-red-500 font-medium">
                  {credential.lastErrorAt ? `${formatTime(credential.lastErrorAt)} - ` : ''}
                  {credential.lastError.length > 160
                    ? credential.lastError.slice(0, 160) + '...'
                    : credential.lastError}
                </span>
              </div>
            )}
            {credential.hasProfileArn && (
              <div className="col-span-2">
                <Badge variant="secondary">有 Profile ARN</Badge>
              </div>
            )}
          </div>

          {/* 操作按钮 */}
          <div className="pt-3 border-t space-y-2">
            {/* 第一行：常规操作 */}
            <div className="grid grid-cols-4 gap-2">
              <Button
                size="sm"
                variant="outline"
                className="w-full"
                onClick={handleReset}
                disabled={resetFailure.isPending || credential.failureCount === 0}
              >
                <RefreshCw className="h-4 w-4 mr-1" />
                重置失败
              </Button>
              <Button
                size="sm"
                variant="outline"
                className="w-full"
                onClick={() => {
                  const newPriority = Math.max(0, credential.priority - 1)
                  setPriority.mutate(
                    { id: credential.id, priority: newPriority },
                    {
                      onSuccess: (res) => toast.success(res.message),
                      onError: (err) => toast.error('操作失败: ' + (err as Error).message),
                    }
                  )
                }}
                disabled={setPriority.isPending || credential.priority === 0}
              >
                <ChevronUp className="h-4 w-4 mr-1" />
                提高优先级
              </Button>
              <Button
                size="sm"
                variant="outline"
                className="w-full"
                onClick={() => {
                  const newPriority = credential.priority + 1
                  setPriority.mutate(
                    { id: credential.id, priority: newPriority },
                    {
                      onSuccess: (res) => toast.success(res.message),
                      onError: (err) => toast.error('操作失败: ' + (err as Error).message),
                    }
                  )
                }}
                disabled={setPriority.isPending}
              >
                <ChevronDown className="h-4 w-4 mr-1" />
                降低优先级
              </Button>
              <Button
                size="sm"
                variant="outline"
                className="w-full"
                onClick={() => setStatsDialogOpen(true)}
              >
                <BarChart3 className="h-4 w-4 mr-1" />
                统计详情
              </Button>
            </div>
            {/* 第二行：危险操作 + 查看余额 */}
            <div className="grid grid-cols-3 gap-2">
              <Button
                size="sm"
                variant="destructive"
                className="w-full"
                onClick={() => setShowDeleteDialog(true)}
                disabled={deleteCredential.isPending}
              >
                <Trash className="h-4 w-4 mr-1" />
                删除凭据
              </Button>
              <Button
                size="sm"
                variant="destructive"
                className="w-full"
                onClick={() => {
                  const ok = window.confirm(`确定清空凭据 #${credential.id} 的统计吗？此操作不可恢复。`)
                  if (!ok) return
                  resetStats.mutate(credential.id, {
                    onSuccess: (res) => toast.success(res.message),
                    onError: (err) => toast.error('操作失败: ' + (err as Error).message),
                  })
                }}
                disabled={resetStats.isPending}
              >
                <Trash2 className="h-4 w-4 mr-1" />
                清空统计
              </Button>
              <Button
                size="sm"
                variant="default"
                className="w-full"
                onClick={() => {
                  void balanceQuery.refetch()
                  onViewBalance(credential.id)
                }}
              >
                <Wallet className="h-4 w-4 mr-1" />
                查看余额
              </Button>
            </div>
          </div>

          <StatsDialog
            credentialId={credential.id}
            open={statsDialogOpen}
            onOpenChange={setStatsDialogOpen}
          />
        </CardContent>
      </Card>

      {/* 删除确认对话框 */}
      <Dialog open={showDeleteDialog} onOpenChange={setShowDeleteDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>确认删除凭据</DialogTitle>
            <DialogDescription>
              您确定要删除凭据 #{credential.id} 吗？此操作无法撤销，且会从凭据文件中移除。
              {credential.isCurrent && (
                <span className="block mt-2 text-yellow-600">
                  注意：这是当前凭据，删除后会自动切换到其他可用凭据；如果没有可用凭据，将变为无凭据状态。
                </span>
              )}
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
              disabled={deleteCredential.isPending}
            >
              确认删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
