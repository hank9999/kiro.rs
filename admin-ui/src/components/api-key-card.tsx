import { useState } from 'react'
import { Copy, Edit2, Power, Trash2, Check } from 'lucide-react'
import { toast } from 'sonner'
import { Card, CardContent } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { useUpdateApiKey, useDeleteApiKey } from '@/hooks/use-api-keys'
import type { ApiKeyInfo } from '@/types/api'

interface ApiKeyCardProps {
  apiKey: ApiKeyInfo
}

export function ApiKeyCard({ apiKey }: ApiKeyCardProps) {
  const [isEditing, setIsEditing] = useState(false)
  const [editedName, setEditedName] = useState(apiKey.name)
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false)

  const { mutate: updateApiKey, isPending: isUpdating } = useUpdateApiKey()
  const { mutate: deleteApiKey, isPending: isDeleting } = useDeleteApiKey()

  const handleCopy = () => {
    navigator.clipboard.writeText(apiKey.key)
    toast.success('API Key 已复制到剪贴板')
  }

  const handleToggleEnabled = () => {
    updateApiKey(
      { id: apiKey.id, request: { enabled: !apiKey.enabled } },
      {
        onSuccess: () => {
          toast.success(apiKey.enabled ? 'API Key 已禁用' : 'API Key 已启用')
        },
        onError: (error: any) => {
          toast.error(error.response?.data?.error?.message || '操作失败')
        },
      }
    )
  }

  const handleSaveName = () => {
    if (editedName.trim() === apiKey.name) {
      setIsEditing(false)
      return
    }

    updateApiKey(
      { id: apiKey.id, request: { name: editedName.trim() } },
      {
        onSuccess: () => {
          toast.success('名称已更新')
          setIsEditing(false)
        },
        onError: (error: any) => {
          toast.error(error.response?.data?.error?.message || '更新失败')
        },
      }
    )
  }

  const handleDelete = () => {
    deleteApiKey(apiKey.id, {
      onSuccess: () => {
        toast.success('API Key 已删除')
        setDeleteDialogOpen(false)
      },
      onError: (error: any) => {
        toast.error(error.response?.data?.error?.message || '删除失败')
      },
    })
  }

  const formatDate = (dateStr?: string) => {
    if (!dateStr || dateStr === 'N/A') return 'N/A'
    try {
      return new Date(dateStr).toLocaleString('zh-CN')
    } catch {
      return dateStr
    }
  }

  return (
    <>
      <Card className={!apiKey.enabled ? 'opacity-60' : ''}>
        <CardContent className="p-4">
          <div className="space-y-3">
            {/* 头部：名称和状态 */}
            <div className="flex items-start justify-between">
              <div className="flex-1 min-w-0">
                {isEditing ? (
                  <div className="flex items-center gap-2">
                    <Input
                      value={editedName}
                      onChange={(e) => setEditedName(e.target.value)}
                      className="h-8"
                      autoFocus
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') handleSaveName()
                        if (e.key === 'Escape') {
                          setEditedName(apiKey.name)
                          setIsEditing(false)
                        }
                      }}
                    />
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={handleSaveName}
                      disabled={isUpdating}
                    >
                      <Check className="h-4 w-4" />
                    </Button>
                  </div>
                ) : (
                  <div className="flex items-center gap-2">
                    <h3 className="font-medium truncate">{apiKey.name}</h3>
                    {!apiKey.isPrimary && (
                      <Button
                        size="sm"
                        variant="ghost"
                        className="h-6 w-6 p-0"
                        onClick={() => setIsEditing(true)}
                      >
                        <Edit2 className="h-3 w-3" />
                      </Button>
                    )}
                  </div>
                )}
              </div>
              <div className="flex items-center gap-2 ml-2">
                {apiKey.isPrimary && (
                  <Badge variant="secondary">主Key</Badge>
                )}
                <Badge variant={apiKey.enabled ? 'default' : 'secondary'}>
                  {apiKey.enabled ? '启用' : '禁用'}
                </Badge>
              </div>
            </div>

            {/* API Key */}
            <div className="space-y-1">
              <div className="text-xs text-muted-foreground">API Key</div>
              <div className="flex items-center gap-2">
                <code className="flex-1 text-sm bg-muted px-2 py-1 rounded font-mono break-all">
                  {apiKey.key}
                </code>
                <Button size="sm" variant="outline" onClick={handleCopy}>
                  <Copy className="h-4 w-4" />
                </Button>
              </div>
            </div>

            {/* 时间信息 */}
            <div className="grid grid-cols-2 gap-2 text-xs text-muted-foreground">
              <div>
                <div>创建时间</div>
                <div className="font-mono">{formatDate(apiKey.createdAt)}</div>
              </div>
              <div>
                <div>最后使用</div>
                <div className="font-mono">{formatDate(apiKey.lastUsedAt)}</div>
              </div>
            </div>

            {/* 操作按钮 */}
            {!apiKey.isPrimary && (
              <div className="flex gap-2 pt-2 border-t">
                <Button
                  size="sm"
                  variant="outline"
                  className="flex-1"
                  onClick={handleToggleEnabled}
                  disabled={isUpdating}
                >
                  <Power className="h-4 w-4 mr-1" />
                  {apiKey.enabled ? '禁用' : '启用'}
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  className="text-destructive hover:bg-destructive hover:text-destructive-foreground"
                  onClick={() => setDeleteDialogOpen(true)}
                  disabled={isDeleting}
                >
                  <Trash2 className="h-4 w-4 mr-1" />
                  删除
                </Button>
              </div>
            )}
          </div>
        </CardContent>
      </Card>

      {/* 删除确认对话框 */}
      <AlertDialog open={deleteDialogOpen} onOpenChange={setDeleteDialogOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>确认删除</AlertDialogTitle>
            <AlertDialogDescription>
              确定要删除 API Key "{apiKey.name}" 吗？此操作无法撤销。
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction onClick={handleDelete} disabled={isDeleting}>
              删除
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
