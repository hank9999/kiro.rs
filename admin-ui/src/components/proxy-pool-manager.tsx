import { useState } from 'react'
import { toast } from 'sonner'
import { Plus, Trash2, Play, Pause, Wifi, Upload } from 'lucide-react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog'
import {
  useProxyPool,
  useAddProxy,
  useBatchAddProxies,
  useDeleteProxy,
  useSetProxyDisabled,
  useTestProxy,
} from '@/hooks/use-proxy-pool'
import { extractErrorMessage } from '@/lib/utils'
import type { ProxyEntry } from '@/types/proxy-pool'

export function ProxyPoolManager() {
  const [addDialogOpen, setAddDialogOpen] = useState(false)
  const [batchDialogOpen, setBatchDialogOpen] = useState(false)
  const [addUrl, setAddUrl] = useState('')
  const [addUsername, setAddUsername] = useState('')
  const [addPassword, setAddPassword] = useState('')
  const [addLabel, setAddLabel] = useState('')
  const [batchText, setBatchText] = useState('')
  const [testingIds, setTestingIds] = useState<Set<number>>(new Set())

  const { data, isLoading } = useProxyPool()
  const { mutate: addProxy, isPending: isAdding } = useAddProxy()
  const { mutate: batchAdd, isPending: isBatchAdding } = useBatchAddProxies()
  const { mutate: deleteProxy } = useDeleteProxy()
  const { mutate: setDisabled } = useSetProxyDisabled()
  const { mutateAsync: testProxy } = useTestProxy()

  const handleAdd = () => {
    if (!addUrl.trim()) {
      toast.error('请输入代理 URL')
      return
    }
    addProxy(
      {
        url: addUrl.trim(),
        username: addUsername.trim() || undefined,
        password: addPassword.trim() || undefined,
        label: addLabel.trim() || undefined,
      },
      {
        onSuccess: (res) => {
          toast.success(res.message)
          setAddDialogOpen(false)
          setAddUrl('')
          setAddUsername('')
          setAddPassword('')
          setAddLabel('')
        },
        onError: (err) => toast.error(`添加失败: ${extractErrorMessage(err)}`),
      }
    )
  }

  const handleBatchAdd = () => {
    const lines = batchText.split('\n').filter((l) => l.trim())
    if (lines.length === 0) {
      toast.error('请输入至少一行代理配置')
      return
    }
    batchAdd(
      { lines },
      {
        onSuccess: (res) => {
          toast.success(res.message)
          setBatchDialogOpen(false)
          setBatchText('')
        },
        onError: (err) => toast.error(`批量添加失败: ${extractErrorMessage(err)}`),
      }
    )
  }

  const handleDelete = (proxy: ProxyEntry) => {
    if (proxy.assignedTo) {
      toast.error(`代理正在被凭据 #${proxy.assignedTo} 使用，请先解绑`)
      return
    }
    if (!confirm(`确定删除代理 #${proxy.id}${proxy.label ? ` (${proxy.label})` : ''}？`)) return
    deleteProxy(proxy.id, {
      onSuccess: () => toast.success(`代理 #${proxy.id} 已删除`),
      onError: (err) => toast.error(`删除失败: ${extractErrorMessage(err)}`),
    })
  }

  const handleToggleDisabled = (proxy: ProxyEntry) => {
    setDisabled(
      { id: proxy.id, disabled: !proxy.disabled },
      {
        onSuccess: () =>
          toast.success(`代理 #${proxy.id} 已${proxy.disabled ? '启用' : '禁用'}`),
        onError: (err) => toast.error(extractErrorMessage(err)),
      }
    )
  }

  const handleTest = async (proxy: ProxyEntry) => {
    setTestingIds((prev) => new Set(prev).add(proxy.id))
    try {
      const result = await testProxy(proxy.id)
      if (result.success) {
        toast.success(`代理 #${proxy.id}: ${result.message}`)
      } else {
        toast.error(`代理 #${proxy.id}: ${result.message}`)
      }
    } catch (err) {
      toast.error(`测试失败: ${extractErrorMessage(err)}`)
    } finally {
      setTestingIds((prev) => {
        const next = new Set(prev)
        next.delete(proxy.id)
        return next
      })
    }
  }

  if (isLoading) {
    return (
      <div className="flex items-center justify-center py-12">
        <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
      </div>
    )
  }

  return (
    <div className="space-y-4">
      {/* 统计 */}
      <div className="grid gap-4 md:grid-cols-3 mb-6">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">代理总数</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">{data?.total || 0}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">空闲代理</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold text-green-600">{data?.available || 0}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">已分配</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold text-blue-600">
              {(data?.total || 0) - (data?.available || 0)}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* 操作栏 */}
      <div className="flex items-center justify-between">
        <h2 className="text-xl font-semibold">代理池管理</h2>
        <div className="flex gap-2">
          <Button onClick={() => setBatchDialogOpen(true)} size="sm" variant="outline">
            <Upload className="h-4 w-4 mr-2" />
            批量导入
          </Button>
          <Button onClick={() => setAddDialogOpen(true)} size="sm">
            <Plus className="h-4 w-4 mr-2" />
            添加代理
          </Button>
        </div>
      </div>

      {/* 代理列表 */}
      {!data?.proxies.length ? (
        <Card>
          <CardContent className="py-8 text-center text-muted-foreground">
            暂无代理，点击"添加代理"开始配置
          </CardContent>
        </Card>
      ) : (
        <div className="grid gap-3 md:grid-cols-2 lg:grid-cols-3">
          {data.proxies.map((proxy) => (
            <ProxyCard
              key={proxy.id}
              proxy={proxy}
              testing={testingIds.has(proxy.id)}
              onTest={() => handleTest(proxy)}
              onToggleDisabled={() => handleToggleDisabled(proxy)}
              onDelete={() => handleDelete(proxy)}
            />
          ))}
        </div>
      )}

      {/* 添加代理对话框 */}
      <Dialog open={addDialogOpen} onOpenChange={setAddDialogOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>添加代理</DialogTitle>
          </DialogHeader>
          <div className="space-y-4 py-4">
            <div className="space-y-2">
              <label className="text-sm font-medium">
                代理 URL <span className="text-red-500">*</span>
              </label>
              <Input
                placeholder="http://ip:port 或 socks5://ip:port"
                value={addUrl}
                onChange={(e) => setAddUrl(e.target.value)}
                disabled={isAdding}
              />
            </div>
            <div className="grid grid-cols-2 gap-2">
              <div className="space-y-2">
                <label className="text-sm font-medium">用户名</label>
                <Input
                  placeholder="可选"
                  value={addUsername}
                  onChange={(e) => setAddUsername(e.target.value)}
                  disabled={isAdding}
                />
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium">密码</label>
                <Input
                  type="password"
                  placeholder="可选"
                  value={addPassword}
                  onChange={(e) => setAddPassword(e.target.value)}
                  disabled={isAdding}
                />
              </div>
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">备注标签</label>
              <Input
                placeholder="如 IPRoyal-US-01"
                value={addLabel}
                onChange={(e) => setAddLabel(e.target.value)}
                disabled={isAdding}
              />
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setAddDialogOpen(false)} disabled={isAdding}>
              取消
            </Button>
            <Button onClick={handleAdd} disabled={isAdding}>
              {isAdding ? '添加中...' : '添加'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 批量导入对话框 */}
      <Dialog open={batchDialogOpen} onOpenChange={setBatchDialogOpen}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>批量导入代理</DialogTitle>
          </DialogHeader>
          <div className="space-y-4 py-4">
            <p className="text-sm text-muted-foreground">
              每行一个代理，格式：<code>url [用户名 密码 [标签]]</code>
            </p>
            <textarea
              className="flex min-h-[200px] w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50 font-mono"
              placeholder={`http://1.2.3.4:8080\nhttp://5.6.7.8:3128 user pass\nsocks5://9.10.11.12:1080 user pass US-01`}
              value={batchText}
              onChange={(e) => setBatchText(e.target.value)}
              disabled={isBatchAdding}
            />
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setBatchDialogOpen(false)} disabled={isBatchAdding}>
              取消
            </Button>
            <Button onClick={handleBatchAdd} disabled={isBatchAdding}>
              {isBatchAdding ? '导入中...' : '导入'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}

// 单个代理卡片
function ProxyCard({
  proxy,
  testing,
  onTest,
  onToggleDisabled,
  onDelete,
}: {
  proxy: ProxyEntry
  testing: boolean
  onTest: () => void
  onToggleDisabled: () => void
  onDelete: () => void
}) {
  // 从 URL 中提取 IP 用于显示
  const displayUrl = (() => {
    try {
      const u = new URL(proxy.url)
      return `${u.protocol}//${u.hostname}:${u.port}`
    } catch {
      return proxy.url
    }
  })()

  return (
    <Card className={`${proxy.disabled ? 'opacity-60' : ''}`}>
      <CardContent className="pt-4 pb-3 space-y-2">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <span className="text-xs text-muted-foreground">#{proxy.id}</span>
            {proxy.label && (
              <Badge variant="outline" className="text-xs">
                {proxy.label}
              </Badge>
            )}
          </div>
          <div className="flex items-center gap-1">
            {proxy.disabled ? (
              <Badge variant="destructive" className="text-xs">已禁用</Badge>
            ) : proxy.assignedTo ? (
              <Badge variant="secondary" className="text-xs">凭据 #{proxy.assignedTo}</Badge>
            ) : (
              <Badge variant="success" className="text-xs">空闲</Badge>
            )}
          </div>
        </div>

        <div className="font-mono text-sm truncate" title={proxy.url}>
          {displayUrl}
        </div>

        {proxy.username && (
          <div className="text-xs text-muted-foreground truncate">
            认证: {proxy.username}
          </div>
        )}

        <div className="flex items-center gap-1 pt-1">
          <Button
            variant="ghost"
            size="sm"
            onClick={onTest}
            disabled={testing}
            title="测试连通性"
          >
            <Wifi className={`h-3.5 w-3.5 ${testing ? 'animate-pulse' : ''}`} />
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={onToggleDisabled}
            title={proxy.disabled ? '启用' : '禁用'}
          >
            {proxy.disabled ? <Play className="h-3.5 w-3.5" /> : <Pause className="h-3.5 w-3.5" />}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={onDelete}
            disabled={!!proxy.assignedTo}
            title={proxy.assignedTo ? '正在使用中，无法删除' : '删除'}
            className="text-destructive hover:text-destructive"
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}
