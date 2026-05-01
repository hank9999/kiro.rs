import { useState } from 'react'
import { toast } from 'sonner'
import { Copy, Loader2, RefreshCw, RotateCcw, Trash2 } from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import {
  useDeletePremiumCredential,
  useExportPremiumCredentials,
  usePremiumCredentials,
  useRestorePremiumCredential,
} from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'
import type { PremiumCredentialItem } from '@/types/api'

interface PremiumCredentialsDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

function formatDate(value?: string | null): string {
  if (!value) return '未知'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleString('zh-CN')
}

function credentialTitle(credential: PremiumCredentialItem): string {
  return credential.email || `高级凭证 #${credential.id}`
}

function credentialHash(credential: PremiumCredentialItem): string {
  return credential.refreshTokenHash || credential.apiKeyHash || '无 hash'
}

export function PremiumCredentialsDialog({
  open,
  onOpenChange,
}: PremiumCredentialsDialogProps) {
  const [exportText, setExportText] = useState('')
  const { data, isLoading, error, refetch, isFetching } = usePremiumCredentials(open)
  const exportPremiumCredentials = useExportPremiumCredentials()
  const restorePremiumCredential = useRestorePremiumCredential()
  const deletePremiumCredential = useDeletePremiumCredential()

  const handleExport = async () => {
    try {
      const response = await exportPremiumCredentials.mutateAsync()
      const text = JSON.stringify(response.credentials, null, 2)
      setExportText(text)

      if (navigator.clipboard) {
        await navigator.clipboard.writeText(text)
        toast.success(`已复制 ${response.total} 个高级凭证`)
      } else {
        toast.success(`已生成 ${response.total} 个高级凭证导出内容`)
      }
    } catch (err) {
      toast.error(`导出失败: ${extractErrorMessage(err)}`)
    }
  }

  const handleRestore = (credential: PremiumCredentialItem) => {
    if (!confirm(`确定要把 ${credentialTitle(credential)} 恢复到普通凭证池吗？`)) {
      return
    }

    restorePremiumCredential.mutate(credential.id, {
      onSuccess: (response) => {
        toast.success(response.message)
      },
      onError: (err) => {
        toast.error(`恢复失败: ${extractErrorMessage(err)}`)
      },
    })
  }

  const handleDelete = (credential: PremiumCredentialItem) => {
    if (!confirm(`确定要从高级凭证库删除 ${credentialTitle(credential)} 吗？此操作无法撤销。`)) {
      return
    }

    deletePremiumCredential.mutate(credential.id, {
      onSuccess: () => {
        toast.success(`已删除 ${credentialTitle(credential)}`)
      },
      onError: (err) => {
        toast.error(`删除失败: ${extractErrorMessage(err)}`)
      },
    })
  }

  const credentials = data?.credentials || []

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-4xl max-h-[85vh] flex flex-col">
        <DialogHeader>
          <DialogTitle>高级凭证库</DialogTitle>
          <DialogDescription>
            这里保存已验证可调用高级模型、并已从普通池移出的凭证。列表不显示明文 Token，导出会返回完整凭证。
          </DialogDescription>
        </DialogHeader>

        <div className="flex items-center justify-between gap-3">
          <div className="text-sm text-muted-foreground">
            共 {data?.total ?? 0} 个高级凭证
          </div>
          <div className="flex gap-2">
            <Button
              size="sm"
              variant="outline"
              onClick={() => refetch()}
              disabled={isFetching}
            >
              <RefreshCw className={`h-4 w-4 mr-2 ${isFetching ? 'animate-spin' : ''}`} />
              刷新
            </Button>
            <Button
              size="sm"
              onClick={handleExport}
              disabled={exportPremiumCredentials.isPending || credentials.length === 0}
            >
              <Copy className="h-4 w-4 mr-2" />
              {exportPremiumCredentials.isPending ? '导出中...' : '导出并复制'}
            </Button>
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto space-y-3 pr-1">
          {isLoading ? (
            <div className="py-10 text-center text-muted-foreground">
              <Loader2 className="h-5 w-5 animate-spin mx-auto mb-2" />
              加载高级凭证库...
            </div>
          ) : error ? (
            <div className="rounded-md border border-destructive/40 p-4 text-sm text-destructive">
              加载失败：{extractErrorMessage(error)}
            </div>
          ) : credentials.length === 0 ? (
            <div className="rounded-md border p-8 text-center text-muted-foreground">
              暂无高级凭证。启用探针后，成功调用高级模型的凭证会自动移入这里。
            </div>
          ) : (
            credentials.map((credential) => (
              <div key={credential.id} className="rounded-lg border p-4 space-y-3">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-medium truncate">{credentialTitle(credential)}</span>
                      <Badge variant="success">高级</Badge>
                      {credential.authMethod && (
                        <Badge variant="secondary">
                          {credential.authMethod === 'api_key' ? 'API Key' : credential.authMethod}
                        </Badge>
                      )}
                      {credential.premiumVaultStatus && (
                        <Badge variant="outline">{credential.premiumVaultStatus}</Badge>
                      )}
                    </div>
                    <div className="mt-1 text-xs text-muted-foreground font-mono break-all">
                      {credentialHash(credential)}
                    </div>
                  </div>
                  <div className="flex shrink-0 gap-2">
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => handleRestore(credential)}
                      disabled={restorePremiumCredential.isPending}
                    >
                      <RotateCcw className="h-4 w-4 mr-1" />
                      恢复
                    </Button>
                    <Button
                      size="sm"
                      variant="destructive"
                      onClick={() => handleDelete(credential)}
                      disabled={deletePremiumCredential.isPending}
                    >
                      <Trash2 className="h-4 w-4 mr-1" />
                      删除
                    </Button>
                  </div>
                </div>

                <div className="grid gap-2 text-sm md:grid-cols-2">
                  <div>
                    <span className="text-muted-foreground">来源模型：</span>
                    <span className="font-medium">{credential.premiumModelAccessSourceModel || '未知'}</span>
                  </div>
                  <div>
                    <span className="text-muted-foreground">探针模型：</span>
                    <span className="font-medium">{credential.premiumModelAccessProbeModel || '未知'}</span>
                  </div>
                  <div>
                    <span className="text-muted-foreground">校验时间：</span>
                    <span className="font-medium">{formatDate(credential.premiumModelAccessCheckedAt)}</span>
                  </div>
                  {credential.maskedApiKey && (
                    <div>
                      <span className="text-muted-foreground">API Key：</span>
                      <span className="font-mono font-medium">{credential.maskedApiKey}</span>
                    </div>
                  )}
                </div>
              </div>
            ))
          )}
        </div>

        {exportText && (
          <div className="space-y-2">
            <div className="text-sm font-medium">导出内容</div>
            <textarea
              className="h-32 w-full rounded-md border bg-background p-3 font-mono text-xs"
              readOnly
              value={exportText}
            />
          </div>
        )}

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            关闭
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
