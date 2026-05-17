import { useState, useEffect } from 'react'
import { Settings, Check, Loader2 } from 'lucide-react'
import { toast } from 'sonner'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { useSummaryModel, useSetSummaryModel } from '@/hooks/use-credentials'

const MODEL_DISPLAY_NAMES: Record<string, string> = {
  'claude-sonnet-4.5': 'Claude Sonnet 4.5',
  'claude-sonnet-4': 'Claude Sonnet 4',
  'claude-haiku-4.5': 'Claude Haiku 4.5',
}

export function SummaryModelSetting() {
  const { data, isLoading, error } = useSummaryModel()
  const setModel = useSetSummaryModel()
  const [selectedModel, setSelectedModel] = useState<string>('')

  useEffect(() => {
    if (data?.currentModel) {
      setSelectedModel(data.currentModel)
    }
  }, [data?.currentModel])

  const handleSave = () => {
    if (!selectedModel || selectedModel === data?.currentModel) return

    setModel.mutate(selectedModel, {
      onSuccess: (res) => {
        toast.success(res.message)
      },
      onError: (err) => {
        toast.error('设置失败: ' + (err as Error).message)
      },
    })
  }

  if (isLoading) {
    return (
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium text-muted-foreground flex items-center gap-2">
            <Settings className="h-4 w-4" />
            摘要模型
          </CardTitle>
        </CardHeader>
        <CardContent>
          <div className="flex items-center gap-2 text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            加载中...
          </div>
        </CardContent>
      </Card>
    )
  }

  if (error) {
    return (
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium text-muted-foreground flex items-center gap-2">
            <Settings className="h-4 w-4" />
            摘要模型
          </CardTitle>
        </CardHeader>
        <CardContent>
          <div className="text-sm text-red-500">
            加载失败: {(error as Error).message}
          </div>
        </CardContent>
      </Card>
    )
  }

  const availableModels = data?.availableModels || []
  const hasChanges = selectedModel !== data?.currentModel

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium text-muted-foreground flex items-center gap-2">
          <Settings className="h-4 w-4" />
          智能摘要模型
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <div className="text-xs text-muted-foreground">
          用于历史消息压缩时生成摘要的模型
        </div>
        <div className="flex flex-wrap gap-2">
          {availableModels.map((model) => (
            <button
              key={model}
              onClick={() => setSelectedModel(model)}
              className={`
                px-3 py-1.5 text-sm rounded-md transition-all duration-200
                ${
                  selectedModel === model
                    ? 'bg-emerald-600 text-white shadow-sm'
                    : 'bg-muted hover:bg-muted/80 text-foreground'
                }
              `}
            >
              {MODEL_DISPLAY_NAMES[model] || model}
              {selectedModel === model && (
                <Check className="inline-block ml-1.5 h-3.5 w-3.5" />
              )}
            </button>
          ))}
        </div>
        {hasChanges && (
          <div className="flex items-center gap-2 pt-1">
            <Button
              size="sm"
              onClick={handleSave}
              disabled={setModel.isPending}
              className="transition-all duration-200"
            >
              {setModel.isPending ? (
                <>
                  <Loader2 className="h-4 w-4 mr-1.5 animate-spin" />
                  保存中...
                </>
              ) : (
                '保存设置'
              )}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => setSelectedModel(data?.currentModel || '')}
              disabled={setModel.isPending}
            >
              取消
            </Button>
          </div>
        )}
      </CardContent>
    </Card>
  )
}
