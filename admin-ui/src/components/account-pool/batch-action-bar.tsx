import {
  CheckCircle2,
  Power,
  PowerOff,
  RefreshCw,
  RotateCcw,
  Trash2,
  Upload,
  X,
} from 'lucide-react'
import { Button } from '@/components/ui/button'

interface BatchActionBarProps {
  selectedCount: number
  canVerifyCount: number
  canRefreshCount: number
  canResetCount: number
  canDisableCount: number
  canEnableCount: number
  selectedDisabledCount: number
  verifying: boolean
  batchRefreshing: boolean
  batchRefreshProgress: { current: number; total: number }
  batchResetting: boolean
  batchResetProgress: { current: number; total: number }
  batchDeleting: boolean
  batchDeleteProgress: { current: number; total: number }
  batchTogglingDisabled: boolean
  batchToggleProgress: { current: number; total: number }
  batchToggleAction: 'enable' | 'disable' | null
  onBatchVerify: () => void
  onBatchForceRefresh: () => void
  onBatchResetFailure: () => void
  onBatchSetDisabled: (disabled: boolean) => void
  onBatchDelete: () => void
  onExportSelected: () => void
  onDeselectAll: () => void
}

export function BatchActionBar({
  selectedCount,
  canVerifyCount,
  canRefreshCount,
  canResetCount,
  canDisableCount,
  canEnableCount,
  selectedDisabledCount,
  verifying,
  batchRefreshing,
  batchRefreshProgress,
  batchResetting,
  batchResetProgress,
  batchDeleting,
  batchDeleteProgress,
  batchTogglingDisabled,
  batchToggleProgress,
  batchToggleAction,
  onBatchVerify,
  onBatchForceRefresh,
  onBatchResetFailure,
  onBatchSetDisabled,
  onBatchDelete,
  onExportSelected,
  onDeselectAll,
}: BatchActionBarProps) {
  const batchBusy =
    verifying ||
    batchRefreshing ||
    batchResetting ||
    batchDeleting ||
    batchTogglingDisabled
  const isDisabling = batchTogglingDisabled && batchToggleAction === 'disable'
  const isEnabling = batchTogglingDisabled && batchToggleAction === 'enable'

  return (
    <div className="sticky top-16 z-30 max-w-full overflow-hidden rounded-lg border bg-card/95 p-3 shadow-sm backdrop-blur">
      <div className="flex min-w-0 flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
        <div className="min-w-0 shrink-0 truncate text-sm font-medium">
          已选择 {selectedCount} 个账号
        </div>
        <div className="flex min-w-0 max-w-full flex-wrap items-center gap-2 xl:justify-end">
          <Button
            size="sm"
            variant="outline"
            onClick={onBatchVerify}
            disabled={batchBusy || canVerifyCount === 0}
            title={canVerifyCount === 0 ? '没有可验活的启用账号' : undefined}
          >
            <CheckCircle2 className="h-4 w-4" />
            {verifying ? '验活中' : '验活'}
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={onBatchForceRefresh}
            disabled={batchBusy || canRefreshCount === 0}
            title={canRefreshCount === 0 ? '没有可刷新 Token 的启用 OAuth 账号' : undefined}
          >
            <RefreshCw className={batchRefreshing ? 'h-4 w-4 animate-spin' : 'h-4 w-4'} />
            {batchRefreshing
              ? `刷新中 ${batchRefreshProgress.current}/${batchRefreshProgress.total}`
              : '刷新 Token'}
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={onBatchResetFailure}
            disabled={batchBusy || canResetCount === 0}
            title={canResetCount === 0 ? '没有可恢复的异常账号' : undefined}
          >
            <RotateCcw className="h-4 w-4" />
            {batchResetting
              ? `恢复中 ${batchResetProgress.current}/${batchResetProgress.total}`
              : '恢复异常'}
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={() => onBatchSetDisabled(true)}
            disabled={batchBusy || canDisableCount === 0}
          >
            <PowerOff className="h-4 w-4" />
            {isDisabling
              ? `处理中 ${batchToggleProgress.current}/${batchToggleProgress.total}`
              : '禁用'}
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={() => onBatchSetDisabled(false)}
            disabled={batchBusy || canEnableCount === 0}
            title={canEnableCount === 0 ? '没有可启用的账号' : undefined}
          >
            <Power className="h-4 w-4" />
            {isEnabling
              ? `处理中 ${batchToggleProgress.current}/${batchToggleProgress.total}`
              : '启用'}
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={onExportSelected}
            disabled={batchBusy || selectedCount === 0}
            title="导出选中账号为一个 JSON 文件"
          >
            <Upload className="h-4 w-4" />
            导出
          </Button>
          <Button
            size="sm"
            variant="destructive"
            onClick={onBatchDelete}
            disabled={batchBusy || selectedDisabledCount === 0}
            title={selectedDisabledCount === 0 ? '只能删除已禁用账号' : undefined}
          >
            <Trash2 className="h-4 w-4" />
            {batchDeleting
              ? `删除中 ${batchDeleteProgress.current}/${batchDeleteProgress.total}`
              : '删除已禁用'}
          </Button>
          <Button size="sm" variant="ghost" onClick={onDeselectAll} disabled={batchBusy}>
            <X className="h-4 w-4" />
            取消选择
          </Button>
        </div>
      </div>
    </div>
  )
}
