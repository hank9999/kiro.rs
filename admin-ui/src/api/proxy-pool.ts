import axios from 'axios'
import { storage } from '@/lib/storage'
import type {
  ProxyPoolListResponse,
  AddProxyRequest,
  BatchAddProxyRequest,
  UpdateProxyRequest,
  ProxyTestResponse,
} from '@/types/proxy-pool'
import type { SuccessResponse } from '@/types/api'

const api = axios.create({
  baseURL: '/api/admin',
  headers: { 'Content-Type': 'application/json' },
})

api.interceptors.request.use((config) => {
  const apiKey = storage.getApiKey()
  if (apiKey) {
    config.headers['x-api-key'] = apiKey
  }
  return config
})

// 获取代理池列表
export async function getProxyPool(): Promise<ProxyPoolListResponse> {
  const { data } = await api.get<ProxyPoolListResponse>('/proxy-pool')
  return data
}

// 添加代理
export async function addProxy(req: AddProxyRequest): Promise<{ success: boolean; message: string; id: number }> {
  const { data } = await api.post('/proxy-pool', req)
  return data
}

// 批量添加代理
export async function batchAddProxies(req: BatchAddProxyRequest): Promise<{ success: boolean; message: string; ids: number[] }> {
  const { data } = await api.post('/proxy-pool/batch', req)
  return data
}

// 编辑代理
export async function updateProxy(id: number, req: UpdateProxyRequest): Promise<SuccessResponse> {
  const { data } = await api.put<SuccessResponse>(`/proxy-pool/${id}`, req)
  return data
}

// 删除代理
export async function deleteProxy(id: number): Promise<SuccessResponse> {
  const { data } = await api.delete<SuccessResponse>(`/proxy-pool/${id}`)
  return data
}

// 启用/禁用代理
export async function setProxyDisabled(id: number, disabled: boolean): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(`/proxy-pool/${id}/disabled`, { disabled })
  return data
}

// 测试代理连通性
export async function testProxy(id: number): Promise<ProxyTestResponse> {
  const { data } = await api.post<ProxyTestResponse>(`/proxy-pool/${id}/test`)
  return data
}
