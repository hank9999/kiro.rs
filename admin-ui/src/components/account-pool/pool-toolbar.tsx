import {
  CheckCircle2,
  ChevronDown,
  Download,
  FileDown,
  MoreHorizontal,
  Plus,
  RefreshCw,
  Search,
  Trash2,
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import type {
  AuthMethodFilter,
  CredentialStatusFilter,
} from '@/lib/credential-status'

export type CredentialSortKey =
  | 'priority'
  | 'status'
  | 'email'
  | 'failures'
  | 'success'
  | 'lastUsed'
  | 'id'

export type SortDirection = 'asc' | 'desc'

interface PoolToolbarProps {
  totalCount: number
  filteredCount: number
  searchQuery: string
  statusFilter: CredentialStatusFilter
  authMethodFilter: AuthMethodFilter
  endpointFilter: string
  endpointOptions: string[]
  sortKey: CredentialSortKey
  sortDirection: SortDirection
  pageSize: number
  queryingInfo: boolean
  queryInfoProgress: { current: number; total: number }
  disabledCredentialCount: number
  verifying: boolean
  verifyDialogOpen: boolean
  verifyProgress: { current: number; total: number }
  onSearchQueryChange: (value: string) => void
  onStatusFilterChange: (value: CredentialStatusFilter) => void
  onAuthMethodFilterChange: (value: AuthMethodFilter) => void
  onEndpointFilterChange: (value: string) => void
  onSortKeyChange: (value: CredentialSortKey) => void
  onSortDirectionChange: (value: SortDirection) => void
  onPageSizeChange: (value: number) => void
  onQueryCurrentPageInfo: () => void
  onClearDisabled: () => void
  onOpenVerifyDialog: () => void
  onOpenKamImport: () => void
  onOpenBatchImport: () => void
  onAddCredential: () => void
}

const statusFilterOptions: Array<{
  value: CredentialStatusFilter
  label: string
}> = [
  { value: 'all', label: '全部状态' },
  { value: 'available', label: '可用' },
  { value: 'current', label: '当前' },
  { value: 'problem', label: '异常' },
  { value: 'disabled', label: '已禁用' },
]

const authMethodOptions: Array<{
  value: AuthMethodFilter
  label: string
}> = [
  { value: 'all', label: '全部认证' },
  { value: 'social', label: 'Social' },
  { value: 'idc', label: 'IdC' },
  { value: 'api_key', label: 'API Key' },
]

const sortOptions: Array<{ value: CredentialSortKey; label: string }> = [
  { value: 'priority', label: '优先级' },
  { value: 'status', label: '状态' },
  { value: 'email', label: '账号' },
  { value: 'failures', label: '失败次数' },
  { value: 'success', label: '成功次数' },
  { value: 'lastUsed', label: '最近调用' },
  { value: 'id', label: '凭据 ID' },
]

function SelectControl<T extends string>({
  value,
  onChange,
  options,
  label,
}: {
  value: T
  onChange: (value: T) => void
  options: Array<{ value: T; label: string }>
  label: string
}) {
  return (
    <label className="min-w-0 max-w-full shrink-0">
      <span className="sr-only">{label}</span>
      <select
        value={value}
        onChange={(event) => onChange(event.target.value as T)}
        className="h-9 w-full max-w-[12rem] truncate rounded-md border border-input bg-background px-3 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
      >
        {options.map(option => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
    </label>
  )
}

export function PoolToolbar({
  totalCount,
  filteredCount,
  searchQuery,
  statusFilter,
  authMethodFilter,
  endpointFilter,
  endpointOptions,
  sortKey,
  sortDirection,
  pageSize,
  queryingInfo,
  queryInfoProgress,
  disabledCredentialCount,
  verifying,
  verifyDialogOpen,
  verifyProgress,
  onSearchQueryChange,
  onStatusFilterChange,
  onAuthMethodFilterChange,
  onEndpointFilterChange,
  onSortKeyChange,
  onSortDirectionChange,
  onPageSizeChange,
  onQueryCurrentPageInfo,
  onClearDisabled,
  onOpenVerifyDialog,
  onOpenKamImport,
  onOpenBatchImport,
  onAddCredential,
}: PoolToolbarProps) {
  const endpointSelectOptions = [
    { value: 'all', label: '全部端点' },
    ...endpointOptions.map(endpoint => ({ value: endpoint, label: endpoint })),
  ]

  return (
    <div className="max-w-full overflow-hidden rounded-lg border bg-card p-4 shadow-sm">
      <div className="flex min-w-0 flex-col gap-4">
        <div className="flex min-w-0 flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
          <div className="min-w-0 shrink-0">
            <h2 className="text-xl font-semibold">凭据管理</h2>
            <p className="text-sm text-muted-foreground">
              显示 {filteredCount} / {totalCount} 个凭据
            </p>
          </div>

          <div className="flex min-w-0 max-w-full flex-wrap items-center gap-2 xl:justify-end">
            {verifying && !verifyDialogOpen && (
              <Button onClick={onOpenVerifyDialog} size="sm" variant="secondary">
                <CheckCircle2 className="h-4 w-4 animate-spin" />
                验活中 {verifyProgress.current}/{verifyProgress.total}
              </Button>
            )}
            <Button
              onClick={onQueryCurrentPageInfo}
              size="sm"
              variant="outline"
              disabled={queryingInfo || filteredCount === 0}
            >
              <RefreshCw className={queryingInfo ? 'h-4 w-4 animate-spin' : 'h-4 w-4'} />
              {queryingInfo
                ? `查询中 ${queryInfoProgress.current}/${queryInfoProgress.total}`
                : '查询当前页'}
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button size="sm" variant="outline">
                  <Download className="h-4 w-4" />
                  导入
                  <ChevronDown className="h-4 w-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onClick={onOpenKamImport}>
                  <FileDown className="h-4 w-4" />
                  Kiro Account Manager 导入
                </DropdownMenuItem>
                <DropdownMenuItem onClick={onOpenBatchImport}>
                  <Download className="h-4 w-4" />
                  批量 JSON 导入
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
            <Button onClick={onAddCredential} size="sm">
              <Plus className="h-4 w-4" />
              添加凭据
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button size="sm" variant="ghost" className="h-9 w-9 p-0">
                  <MoreHorizontal className="h-4 w-4" />
                  <span className="sr-only">更多操作</span>
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem
                  onClick={onClearDisabled}
                  disabled={disabledCredentialCount === 0}
                  className="text-destructive focus:text-destructive"
                >
                  <Trash2 className="h-4 w-4" />
                  清除已禁用
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>

        <div className="flex min-w-0 flex-col gap-3 2xl:flex-row 2xl:items-center 2xl:justify-between">
          <div className="relative min-w-0 flex-1 2xl:max-w-md">
            <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={searchQuery}
              onChange={(event) => onSearchQueryChange(event.target.value)}
              placeholder="搜索邮箱、ID、端点、认证方式"
              className="pl-9"
            />
          </div>

          <div className="flex min-w-0 max-w-full flex-wrap items-center gap-2 2xl:justify-end">
            <SelectControl
              label="状态筛选"
              value={statusFilter}
              onChange={onStatusFilterChange}
              options={statusFilterOptions}
            />
            <SelectControl
              label="认证筛选"
              value={authMethodFilter}
              onChange={onAuthMethodFilterChange}
              options={authMethodOptions}
            />
            <SelectControl
              label="端点筛选"
              value={endpointFilter}
              onChange={onEndpointFilterChange}
              options={endpointSelectOptions}
            />
            <SelectControl
              label="排序字段"
              value={sortKey}
              onChange={onSortKeyChange}
              options={sortOptions}
            />
            <SelectControl
              label="排序方向"
              value={sortDirection}
              onChange={onSortDirectionChange}
              options={[
                { value: 'asc', label: '升序' },
                { value: 'desc', label: '降序' },
              ]}
            />
            <label className="shrink-0">
              <span className="sr-only">每页数量</span>
              <select
                value={pageSize}
                onChange={(event) => onPageSizeChange(Number(event.target.value))}
                className="h-9 rounded-md border border-input bg-background px-3 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
              >
                <option value={12}>每页 12</option>
                <option value={24}>每页 24</option>
                <option value={48}>每页 48</option>
              </select>
            </label>
          </div>
        </div>
      </div>
    </div>
  )
}
