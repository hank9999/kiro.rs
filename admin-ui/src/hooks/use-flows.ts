import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { getFlows, getFlowStats, clearFlows, checkFlowMonitorAvailable } from '@/api/flows'
import type { FlowQuery } from '@/types/api'

// 查询流量记录
export function useFlows(query: FlowQuery) {
  return useQuery({
    queryKey: ['flows', query],
    queryFn: () => getFlows(query),
    refetchInterval: 10000, // 每 10 秒刷新
  })
}

// 查询流量统计
export function useFlowStats() {
  return useQuery({
    queryKey: ['flowStats'],
    queryFn: getFlowStats,
    refetchInterval: 30000, // 每 30 秒刷新
  })
}

// 清空流量记录
export function useClearFlows() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (before?: string) => clearFlows(before),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['flows'] })
      queryClient.invalidateQueries({ queryKey: ['flowStats'] })
    },
  })
}

// 检查流量监控是否可用
export function useFlowMonitorAvailable() {
  return useQuery({
    queryKey: ['flowMonitorAvailable'],
    queryFn: checkFlowMonitorAvailable,
    staleTime: 60000,
    retry: 1,
    retryDelay: 3000,
  })
}
