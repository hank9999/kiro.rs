import { useState } from 'react'
import { Plus, RefreshCw, Key } from 'lucide-react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { ApiKeyCard } from '@/components/api-key-card'
import { AddApiKeyDialog } from '@/components/add-api-key-dialog'
import { useApiKeys } from '@/hooks/use-api-keys'
import { toast } from 'sonner'

export function ApiKeyManagement() {
  const [addDialogOpen, setAddDialogOpen] = useState(false)
  const { data, isLoading, error, refetch } = useApiKeys()

  const handleRefresh = () => {
    refetch()
    toast.success('已刷新 API Keys 列表')
  }

  if (error) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-center space-y-2">
          <p className="text-destructive">加载失败</p>
          <Button variant="outline" onClick={handleRefresh}>
            重试
          </Button>
        </div>
      </div>
    )
  }

  const allKeys = [
    ...(data?.primaryKey ? [data.primaryKey] : []),
    ...(data?.apiKeys || []),
  ]

  const enabledCount = allKeys.filter((k) => k.enabled).length
  const totalCount = allKeys.length

  return (
    <div className="space-y-6">
      {/* 统计卡片 */}
      <div className="grid gap-4 md:grid-cols-3">
        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium">总 Keys</CardTitle>
            <Key className="h-4 w-4 text-muted-foreground" />
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">{totalCount}</div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium">启用中</CardTitle>
            <Key className="h-4 w-4 text-green-600" />
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">{enabledCount}</div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
            <CardTitle className="text-sm font-medium">禁用</CardTitle>
            <Key className="h-4 w-4 text-muted-foreground" />
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">{totalCount - enabledCount}</div>
          </CardContent>
        </Card>
      </div>

      {/* 操作栏 */}
      <div className="flex items-center justify-between">
        <h2 className="text-2xl font-bold">API Keys</h2>
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={handleRefresh}>
            <RefreshCw className="h-4 w-4 mr-1" />
            刷新
          </Button>
          <Button size="sm" onClick={() => setAddDialogOpen(true)}>
            <Plus className="h-4 w-4 mr-1" />
            添加 Key
          </Button>
        </div>
      </div>

      {/* Keys 列表 */}
      {isLoading ? (
        <div className="flex items-center justify-center h-64">
          <div className="text-muted-foreground">加载中...</div>
        </div>
      ) : allKeys.length === 0 ? (
        <Card>
          <CardContent className="flex flex-col items-center justify-center h-64 space-y-4">
            <Key className="h-12 w-12 text-muted-foreground" />
            <div className="text-center space-y-2">
              <p className="text-lg font-medium">暂无 API Keys</p>
              <p className="text-sm text-muted-foreground">
                点击"添加 Key"按钮创建第一个 API Key
              </p>
            </div>
            <Button onClick={() => setAddDialogOpen(true)}>
              <Plus className="h-4 w-4 mr-1" />
              添加 Key
            </Button>
          </CardContent>
        </Card>
      ) : (
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {allKeys.map((apiKey) => (
            <ApiKeyCard key={apiKey.id} apiKey={apiKey} />
          ))}
        </div>
      )}

      {/* 添加对话框 */}
      <AddApiKeyDialog open={addDialogOpen} onOpenChange={setAddDialogOpen} />
    </div>
  )
}
