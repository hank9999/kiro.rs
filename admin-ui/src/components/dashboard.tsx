import { useState, useEffect, useMemo, useRef } from 'react'
import { RefreshCw, LogOut, Moon, Sun, Server } from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { storage } from '@/lib/storage'
import { Card, CardContent } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { BatchActionBar } from '@/components/account-pool/batch-action-bar'
import { CredentialCompactCard } from '@/components/account-pool/credential-compact-card'
import { CredentialDetailSheet } from '@/components/account-pool/credential-detail-sheet'
import { CredentialPoolTable } from '@/components/account-pool/credential-pool-table'
import {
  PoolToolbar,
  type CredentialSortKey,
  type SortDirection,
} from '@/components/account-pool/pool-toolbar'
import { BalanceDialog } from '@/components/balance-dialog'
import { AddCredentialDialog } from '@/components/add-credential-dialog'
import { BatchImportDialog } from '@/components/batch-import-dialog'
import { KamImportDialog } from '@/components/kam-import-dialog'
import { BatchVerifyDialog, type VerifyResult } from '@/components/batch-verify-dialog'
import { useCredentials, useDeleteCredential, useResetFailure, useSetDisabled, useLoadBalancingMode, useSetLoadBalancingMode } from '@/hooks/use-credentials'
import { getCredentialBalance, forceRefreshToken, exportCredentials } from '@/api/credentials'
import {
  canDisableCredential,
  canEnableCredential,
  canRefreshCredentialToken,
  canResetCredentialFailure,
  canVerifyCredential,
  matchesAuthMethodFilter,
  matchesCredentialStatusFilter,
  type AuthMethodFilter,
  type CredentialStatusFilter,
} from '@/lib/credential-status'
import { getCredentialSearchText } from '@/lib/credential-format'
import { extractErrorMessage } from '@/lib/utils'
import type { BalanceResponse, CredentialStatusItem } from '@/types/api'

interface DashboardProps {
  onLogout: () => void
}

function compareNullableString(a: string | null | undefined, b: string | null | undefined) {
  return (a || '').localeCompare(b || '', 'zh-CN')
}

function compareCredential(
  a: CredentialStatusItem,
  b: CredentialStatusItem,
  sortKey: CredentialSortKey
) {
  switch (sortKey) {
    case 'status':
      return Number(a.disabled) - Number(b.disabled) || a.failureCount - b.failureCount
    case 'email':
      return compareNullableString(a.email, b.email) || a.id - b.id
    case 'failures':
      return (
        a.failureCount + a.refreshFailureCount -
          (b.failureCount + b.refreshFailureCount) ||
        a.id - b.id
      )
    case 'success':
      return a.successCount - b.successCount || a.id - b.id
    case 'lastUsed':
      return (
        new Date(a.lastUsedAt || 0).getTime() -
          new Date(b.lastUsedAt || 0).getTime() ||
        a.id - b.id
      )
    case 'id':
      return a.id - b.id
    case 'priority':
    default:
      return a.priority - b.priority || a.id - b.id
  }
}

function createExportFileName() {
  const now = new Date()
  const pad = (value: number) => String(value).padStart(2, '0')
  const date = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}`
  const time = `${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`
  return `kiro-credentials-${date}-${time}.json`
}

export function Dashboard({ onLogout }: DashboardProps) {
  const [selectedCredentialId, setSelectedCredentialId] = useState<number | null>(null)
  const [balanceDialogOpen, setBalanceDialogOpen] = useState(false)
  const [addDialogOpen, setAddDialogOpen] = useState(false)
  const [batchImportDialogOpen, setBatchImportDialogOpen] = useState(false)
  const [kamImportDialogOpen, setKamImportDialogOpen] = useState(false)
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set())
  const [verifyDialogOpen, setVerifyDialogOpen] = useState(false)
  const [verifying, setVerifying] = useState(false)
  const [verifyProgress, setVerifyProgress] = useState({ current: 0, total: 0 })
  const [verifyResults, setVerifyResults] = useState<Map<number, VerifyResult>>(new Map())
  const [balanceMap, setBalanceMap] = useState<Map<number, BalanceResponse>>(new Map())
  const [balanceErrorMap, setBalanceErrorMap] = useState<Map<number, string>>(new Map())
  const [loadingBalanceIds, setLoadingBalanceIds] = useState<Set<number>>(new Set())
  const [activeCredentialId, setActiveCredentialId] = useState<number | null>(null)
  const [detailSheetOpen, setDetailSheetOpen] = useState(false)
  const [queryingInfo, setQueryingInfo] = useState(false)
  const [queryInfoProgress, setQueryInfoProgress] = useState({ current: 0, total: 0 })
  const [batchRefreshing, setBatchRefreshing] = useState(false)
  const [batchRefreshProgress, setBatchRefreshProgress] = useState({ current: 0, total: 0 })
  const [batchResetting, setBatchResetting] = useState(false)
  const [batchResetProgress, setBatchResetProgress] = useState({ current: 0, total: 0 })
  const [batchDeleting, setBatchDeleting] = useState(false)
  const [batchDeleteProgress, setBatchDeleteProgress] = useState({ current: 0, total: 0 })
  const [batchTogglingDisabled, setBatchTogglingDisabled] = useState(false)
  const [batchToggleProgress, setBatchToggleProgress] = useState({ current: 0, total: 0 })
  const [batchToggleAction, setBatchToggleAction] = useState<'enable' | 'disable' | null>(null)
  const cancelVerifyRef = useRef(false)
  const [currentPage, setCurrentPage] = useState(1)
  const [pageSize, setPageSize] = useState(12)
  const [searchQuery, setSearchQuery] = useState('')
  const [statusFilter, setStatusFilter] = useState<CredentialStatusFilter>('all')
  const [authMethodFilter, setAuthMethodFilter] = useState<AuthMethodFilter>('all')
  const [endpointFilter, setEndpointFilter] = useState('all')
  const [sortKey, setSortKey] = useState<CredentialSortKey>('priority')
  const [sortDirection, setSortDirection] = useState<SortDirection>('asc')
  const [darkMode, setDarkMode] = useState(() => {
    if (typeof window !== 'undefined') {
      return document.documentElement.classList.contains('dark')
    }
    return false
  })

  const queryClient = useQueryClient()
  const { data, isLoading, error, refetch } = useCredentials()
  const { mutate: deleteCredential } = useDeleteCredential()
  const { mutate: resetFailure } = useResetFailure()
  const { mutate: setDisabled } = useSetDisabled()
  const { data: loadBalancingData, isLoading: isLoadingMode } = useLoadBalancingMode()
  const { mutate: setLoadBalancingMode, isPending: isSettingMode } = useSetLoadBalancingMode()

  const allCredentials = useMemo(() => data?.credentials || [], [data?.credentials])

  const filteredCredentials = useMemo(() => {
    const normalizedQuery = searchQuery.trim().toLowerCase()
    const next = allCredentials.filter((credential) => {
      if (
        normalizedQuery &&
        !getCredentialSearchText(credential).includes(normalizedQuery)
      ) {
        return false
      }

      if (!matchesCredentialStatusFilter(credential, statusFilter)) {
        return false
      }

      if (!matchesAuthMethodFilter(credential, authMethodFilter)) {
        return false
      }

      if (endpointFilter !== 'all' && credential.endpoint !== endpointFilter) {
        return false
      }

      return true
    })

    next.sort((a, b) => {
      const result = compareCredential(a, b, sortKey)
      return sortDirection === 'asc' ? result : -result
    })

    return next
  }, [
    allCredentials,
    authMethodFilter,
    endpointFilter,
    searchQuery,
    sortDirection,
    sortKey,
    statusFilter,
  ])

  // 计算分页
  const totalPages = Math.max(1, Math.ceil(filteredCredentials.length / pageSize))
  const startIndex = (currentPage - 1) * pageSize
  const endIndex = startIndex + pageSize
  const currentCredentials = filteredCredentials.slice(startIndex, endIndex)
  const disabledCredentialCount = allCredentials.filter(credential => credential.disabled).length
  const endpointOptions = useMemo(() => {
    return Array.from(new Set(allCredentials.map(credential => credential.endpoint).filter(Boolean))).sort()
  }, [allCredentials])
  const selectedCredentials = useMemo(() => {
    return Array.from(selectedIds)
      .map(id => allCredentials.find(credential => credential.id === id))
      .filter((credential): credential is CredentialStatusItem => Boolean(credential))
  }, [allCredentials, selectedIds])
  const selectedDisabledCount = Array.from(selectedIds).filter(id => {
    const credential = allCredentials.find(c => c.id === id)
    return Boolean(credential?.disabled)
  }).length
  const selectedCanVerifyCount = selectedCredentials.filter(canVerifyCredential).length
  const selectedCanRefreshCount = selectedCredentials.filter(canRefreshCredentialToken).length
  const selectedCanResetCount = selectedCredentials.filter(canResetCredentialFailure).length
  const selectedCanDisableCount = selectedCredentials.filter(canDisableCredential).length
  const selectedCanEnableCount = selectedCredentials.filter(canEnableCredential).length
  const activeCredential = activeCredentialId === null
    ? null
    : allCredentials.find(credential => credential.id === activeCredentialId) || null

  // 当凭据列表或筛选条件变化时重置到第一页
  useEffect(() => {
    setCurrentPage(1)
  }, [
    allCredentials.length,
    authMethodFilter,
    endpointFilter,
    searchQuery,
    sortDirection,
    sortKey,
    statusFilter,
    pageSize,
  ])

  // 筛选后如果当前页越界，回退到最后一页
  useEffect(() => {
    setCurrentPage(page => Math.min(Math.max(page, 1), totalPages))
  }, [totalPages])

  // 只保留当前仍存在的凭据缓存，避免删除后残留旧数据
  useEffect(() => {
    if (!data?.credentials) {
      setBalanceMap(new Map())
      setBalanceErrorMap(new Map())
      setLoadingBalanceIds(new Set())
      setSelectedIds(new Set())
      setSelectedCredentialId(null)
      setActiveCredentialId(null)
      setDetailSheetOpen(false)
      return
    }

    const validIds = new Set(allCredentials.map(credential => credential.id))

    setBalanceMap(prev => {
      const next = new Map<number, BalanceResponse>()
      prev.forEach((value, id) => {
        if (validIds.has(id)) {
          next.set(id, value)
        }
      })
      return next.size === prev.size ? prev : next
    })

    setLoadingBalanceIds(prev => {
      if (prev.size === 0) {
        return prev
      }
      const next = new Set<number>()
      prev.forEach(id => {
        if (validIds.has(id)) {
          next.add(id)
        }
      })
      return next.size === prev.size ? prev : next
    })

    setBalanceErrorMap(prev => {
      if (prev.size === 0) {
        return prev
      }
      const next = new Map<number, string>()
      prev.forEach((value, id) => {
        if (validIds.has(id)) {
          next.set(id, value)
        }
      })
      return next.size === prev.size ? prev : next
    })

    setSelectedIds(prev => {
      if (prev.size === 0) {
        return prev
      }
      const next = new Set<number>()
      prev.forEach(id => {
        if (validIds.has(id)) {
          next.add(id)
        }
      })
      return next.size === prev.size ? prev : next
    })

    setSelectedCredentialId(prev => {
      if (prev === null || validIds.has(prev)) {
        return prev
      }
      return null
    })

    setActiveCredentialId(prev => {
      if (prev === null || validIds.has(prev)) {
        return prev
      }
      return null
    })
  }, [allCredentials, data?.credentials])

  const toggleDarkMode = () => {
    setDarkMode(!darkMode)
    document.documentElement.classList.toggle('dark')
  }

  const handleViewBalance = (id: number) => {
    setSelectedCredentialId(id)
    setBalanceDialogOpen(true)
  }

  const handleOpenDetails = (id: number) => {
    setActiveCredentialId(id)
    setDetailSheetOpen(true)
  }

  const handleQueryCredentialBalance = async (id: number) => {
    const credential = allCredentials.find(item => item.id === id)
    if (!credential || credential.disabled) {
      toast.error('该凭据当前不可查询余额')
      return
    }

    setLoadingBalanceIds(prev => {
      const next = new Set(prev)
      next.add(id)
      return next
    })
    setBalanceErrorMap(prev => {
      const next = new Map(prev)
      next.delete(id)
      return next
    })

    try {
      const balance = await getCredentialBalance(id)
      setBalanceMap(prev => {
        const next = new Map(prev)
        next.set(id, balance)
        return next
      })
      toast.success(`凭据 #${id} 余额查询完成`)
    } catch (error) {
      const message = extractErrorMessage(error)
      setBalanceErrorMap(prev => {
        const next = new Map(prev)
        next.set(id, message)
        return next
      })
      toast.error(`查询失败: ${message}`)
    } finally {
      setLoadingBalanceIds(prev => {
        const next = new Set(prev)
        next.delete(id)
        return next
      })
    }
  }

  const handleRefresh = () => {
    refetch()
    toast.success('已刷新凭据列表')
  }

  const handleLogout = () => {
    storage.removeApiKey()
    queryClient.clear()
    onLogout()
  }

  // 选择管理
  const toggleSelect = (id: number) => {
    const newSelected = new Set(selectedIds)
    if (newSelected.has(id)) {
      newSelected.delete(id)
    } else {
      newSelected.add(id)
    }
    setSelectedIds(newSelected)
  }

  const deselectAll = () => {
    setSelectedIds(new Set())
  }

  const toggleCurrentPageSelection = (checked: boolean) => {
    setSelectedIds(prev => {
      const next = new Set(prev)
      currentCredentials.forEach(credential => {
        if (checked) {
          next.add(credential.id)
        } else {
          next.delete(credential.id)
        }
      })
      return next
    })
  }

  // 批量删除（仅删除已禁用项）
  const handleBatchDelete = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要删除的凭据')
      return
    }

    const disabledIds = Array.from(selectedIds).filter(id => {
      const credential = allCredentials.find(c => c.id === id)
      return Boolean(credential?.disabled)
    })

    if (disabledIds.length === 0) {
      toast.error('选中的凭据中没有已禁用项')
      return
    }

    const skippedCount = selectedIds.size - disabledIds.length
    const skippedText = skippedCount > 0 ? `（将跳过 ${skippedCount} 个未禁用凭据）` : ''

    if (!confirm(`确定要删除 ${disabledIds.length} 个已禁用凭据吗？此操作无法撤销。${skippedText}`)) {
      return
    }

    let successCount = 0
    let failCount = 0
    setBatchDeleting(true)
    setBatchDeleteProgress({ current: 0, total: disabledIds.length })

    try {
      for (let i = 0; i < disabledIds.length; i++) {
        const id = disabledIds[i]
        try {
          await new Promise<void>((resolve, reject) => {
            deleteCredential(id, {
              onSuccess: () => {
                successCount++
                resolve()
              },
              onError: (err) => {
                failCount++
                reject(err)
              }
            })
          })
        } catch (error) {
          // 错误已在 onError 中处理
        }
        setBatchDeleteProgress({ current: i + 1, total: disabledIds.length })
      }
    } finally {
      setBatchDeleting(false)
    }

    const skippedResultText = skippedCount > 0 ? `，已跳过 ${skippedCount} 个未禁用凭据` : ''

    if (failCount === 0) {
      toast.success(`成功删除 ${successCount} 个已禁用凭据${skippedResultText}`)
    } else {
      toast.warning(`删除已禁用凭据：成功 ${successCount} 个，失败 ${failCount} 个${skippedResultText}`)
    }

    deselectAll()
  }

  // 批量恢复异常
  const handleBatchResetFailure = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要恢复的凭据')
      return
    }

    const failedIds = Array.from(selectedIds).filter(id => {
      const cred = allCredentials.find(c => c.id === id)
      return cred && canResetCredentialFailure(cred)
    })

    if (failedIds.length === 0) {
      toast.error('选中的凭据中没有可恢复的异常凭据')
      return
    }

    let successCount = 0
    let failCount = 0
    setBatchResetting(true)
    setBatchResetProgress({ current: 0, total: failedIds.length })

    try {
      for (let i = 0; i < failedIds.length; i++) {
        const id = failedIds[i]
        try {
          await new Promise<void>((resolve, reject) => {
            resetFailure(id, {
              onSuccess: () => {
                successCount++
                resolve()
              },
              onError: (err) => {
                failCount++
                reject(err)
              }
            })
          })
        } catch (error) {
          // 错误已在 onError 中处理
        }
        setBatchResetProgress({ current: i + 1, total: failedIds.length })
      }
    } finally {
      setBatchResetting(false)
    }

    if (failCount === 0) {
      toast.success(`成功恢复 ${successCount} 个凭据`)
    } else {
      toast.warning(`成功 ${successCount} 个，失败 ${failCount} 个`)
    }

    deselectAll()
  }

  // 批量刷新 Token
  const handleBatchForceRefresh = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要刷新的凭据')
      return
    }

    const refreshableIds = Array.from(selectedIds).filter(id => {
      const cred = allCredentials.find(c => c.id === id)
      return cred && canRefreshCredentialToken(cred)
    })

    if (refreshableIds.length === 0) {
      toast.error('选中的凭据中没有可刷新 Token 的启用 OAuth 凭据')
      return
    }

    setBatchRefreshing(true)
    setBatchRefreshProgress({ current: 0, total: refreshableIds.length })

    let successCount = 0
    let failCount = 0

    for (let i = 0; i < refreshableIds.length; i++) {
      try {
        await forceRefreshToken(refreshableIds[i])
        successCount++
      } catch {
        failCount++
      }
      setBatchRefreshProgress({ current: i + 1, total: refreshableIds.length })
    }

    setBatchRefreshing(false)
    queryClient.invalidateQueries({ queryKey: ['credentials'] })

    const skippedCount = selectedIds.size - refreshableIds.length
    const skippedText = skippedCount > 0 ? `，跳过 ${skippedCount} 个不可刷新凭据` : ''

    if (failCount === 0) {
      toast.success(`成功刷新 ${successCount} 个凭据的 Token${skippedText}`)
    } else {
      toast.warning(`刷新 Token：成功 ${successCount} 个，失败 ${failCount} 个${skippedText}`)
    }

    deselectAll()
  }

  // 批量启用/禁用
  const handleBatchSetDisabled = async (disabled: boolean) => {
    if (selectedIds.size === 0) {
      toast.error(disabled ? '请先选择要禁用的凭据' : '请先选择要启用的凭据')
      return
    }

    const targetIds = Array.from(selectedIds).filter(id => {
      const cred = allCredentials.find(c => c.id === id)
      if (!cred) return false
      if (disabled) return canDisableCredential(cred)
      return canEnableCredential(cred)
    })

    if (targetIds.length === 0) {
      toast.error(disabled ? '选中的凭据中没有可禁用项' : '选中的凭据中没有可启用项')
      return
    }

    setBatchTogglingDisabled(true)
    setBatchToggleProgress({ current: 0, total: targetIds.length })
    setBatchToggleAction(disabled ? 'disable' : 'enable')

    let successCount = 0
    let failCount = 0

    try {
      for (let i = 0; i < targetIds.length; i++) {
        const id = targetIds[i]
        try {
          await new Promise<void>((resolve, reject) => {
            setDisabled(
              { id, disabled },
              {
                onSuccess: () => {
                  successCount++
                  resolve()
                },
                onError: (err) => {
                  failCount++
                  reject(err)
                },
              }
            )
          })
        } catch {
          // 错误计数已在 onError 中处理
        }

        setBatchToggleProgress({ current: i + 1, total: targetIds.length })
      }
    } finally {
      setBatchTogglingDisabled(false)
      setBatchToggleAction(null)
    }

    const skippedCount = selectedIds.size - targetIds.length
    const skippedText = skippedCount > 0 ? `，跳过 ${skippedCount} 个不可操作凭据` : ''
    const action = disabled ? '禁用' : '启用'

    if (failCount === 0) {
      toast.success(`成功${action} ${successCount} 个凭据${skippedText}`)
    } else {
      toast.warning(`${action}完成：成功 ${successCount} 个，失败 ${failCount} 个${skippedText}`)
    }

    deselectAll()
  }

  // 导出选中凭据为批量导入兼容 JSON
  const handleExportSelected = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要导出的凭据')
      return
    }

    const ids = Array.from(selectedIds)

    try {
      const credentials = await exportCredentials(ids)
      if (credentials.length === 0) {
        toast.error('没有可导出的凭据')
        return
      }

      const json = JSON.stringify(credentials, null, 2)
      const blob = new Blob([json], { type: 'application/json;charset=utf-8' })
      const url = URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = url
      link.download = createExportFileName()
      document.body.appendChild(link)
      link.click()
      link.remove()
      URL.revokeObjectURL(url)

      toast.success(`已导出 ${credentials.length} 个凭据`)
    } catch (error) {
      toast.error('导出失败: ' + extractErrorMessage(error))
    }
  }

  // 一键清除所有已禁用凭据
  const handleClearAll = async () => {
    if (!data?.credentials || data.credentials.length === 0) {
      toast.error('没有可清除的凭据')
      return
    }

    const disabledCredentials = data.credentials.filter(credential => credential.disabled)

    if (disabledCredentials.length === 0) {
      toast.error('没有可清除的已禁用凭据')
      return
    }

    if (!confirm(`确定要清除所有 ${disabledCredentials.length} 个已禁用凭据吗？此操作无法撤销。`)) {
      return
    }

    let successCount = 0
    let failCount = 0

    for (const credential of disabledCredentials) {
      try {
        await new Promise<void>((resolve, reject) => {
          deleteCredential(credential.id, {
            onSuccess: () => {
              successCount++
              resolve()
            },
            onError: (err) => {
              failCount++
              reject(err)
            }
          })
        })
      } catch (error) {
        // 错误已在 onError 中处理
      }
    }

    if (failCount === 0) {
      toast.success(`成功清除所有 ${successCount} 个已禁用凭据`)
    } else {
      toast.warning(`清除已禁用凭据：成功 ${successCount} 个，失败 ${failCount} 个`)
    }

    deselectAll()
  }

  // 查询当前页凭据信息（逐个查询，避免瞬时并发）
  const handleQueryCurrentPageInfo = async () => {
    if (currentCredentials.length === 0) {
      toast.error('当前页没有可查询的凭据')
      return
    }

    const ids = currentCredentials
      .filter(credential => !credential.disabled)
      .map(credential => credential.id)

    if (ids.length === 0) {
      toast.error('当前页没有可查询的启用凭据')
      return
    }

    setQueryingInfo(true)
    setQueryInfoProgress({ current: 0, total: ids.length })

    let successCount = 0
    let failCount = 0

    for (let i = 0; i < ids.length; i++) {
      const id = ids[i]

      setLoadingBalanceIds(prev => {
        const next = new Set(prev)
        next.add(id)
        return next
      })

      try {
        const balance = await getCredentialBalance(id)
        successCount++

        setBalanceMap(prev => {
          const next = new Map(prev)
          next.set(id, balance)
          return next
        })
        setBalanceErrorMap(prev => {
          const next = new Map(prev)
          next.delete(id)
          return next
        })
      } catch (error) {
        failCount++
        const message = extractErrorMessage(error)
        setBalanceErrorMap(prev => {
          const next = new Map(prev)
          next.set(id, message)
          return next
        })
      } finally {
        setLoadingBalanceIds(prev => {
          const next = new Set(prev)
          next.delete(id)
          return next
        })
      }

      setQueryInfoProgress({ current: i + 1, total: ids.length })
    }

    setQueryingInfo(false)

    if (failCount === 0) {
      toast.success(`查询完成：成功 ${successCount}/${ids.length}`)
    } else {
      toast.warning(`查询完成：成功 ${successCount} 个，失败 ${failCount} 个`)
    }
  }

  // 批量验活
  const handleBatchVerify = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要验活的凭据')
      return
    }

    // 初始化状态
    setVerifying(true)
    cancelVerifyRef.current = false
    const ids = Array.from(selectedIds).filter(id => {
      const cred = allCredentials.find(c => c.id === id)
      return cred && canVerifyCredential(cred)
    })

    if (ids.length === 0) {
      setVerifying(false)
      toast.error('选中的凭据中没有可验活的启用凭据')
      return
    }

    setVerifyProgress({ current: 0, total: ids.length })

    let successCount = 0

    // 初始化结果，所有凭据状态为 pending
    const initialResults = new Map<number, VerifyResult>()
    ids.forEach(id => {
      initialResults.set(id, { id, status: 'pending' })
    })
    setVerifyResults(initialResults)
    setVerifyDialogOpen(true)

    // 开始验活
    for (let i = 0; i < ids.length; i++) {
      // 检查是否取消
      if (cancelVerifyRef.current) {
        toast.info('已取消验活')
        break
      }

      const id = ids[i]

      // 更新当前凭据状态为 verifying
      setVerifyResults(prev => {
        const newResults = new Map(prev)
        newResults.set(id, { id, status: 'verifying' })
        return newResults
      })

      try {
        const balance = await getCredentialBalance(id)
        successCount++

        // 更新为成功状态
        setVerifyResults(prev => {
          const newResults = new Map(prev)
          newResults.set(id, {
            id,
            status: 'success',
            usage: `${balance.currentUsage}/${balance.usageLimit}`
          })
          return newResults
        })
      } catch (error) {
        // 更新为失败状态
        setVerifyResults(prev => {
          const newResults = new Map(prev)
          newResults.set(id, {
            id,
            status: 'failed',
            error: extractErrorMessage(error)
          })
          return newResults
        })
      }

      // 更新进度
      setVerifyProgress({ current: i + 1, total: ids.length })

      // 添加延迟防止封号（最后一个不需要延迟）
      if (i < ids.length - 1 && !cancelVerifyRef.current) {
        await new Promise(resolve => setTimeout(resolve, 2000))
      }
    }

    setVerifying(false)

    if (!cancelVerifyRef.current) {
      const skippedCount = selectedIds.size - ids.length
      const skippedText = skippedCount > 0 ? `，跳过 ${skippedCount} 个禁用凭据` : ''
      toast.success(`验活完成：成功 ${successCount}/${ids.length}${skippedText}`)
    }
  }

  // 取消验活
  const handleCancelVerify = () => {
    cancelVerifyRef.current = true
    setVerifying(false)
  }

  // 切换负载均衡模式
  const handleToggleLoadBalancing = () => {
    const currentMode = loadBalancingData?.mode || 'priority'
    const newMode = currentMode === 'priority' ? 'balanced' : 'priority'

    setLoadBalancingMode(newMode, {
      onSuccess: () => {
        const modeName = newMode === 'priority' ? '优先级模式' : '均衡负载模式'
        toast.success(`已切换到${modeName}`)
      },
      onError: (error) => {
        toast.error(`切换失败: ${extractErrorMessage(error)}`)
      }
    })
  }

  if (isLoading) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background">
        <div className="text-center">
          <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-primary mx-auto mb-4"></div>
          <p className="text-muted-foreground">加载中...</p>
        </div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background p-4">
        <Card className="w-full max-w-md">
          <CardContent className="pt-6 text-center">
            <div className="text-red-500 mb-4">加载失败</div>
            <p className="text-muted-foreground mb-4">{(error as Error).message}</p>
            <div className="space-x-2">
              <Button onClick={() => refetch()}>重试</Button>
              <Button variant="outline" onClick={handleLogout}>重新登录</Button>
            </div>
          </CardContent>
        </Card>
      </div>
    )
  }

  const currentCredentialId = data?.currentId && data.currentId > 0
    ? data.currentId
    : null
  const currentCredential = currentCredentialId === null
    ? null
    : allCredentials.find(credential => credential.id === currentCredentialId) || null
  const totalCredentialCount = data?.total || 0
  const availableCredentialCount = data?.available || 0
  const disabledSummaryCount = Math.max(totalCredentialCount - availableCredentialCount, 0)
  const availabilityRate = totalCredentialCount > 0
    ? Math.round((availableCredentialCount / totalCredentialCount) * 100)
    : 0

  return (
    <div className="flex h-screen min-w-0 flex-col overflow-hidden bg-background">
      {/* 顶部导航 */}
      <header className="z-50 w-full shrink-0 border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60">
        <div className="flex h-14 w-full min-w-0 items-center justify-between px-4 md:px-6 lg:px-8">
          <div className="flex min-w-0 items-center gap-2">
            <Server className="h-5 w-5" />
            <span className="truncate font-semibold">Kiro Admin</span>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={handleToggleLoadBalancing}
              disabled={isLoadingMode || isSettingMode}
              title="切换负载均衡模式"
            >
              {isLoadingMode ? '加载中...' : (loadBalancingData?.mode === 'priority' ? '优先级模式' : '均衡负载')}
            </Button>
            <Button variant="ghost" size="icon" onClick={toggleDarkMode}>
              {darkMode ? <Sun className="h-5 w-5" /> : <Moon className="h-5 w-5" />}
            </Button>
            <Button variant="ghost" size="icon" onClick={handleRefresh}>
              <RefreshCw className="h-5 w-5" />
            </Button>
            <Button variant="ghost" size="icon" onClick={handleLogout}>
              <LogOut className="h-5 w-5" />
            </Button>
          </div>
        </div>
      </header>

      {/* 主内容 */}
      <main className="flex min-h-0 min-w-0 w-full max-w-full flex-1 flex-col overflow-hidden px-4 py-5 md:px-6 lg:px-8">
        {/* 统计卡片 */}
        <Card className="mb-4 max-w-full shrink-0 overflow-hidden rounded-lg shadow-sm">
          <CardContent className="grid min-w-0 gap-0 p-0 md:grid-cols-3">
            <div className="flex min-h-20 min-w-0 flex-col items-center justify-center px-4 py-3 text-center">
              <div className="flex max-w-full min-w-0 items-baseline justify-center gap-3">
                <span className="min-w-0 truncate text-sm font-medium text-muted-foreground">
                  凭据总数
                </span>
                <span className="shrink-0 text-2xl font-semibold tabular-nums">
                  {totalCredentialCount}
                </span>
              </div>
              <div className="mt-1 w-full truncate text-xs text-muted-foreground">
                已禁用 {disabledSummaryCount} 个
              </div>
            </div>
            <div className="flex min-h-20 min-w-0 flex-col items-center justify-center border-t px-4 py-3 text-center md:border-l md:border-t-0">
              <div className="flex max-w-full min-w-0 items-center justify-center gap-2">
                <span className="min-w-0 truncate text-sm font-medium text-muted-foreground">
                  当前活跃
                </span>
                <span className="shrink-0 text-2xl font-semibold tabular-nums">
                  {currentCredentialId === null ? '-' : `#${currentCredentialId}`}
                </span>
                {currentCredentialId !== null && (
                  <Badge variant="success" className="shrink-0">
                    活跃
                  </Badge>
                )}
              </div>
              <div
                className="mt-1 block w-full max-w-full truncate px-2 text-xs text-muted-foreground"
                title={currentCredential?.email || undefined}
              >
                {currentCredential?.email || '未选择当前凭据'}
              </div>
            </div>
            <div className="flex min-h-20 min-w-0 flex-col items-center justify-center border-t px-4 py-3 text-center md:border-l md:border-t-0">
              <div className="flex max-w-full min-w-0 items-baseline justify-center gap-3">
                <span className="min-w-0 truncate text-sm font-medium text-muted-foreground">
                  可用凭据
                </span>
                <span className="shrink-0 text-2xl font-semibold tabular-nums text-green-600">
                  {availableCredentialCount}
                </span>
              </div>
              <div className="mt-1 w-full truncate text-xs text-muted-foreground">
                可用率 {availabilityRate}%
              </div>
            </div>
          </CardContent>
        </Card>

        {/* 凭据列表 */}
        <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-4">
          <PoolToolbar
            totalCount={allCredentials.length}
            filteredCount={filteredCredentials.length}
            searchQuery={searchQuery}
            statusFilter={statusFilter}
            authMethodFilter={authMethodFilter}
            endpointFilter={endpointFilter}
            endpointOptions={endpointOptions}
            sortKey={sortKey}
            sortDirection={sortDirection}
            pageSize={pageSize}
            queryingInfo={queryingInfo}
            queryInfoProgress={queryInfoProgress}
            disabledCredentialCount={disabledCredentialCount}
            verifying={verifying}
            verifyDialogOpen={verifyDialogOpen}
            verifyProgress={verifyProgress}
            onSearchQueryChange={setSearchQuery}
            onStatusFilterChange={setStatusFilter}
            onAuthMethodFilterChange={setAuthMethodFilter}
            onEndpointFilterChange={setEndpointFilter}
            onSortKeyChange={setSortKey}
            onSortDirectionChange={setSortDirection}
            onPageSizeChange={setPageSize}
            onQueryCurrentPageInfo={handleQueryCurrentPageInfo}
            onClearDisabled={handleClearAll}
            onOpenVerifyDialog={() => setVerifyDialogOpen(true)}
            onOpenKamImport={() => setKamImportDialogOpen(true)}
            onOpenBatchImport={() => setBatchImportDialogOpen(true)}
            onAddCredential={() => setAddDialogOpen(true)}
          />

          {selectedIds.size > 0 && (
            <BatchActionBar
              selectedCount={selectedIds.size}
              canVerifyCount={selectedCanVerifyCount}
              canRefreshCount={selectedCanRefreshCount}
              canResetCount={selectedCanResetCount}
              canDisableCount={selectedCanDisableCount}
              canEnableCount={selectedCanEnableCount}
              selectedDisabledCount={selectedDisabledCount}
              verifying={verifying}
              batchRefreshing={batchRefreshing}
              batchRefreshProgress={batchRefreshProgress}
              batchResetting={batchResetting}
              batchResetProgress={batchResetProgress}
              batchDeleting={batchDeleting}
              batchDeleteProgress={batchDeleteProgress}
              batchTogglingDisabled={batchTogglingDisabled}
              batchToggleProgress={batchToggleProgress}
              batchToggleAction={batchToggleAction}
              onBatchVerify={handleBatchVerify}
              onBatchForceRefresh={handleBatchForceRefresh}
              onBatchResetFailure={handleBatchResetFailure}
              onBatchSetDisabled={handleBatchSetDisabled}
              onBatchDelete={handleBatchDelete}
              onExportSelected={handleExportSelected}
              onDeselectAll={deselectAll}
            />
          )}

          <div className="min-h-0 min-w-0 flex-1 pb-3">
            {allCredentials.length === 0 ? (
            <Card className="h-full">
              <CardContent className="py-8 text-center text-muted-foreground">
                暂无凭据
              </CardContent>
            </Card>
          ) : filteredCredentials.length === 0 ? (
            <Card className="h-full">
              <CardContent className="py-8 text-center text-muted-foreground">
                没有符合条件的凭据
              </CardContent>
            </Card>
          ) : (
            <div className="flex h-full min-h-0 min-w-0 flex-col gap-4">
              <div className="hidden min-h-0 min-w-0 flex-1 xl:block">
                <CredentialPoolTable
                  credentials={currentCredentials}
                  selectedIds={selectedIds}
                  balanceMap={balanceMap}
                  loadingBalanceIds={loadingBalanceIds}
                  onToggleSelect={toggleSelect}
                  onTogglePageSelection={toggleCurrentPageSelection}
                  onOpenDetails={handleOpenDetails}
                  onViewBalance={handleViewBalance}
                />
              </div>

              <div className="min-h-0 flex-1 space-y-3 overflow-y-auto pr-1 xl:hidden">
                {currentCredentials.map((credential) => (
                  <CredentialCompactCard
                    key={credential.id}
                    credential={credential}
                    selected={selectedIds.has(credential.id)}
                    balance={balanceMap.get(credential.id) || null}
                    loadingBalance={loadingBalanceIds.has(credential.id)}
                    onToggleSelect={() => toggleSelect(credential.id)}
                    onOpenDetails={handleOpenDetails}
                    onViewBalance={handleViewBalance}
                  />
                ))}
              </div>

              {/* 分页控件 */}
              {totalPages > 1 && (
                <div className="flex shrink-0 items-center justify-center gap-4">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setCurrentPage(p => Math.max(1, p - 1))}
                    disabled={currentPage === 1}
                  >
                    上一页
                  </Button>
                  <span className="text-sm text-muted-foreground">
                    第 {currentPage} / {totalPages} 页（共 {filteredCredentials.length} 个凭据）
                  </span>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setCurrentPage(p => Math.min(totalPages, p + 1))}
                    disabled={currentPage === totalPages}
                  >
                    下一页
                  </Button>
                </div>
              )}
            </div>
          )}
          </div>
        </div>
      </main>

      {/* 余额对话框 */}
      <BalanceDialog
        credentialId={selectedCredentialId}
        open={balanceDialogOpen}
        onOpenChange={setBalanceDialogOpen}
      />

      <CredentialDetailSheet
        credential={activeCredential}
        open={detailSheetOpen}
        balance={activeCredentialId === null ? null : balanceMap.get(activeCredentialId) || null}
        loadingBalance={activeCredentialId !== null && loadingBalanceIds.has(activeCredentialId)}
        balanceError={activeCredentialId === null ? null : balanceErrorMap.get(activeCredentialId) || null}
        onOpenChange={setDetailSheetOpen}
        onQueryBalance={handleQueryCredentialBalance}
      />

      {/* 添加凭据对话框 */}
      <AddCredentialDialog
        open={addDialogOpen}
        onOpenChange={setAddDialogOpen}
      />

      {/* 批量导入对话框 */}
      <BatchImportDialog
        open={batchImportDialogOpen}
        onOpenChange={setBatchImportDialogOpen}
      />

      {/* KAM 账号导入对话框 */}
      <KamImportDialog
        open={kamImportDialogOpen}
        onOpenChange={setKamImportDialogOpen}
      />

      {/* 批量验活对话框 */}
      <BatchVerifyDialog
        open={verifyDialogOpen}
        onOpenChange={setVerifyDialogOpen}
        verifying={verifying}
        progress={verifyProgress}
        results={verifyResults}
        onCancel={handleCancelVerify}
      />
    </div>
  )
}
