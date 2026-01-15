import { useState, useMemo } from 'react'
import { toast } from 'sonner'
import { ShieldCheck, Clock, Loader2 } from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
  DialogDescription,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { useCredentials, useValidateCredentials } from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'
import type {
  ValidateCredentialsResponse,
  CredentialValidationResult,
  ValidationStatus,
} from '@/types/api'

interface ValidateCredentialsDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

type ModelType = 'sonnet' | 'opus' | 'haiku'

export function ValidateCredentialsDialog({
  open,
  onOpenChange,
}: ValidateCredentialsDialogProps) {
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set())
  const [model, setModel] = useState<ModelType>('sonnet')
  const [result, setResult] = useState<ValidateCredentialsResponse | null>(null)
  const [isValidating, setIsValidating] = useState(false)

  const { data: credentialsData } = useCredentials()
  const validateMutation = useValidateCredentials()

  const credentials = useMemo(
    () => credentialsData?.credentials ?? [],
    [credentialsData]
  )

  const resetState = () => {
    setSelectedIds(new Set())
    setModel('sonnet')
    setResult(null)
    setIsValidating(false)
  }

  const handleClose = () => {
    onOpenChange(false)
    setTimeout(resetState, 200)
  }

  const handleToggleCredential = (id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev)
      if (next.has(id)) {
        next.delete(id)
      } else {
        next.add(id)
      }
      return next
    })
  }

  const handleSelectAll = () => {
    if (selectedIds.size === credentials.length) {
      setSelectedIds(new Set())
    } else {
      setSelectedIds(new Set(credentials.map((c) => c.id)))
    }
  }

  const handleValidate = async () => {
    if (selectedIds.size === 0) {
      toast.error('请至少选择一个凭据')
      return
    }

    setIsValidating(true)
    try {
      const response = await validateMutation.mutateAsync({
        credentialIds: Array.from(selectedIds),
        model,
      })
      setResult(response)

      const { summary } = response
      if (summary.ok === summary.total) {
        toast.success(`全部 ${summary.total} 个凭据验证通过`)
      } else if (summary.ok > 0) {
        toast.warning(`${summary.ok}/${summary.total} 个凭据验证通过`)
      } else {
        toast.error(`全部 ${summary.total} 个凭据验证失败`)
      }
    } catch (error) {
      toast.error(`验证失败: ${extractErrorMessage(error)}`)
    } finally {
      setIsValidating(false)
    }
  }

  const handleRetry = () => {
    setResult(null)
  }

  const getStatusBadge = (status: ValidationStatus) => {
    switch (status) {
      case 'ok':
        return (
          <Badge className="bg-green-500 hover:bg-green-500/80 text-white border-transparent">
            正常
          </Badge>
        )
      case 'denied':
        return (
          <Badge className="bg-red-500 hover:bg-red-500/80 text-white border-transparent">
            拒绝
          </Badge>
        )
      case 'invalid':
        return (
          <Badge className="bg-orange-500 hover:bg-orange-500/80 text-white border-transparent">
            无效
          </Badge>
        )
      case 'transient':
        return (
          <Badge className="bg-yellow-500 hover:bg-yellow-500/80 text-white border-transparent">
            暂时错误
          </Badge>
        )
      case 'not_found':
        return (
          <Badge className="bg-gray-500 hover:bg-gray-500/80 text-white border-transparent">
            未找到
          </Badge>
        )
      default:
        return <Badge variant="outline">未知</Badge>
    }
  }

  const renderSummary = (data: ValidateCredentialsResponse) => {
    const { summary } = data
    return (
      <div className="flex items-center justify-center gap-4 py-4 px-4 bg-muted/50 rounded-lg">
        <div className="text-center">
          <div className="text-xl font-semibold tabular-nums">{summary.total}</div>
          <div className="text-xs text-muted-foreground">总计</div>
        </div>
        <div className="h-8 w-px bg-border" />
        <div className="text-center">
          <div className="text-xl font-semibold tabular-nums text-green-600">
            {summary.ok}
          </div>
          <div className="text-xs text-muted-foreground">正常</div>
        </div>
        <div className="h-8 w-px bg-border" />
        <div className="text-center">
          <div className="text-xl font-semibold tabular-nums text-red-600">
            {summary.denied}
          </div>
          <div className="text-xs text-muted-foreground">拒绝</div>
        </div>
        <div className="h-8 w-px bg-border" />
        <div className="text-center">
          <div className="text-xl font-semibold tabular-nums text-orange-600">
            {summary.invalid}
          </div>
          <div className="text-xs text-muted-foreground">无效</div>
        </div>
        <div className="h-8 w-px bg-border" />
        <div className="text-center">
          <div className="text-xl font-semibold tabular-nums text-yellow-600">
            {summary.transient}
          </div>
          <div className="text-xs text-muted-foreground">暂时</div>
        </div>
      </div>
    )
  }

  const renderResultList = (results: CredentialValidationResult[]) => {
    if (results.length === 0) {
      return (
        <div className="py-8 text-center text-muted-foreground">没有验证结果</div>
      )
    }

    return (
      <div className="border rounded-lg overflow-hidden">
        <div className="max-h-56 overflow-auto">
          <table className="w-full text-sm">
            <thead className="bg-muted/50 sticky top-0">
              <tr>
                <th className="text-left font-medium px-3 py-2 text-muted-foreground">
                  凭据
                </th>
                <th className="text-left font-medium px-3 py-2 text-muted-foreground">
                  状态
                </th>
                <th className="text-right font-medium px-3 py-2 text-muted-foreground">
                  延迟
                </th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {results.map((item) => (
                <tr key={item.id} className="hover:bg-muted/30 transition-colors">
                  <td className="px-3 py-2">
                    <span className="font-medium">#{item.id}</span>
                    {item.message && (
                      <div className="text-xs text-muted-foreground mt-0.5 truncate max-w-[200px]" title={item.message}>
                        {item.message}
                      </div>
                    )}
                  </td>
                  <td className="px-3 py-2">{getStatusBadge(item.status)}</td>
                  <td className="px-3 py-2 text-right tabular-nums">
                    {item.latencyMs !== undefined ? (
                      <span className="flex items-center justify-end gap-1 text-muted-foreground">
                        <Clock className="h-3 w-3" />
                        {item.latencyMs}ms
                      </span>
                    ) : (
                      <span className="text-muted-foreground">-</span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    )
  }

  const renderCredentialSelection = () => {
    if (credentials.length === 0) {
      return (
        <div className="py-8 text-center text-muted-foreground">暂无凭据</div>
      )
    }

    const allSelected = selectedIds.size === credentials.length

    return (
      <div className="space-y-4">
        {/* 模型选择 */}
        <div className="flex items-center gap-3">
          <label className="text-sm font-medium text-muted-foreground shrink-0">
            模型
          </label>
          <select
            value={model}
            onChange={(e) => setModel(e.target.value as ModelType)}
            className="flex h-9 w-full rounded-md border border-input bg-background px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          >
            <option value="sonnet">Sonnet</option>
            <option value="opus">Opus</option>
            <option value="haiku">Haiku</option>
          </select>
        </div>

        {/* 全选 */}
        <div className="flex items-center justify-between pb-2 border-b">
          <span className="text-sm font-medium">选择凭据</span>
          <button
            type="button"
            onClick={handleSelectAll}
            className="text-sm text-primary hover:underline"
          >
            {allSelected ? '取消全选' : '全选'}
          </button>
        </div>

        {/* 凭据列表 */}
        <div className="border rounded-lg overflow-hidden">
          <div className="max-h-48 overflow-auto">
            {credentials.map((credential) => {
              const isSelected = selectedIds.has(credential.id)
              return (
                <label
                  key={credential.id}
                  className={`flex items-center gap-3 px-3 py-2.5 cursor-pointer transition-colors hover:bg-muted/50 ${
                    isSelected ? 'bg-muted/30' : ''
                  } ${credential.disabled ? 'opacity-50' : ''}`}
                >
                  <input
                    type="checkbox"
                    checked={isSelected}
                    onChange={() => handleToggleCredential(credential.id)}
                    className="h-4 w-4 rounded border-gray-300 text-primary focus:ring-primary"
                  />
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="font-medium">#{credential.id}</span>
                      {credential.isCurrent && (
                        <Badge variant="success" className="text-[10px] px-1.5 py-0">
                          当前
                        </Badge>
                      )}
                      {credential.disabled && (
                        <Badge variant="destructive" className="text-[10px] px-1.5 py-0">
                          禁用
                        </Badge>
                      )}
                    </div>
                    <div className="text-xs text-muted-foreground">
                      {credential.authMethod || '未知'} · 优先级 {credential.priority}
                    </div>
                  </div>
                </label>
              )
            })}
          </div>
        </div>

        <div className="text-xs text-muted-foreground text-center">
          已选择 {selectedIds.size} / {credentials.length} 个凭据
        </div>
      </div>
    )
  }

  return (
    <Dialog open={open} onOpenChange={handleClose}>
      <DialogContent className="sm:max-w-lg max-h-[80vh] overflow-hidden flex flex-col">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <ShieldCheck className="h-5 w-5" />
            {result ? '验证结果' : '验证凭据'}
          </DialogTitle>
          {!result && (
            <DialogDescription>
              选择要验证的凭据，测试其是否可用
            </DialogDescription>
          )}
        </DialogHeader>

        <div className="flex-1 overflow-auto py-4">
          {result ? (
            <div className="space-y-4">
              {renderSummary(result)}
              {renderResultList(result.results)}
            </div>
          ) : (
            renderCredentialSelection()
          )}
        </div>

        <DialogFooter>
          {result ? (
            <>
              <Button variant="outline" onClick={handleRetry}>
                重新验证
              </Button>
              <Button onClick={handleClose}>完成</Button>
            </>
          ) : (
            <>
              <Button variant="outline" onClick={handleClose}>
                取消
              </Button>
              <Button
                onClick={handleValidate}
                disabled={selectedIds.size === 0 || isValidating}
              >
                {isValidating ? (
                  <>
                    <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                    验证中...
                  </>
                ) : (
                  `验证 (${selectedIds.size})`
                )}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
