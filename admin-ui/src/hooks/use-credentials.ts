import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  getCredentials,
  deleteCredential,
  setCredentialDisabled,
  setCredentialPriority,
  setCredentialEnabledModels,
  resetCredentialFailure,
  getCredentialBalance,
  getCredentialAccountInfo,
  addCredential,
  getCredentialStats,
  resetCredentialStats,
  resetAllStats,
  getSummaryModel,
  setSummaryModel,
} from '@/api/credentials'
import type { AddCredentialRequest } from '@/types/api'

// 查询凭据列表
export function useCredentials() {
  return useQuery({
    queryKey: ['credentials'],
    queryFn: getCredentials,
    refetchInterval: 30000, // 每 30 秒刷新一次
  })
}

// 查询凭据余额
interface UseCredentialBalanceOptions {
  enabled?: boolean
  refetchInterval?: number | false
}

export function useCredentialBalance(
  id: number | null,
  options?: UseCredentialBalanceOptions
) {
  const enabled = (options?.enabled ?? true) && id !== null
  return useQuery({
    queryKey: ['credential-balance', id],
    queryFn: () => {
      if (id === null) {
        return Promise.reject(new Error('Invalid credential id'))
      }
      return getCredentialBalance(id)
    },
    enabled,
    retry: false, // 余额查询失败时不重试（避免重复请求被封禁的账号）
    refetchInterval: options?.refetchInterval,
    staleTime: options?.refetchInterval || 5 * 60 * 1000, // 默认 5 分钟内不重新请求
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  })
}

// 查询凭据账号信息（套餐/用量/邮箱等）
export function useCredentialAccountInfo(id: number | null, enabled: boolean) {
  return useQuery({
    queryKey: ['credential-account', id],
    queryFn: () => getCredentialAccountInfo(id!),
    enabled: enabled && id !== null,
    retry: false,
    refetchInterval: 10 * 60 * 1000, // 每 10 分钟刷新一次
    staleTime: 5 * 60 * 1000, // 5 分钟内不重新请求
  })
}

// 删除指定凭据
export function useDeleteCredential() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => deleteCredential(id),
    onSuccess: (_res, id) => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
      queryClient.invalidateQueries({ queryKey: ['credential-balance', id] })
      queryClient.invalidateQueries({ queryKey: ['credential-account', id] })
      queryClient.invalidateQueries({ queryKey: ['credential-stats', id] })
    },
  })
}

// 设置禁用状态
export function useSetDisabled() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, disabled }: { id: number; disabled: boolean }) =>
      setCredentialDisabled(id, disabled),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 设置优先级
export function useSetPriority() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, priority }: { id: number; priority: number }) =>
      setCredentialPriority(id, priority),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 设置模型开关
export function useSetEnabledModels() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, enabledModels }: { id: number; enabledModels: string[] }) =>
      setCredentialEnabledModels(id, enabledModels),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 重置失败计数
export function useResetFailure() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => resetCredentialFailure(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 添加新凭据
export function useAddCredential() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (req: AddCredentialRequest) => addCredential(req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 查询指定凭据统计
export function useCredentialStats(id: number | null, enabled: boolean) {
  return useQuery({
    queryKey: ['credential-stats', id],
    queryFn: () => getCredentialStats(id!),
    enabled: enabled && id !== null,
    retry: false,
  })
}

// 清空指定凭据统计
export function useResetCredentialStats() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => resetCredentialStats(id),
    onSuccess: (_res, id) => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
      queryClient.invalidateQueries({ queryKey: ['credential-stats', id] })
    },
  })
}

// 清空全部统计
export function useResetAllStats() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => resetAllStats(),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
      queryClient.invalidateQueries({ queryKey: ['credential-stats'] })
    },
  })
}

// ===== 摘要模型设置 =====

// 查询摘要模型设置
export function useSummaryModel() {
  return useQuery({
    queryKey: ['summary-model'],
    queryFn: getSummaryModel,
    staleTime: 5 * 60 * 1000, // 5 分钟内不重新请求
  })
}

// 设置摘要模型
export function useSetSummaryModel() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (model: string) => setSummaryModel(model),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['summary-model'] })
    },
  })
}
