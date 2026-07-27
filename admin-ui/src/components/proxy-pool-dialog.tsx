import { useState } from 'react'
import { toast } from 'sonner'
import { Trash2, Plus, Upload, ToggleLeft, ToggleRight, Globe } from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  getProxyPool,
  addProxy,
  batchAddProxies,
  deleteProxy,
  setProxyEnabled,
  getGlobalProxy,
  setGlobalProxy,
} from '@/api/credentials'
import { extractErrorMessage, maskProxyUrl } from '@/lib/utils'
import type { ProxyPoolEntry } from '@/types/api'

interface ProxyPoolDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  /** 点击"分配"按钮时的回调（传入代理 URL，用于编辑凭据） */
  onSelectProxy?: (url: string) => void
}


export function ProxyPoolDialog({ open, onOpenChange, onSelectProxy }: ProxyPoolDialogProps) {
  const [newUrl, setNewUrl] = useState('')
  const [newLabel, setNewLabel] = useState('')
  const [batchText, setBatchText] = useState('')
  const [showBatch, setShowBatch] = useState(false)
  const [batchErrors, setBatchErrors] = useState<string[]>([])
  const queryClient = useQueryClient()

  const { data, isLoading } = useQuery({
    queryKey: ['proxy-pool'],
    queryFn: getProxyPool,
    enabled: open,
  })

  const { data: globalProxyData } = useQuery({
    queryKey: ['global-proxy'],
    queryFn: getGlobalProxy,
    enabled: open,
  })

  const setGlobalProxyMutation = useMutation({
    mutationFn: (url: string | null) => setGlobalProxy({ proxyUrl: url }),
    onSuccess: (_, url) => {
      toast.success(url ? `已设置全局代理: ${maskProxyUrl(url)}` : '已清除全局代理')
      queryClient.invalidateQueries({ queryKey: ['global-proxy'] })
    },
    onError: (err) => toast.error(`操作失败: ${extractErrorMessage(err)}`),
  })

  const currentGlobalProxy = globalProxyData?.proxyUrl ?? null

  const addMutation = useMutation({
    mutationFn: () => addProxy({ url: newUrl.trim(), label: newLabel.trim() || undefined }),
    onSuccess: (entry) => {
      toast.success(`代理已添加：${entry.url}`)
      setNewUrl('')
      setNewLabel('')
      queryClient.invalidateQueries({ queryKey: ['proxy-pool'] })
    },
    onError: (err) => toast.error(`添加失败: ${extractErrorMessage(err)}`),
  })

  const batchMutation = useMutation({
    mutationFn: () =>
      batchAddProxies({
        urls: batchText.split('\n').map((l) => l.trim()).filter(Boolean),
      }),
    onSuccess: (res) => {
      if (res.errors === 0) {
        toast.success(`批量导入完成：成功 ${res.added} 个`)
      } else {
        toast.info(`批量导入完成：成功 ${res.added} 个，跳过 ${res.errors} 个`)
      }
      setBatchErrors(res.errorMessages)
      setBatchText('')
      queryClient.invalidateQueries({ queryKey: ['proxy-pool'] })
    },
    onError: (err) => toast.error(`批量导入失败: ${extractErrorMessage(err)}`),
  })

  const deleteMutation = useMutation({
    mutationFn: (id: number) => deleteProxy(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxy-pool'] })
    },
    onError: (err) => toast.error(`删除失败: ${extractErrorMessage(err)}`),
  })

  const toggleMutation = useMutation({
    mutationFn: ({ id, enabled }: { id: number; enabled: boolean }) =>
      setProxyEnabled(id, enabled),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxy-pool'] })
    },
    onError: (err) => toast.error(`操作失败: ${extractErrorMessage(err)}`),
  })

  const handleAdd = (e: React.FormEvent) => {
    e.preventDefault()
    if (!newUrl.trim()) return
    addMutation.mutate()
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-2xl max-h-[85vh] flex flex-col">
        <DialogHeader>
          <DialogTitle>代理 IP 池管理</DialogTitle>
        </DialogHeader>

        <div className="flex-1 overflow-y-auto space-y-4 py-2">
          {/* 单条添加 */}
          {!showBatch && (
            <form onSubmit={handleAdd} className="flex gap-2">
              <Input
                placeholder="代理 URL（如 socks5://user:pass@host:port）"
                value={newUrl}
                onChange={(e) => setNewUrl(e.target.value)}
                className="flex-1 font-mono text-sm"
              />
              <Input
                placeholder="备注（可选）"
                value={newLabel}
                onChange={(e) => setNewLabel(e.target.value)}
                className="w-32"
              />
              <Button type="submit" size="sm" disabled={addMutation.isPending || !newUrl.trim()}>
                <Plus className="h-4 w-4 mr-1" />
                添加
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                onClick={() => setShowBatch(true)}
              >
                <Upload className="h-4 w-4 mr-1" />
                批量
              </Button>
            </form>
          )}

          {/* 批量导入 */}
          {showBatch && (
            <div className="space-y-2">
              <label className="text-sm font-medium">
                批量导入（每行一个代理 URL，# 开头为注释）
              </label>
              <textarea
                placeholder={'# 每行一个代理 URL\nsocks5://user:pass@host1:1080\nsocks5://user:pass@host2:1080\nhttp://user:pass@host3:8080'}
                value={batchText}
                onChange={(e) => setBatchText(e.target.value)}
                className="flex min-h-[120px] w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              />
              <div className="flex gap-2">
                <Button
                  size="sm"
                  onClick={() => batchMutation.mutate()}
                  disabled={batchMutation.isPending || !batchText.trim()}
                >
                  导入
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => { setShowBatch(false); setBatchText(''); setBatchErrors([]) }}
                >
                  {batchMutation.isSuccess ? '关闭' : '取消'}
                </Button>
              </div>
              {/* 批量导入失败明细 */}
              {batchErrors.length > 0 && (
                <div className="text-xs text-muted-foreground space-y-1 max-h-24 overflow-y-auto border rounded-md p-2">
                  <div className="font-medium text-yellow-600 dark:text-yellow-400">跳过的条目：</div>
                  {batchErrors.map((msg, i) => (
                    <div key={i}>{msg}</div>
                  ))}
                </div>
              )}
            </div>
          )}

          {/* 全局代理显示 */}
          <div className="rounded-md border p-3 space-y-2">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <Globe className="h-4 w-4 text-muted-foreground" />
                <span className="text-sm font-medium">全局代理</span>
              </div>
              {currentGlobalProxy && (
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-6 text-xs text-destructive hover:text-destructive"
                  onClick={() => setGlobalProxyMutation.mutate(null)}
                  disabled={setGlobalProxyMutation.isPending}
                >
                  清除
                </Button>
              )}
            </div>
            <div className="text-xs font-mono text-muted-foreground">
              {currentGlobalProxy ? maskProxyUrl(currentGlobalProxy) : '未配置（直连）'}
            </div>
          </div>

          {/* 代理列表 */}
          <div className="space-y-1">
            <div className="text-sm text-muted-foreground">
              共 {data?.total ?? 0} 个代理
            </div>

            {isLoading && (
              <div className="text-sm text-muted-foreground py-4 text-center">加载中...</div>
            )}

            {data?.proxies.length === 0 && !isLoading && (
              <div className="text-sm text-muted-foreground py-4 text-center">
                暂无代理，请添加
              </div>
            )}

            <div className="border rounded-md divide-y max-h-[320px] overflow-y-auto">
              {data?.proxies.map((proxy: ProxyPoolEntry) => (
                <div key={proxy.id} className="flex items-center gap-3 p-3">
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="font-mono text-xs truncate">
                        {maskProxyUrl(proxy.url)}
                      </span>
                      {proxy.label && (
                        <Badge variant="secondary" className="text-xs">{proxy.label}</Badge>
                      )}
                      {!proxy.enabled && (
                        <Badge variant="outline" className="text-xs text-muted-foreground">已禁用</Badge>
                      )}
                    </div>
                    {proxy.credentialCount > 0 && (
                      <div className="text-xs text-muted-foreground mt-0.5">
                        {proxy.credentialCount} 个凭据使用中
                      </div>
                    )}
                  </div>
                  <div className="flex items-center gap-1 shrink-0">
                    {onSelectProxy && proxy.enabled && (
                      <Button
                        size="sm"
                        variant="outline"
                        className="h-7 text-xs"
                        onClick={() => {
                          onSelectProxy(proxy.url)
                          onOpenChange(false)
                        }}
                      >
                        选用
                      </Button>
                    )}
                    {proxy.enabled && proxy.url !== currentGlobalProxy && (
                      <Button
                        size="sm"
                        variant="outline"
                        className="h-7 text-xs"
                        onClick={() => setGlobalProxyMutation.mutate(proxy.url)}
                        disabled={setGlobalProxyMutation.isPending}
                        title="设为全局代理"
                      >
                        <Globe className="h-3 w-3 mr-1" />
                        全局
                      </Button>
                    )}
                    {proxy.url === currentGlobalProxy && (
                      <Badge variant="secondary" className="text-xs h-7">全局</Badge>
                    )}
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-7 w-7 p-0"
                      onClick={() => toggleMutation.mutate({ id: proxy.id, enabled: !proxy.enabled })}
                      title={proxy.enabled ? '禁用此代理' : '启用此代理'}
                    >
                      {proxy.enabled ? (
                        <ToggleRight className="h-4 w-4 text-green-500" />
                      ) : (
                        <ToggleLeft className="h-4 w-4 text-muted-foreground" />
                      )}
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-7 w-7 p-0 text-destructive hover:text-destructive"
                      onClick={() => deleteMutation.mutate(proxy.id)}
                      disabled={deleteMutation.isPending}
                    >
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  </div>
                </div>
              ))}
            </div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
