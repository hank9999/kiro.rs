import { useState } from 'react'
import { toast } from 'sonner'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useAddApiKey, useGenerateApiKey } from '@/hooks/use-api-keys'

interface AddApiKeyDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function AddApiKeyDialog({ open, onOpenChange }: AddApiKeyDialogProps) {
  const [tab, setTab] = useState<'manual' | 'generate'>('manual')

  // 手动输入
  const [manualKey, setManualKey] = useState('')
  const [manualName, setManualName] = useState('')

  // 自动生成
  const [generateName, setGenerateName] = useState('')
  const [generateLength, setGenerateLength] = useState(32)
  const [generatedKey, setGeneratedKey] = useState('')

  const { mutate: addApiKey, isPending: isAdding } = useAddApiKey()
  const { mutate: generateApiKey, isPending: isGenerating } = useGenerateApiKey()

  const handleReset = () => {
    setManualKey('')
    setManualName('')
    setGenerateName('')
    setGenerateLength(32)
    setGeneratedKey('')
    setTab('manual')
  }

  const handleClose = () => {
    handleReset()
    onOpenChange(false)
  }

  const handleManualAdd = () => {
    if (!manualKey.trim()) {
      toast.error('请输入 API Key')
      return
    }
    if (!manualName.trim()) {
      toast.error('请输入名称')
      return
    }

    addApiKey(
      { key: manualKey.trim(), name: manualName.trim() },
      {
        onSuccess: () => {
          toast.success('API Key 已添加')
          handleClose()
        },
        onError: (error: any) => {
          toast.error(error.response?.data?.error?.message || '添加失败')
        },
      }
    )
  }

  const handleGenerate = () => {
    if (!generateName.trim()) {
      toast.error('请输入名称')
      return
    }

    generateApiKey(
      { name: generateName.trim(), length: generateLength },
      {
        onSuccess: (data) => {
          setGeneratedKey(data.key)
          toast.success('API Key 已生成')
        },
        onError: (error: any) => {
          toast.error(error.response?.data?.error?.message || '生成失败')
        },
      }
    )
  }

  const handleCopyAndClose = () => {
    if (generatedKey) {
      navigator.clipboard.writeText(generatedKey)
      toast.success('API Key 已复制到剪贴板')
    }
    handleClose()
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[500px]">
        <DialogHeader>
          <DialogTitle>添加 API Key</DialogTitle>
          <DialogDescription>
            手动输入现有的 Key 或生成新的随机 Key
          </DialogDescription>
        </DialogHeader>

        <Tabs value={tab} onValueChange={(v) => setTab(v as any)}>
          <TabsList className="grid w-full grid-cols-2">
            <TabsTrigger value="manual">手动输入</TabsTrigger>
            <TabsTrigger value="generate">自动生成</TabsTrigger>
          </TabsList>

          <TabsContent value="manual" className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="manual-key">API Key</Label>
              <Input
                id="manual-key"
                placeholder="输入 API Key"
                value={manualKey}
                onChange={(e) => setManualKey(e.target.value)}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="manual-name">名称</Label>
              <Input
                id="manual-name"
                placeholder="例如：生产环境、测试应用"
                value={manualName}
                onChange={(e) => setManualName(e.target.value)}
              />
            </div>
          </TabsContent>

          <TabsContent value="generate" className="space-y-4">
            {!generatedKey ? (
              <>
                <div className="space-y-2">
                  <Label htmlFor="generate-name">名称</Label>
                  <Input
                    id="generate-name"
                    placeholder="例如：生产环境、测试应用"
                    value={generateName}
                    onChange={(e) => setGenerateName(e.target.value)}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="generate-length">Key 长度</Label>
                  <Input
                    id="generate-length"
                    type="number"
                    min={16}
                    max={64}
                    value={generateLength}
                    onChange={(e) => setGenerateLength(Number(e.target.value))}
                  />
                  <p className="text-xs text-muted-foreground">
                    推荐长度：32-64 个字符
                  </p>
                </div>
              </>
            ) : (
              <div className="space-y-2">
                <Label>生成的 API Key</Label>
                <div className="p-3 bg-muted rounded-md">
                  <code className="text-sm font-mono break-all">{generatedKey}</code>
                </div>
                <p className="text-xs text-destructive">
                  ⚠️ 请立即复制保存，关闭后将无法再次查看
                </p>
              </div>
            )}
          </TabsContent>
        </Tabs>

        <DialogFooter>
          {tab === 'manual' ? (
            <>
              <Button variant="outline" onClick={handleClose}>
                取消
              </Button>
              <Button onClick={handleManualAdd} disabled={isAdding}>
                添加
              </Button>
            </>
          ) : (
            <>
              {!generatedKey ? (
                <>
                  <Button variant="outline" onClick={handleClose}>
                    取消
                  </Button>
                  <Button onClick={handleGenerate} disabled={isGenerating}>
                    生成
                  </Button>
                </>
              ) : (
                <Button onClick={handleCopyAndClose}>
                  复制并关闭
                </Button>
              )}
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
