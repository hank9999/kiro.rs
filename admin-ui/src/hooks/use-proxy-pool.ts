import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  getProxyPool,
  addProxy,
  batchAddProxies,
  updateProxy,
  deleteProxy,
  setProxyDisabled,
  testProxy,
} from '@/api/proxy-pool'
import type { AddProxyRequest, BatchAddProxyRequest, UpdateProxyRequest } from '@/types/proxy-pool'

// 查询代理池列表
export function useProxyPool() {
  return useQuery({
    queryKey: ['proxy-pool'],
    queryFn: getProxyPool,
    refetchInterval: 30000,
  })
}

// 添加代理
export function useAddProxy() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (req: AddProxyRequest) => addProxy(req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxy-pool'] })
    },
  })
}

// 批量添加代理
export function useBatchAddProxies() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (req: BatchAddProxyRequest) => batchAddProxies(req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxy-pool'] })
    },
  })
}

// 编辑代理
export function useUpdateProxy() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, ...req }: UpdateProxyRequest & { id: number }) => updateProxy(id, req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxy-pool'] })
    },
  })
}

// 删除代理
export function useDeleteProxy() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => deleteProxy(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxy-pool'] })
    },
  })
}

// 启用/禁用代理
export function useSetProxyDisabled() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, disabled }: { id: number; disabled: boolean }) =>
      setProxyDisabled(id, disabled),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['proxy-pool'] })
    },
  })
}

// 测试代理连通性
export function useTestProxy() {
  return useMutation({
    mutationFn: (id: number) => testProxy(id),
  })
}
