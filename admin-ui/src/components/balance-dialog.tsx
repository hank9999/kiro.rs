import { useQueryClient } from '@tanstack/react-query'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Progress } from '@/components/ui/progress'
import { useCredentialBalance } from '@/hooks/use-credentials'
import { parseError } from '@/lib/utils'

interface BalanceDialogProps {
  credentialId: number | null
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function BalanceDialog({ credentialId, open, onOpenChange }: BalanceDialogProps) {
  const queryClient = useQueryClient()
  const { data: balance, isLoading, error } = useCredentialBalance(credentialId)

  const handleOpenChange = (newOpen: boolean) => {
    if (!newOpen && credentialId !== null) {
      // 关闭对话框时，使账户信息查询失效，触发主账户信息卡片刷新
      queryClient.invalidateQueries({ queryKey: ['credential-account', credentialId] })
    }
    onOpenChange(newOpen)
  }

  const formatDate = (dateStr: string | null) => {
    if (!dateStr) return '未知'
    const date = new Date(dateStr)
    if (isNaN(date.getTime())) return '未知'
    return date.toLocaleDateString('zh-CN', {
      year: 'numeric',
      month: 'long',
      day: 'numeric',
    })
  }

  const formatNumber = (num: number) => {
    return num.toLocaleString('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>
            凭据 #{credentialId} 余额信息
          </DialogTitle>
          <DialogDescription className="sr-only">
            查看凭据的使用额度和剩余余额
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
                  <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.707 7.293a1 1 0 00-1.414 1.414L8.586 10l-1.293 1.293a1 1 0 101.414 1.414L10 11.414l1.293 1.293a1 1 0 001.414-1.414L11.414 10l1.293-1.293a1 1 0 00-1.414-1.414L10 8.586 8.707 7.293z" clipRule="evenodd" />
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

        {balance && (
          <div className="space-y-4">
            {/* 订阅类型 */}
            <div className="text-center">
              <span className="text-lg font-semibold">
                {balance.subscriptionTitle || '未知订阅类型'}
              </span>
            </div>

            {/* 使用进度 */}
            <div className="space-y-2">
              <div className="flex justify-between text-sm">
                <span>已使用: ${formatNumber(balance.currentUsage)}</span>
                <span>限额: ${formatNumber(balance.usageLimit)}</span>
              </div>
              <Progress value={balance.usagePercentage} />
              <div className="text-center text-sm text-muted-foreground">
                {balance.usagePercentage.toFixed(1)}% 已使用
              </div>
            </div>

            {/* 详细信息 */}
            <div className="grid grid-cols-2 gap-4 pt-4 border-t text-sm">
              <div>
                <span className="text-muted-foreground">剩余额度：</span>
                <span className="font-medium text-green-600">
                  ${formatNumber(balance.remaining)}
                </span>
              </div>
              <div>
                <span className="text-muted-foreground">下次重置：</span>
                <span className="font-medium">
                  {formatDate(balance.nextResetAt)}
                </span>
              </div>
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  )
}
