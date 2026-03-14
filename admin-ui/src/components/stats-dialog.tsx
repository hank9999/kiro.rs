import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { useCredentialStats } from '@/hooks/use-credentials'
import { parseError } from '@/lib/utils'
import { formatCompactNumber, formatTokensPair } from '@/lib/format'

interface StatsDialogProps {
  credentialId: number
  open: boolean
  onOpenChange: (open: boolean) => void
}

function formatTime(t: string | null) {
  if (!t) return '从未'
  const d = new Date(t)
  if (isNaN(d.getTime())) return t
  return d.toLocaleString('zh-CN')
}

function truncate(s: string, max = 200) {
  if (s.length <= max) return s
  return s.slice(0, max) + '...'
}

export function StatsDialog({ credentialId, open, onOpenChange }: StatsDialogProps) {
  const { data, isLoading, error } = useCredentialStats(credentialId, open)

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>凭据 #{credentialId} 统计详情</DialogTitle>
          <DialogDescription className="sr-only">
            查看凭据的调用统计和使用情况
          </DialogDescription>
        </DialogHeader>

        {isLoading && (
          <div className="flex items-center justify-center py-8">
            <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
          </div>
        )}

        {error && (() => {
          const parsed = parseError(error)
          return (
            <div className="py-6 space-y-3">
              <div className="flex items-center justify-center gap-2 text-red-500">
                <svg className="h-5 w-5" viewBox="0 0 20 20" fill="currentColor">
                  <path
                    fillRule="evenodd"
                    d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.707 7.293a1 1 0 00-1.414 1.414L8.586 10l-1.293 1.293a1 1 0 101.414 1.414L10 11.414l1.293 1.293a1 1 0 001.414-1.414L11.414 10l1.293-1.293a1 1 0 00-1.414-1.414L10 8.586 8.707 7.293z"
                    clipRule="evenodd"
                  />
                </svg>
                <span className="font-medium">{parsed.title}</span>
              </div>
              {parsed.detail && (
                <div className="text-sm text-muted-foreground text-center px-4">
                  {parsed.detail}
                </div>
              )}
            </div>
          )
        })()}

        {data && (
          <div className="space-y-6">
            {/* 总览 */}
            <div className="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm">
              <div className="p-3 border rounded">
                <div className="text-muted-foreground">调用总</div>
                <div className="text-lg font-semibold">{data.callsTotal}</div>
              </div>
              <div className="p-3 border rounded">
                <div className="text-muted-foreground">成功</div>
                <div className="text-lg font-semibold text-green-600">{data.callsOk}</div>
              </div>
              <div className="p-3 border rounded">
                <div className="text-muted-foreground">失败</div>
                <div className={data.callsErr > 0 ? 'text-lg font-semibold text-red-500' : 'text-lg font-semibold'}>
                  {data.callsErr}
                </div>
              </div>
              <div className="p-3 border rounded">
                <div className="text-muted-foreground">累计 Tokens</div>
                <div className="text-base font-semibold">
                  {formatTokensPair(data.inputTokensTotal, data.outputTokensTotal)}
                </div>
              </div>
              <div className="p-3 border rounded col-span-2">
                <div className="text-muted-foreground">最后调用</div>
                <div className="font-medium">{formatTime(data.lastCallAt)}</div>
              </div>
              <div className="p-3 border rounded col-span-2">
                <div className="text-muted-foreground">最后成功</div>
                <div className="font-medium">{formatTime(data.lastSuccessAt)}</div>
              </div>
              <div className="p-3 border rounded col-span-2">
                <div className="text-muted-foreground">最后错误时间</div>
                <div className="font-medium">{formatTime(data.lastErrorAt)}</div>
              </div>
              <div className="p-3 border rounded col-span-2">
                <div className="text-muted-foreground">最后错误</div>
                <div className={data.lastError ? 'font-medium text-red-500' : 'font-medium'}>
                  {data.lastError ? truncate(data.lastError, 260) : '无'}
                </div>
              </div>
            </div>

            {/* 按模型 */}
            <div className="space-y-2">
              <div className="font-semibold">按模型</div>
              {data.byModel.length === 0 ? (
                <div className="text-sm text-muted-foreground">暂无数据</div>
              ) : (
                <div className="border rounded overflow-hidden">
                  <div className="grid grid-cols-12 gap-2 px-3 py-2 text-xs text-muted-foreground bg-muted">
                    <div className="col-span-5">模型</div>
                    <div className="col-span-3">调用（总/成/败）</div>
                    <div className="col-span-4">Tokens（in/out）</div>
                  </div>
                  <div className="max-h-56 overflow-y-auto">
                    {data.byModel.map((b) => (
                      <div key={b.key} className="grid grid-cols-12 gap-2 px-3 py-2 text-sm border-t">
                        <div className="col-span-5 font-medium">{b.key}</div>
                        <div className="col-span-3">
                          {b.callsTotal}/{b.callsOk}/
                          <span className={b.callsErr > 0 ? 'text-red-500 font-medium' : ''}>{b.callsErr}</span>
                        </div>
                        <div className="col-span-4">
                          {formatCompactNumber(b.inputTokensTotal)}/{formatCompactNumber(b.outputTokensTotal)}
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>

            {/* 按日 */}
            <div className="space-y-2">
              <div className="font-semibold">按日</div>
              {data.byDay.length === 0 ? (
                <div className="text-sm text-muted-foreground">暂无数据</div>
              ) : (
                <div className="border rounded overflow-hidden">
                  <div className="grid grid-cols-12 gap-2 px-3 py-2 text-xs text-muted-foreground bg-muted">
                    <div className="col-span-3">日期</div>
                    <div className="col-span-3">调用（总/成/败）</div>
                    <div className="col-span-4">Tokens（in/out）</div>
                    <div className="col-span-2">最后错误</div>
                  </div>
                  <div className="max-h-56 overflow-y-auto">
                    {data.byDay.map((b) => (
                      <div key={b.key} className="grid grid-cols-12 gap-2 px-3 py-2 text-sm border-t">
                        <div className="col-span-3 font-medium">{b.key}</div>
                        <div className="col-span-3">
                          {b.callsTotal}/{b.callsOk}/
                          <span className={b.callsErr > 0 ? 'text-red-500 font-medium' : ''}>{b.callsErr}</span>
                        </div>
                        <div className="col-span-4">
                          {formatCompactNumber(b.inputTokensTotal)}/{formatCompactNumber(b.outputTokensTotal)}
                        </div>
                        <div className="col-span-2 text-xs text-muted-foreground">
                          {b.lastError ? truncate(b.lastError, 60) : '-'}
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  )
}
