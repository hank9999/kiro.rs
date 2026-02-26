import axios from 'axios'
import { storage } from '@/lib/storage'
import type {
  CredentialsStatusResponse,
  BalanceResponse,
  SuccessResponse,
  SetDisabledRequest,
  SetPriorityRequest,
  AddCredentialRequest,
  AddCredentialResponse,
  EmailConfigResponse,
  SaveEmailConfigRequest,
  TestEmailRequest,
  TestWebhookRequest,
} from '@/types/api'

// 创建 axios 实例
const api = axios.create({
  baseURL: '/api/admin',
  headers: {
    'Content-Type': 'application/json',
  },
})

// 请求拦截器添加 API Key
api.interceptors.request.use((config) => {
  const apiKey = storage.getApiKey()
  if (apiKey) {
    config.headers['x-api-key'] = apiKey
  }
  return config
})

// 获取所有凭据状态
export async function getCredentials(): Promise<CredentialsStatusResponse> {
  const { data } = await api.get<CredentialsStatusResponse>('/credentials')
  return data
}

// 设置凭据禁用状态
export async function setCredentialDisabled(
  id: number,
  disabled: boolean
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(
    `/credentials/${id}/disabled`,
    { disabled } as SetDisabledRequest
  )
  return data
}

// 设置凭据优先级
export async function setCredentialPriority(
  id: number,
  priority: number
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(
    `/credentials/${id}/priority`,
    { priority } as SetPriorityRequest
  )
  return data
}

// 重置失败计数
export async function resetCredentialFailure(
  id: number
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(`/credentials/${id}/reset`)
  return data
}

// 获取凭据余额
export async function getCredentialBalance(id: number): Promise<BalanceResponse> {
  const { data } = await api.get<BalanceResponse>(`/credentials/${id}/balance`)
  return data
}

// 添加新凭据
export async function addCredential(
  req: AddCredentialRequest
): Promise<AddCredentialResponse> {
  const { data } = await api.post<AddCredentialResponse>('/credentials', req)
  return data
}

// 删除凭据
export async function deleteCredential(id: number): Promise<SuccessResponse> {
  const { data } = await api.delete<SuccessResponse>(`/credentials/${id}`)
  return data
}

// 获取负载均衡模式
export async function getLoadBalancingMode(): Promise<{ mode: 'priority' | 'balanced' }> {
  const { data } = await api.get<{ mode: 'priority' | 'balanced' }>('/config/load-balancing')
  return data
}

// 设置负载均衡模式
export async function setLoadBalancingMode(mode: 'priority' | 'balanced'): Promise<{ mode: 'priority' | 'balanced' }> {
  const { data } = await api.put<{ mode: 'priority' | 'balanced' }>('/config/load-balancing', { mode })
  return data
}

// 获取邮件配置
export async function getEmailConfig(): Promise<EmailConfigResponse> {
  const { data } = await api.get<EmailConfigResponse>('/config/email')
  return data
}

// 保存邮件配置
export async function saveEmailConfig(req: SaveEmailConfigRequest): Promise<SuccessResponse> {
  const { data } = await api.put<SuccessResponse>('/config/email', req)
  return data
}

// 发送测试邮件
export async function testEmail(req: TestEmailRequest): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>('/config/email/test', req)
  return data
}

// 获取 Webhook 配置
export async function getWebhookUrl(): Promise<{ url: string | null; body: string | null }> {
  const { data } = await api.get<{ url: string | null; body: string | null }>('/config/webhook')
  return data
}

// 设置 Webhook 配置
export async function setWebhookUrl(params: { url: string | null; body: string | null }): Promise<{ url: string | null; body: string | null }> {
  const { data } = await api.put<{ url: string | null; body: string | null }>('/config/webhook', params)
  return data
}

// 发送测试 Webhook
export async function testWebhook(req: TestWebhookRequest): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>('/config/webhook/test', req)
  return data
}
