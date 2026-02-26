import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  getCredentials,
  setCredentialDisabled,
  setCredentialPriority,
  resetCredentialFailure,
  getCredentialBalance,
  addCredential,
  deleteCredential,
  getLoadBalancingMode,
  setLoadBalancingMode,
  getEmailConfig,
  saveEmailConfig,
  testEmail,
  getWebhookUrl,
  setWebhookUrl,
  testWebhook,
} from '@/api/credentials'
import type { AddCredentialRequest, SaveEmailConfigRequest, TestEmailRequest, TestWebhookRequest } from '@/types/api'

// 查询凭据列表
export function useCredentials() {
  return useQuery({
    queryKey: ['credentials'],
    queryFn: getCredentials,
    refetchInterval: 30000, // 每 30 秒刷新一次
  })
}

// 查询凭据余额
export function useCredentialBalance(id: number | null) {
  return useQuery({
    queryKey: ['credential-balance', id],
    queryFn: () => getCredentialBalance(id!),
    enabled: id !== null,
    retry: false, // 余额查询失败时不重试（避免重复请求被封禁的账号）
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

// 删除凭据
export function useDeleteCredential() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => deleteCredential(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 获取负载均衡模式
export function useLoadBalancingMode() {
  return useQuery({
    queryKey: ['loadBalancingMode'],
    queryFn: getLoadBalancingMode,
  })
}

// 设置负载均衡模式
export function useSetLoadBalancingMode() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: setLoadBalancingMode,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['loadBalancingMode'] })
    },
  })
}

// 获取邮件配置
export function useEmailConfig() {
  return useQuery({
    queryKey: ['emailConfig'],
    queryFn: getEmailConfig,
  })
}

// 保存邮件配置
export function useSaveEmailConfig() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (req: SaveEmailConfigRequest) => saveEmailConfig(req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['emailConfig'] })
    },
  })
}

// 发送测试邮件
export function useTestEmail() {
  return useMutation({
    mutationFn: (req: TestEmailRequest) => testEmail(req),
  })
}

// 获取 Webhook URL
export function useWebhookUrl() {
  return useQuery({
    queryKey: ['webhookUrl'],
    queryFn: getWebhookUrl,
  })
}

// 设置 Webhook URL
export function useSetWebhookUrl() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: setWebhookUrl,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['webhookUrl'] })
    },
  })
}

// 发送测试 Webhook
export function useTestWebhook() {
  return useMutation({
    mutationFn: (req: TestWebhookRequest) => testWebhook(req),
  })
}
