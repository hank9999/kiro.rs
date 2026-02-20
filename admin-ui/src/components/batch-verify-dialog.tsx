import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'

export interface VerifyResult {
  id: number
  status: 'pending' | 'verifying' | 'success' | 'failed'
  usage?: string
  error?: string
}

interface BatchVerifyDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  verifying: boolean
  progress: { current: number; total: number }
  results: Map<number, VerifyResult>
  onCancel: () => void
}

export function BatchVerifyDialog({
  open,
  onOpenChange,
  verifying,
  progress,
  results,
  onCancel,
}: BatchVerifyDialogProps) {
  const resultsArray = Array.from(results.values())
  const successCount = resultsArray.filter(r => r.status === 'success').length
  const failedCount = resultsArray.filter(r => r.status === 'failed').length

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>批量验活</DialogTitle>
        </DialogHeader>

        <div className="space-y-4 py-4">
          {/* 进度显示 */}
          {verifying && (
            <div className="space-y-2">
              <div className="flex justify-between text-sm">
                <span>验活进度</span>
                <span>{progress.current} / {progress.total}</span>
              </div>
              <div className="w-full bg-secondary rounded-full h-2">
                <div
                  className="bg-primary h-2 rounded-full transition-all"
                  style={{ width: `${(progress.current / progress.total) * 100}%` }}
                />
              </div>
            </div>
          )}

          {/* 统计信息 */}
          {results.size > 0 && (
            <div className="flex justify-between text-sm font-medium">
              <span>验活结果</span>
              <span>
                成功: {successCount} / 失败: {failedCount}
              </span>
            </div>
          )}

          {/* 结果列表 */}
          {results.size > 0 && (
            <div className="max-h-[400px] overflow-y-auto border rounded-md p-2 space-y-1">
              {resultsArray.map((result) => (
                <div
                  key={result.id}
                  className={`text-sm p-2 rounded ${
                    result.status === 'success'
                      ? 'bg-green-50 text-green-700 dark:bg-green-950 dark:text-green-300'
                      : result.status === 'failed'
                      ? 'bg-red-50 text-red-700 dark:bg-red-950 dark:text-red-300'
                      : result.status === 'verifying'
                      ? 'bg-blue-50 text-blue-700 dark:bg-blue-950 dark:text-blue-300'
                      : 'bg-gray-50 text-gray-700 dark:bg-gray-950 dark:text-gray-300'
                  }`}
                >
                  <div className="flex items-start justify-between gap-2">
                    <div className="flex items-center gap-2">
                      <span className="font-medium">凭据 #{result.id}</span>
                      {result.status === 'success' && result.usage && (
                        <Badge variant="secondary" className="text-xs">
                          {result.usage}
                        </Badge>
                      )}
                    </div>
                    <span>
                      {result.status === 'success' && '✓'}
                      {result.status === 'failed' && '✗'}
                      {result.status === 'verifying' && '⏳'}
                      {result.status === 'pending' && '⋯'}
                    </span>
                  </div>
                  {result.error && (
                    <div className="text-xs mt-1 opacity-90">
                      错误: {result.error}
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}

          {/* 提示信息 */}
          {verifying && (
            <p className="text-xs text-muted-foreground">
              💡 验活过程中每次请求间隔 2 秒，防止被封号。你可以关闭此窗口，验活会在后台继续进行。
            </p>
          )}
        </div>

        <div className="flex justify-end gap-2">
          {verifying ? (
            <>
              <Button
                type="button"
                variant="outline"
                onClick={() => onOpenChange(false)}
              >
                后台运行
              </Button>
              <Button
                type="button"
                variant="destructive"
                onClick={onCancel}
              >
                取消验活
              </Button>
            </>
          ) : (
            <Button
              type="button"
              onClick={() => onOpenChange(false)}
            >
              关闭
            </Button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
