import { useState } from 'react'
import { toast } from 'sonner'
import { Upload, FileJson, FileUp } from 'lucide-react'
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
import { useImportTokenJson } from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'
import type { ImportTokenJsonResponse, ImportItemResult, TokenJsonItem } from '@/types/api'

interface ImportTokenJsonDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

type Step = 'input' | 'preview' | 'result'

export function ImportTokenJsonDialog({ open, onOpenChange }: ImportTokenJsonDialogProps) {
  const [step, setStep] = useState<Step>('input')
  const [jsonInput, setJsonInput] = useState('')
  const [parsedItems, setParsedItems] = useState<TokenJsonItem[]>([])
  const [previewResult, setPreviewResult] = useState<ImportTokenJsonResponse | null>(null)
  const [importResult, setImportResult] = useState<ImportTokenJsonResponse | null>(null)
  const [isDragOver, setIsDragOver] = useState(false)

  const importMutation = useImportTokenJson()

  const resetState = () => {
    setStep('input')
    setJsonInput('')
    setParsedItems([])
    setPreviewResult(null)
    setImportResult(null)
    setIsDragOver(false)
  }

  const handleClose = () => {
    onOpenChange(false)
    // 延迟重置状态，避免关闭动画时看到状态变化
    setTimeout(resetState, 200)
  }

  // 解析 JSON 输入
  const parseJsonInput = (): TokenJsonItem[] | null => {
    try {
      const parsed = JSON.parse(jsonInput.trim())
      // 支持单个对象或数组
      if (Array.isArray(parsed)) {
        return parsed
      } else if (typeof parsed === 'object' && parsed !== null) {
        return [parsed]
      }
      toast.error('无效的 JSON 格式：需要对象或数组')
      return null
    } catch {
      toast.error('JSON 解析失败：请检查格式是否正确')
      return null
    }
  }

  // 预览（dry-run）
  const handlePreview = async () => {
    const items = parseJsonInput()
    if (!items || items.length === 0) {
      toast.error('没有可导入的凭据')
      return
    }

    setParsedItems(items)

    try {
      const result = await importMutation.mutateAsync({
        dryRun: true,
        items,
      })
      setPreviewResult(result)
      setStep('preview')
    } catch (error) {
      toast.error(`预览失败: ${extractErrorMessage(error)}`)
    }
  }

  // 确认导入
  const handleImport = async () => {
    if (parsedItems.length === 0) return

    try {
      const result = await importMutation.mutateAsync({
        dryRun: false,
        items: parsedItems,
      })
      setImportResult(result)
      setStep('result')

      if (result.summary.added > 0) {
        toast.success(`成功导入 ${result.summary.added} 个凭据`)
      }
    } catch (error) {
      toast.error(`导入失败: ${extractErrorMessage(error)}`)
    }
  }

  // 处理文件读取
  const readFile = (file: File) => {
    if (!file) return
    const reader = new FileReader()
    reader.onload = (event) => {
      const content = event.target?.result as string
      setJsonInput(content)
    }
    reader.onerror = () => {
      toast.error('文件读取失败')
    }
    reader.readAsText(file)
  }

  // 处理文件上传
  const handleFileUpload = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0]
    if (file) readFile(file)
  }

  // 处理拖放
  const handleDrop = (e: React.DragEvent) => {
    e.preventDefault()
    setIsDragOver(false)
    const file = e.dataTransfer.files?.[0]
    if (file && file.name.endsWith('.json')) {
      readFile(file)
    } else if (file) {
      toast.error('请上传 .json 文件')
    }
  }

  const getActionBadge = (action: ImportItemResult['action']) => {
    switch (action) {
      case 'added':
        return <Badge variant="success">添加</Badge>
      case 'skipped':
        return <Badge variant="warning">跳过</Badge>
      case 'invalid':
        return <Badge variant="destructive">无效</Badge>
    }
  }

  const getResultBadge = (action: ImportItemResult['action']) => {
    switch (action) {
      case 'added':
        return <Badge variant="success">已添加</Badge>
      case 'skipped':
        return <Badge variant="warning">已跳过</Badge>
      case 'invalid':
        return <Badge variant="destructive">无效</Badge>
    }
  }

  // 渲染汇总统计
  const renderSummary = (data: ImportTokenJsonResponse, isResult: boolean) => {
    const { summary } = data
    return (
      <div className="flex items-center justify-center gap-6 py-4 px-6 bg-muted/50 rounded-lg">
        <div className="text-center">
          <div className="text-2xl font-semibold tabular-nums">{summary.parsed}</div>
          <div className="text-xs text-muted-foreground">解析</div>
        </div>
        <div className="h-8 w-px bg-border" />
        <div className="text-center">
          <div className="text-2xl font-semibold tabular-nums text-green-600">
            {summary.added}
          </div>
          <div className="text-xs text-muted-foreground">
            {isResult ? '已添加' : '将添加'}
          </div>
        </div>
        <div className="h-8 w-px bg-border" />
        <div className="text-center">
          <div className="text-2xl font-semibold tabular-nums text-yellow-600">
            {summary.skipped}
          </div>
          <div className="text-xs text-muted-foreground">跳过</div>
        </div>
        <div className="h-8 w-px bg-border" />
        <div className="text-center">
          <div className="text-2xl font-semibold tabular-nums text-red-600">
            {summary.invalid}
          </div>
          <div className="text-xs text-muted-foreground">无效</div>
        </div>
      </div>
    )
  }

  // 渲染项目列表
  const renderItemList = (items: ImportItemResult[], isResult: boolean) => {
    if (items.length === 0) {
      return (
        <div className="py-8 text-center text-muted-foreground">没有可显示的项目</div>
      )
    }

    return (
      <div className="border rounded-lg overflow-hidden">
        <div className="max-h-56 overflow-auto">
          <table className="w-full text-sm">
            <thead className="bg-muted/50 sticky top-0">
              <tr>
                <th className="text-left font-medium px-3 py-2 text-muted-foreground">#</th>
                <th className="text-left font-medium px-3 py-2 text-muted-foreground">指纹</th>
                <th className="text-left font-medium px-3 py-2 text-muted-foreground">状态</th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {items.map((item) => (
                <tr key={item.index} className="hover:bg-muted/30 transition-colors">
                  <td className="px-3 py-2 text-muted-foreground tabular-nums">
                    {item.index + 1}
                  </td>
                  <td className="px-3 py-2">
                    <div
                      className="font-mono text-xs truncate max-w-[280px]"
                      title={item.fingerprint}
                    >
                      {item.fingerprint}
                    </div>
                    {item.reason && (
                      <div className="text-xs text-muted-foreground mt-0.5">
                        {item.reason}
                      </div>
                    )}
                    {isResult && item.credentialId && (
                      <div className="text-xs text-green-600 mt-0.5">
                        ID: #{item.credentialId}
                      </div>
                    )}
                  </td>
                  <td className="px-3 py-2">
                    {isResult ? getResultBadge(item.action) : getActionBadge(item.action)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    )
  }

  return (
    <Dialog open={open} onOpenChange={handleClose}>
      <DialogContent className="sm:max-w-2xl max-h-[80vh] overflow-hidden flex flex-col">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <FileJson className="h-5 w-5" />
            {step === 'input' && '导入 token.json'}
            {step === 'preview' && '预览导入结果'}
            {step === 'result' && '导入完成'}
          </DialogTitle>
          {step === 'input' && (
            <DialogDescription>上传或粘贴 token.json 文件内容</DialogDescription>
          )}
        </DialogHeader>

        <div className="flex-1 overflow-auto py-4">
          {/* Step 1: 输入 */}
          {step === 'input' && (
            <div className="space-y-4">
              {/* 拖放区域 */}
              <div
                className={`
                  relative border-2 border-dashed rounded-lg p-6 text-center transition-colors
                  ${
                    isDragOver
                      ? 'border-primary bg-primary/5'
                      : 'border-muted-foreground/25 hover:border-muted-foreground/50'
                  }
                `}
                onDragOver={(e) => {
                  e.preventDefault()
                  setIsDragOver(true)
                }}
                onDragLeave={() => setIsDragOver(false)}
                onDrop={handleDrop}
              >
                <FileUp className="h-8 w-8 mx-auto mb-2 text-muted-foreground" />
                <p className="text-sm text-muted-foreground mb-2">
                  拖放 token.json 文件到此处
                </p>
                <Button variant="outline" size="sm" asChild>
                  <label htmlFor="file-upload" className="cursor-pointer">
                    <Upload className="h-4 w-4 mr-2" />
                    选择文件
                  </label>
                </Button>
                <input
                  id="file-upload"
                  type="file"
                  accept=".json"
                  onChange={handleFileUpload}
                  className="hidden"
                />
              </div>

              <div className="relative">
                <div className="absolute inset-0 flex items-center">
                  <span className="w-full border-t" />
                </div>
                <div className="relative flex justify-center text-xs uppercase">
                  <span className="bg-background px-2 text-muted-foreground">或粘贴 JSON</span>
                </div>
              </div>

              <textarea
                value={jsonInput}
                onChange={(e) => setJsonInput(e.target.value)}
                placeholder='{"refreshToken": "...", "clientId": "...", ...}'
                className="w-full h-40 p-3 font-mono text-sm border rounded-lg resize-none bg-muted/30 focus:outline-none focus:ring-2 focus:ring-ring focus:bg-background transition-colors"
              />
            </div>
          )}

          {/* Step 2: 预览 */}
          {step === 'preview' && previewResult && (
            <div className="space-y-4">
              {renderSummary(previewResult, false)}
              {renderItemList(previewResult.items, false)}
            </div>
          )}

          {/* Step 3: 结果 */}
          {step === 'result' && importResult && (
            <div className="space-y-4">
              {renderSummary(importResult, true)}
              {renderItemList(importResult.items, true)}
            </div>
          )}
        </div>

        <DialogFooter>
          {step === 'input' && (
            <>
              <Button variant="outline" onClick={handleClose}>
                取消
              </Button>
              <Button
                onClick={handlePreview}
                disabled={!jsonInput.trim() || importMutation.isPending}
              >
                {importMutation.isPending ? '解析中...' : '预览'}
              </Button>
            </>
          )}

          {step === 'preview' && (
            <>
              <Button variant="outline" onClick={() => setStep('input')}>
                返回修改
              </Button>
              <Button
                onClick={handleImport}
                disabled={importMutation.isPending || previewResult?.summary.added === 0}
              >
                {importMutation.isPending
                  ? '导入中...'
                  : `确认导入 (${previewResult?.summary.added || 0})`}
              </Button>
            </>
          )}

          {step === 'result' && <Button onClick={handleClose}>完成</Button>}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
