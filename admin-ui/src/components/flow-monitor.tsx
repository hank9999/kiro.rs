import { useState } from 'react'
import { Activity, Zap, Clock, AlertTriangle, Trash2, RefreshCw } from 'lucide-react'
import { toast } from 'sonner'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import { useFlows, useFlowStats, useClearFlows } from '@/hooks/use-flows'
import type { FlowQuery } from '@/types/api'

function formatTokens(n: number | null | undefined) {
  if (n == null) return '-'
  if (n >= 1000000) return `${(n / 1000000).toFixed(1)}M`
  if (n >= 1000) return `${(n / 1000).toFixed(1)}K`
  return n.toString()
}

function formatDuration(ms: number) {
  if (ms >= 1000) return `${(ms / 1000).toFixed(1)}s`
  return `${ms}ms`
}

function formatTime(ts: string) {
  try {
    const d = new Date(ts)
    return d.toLocaleString('zh-CN', { hour12: false })
  } catch {
    return ts
  }
}

export function FlowMonitor() {
  const [query, setQuery] = useState<FlowQuery>({ page: 1, pageSize: 20 })
  const [modelFilter, setModelFilter] = useState<string>('')
  const [statusFilter, setStatusFilter] = useState<'' | 'success' | 'error'>('')
  const [clearDialogOpen, setClearDialogOpen] = useState(false)

  const { data: flowsData, isLoading: flowsLoading, error: flowsError, refetch: refetchFlows } = useFlows({
    ...query,
    model: modelFilter || undefined,
    status: statusFilter || undefined,
  })
  const { data: stats, isLoading: statsLoading, error: statsError } = useFlowStats()
  const { mutate: clearFlowsMutation, isPending: clearing } = useClearFlows()

  const handleClear = () => {
    setClearDialogOpen(true)
  }

  const handleConfirmClear = () => {
    setClearDialogOpen(false)
    clearFlowsMutation(undefined, {
      onSuccess: () => toast.success('已清空流量记录'),
      onError: (e) => toast.error(`清空失败: ${(e as Error).message}`),
    })
  }

  const handlePageChange = (newPage: number) => {
    setQuery(prev => ({ ...prev, page: newPage }))
  }

  const totalPages = flowsData ? Math.ceil(flowsData.total / (query.pageSize || 20)) : 0

  return (
    <div className="space-y-6">
      {/* 统计卡片 */}
      <div className="grid gap-4 md:grid-cols-4">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground flex items-center gap-2">
              <Activity className="h-4 w-4" />
              总请求数
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {statsLoading ? '...' : formatTokens(stats?.totalRequests)}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground flex items-center gap-2">
              <Zap className="h-4 w-4" />
              总 Token 用量
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {statsLoading ? '...' : formatTokens(stats?.totalTokens)}
            </div>
            {stats && (
              <p className="text-xs text-muted-foreground mt-1">
                入 {formatTokens(stats.totalInputTokens)} / 出 {formatTokens(stats.totalOutputTokens)}
              </p>
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground flex items-center gap-2">
              <Clock className="h-4 w-4" />
              平均延迟
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {statsLoading ? '...' : stats ? formatDuration(stats.avgDurationMs) : '-'}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground flex items-center gap-2">
              <AlertTriangle className="h-4 w-4" />
              错误率
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {statsLoading ? '...' : stats ? `${(stats.errorRate * 100).toFixed(1)}%` : '-'}
            </div>
            {stats && stats.errorCount > 0 && (
              <p className="text-xs text-red-500 mt-1">{stats.errorCount} 个错误</p>
            )}
          </CardContent>
        </Card>
      </div>

      {/* 模型用量分布 */}
      {stats && stats.models.length > 0 && (
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium">模型用量分布</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex flex-wrap gap-2">
              {stats.models.map((m) => (
                <Badge
                  key={m.model}
                  variant={modelFilter === m.model ? 'default' : 'secondary'}
                  className="cursor-pointer"
                  onClick={() => {
                    setModelFilter(modelFilter === m.model ? '' : m.model)
                    setQuery(prev => ({ ...prev, page: 1 }))
                  }}
                >
                  {m.model}: {m.count} 次 · {formatTokens(m.totalInputTokens + m.totalOutputTokens)} tokens
                </Badge>
              ))}
            </div>
          </CardContent>
        </Card>
      )}

      {/* 过滤器和操作栏 */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <h2 className="text-xl font-semibold">流量记录</h2>
          <div className="flex gap-1">
            {(['', 'success', 'error'] as const).map((s) => (
              <Button
                key={s}
                size="sm"
                variant={statusFilter === s ? 'default' : 'ghost'}
                onClick={() => {
                  setStatusFilter(s)
                  setQuery(prev => ({ ...prev, page: 1 }))
                }}
              >
                {s === '' ? '全部' : s === 'success' ? '成功' : '错误'}
              </Button>
            ))}
          </div>
          {modelFilter && (
            <Badge variant="outline" className="cursor-pointer" onClick={() => { setModelFilter(''); setQuery(prev => ({ ...prev, page: 1 })) }}>
              模型: {modelFilter} ✕
            </Badge>
          )}
        </div>
        <div className="flex gap-2">
          <Button size="sm" variant="outline" onClick={() => refetchFlows()}>
            <RefreshCw className="h-4 w-4 mr-2" />
            刷新
          </Button>
          <Button
            size="sm"
            variant="outline"
            className="text-destructive hover:text-destructive"
            onClick={handleClear}
            disabled={clearing}
          >
            <Trash2 className="h-4 w-4 mr-2" />
            清空日志
          </Button>
        </div>
      </div>

      {/* 流量记录表格 */}
      <Card>
        <CardContent className="p-0">
          {flowsLoading ? (
            <div className="p-8 text-center text-muted-foreground">加载中...</div>
          ) : flowsError || statsError ? (
            <div className="p-8 text-center text-muted-foreground">流量监控不可用</div>
          ) : !flowsData || flowsData.records.length === 0 ? (
            <div className="p-8 text-center text-muted-foreground">暂无流量记录</div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b bg-muted/50">
                    <th className="text-left p-3 font-medium">时间</th>
                    <th className="text-left p-3 font-medium">模型</th>
                    <th className="text-left p-3 font-medium">路径</th>
                    <th className="text-right p-3 font-medium">Token (入/出)</th>
                    <th className="text-right p-3 font-medium">延迟</th>
                    <th className="text-center p-3 font-medium">状态</th>
                  </tr>
                </thead>
                <tbody>
                  {flowsData.records.map((record) => (
                    <tr
                      key={record.id}
                      className={`border-b hover:bg-muted/30 ${record.statusCode >= 400 ? 'bg-red-50 dark:bg-red-950/20' : ''}`}
                    >
                      <td className="p-3 whitespace-nowrap text-muted-foreground">
                        {formatTime(record.timestamp)}
                      </td>
                      <td className="p-3">
                        <Badge variant="outline" className="font-mono text-xs">
                          {record.model}
                        </Badge>
                        {record.stream && (
                          <Badge variant="secondary" className="ml-1 text-xs">流式</Badge>
                        )}
                      </td>
                      <td className="p-3 font-mono text-xs text-muted-foreground">
                        {record.path}
                      </td>
                      <td className="p-3 text-right font-mono text-xs">
                        {formatTokens(record.inputTokens)} / {formatTokens(record.outputTokens)}
                      </td>
                      <td className="p-3 text-right font-mono text-xs">
                        {formatDuration(record.durationMs)}
                      </td>
                      <td className="p-3 text-center">
                        {record.statusCode >= 400 ? (
                          <Badge variant="destructive" className="text-xs" title={record.error || undefined}>
                            {record.statusCode}
                          </Badge>
                        ) : (
                          <Badge variant="success" className="text-xs">
                            {record.statusCode}
                          </Badge>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>

      {/* 分页 */}
      {totalPages > 1 && (
        <div className="flex justify-center items-center gap-4">
          <Button
            variant="outline"
            size="sm"
            onClick={() => handlePageChange(Math.max(1, (query.page || 1) - 1))}
            disabled={(query.page || 1) <= 1}
          >
            上一页
          </Button>
          <span className="text-sm text-muted-foreground">
            第 {query.page || 1} / {totalPages} 页（共 {flowsData?.total || 0} 条记录）
          </span>
          <Button
            variant="outline"
            size="sm"
            onClick={() => handlePageChange(Math.min(totalPages, (query.page || 1) + 1))}
            disabled={(query.page || 1) >= totalPages}
          >
            下一页
          </Button>
        </div>
      )}

      {/* 清空确认对话框 */}
      <Dialog open={clearDialogOpen} onOpenChange={setClearDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>确认清空</DialogTitle>
            <DialogDescription>
              确定要清空所有流量记录吗？此操作无法撤销。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setClearDialogOpen(false)}>
              取消
            </Button>
            <Button variant="destructive" onClick={handleConfirmClear} disabled={clearing}>
              确认清空
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
