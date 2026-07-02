import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  getProxyPool,
  testProxyPool,
  updateCredentialProxy,
  updateProxyPool,
} from "@/api/proxy";
import type {
  ProxyPoolConfig,
  TestProxyPoolRequest,
  UpdateCredentialProxyRequest,
} from "@/types/api";

// 查询代理池配置与运行时状态
// 冷却状态需要较高刷新频率以展示倒计时，这里 5s 一次
export function useProxyPool() {
  return useQuery({
    queryKey: ["proxy-pool"],
    queryFn: getProxyPool,
    refetchInterval: 5000,
  });
}

// 更新代理池配置
export function useUpdateProxyPool() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (payload: ProxyPoolConfig) => updateProxyPool(payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["proxy-pool"] });
    },
  });
}

// 测试代理池连通性
export function useTestProxyPool() {
  return useMutation({
    mutationFn: (payload: TestProxyPoolRequest = {}) => testProxyPool(payload),
  });
}

// 更新单个凭据代理
export function useUpdateCredentialProxy() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      payload,
    }: {
      id: number;
      payload: UpdateCredentialProxyRequest;
    }) => updateCredentialProxy(id, payload),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["credentials"] });
    },
  });
}
